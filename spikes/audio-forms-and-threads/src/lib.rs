//! Audio forms and threads spike: the model. Throwaway, never product code.
//!
//! - a wait-free ring between one producer and one consumer (`ring`);
//! - a mixer with a fixed set of voices that applies commands at their target
//!   sample inside a buffer and never allocates (`Mixer`);
//! - the offline form's count of frames per tick, computed from the tick
//!   index alone (`frames_of_tick`);
//! - an allocator that counts what the mixer's thread allocates (`Counting`);
//! - the audio state read from the reference workload after a tick
//!   (`extract`), which never writes the simulation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, UnsafeCell};
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use reference_workload::{State, WORLD_SIDE};

pub const VOICES: usize = 64;
pub const PENDING: usize = 512;
/// Voices driven by the simulation's first entities.
pub const EMITTERS: usize = 16;
/// The voice that plays the streamed sound.
pub const STREAMED_VOICE: u32 = 63;

// ---------------------------------------------------------------- allocator

thread_local! {
    static IN_MIXER: Cell<bool> = const { Cell::new(false) };
}

static MIXER_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

/// Counts the allocations and releases made while the current thread is
/// inside the mixer.
pub struct Counting;

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if IN_MIXER.try_with(Cell::get).unwrap_or(false) {
            MIXER_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if IN_MIXER.try_with(Cell::get).unwrap_or(false) {
            MIXER_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.dealloc(pointer, layout) }
    }
}

pub fn mixer_allocations() -> u64 {
    MIXER_ALLOCATIONS.load(Ordering::Relaxed)
}

/// Runs `work` as the mixer's thread does: what it allocates is counted.
pub fn inside_the_mixer<T>(work: impl FnOnce() -> T) -> T {
    IN_MIXER.with(|flag| flag.set(true));
    let result = work();
    IN_MIXER.with(|flag| flag.set(false));
    result
}

// --------------------------------------------------------------------- ring

struct Shared<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY: one producer writes a slot before publishing it through `tail`, one
// consumer reads it before releasing it through `head`; a slot is never
// touched by both at once.
unsafe impl<T: Send> Sync for Shared<T> {}

pub struct Producer<T> {
    shared: Arc<Shared<T>>,
}

pub struct Consumer<T> {
    shared: Arc<Shared<T>>,
}

/// A ring of `capacity` values between one producer and one consumer. Neither
/// side waits, locks or allocates after creation.
pub fn ring<T: Copy + Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let slots = (0..capacity + 1).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect();
    let shared = Arc::new(Shared { slots, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer { shared: shared.clone() }, Consumer { shared })
}

impl<T: Copy + Send> Producer<T> {
    /// False when the ring is full: the value is not written.
    pub fn push(&mut self, value: T) -> bool {
        let shared = &*self.shared;
        let tail = shared.tail.load(Ordering::Relaxed);
        let next = (tail + 1) % shared.slots.len();
        if next == shared.head.load(Ordering::Acquire) {
            return false;
        }
        // SAFETY: the slot at `tail` is the producer's until `tail` moves.
        unsafe { (*shared.slots[tail].get()).write(value) };
        shared.tail.store(next, Ordering::Release);
        true
    }
}

impl<T: Copy + Send> Consumer<T> {
    pub fn pop(&mut self) -> Option<T> {
        let shared = &*self.shared;
        let head = shared.head.load(Ordering::Relaxed);
        if head == shared.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: the slot at `head` was written before `tail` passed it.
        let value = unsafe { (*shared.slots[head].get()).assume_init() };
        shared.head.store((head + 1) % shared.slots.len(), Ordering::Release);
        Some(value)
    }
}

// -------------------------------------------------------------------- mixer

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wave {
    /// The platform's sine function.
    PlatformSine,
    /// A triangle from an integer phase: additions, multiplications and exact
    /// conversions only.
    Triangle,
    /// Samples decoded elsewhere.
    Streamed,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Start { voice: u32, wave: Wave, millihertz: u32, gain: f32, pan: f32, at: u64 },
    /// Ramp the gain and the pan to their targets over `over` frames.
    Target { voice: u32, gain: f32, pan: f32, at: u64, over: u32 },
    Stop { voice: u32, at: u64 },
}

impl Command {
    fn at(&self) -> u64 {
        match *self {
            Command::Start { at, .. } | Command::Target { at, .. } | Command::Stop { at, .. } => at,
        }
    }
}

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    wave: Wave,
    phase: u32,
    phase_step: u32,
    gain: f32,
    pan: f32,
    gain_step: f32,
    pan_step: f32,
    ramp_left: u32,
    gain_target: f32,
    pan_target: f32,
}

const SILENT: Voice = Voice {
    active: false,
    wave: Wave::Triangle,
    phase: 0,
    phase_step: 0,
    gain: 0.0,
    pan: 0.0,
    gain_step: 0.0,
    pan_step: 0.0,
    ramp_left: 0,
    gain_target: 0.0,
    pan_target: 0.0,
};

/// Where the streamed voice's samples come from.
pub enum Stream {
    None,
    /// The mixer decodes as it pulls: the offline form.
    Synchronous(Decoder),
    /// A decoding thread fills a ring: the device form.
    Ring(Consumer<f32>),
}

/// A stand-in for a decoder: a deterministic sequence in integer arithmetic.
#[derive(Clone)]
pub struct Decoder {
    state: u64,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder { state: 0x5eed_5eed_5eed_5eed }
    }

    pub fn next_sample(&mut self) -> f32 {
        self.state = self.state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let value = (self.state >> 40) as i32 - (1 << 23);
        value as f32 / (1 << 23) as f32
    }
}

impl Default for Decoder {
    fn default() -> Decoder {
        Decoder::new()
    }
}

pub struct Mixer {
    rate: u32,
    position: u64,
    voices: [Voice; VOICES],
    pending: [Option<Command>; PENDING],
    commands: Consumer<Command>,
    stream: Stream,
    pub commands_dropped: u64,
    pub stream_underruns: u64,
}

fn triangle(phase: u32) -> f32 {
    // Two halves of the cycle, each a straight line between -1 and 1.
    let half = (phase >> 8) as i32 - (1 << 23);
    let folded = if half < 0 { -half } else { half };
    (folded - (1 << 22)) as f32 / (1 << 22) as f32
}

impl Mixer {
    pub fn new(rate: u32, commands: Consumer<Command>, stream: Stream) -> Mixer {
        Mixer { rate, position: 0, voices: [SILENT; VOICES], pending: [None; PENDING], commands, stream, commands_dropped: 0, stream_underruns: 0 }
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::Start { voice, wave, millihertz, gain, pan, .. } => {
                let step = (u64::from(millihertz) << 32) / (u64::from(self.rate) * 1_000);
                self.voices[voice as usize] = Voice { active: true, wave, phase_step: step as u32, gain, pan, gain_target: gain, pan_target: pan, ..SILENT };
            }
            Command::Target { voice, gain, pan, over, .. } => {
                let v = &mut self.voices[voice as usize];
                let over = over.max(1);
                v.gain_step = (gain - v.gain) / over as f32;
                v.pan_step = (pan - v.pan) / over as f32;
                v.gain_target = gain;
                v.pan_target = pan;
                v.ramp_left = over;
            }
            Command::Stop { voice, .. } => self.voices[voice as usize].active = false,
        }
    }

    fn mix_segment(&mut self, out: &mut [f32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut left = 0.0f32;
            let mut right = 0.0f32;
            for voice in self.voices.iter_mut().filter(|voice| voice.active) {
                let sample = match voice.wave {
                    Wave::Triangle => triangle(voice.phase),
                    Wave::PlatformSine => (voice.phase as f32 * (std::f32::consts::TAU / 4_294_967_296.0)).sin(),
                    Wave::Streamed => match &mut self.stream {
                        Stream::Synchronous(decoder) => decoder.next_sample(),
                        Stream::Ring(samples) => samples.pop().unwrap_or_else(|| {
                            self.stream_underruns += 1;
                            0.0
                        }),
                        Stream::None => 0.0,
                    },
                };
                voice.phase = voice.phase.wrapping_add(voice.phase_step);
                left += sample * voice.gain * (1.0 - voice.pan) * 0.5;
                right += sample * voice.gain * (1.0 + voice.pan) * 0.5;
                if voice.ramp_left > 0 {
                    voice.ramp_left -= 1;
                    if voice.ramp_left == 0 {
                        voice.gain = voice.gain_target;
                        voice.pan = voice.pan_target;
                    } else {
                        voice.gain += voice.gain_step;
                        voice.pan += voice.pan_step;
                    }
                }
            }
            frame[0] = left;
            frame[1] = right;
        }
        self.position += (out.len() / 2) as u64;
    }

    /// Fills `out` (interleaved stereo). Commands are applied at their target
    /// sample, inside the buffer; a command whose time has passed is applied
    /// at once.
    pub fn mix(&mut self, out: &mut [f32]) {
        while let Some(command) = self.commands.pop() {
            match self.pending.iter_mut().find(|slot| slot.is_none()) {
                Some(slot) => *slot = Some(command),
                None => self.commands_dropped += 1,
            }
        }
        let end = self.position + (out.len() / 2) as u64;
        let mut written = 0usize;
        loop {
            let mut earliest: Option<usize> = None;
            for (index, slot) in self.pending.iter().enumerate() {
                if let Some(command) = slot
                    && command.at() < end
                    && earliest.is_none_or(|best| command.at() < self.pending[best].expect("the best slot is filled").at())
                {
                    earliest = Some(index);
                }
            }
            let Some(index) = earliest else { break };
            let command = self.pending[index].take().expect("the earliest slot is filled");
            let until = (command.at().max(self.position) - (end - (out.len() / 2) as u64)) as usize * 2;
            self.mix_segment(&mut out[written..until]);
            written = until;
            self.apply(command);
        }
        self.mix_segment(&mut out[written..]);
    }
}

// ------------------------------------------------------------- offline form

/// The frames the offline form mixes for one tick: the difference of the
/// frames elapsed at the tick's end and at its start, each the floor of the
/// rate times the elapsed simulated time. The step is `numerator/denominator`
/// seconds. Nothing is carried from one tick to the next.
pub fn frames_of_tick(tick: u64, rate: u64, numerator: u64, denominator: u64) -> u64 {
    (tick + 1) * rate * numerator / denominator - tick * rate * numerator / denominator
}

// --------------------------------------------------------------- extraction

/// The audio state read from the simulation after a tick: one emitter per
/// entity of the first `EMITTERS` slots. Reads only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub alive: bool,
    pub generation: u32,
    pub gain: f32,
    pub pan: f32,
}

pub fn extract(state: &State) -> [Emitter; EMITTERS] {
    std::array::from_fn(|slot| {
        let speed = state.vx[slot].abs() + state.vy[slot].abs();
        Emitter {
            alive: state.alive[slot],
            generation: state.generation[slot],
            gain: (speed * (1.0 / 256.0)).min(0.05),
            pan: state.x[slot] * (2.0 / WORLD_SIDE) - 1.0,
        }
    })
}

/// The commands that take the voices from one extracted state to the next,
/// targeted at `at`, ramps over `over` frames. Returns how many were pushed.
pub fn commands_between(previous: Option<&[Emitter; EMITTERS]>, current: &[Emitter; EMITTERS], wave: Wave, at: u64, over: u32, push: &mut impl FnMut(Command) -> bool) -> u64 {
    let mut pushed = 0;
    for (slot, emitter) in current.iter().enumerate() {
        let voice = slot as u32;
        let before = previous.map(|states| states[slot]);
        let same_entity = before.is_some_and(|b| b.alive && emitter.alive && b.generation == emitter.generation);
        if !same_entity {
            if before.is_some_and(|b| b.alive) {
                pushed += u64::from(push(Command::Stop { voice, at }));
            }
            if emitter.alive {
                let millihertz = 110_000 + 55_000 * voice;
                pushed += u64::from(push(Command::Start { voice, wave, millihertz, gain: emitter.gain, pan: emitter.pan, at }));
            }
        } else if before != Some(*emitter) {
            pushed += u64::from(push(Command::Target { voice, gain: emitter.gain, pan: emitter.pan, at, over }));
        }
    }
    pushed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frames_of_a_tick_sum_to_the_floor_of_the_elapsed_time() {
        for (rate, numerator, denominator) in [(48_000, 1, 60), (48_000, 1, 90), (44_100, 1, 64)] {
            let total: u64 = (0..600).map(|tick| frames_of_tick(tick, rate, numerator, denominator)).sum();
            assert_eq!(total, 600 * rate * numerator / denominator);
        }
        assert_eq!(frames_of_tick(0, 48_000, 1, 90), 533);
        assert_eq!(frames_of_tick(2, 48_000, 1, 90), 534);
    }

    #[test]
    fn a_command_is_applied_at_its_sample_inside_a_buffer() {
        let (mut producer, consumer) = ring(8);
        let mut mixer = Mixer::new(48_000, consumer, Stream::None);
        producer.push(Command::Start { voice: 0, wave: Wave::Triangle, millihertz: 12_000_000, gain: 1.0, pan: 0.0, at: 10 });
        let mut out = [0.0f32; 64];
        mixer.mix(&mut out);
        assert!(out[..20].iter().all(|sample| *sample == 0.0));
        assert!(out[20] != 0.0);
        assert_eq!(mixer.position(), 32);
    }

    #[test]
    fn the_ring_gives_back_what_it_was_given_and_refuses_when_full() {
        let (mut producer, mut consumer) = ring::<u32>(2);
        assert!(producer.push(1));
        assert!(producer.push(2));
        assert!(!producer.push(3));
        assert_eq!(consumer.pop(), Some(1));
        assert_eq!(consumer.pop(), Some(2));
        assert_eq!(consumer.pop(), None);
    }
}
