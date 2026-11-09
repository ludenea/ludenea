//! The data plane: the extracted state, the streams of a session and the
//! thread that copies their frames. Throwaway, never product code.
//!
//! The stepping threads publish the extracted state of every completed tick
//! and go on: [`Extraction::publish`] serializes into a recycled buffer and
//! swaps it in under a short lock, never waiting for a reader. The newest
//! buffer and the one being written are the double buffer a live view reads;
//! a short retention of the ticks before them stands for the history a
//! lossless stream replays from when it resumes by cursor.
//!
//! One thread, standing for the I/O executor, copies each stream's frames from
//! the extracted state into the session's sink: the shared memory ring, or a
//! second socket connection where memory cannot be shared. When the sink is
//! full the stream's policy decides, and nothing of it reaches the world: a
//! lossless stream pauses its copying and, once the world has advanced by the
//! session's budget of ticks while the client made no room for one frame,
//! ends with a structured error from which the client resumes by cursor; a latest-wins stream overwrites (ring)
//! or skips to the newest tick (socket).

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::commands::write_canonical;
use crate::ring::{FrameHeader, RingWriter};
use crate::world::{ExtractionSink, State};

/// The full extracted state of a tick: the canonical bytes its hash covers.
pub const STATE_STREAM: u32 = 1;
/// A fixed-size buffer per tick standing for a rendered frame.
pub const FRAME_STREAM: u32 = 2;

pub const BUDGET_EXCEEDED: &str = "unacknowledged_budget_exceeded";
pub const OUTSIDE_RETENTION: &str = "outside_retention";
pub const CONNECTION_CLOSED: &str = "connection_closed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Off,
    Lossless,
    LatestWins,
}

impl Policy {
    pub fn tag(self) -> u8 {
        self as u8
    }

    pub fn from_tag(tag: u8) -> Option<Policy> {
        [Policy::Off, Policy::Lossless, Policy::LatestWins].get(tag as usize).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            Policy::Off => "off",
            Policy::Lossless => "lossless",
            Policy::LatestWins => "latest-wins",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlaneConfig {
    /// The data bytes of a session's ring.
    pub ring_capacity: usize,
    /// The size of the buffer standing for a rendered frame.
    pub frame_bytes: usize,
    /// The ticks the world may advance while a lossless stream waits on a full
    /// sink without the client acknowledging one frame, before the stream ends.
    pub budget: u32,
    /// The extracted ticks kept for a lossless stream to resume from.
    pub retention: usize,
}

impl Default for PlaneConfig {
    fn default() -> PlaneConfig {
        PlaneConfig { ring_capacity: 4 << 20, frame_bytes: 320 * 180 * 4, budget: 24, retention: 128 }
    }
}

pub struct Extracted {
    pub tick: u32,
    pub hash: u64,
    pub bytes: Vec<u8>,
}

struct Retained {
    ticks: VecDeque<Arc<Extracted>>,
    /// Buffers ready to be written again.
    pool: Vec<Vec<u8>>,
    /// Evicted ticks a copy still reads; their buffers are recycled later.
    retired: Vec<Arc<Extracted>>,
}

impl Retained {
    fn recycle(&mut self, entry: Arc<Extracted>) {
        match Arc::try_unwrap(entry) {
            Ok(entry) => self.pool.push(entry.bytes),
            Err(entry) => self.retired.push(entry),
        }
    }
}

pub struct Extraction {
    enabled: AtomicBool,
    retention: usize,
    retained: Mutex<Retained>,
    published: Condvar,
    latest: AtomicU32,
    extract_nanos: AtomicU64,
    extracted: AtomicU64,
}

impl Extraction {
    pub fn new(retention: usize) -> Extraction {
        Extraction {
            enabled: AtomicBool::new(false),
            retention: retention.max(1),
            retained: Mutex::new(Retained { ticks: VecDeque::new(), pool: Vec::new(), retired: Vec::new() }),
            published: Condvar::new(),
            latest: AtomicU32::new(0),
            extract_nanos: AtomicU64::new(0),
            extracted: AtomicU64::new(0),
        }
    }

    /// Nothing is extracted until a session opens a data plane.
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }

    /// The newest extracted tick, 0 before the first.
    pub fn latest(&self) -> u32 {
        self.latest.load(Ordering::SeqCst)
    }

    pub fn get(&self, tick: u32) -> Option<Arc<Extracted>> {
        let retained = self.retained.lock().unwrap();
        let first = retained.ticks.front()?.tick;
        retained.ticks.get(tick.checked_sub(first)? as usize).cloned()
    }

    /// The ticks extracted and the mean microseconds one extraction took on a stepping thread.
    pub fn cost(&self) -> (u64, f64) {
        let count = self.extracted.load(Ordering::Relaxed);
        (count, if count == 0 { 0.0 } else { self.extract_nanos.load(Ordering::Relaxed) as f64 / count as f64 / 1_000.0 })
    }

    fn wait_beyond(&self, tick: u32, timeout: Duration) {
        let retained = self.retained.lock().unwrap();
        if self.latest() <= tick {
            drop(self.published.wait_timeout(retained, timeout).unwrap());
        }
    }
}

impl ExtractionSink for Extraction {
    fn publish(&self, tick: u32, hash: u64, state: &State) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let started = Instant::now();
        let mut bytes = self.retained.lock().unwrap().pool.pop().unwrap_or_default();
        write_canonical(state, &mut bytes);
        let entry = Arc::new(Extracted { tick, hash, bytes });
        {
            let mut retained = self.retained.lock().unwrap();
            retained.ticks.push_back(entry);
            while retained.ticks.len() > self.retention {
                let evicted = retained.ticks.pop_front().expect("longer than the retention");
                retained.recycle(evicted);
            }
            for entry in std::mem::take(&mut retained.retired) {
                retained.recycle(entry);
            }
            self.latest.store(tick, Ordering::SeqCst);
        }
        self.published.notify_all();
        self.extract_nanos.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.extracted.fetch_add(1, Ordering::Relaxed);
    }
}

/// The second stream: the same frames over a socket that never blocks the
/// copying thread. One frame at most waits for the socket to take it.
pub struct SocketSink {
    stream: Box<dyn Write + Send>,
    pending: Vec<u8>,
    sent: usize,
    /// Every byte the socket took so far.
    taken: u64,
}

impl SocketSink {
    /// `stream` must be in non-blocking mode.
    pub fn new(stream: Box<dyn Write + Send>) -> SocketSink {
        SocketSink { stream, pending: Vec::new(), sent: 0, taken: 0 }
    }

    /// Sends what it can of the waiting frame; whether nothing waits any more.
    fn flush(&mut self) -> io::Result<bool> {
        while self.sent < self.pending.len() {
            match self.stream.write(&self.pending[self.sent..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => {
                    self.sent += count;
                    self.taken += count as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        self.pending.clear();
        self.sent = 0;
        Ok(true)
    }

    fn write(&mut self, header: &FrameHeader, payload: &[u8]) -> io::Result<bool> {
        if !self.flush()? {
            return Ok(false);
        }
        self.pending.extend_from_slice(&header.encode());
        self.pending.extend_from_slice(payload);
        self.flush()?;
        Ok(true)
    }
}

pub enum Sink {
    Ring(RingWriter),
    Socket(SocketSink),
    /// A second stream whose connection the client has not attached yet.
    Unattached,
}

#[derive(Clone, Debug)]
pub struct StreamStatus {
    pub id: u32,
    pub policy: Policy,
    /// The next tick the stream would copy.
    pub cursor: u32,
    /// The code of the structured error the stream ended with, if it ended.
    pub ended: Option<&'static str>,
    pub frames_written: u64,
    /// Ticks a latest-wins stream never wrote because a newer one replaced them.
    pub ticks_skipped: u64,
    /// Frames a latest-wins stream overwrote in the ring before they were read.
    pub frames_evicted: u64,
    /// Latest-wins frames not written because the sink had no room it could take.
    pub frames_refused: u64,
    /// Times a lossless stream paused on a full sink.
    pub pauses: u64,
    /// The newest tick when the pause in progress began.
    pub paused_at: Option<u32>,
    pub copy_nanos: u64,
}

/// What a session's surface reads without waiting for the copying thread.
#[derive(Default)]
pub struct PlaneFlags {
    /// One bit per stream identifier that has ended.
    pub ended: AtomicU32,
}

pub struct Plane {
    session: u64,
    world: u32,
    sink: Sink,
    streams: Vec<StreamStatus>,
    budget: u32,
    frame: Vec<u8>,
    flags: Arc<PlaneFlags>,
}

impl Plane {
    pub fn new(session: u64, sink: Sink, policies: [(u32, Policy); 2], from_tick: u32, config: &PlaneConfig) -> Plane {
        let streams = policies
            .into_iter()
            .filter(|(_, policy)| *policy != Policy::Off)
            .map(|(id, policy)| StreamStatus { id, policy, cursor: from_tick, ended: None, frames_written: 0, ticks_skipped: 0, frames_evicted: 0, frames_refused: 0, pauses: 0, paused_at: None, copy_nanos: 0 })
            .collect();
        let frame = (0..config.frame_bytes).map(|i| ((i * 131) >> 3) as u8).collect();
        Plane { session, world: 1, sink, streams, budget: config.budget, frame, flags: Arc::new(PlaneFlags::default()) }
    }

    pub fn flags(&self) -> Arc<PlaneFlags> {
        self.flags.clone()
    }

    fn end(flags: &PlaneFlags, stream: &mut StreamStatus, code: &'static str) {
        stream.ended = Some(code);
        flags.ended.fetch_or(1 << stream.id, Ordering::SeqCst);
    }

    /// The bytes a socket took so far while a frame still waits for it, 0 otherwise.
    fn flowing(&self) -> u64 {
        match &self.sink {
            Sink::Socket(socket) if !socket.pending.is_empty() => socket.taken,
            _ => 0,
        }
    }

    /// Copies what the sink takes of every stream, one tick per stream in
    /// turn. Returns whether a lossless stream waits for the client to make room.
    fn pump(&mut self, extraction: &Extraction) -> bool {
        let latest = extraction.latest();
        let latest_wins: u32 = self.streams.iter().filter(|stream| stream.policy == Policy::LatestWins).map(|stream| 1 << stream.id).sum();
        let mut waiting = false;
        let mut broken = false;
        if let Sink::Socket(socket) = &mut self.sink {
            // The frame in flight goes on whatever became of its stream.
            broken = socket.flush().is_err();
        }
        // One bit per stream that can write no more in this call.
        let mut blocked = 0_u32;
        let mut progressed = true;
        while progressed && !broken {
            progressed = false;
            for stream in &mut self.streams {
                if stream.ended.is_some() || blocked & (1 << stream.id) != 0 || stream.cursor > latest {
                    continue;
                }
                let overwrite = stream.policy == Policy::LatestWins;
                let tick = if overwrite { latest } else { stream.cursor };
                let Some(entry) = extraction.get(tick) else {
                    Plane::end(&self.flags, stream, OUTSIDE_RETENTION);
                    continue;
                };
                let started = Instant::now();
                let payload: &[u8] = if stream.id == STATE_STREAM {
                    &entry.bytes
                } else {
                    stamp_frame(&mut self.frame, entry.tick, entry.hash);
                    &self.frame
                };
                let header = FrameHeader { world: self.world, stream: stream.id, tick, length: payload.len() as u32, sequence: u64::from(tick) };
                let (written, evicted) = match &mut self.sink {
                    Sink::Ring(ring) => ring.write(&header, payload, overwrite, |id| latest_wins & (1 << id) != 0),
                    Sink::Socket(socket) => match socket.write(&header, payload) {
                        Ok(written) => (written, 0),
                        Err(_) => {
                            broken = true;
                            break;
                        }
                    },
                    Sink::Unattached => (false, 0),
                };
                stream.frames_evicted += u64::from(evicted);
                if written {
                    progressed = true;
                    stream.paused_at = None;
                    stream.copy_nanos += started.elapsed().as_nanos() as u64;
                    stream.frames_written += 1;
                    stream.ticks_skipped += u64::from(tick - stream.cursor);
                    stream.cursor = tick + 1;
                    continue;
                }
                blocked |= 1 << stream.id;
                if overwrite {
                    stream.frames_refused += 1;
                    continue;
                }
                // A lossless stream pauses. The pause ends with the next frame
                // the sink takes; the stream ends once the world has advanced
                // by the budget while the client made no room for one frame.
                waiting = true;
                let since = *stream.paused_at.get_or_insert_with(|| {
                    stream.pauses += 1;
                    latest
                });
                if latest - since >= self.budget {
                    Plane::end(&self.flags, stream, BUDGET_EXCEEDED);
                }
            }
        }
        if broken {
            for stream in &mut self.streams {
                if stream.ended.is_none() {
                    Plane::end(&self.flags, stream, CONNECTION_CLOSED);
                }
            }
            return false;
        }
        waiting && self.streams.iter().any(|stream| stream.ended.is_none() && stream.paused_at.is_some())
    }
}

/// The stand-in for a rendered frame: a fixed pattern stamped with its tick
/// and the tick's hash.
fn stamp_frame(frame: &mut [u8], tick: u32, hash: u64) {
    if frame.len() >= 16 {
        frame[0..4].copy_from_slice(&tick.to_le_bytes());
        frame[8..16].copy_from_slice(&hash.to_le_bytes());
    }
}

/// Whether a frame stream's payload is the stand-in stamped for `tick`:
/// returns the hash it carries.
pub fn read_frame_stamp(payload: &[u8], tick: u32) -> Option<u64> {
    let last = payload.len().checked_sub(1)?;
    let whole = payload.len() >= 16 && payload[0..4] == tick.to_le_bytes() && (last < 16 || payload[last] == ((last * 131) >> 3) as u8);
    whole.then(|| u64::from_le_bytes(payload[8..16].try_into().expect("eight bytes")))
}

struct Shared {
    extraction: Arc<Extraction>,
    planes: Mutex<Vec<Plane>>,
    stop: AtomicBool,
}

/// The thread that copies frames, standing for the I/O executor.
pub struct IoExecutor {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl IoExecutor {
    pub fn start(extraction: Arc<Extraction>) -> IoExecutor {
        let shared = Arc::new(Shared { extraction, planes: Mutex::new(Vec::new()), stop: AtomicBool::new(false) });
        let running = shared.clone();
        let thread = thread::Builder::new()
            .name("io-executor".to_string())
            .spawn(move || {
                let (mut flowed, mut idle_rounds) = (0, 0_u32);
                while !running.stop.load(Ordering::SeqCst) {
                    let seen = running.extraction.latest();
                    let (waiting, flowing) = {
                        let mut planes = running.planes.lock().unwrap();
                        let waiting = planes.iter_mut().fold(false, |waiting, plane| plane.pump(&running.extraction) | waiting);
                        (waiting, planes.iter().map(Plane::flowing).sum::<u64>())
                    };
                    if flowing != 0 && flowing != flowed {
                        // A socket is taking a frame piece by piece: go on at once.
                        (flowed, idle_rounds) = (flowing, 0);
                        thread::yield_now();
                    } else if waiting || flowing != 0 {
                        // A full sink: look again shortly for the room the client
                        // made, less often once it has made none for a while.
                        idle_rounds += 1;
                        thread::sleep(Duration::from_micros(if idle_rounds < 100 { 100 } else { 1_000 }));
                    } else {
                        idle_rounds = 0;
                        running.extraction.wait_beyond(seen, Duration::from_millis(2));
                    }
                }
            })
            .expect("spawn the copying thread");
        IoExecutor { shared, thread: Some(thread) }
    }

    pub fn add(&self, plane: Plane) {
        let mut planes = self.shared.planes.lock().unwrap();
        planes.retain(|existing| existing.session != plane.session);
        planes.push(plane);
    }

    fn with_plane<R>(&self, session: u64, work: impl FnOnce(&mut Plane) -> R) -> Option<R> {
        self.shared.planes.lock().unwrap().iter_mut().find(|plane| plane.session == session).map(work)
    }

    /// Gives a session's second stream the connection the client attached.
    pub fn attach_socket(&self, session: u64, socket: SocketSink) -> bool {
        self.with_plane(session, |plane| {
            if matches!(plane.sink, Sink::Unattached) {
                plane.sink = Sink::Socket(socket);
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
    }

    pub fn status(&self, session: u64) -> Vec<StreamStatus> {
        self.with_plane(session, |plane| plane.streams.clone()).unwrap_or_default()
    }

    /// Restarts an ended stream at the client's cursor. The error is a code:
    /// the cursor is ahead of the stream, or its tick is no longer retained.
    pub fn resume(&self, session: u64, stream: u32, cursor: u32) -> Result<(), &'static str> {
        let extraction = self.shared.extraction.clone();
        self.with_plane(session, |plane| {
            let flags = plane.flags.clone();
            let stream = plane.streams.iter_mut().find(|candidate| candidate.id == stream).ok_or("unknown_stream")?;
            if cursor > stream.cursor {
                return Err("cursor_ahead_of_the_stream");
            }
            if cursor <= extraction.latest() && extraction.get(cursor).is_none() {
                return Err(OUTSIDE_RETENTION);
            }
            stream.cursor = cursor;
            stream.paused_at = None;
            stream.ended = None;
            flags.ended.fetch_and(!(1 << stream.id), Ordering::SeqCst);
            Ok(())
        })
        .unwrap_or(Err("no_data_plane"))
    }
}

impl Drop for IoExecutor {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.extraction.published.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::{DATA_AT, RingReader};
    use crate::shared_memory::Mapping;
    use reference_workload::{Parameters, Size, fnv1a};

    fn state() -> State {
        State::initialize(&Parameters::new(Size::Small, 0, false))
    }

    fn config() -> PlaneConfig {
        PlaneConfig { ring_capacity: 128 << 10, frame_bytes: 4096, budget: 6, retention: 16 }
    }

    fn ring(config: &PlaneConfig) -> (RingWriter, RingReader) {
        let writer = RingWriter::create(Mapping::create(DATA_AT + config.ring_capacity).unwrap()).unwrap();
        let reader = RingReader::attach(writer.mapping().second_view().unwrap()).unwrap();
        (writer, reader)
    }

    fn drain(reader: &mut RingReader) -> Vec<(u32, u64)> {
        let mut buffer = Vec::new();
        std::iter::from_fn(|| reader.read(&mut buffer).map(|header| (header.stream, header.sequence))).collect()
    }

    #[test]
    fn the_extraction_retains_the_newest_ticks_and_recycles_its_buffers() {
        let extraction = Extraction::new(4);
        let state = state();
        extraction.publish(1, 11, &state);
        assert_eq!(extraction.latest(), 0, "nothing is extracted before a data plane opens");
        extraction.enable();
        for tick in 1..=10 {
            extraction.publish(tick, u64::from(tick), &state);
        }
        assert_eq!(extraction.latest(), 10);
        assert!(extraction.get(6).is_none() && extraction.get(11).is_none());
        let entry = extraction.get(7).unwrap();
        assert_eq!((entry.tick, entry.hash), (7, 7));
        assert_eq!(fnv1a(&entry.bytes), state.hash());
        assert_eq!(extraction.cost().0, 10);
        assert!(extraction.retained.lock().unwrap().pool.len() <= 2, "evicted buffers are written again");
    }

    #[test]
    fn a_lossless_stream_pauses_on_a_full_ring_ends_at_its_budget_and_resumes_by_cursor() {
        let config = config();
        let extraction = Arc::new(Extraction::new(config.retention));
        extraction.enable();
        let (writer, mut reader) = ring(&config);
        let mut plane = Plane::new(9, Sink::Ring(writer), [(STATE_STREAM, Policy::Lossless), (FRAME_STREAM, Policy::Off)], 1, &config);
        let flags = plane.flags();
        let state = state();
        // A state frame of the small size takes about 25 KB: five fit in 128 KB.
        let mut ended_at = None;
        for tick in 1..=14 {
            extraction.publish(tick, u64::from(tick), &state);
            plane.pump(&extraction);
            if ended_at.is_none() && flags.ended.load(Ordering::SeqCst) != 0 {
                ended_at = Some(tick);
            }
        }
        let status = plane.streams[0].clone();
        assert_eq!(status.ended, Some(BUDGET_EXCEEDED));
        assert_eq!(status.frames_written, 5);
        assert_eq!(ended_at, Some(12), "the sixth tick waits; six more behind it reach the budget");
        assert_eq!(drain(&mut reader), (1..=5).map(|tick| (STATE_STREAM, tick)).collect::<Vec<_>>());
        assert!(!plane.pump(&extraction), "an ended stream copies nothing");
        assert!(drain(&mut reader).is_empty());

        let executor = IoExecutor::start(extraction.clone());
        executor.add(plane);
        assert_eq!(executor.resume(9, STATE_STREAM, 7), Err("cursor_ahead_of_the_stream"));
        assert_eq!(executor.resume(9, FRAME_STREAM, 6), Err("unknown_stream"));
        assert_eq!(executor.resume(8, STATE_STREAM, 6), Err("no_data_plane"));
        assert_eq!(executor.resume(9, STATE_STREAM, 6), Ok(()));
        assert_eq!(flags.ended.load(Ordering::SeqCst), 0);
        let mut received = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while received.len() < 9 && Instant::now() < deadline {
            received.extend(drain(&mut reader));
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(received, (6..=14).map(|tick| (STATE_STREAM, tick)).collect::<Vec<_>>(), "every tick once, in order");
        for tick in 15..=40 {
            extraction.publish(tick, u64::from(tick), &state);
        }
        assert_eq!(executor.resume(9, STATE_STREAM, 15), Err(OUTSIDE_RETENTION), "the cursor's tick left the retention");
    }

    #[test]
    fn a_latest_wins_stream_overwrites_and_never_waits() {
        let config = config();
        let extraction = Extraction::new(config.retention);
        extraction.enable();
        let (writer, mut reader) = ring(&config);
        let mut plane = Plane::new(1, Sink::Ring(writer), [(STATE_STREAM, Policy::LatestWins), (FRAME_STREAM, Policy::LatestWins)], 1, &config);
        let state = state();
        for tick in 1..=30 {
            extraction.publish(tick, u64::from(tick) << 8, &state);
            assert!(!plane.pump(&extraction));
        }
        assert_eq!(plane.flags().ended.load(Ordering::SeqCst), 0);
        assert_eq!(plane.streams[0].frames_written, 30);
        assert!(plane.streams[0].frames_evicted + plane.streams[1].frames_evicted > 40);
        let mut buffer = Vec::new();
        let mut newest = [0_u64; 3];
        while let Some(header) = reader.read(&mut buffer) {
            assert!(header.sequence > 20, "only the newest ticks remain");
            newest[header.stream as usize] = header.sequence;
            if header.stream == FRAME_STREAM {
                assert_eq!(read_frame_stamp(&buffer, header.tick), Some(header.sequence << 8));
            } else {
                assert_eq!(fnv1a(&buffer), state.hash());
            }
        }
        assert_eq!(newest, [0, 30, 30]);
    }

    struct Choked {
        taken: Arc<Mutex<Vec<u8>>>,
        open: Arc<AtomicBool>,
    }

    impl Write for Choked {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.open.load(Ordering::SeqCst) {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = bytes.len().min(1000);
            self.taken.lock().unwrap().extend_from_slice(&bytes[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_second_stream_keeps_one_frame_waiting_and_skips_or_pauses_by_policy() {
        let config = config();
        let extraction = Extraction::new(config.retention);
        extraction.enable();
        let taken = Arc::new(Mutex::new(Vec::new()));
        let open = Arc::new(AtomicBool::new(false));
        let socket = SocketSink::new(Box::new(Choked { taken: taken.clone(), open: open.clone() }));
        let mut plane = Plane::new(1, Sink::Socket(socket), [(STATE_STREAM, Policy::Lossless), (FRAME_STREAM, Policy::LatestWins)], 1, &config);
        let state = state();
        for tick in 1..=5 {
            extraction.publish(tick, 0, &state);
            plane.pump(&extraction);
        }
        assert_eq!((plane.streams[0].frames_written, plane.streams[1].frames_written), (1, 0), "one frame waits for the socket");
        open.store(true, Ordering::SeqCst);
        assert!(!plane.pump(&extraction));
        let bytes = taken.lock().unwrap().clone();
        let mut sequences = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let header = FrameHeader::decode(bytes[at..at + 32].try_into().unwrap());
            sequences.push((header.stream, header.sequence));
            at += 32 + header.length as usize;
        }
        assert_eq!(at, bytes.len());
        assert_eq!(sequences, [(1, 1), (1, 2), (2, 5), (1, 3), (1, 4), (1, 5)], "the lossless stream sends every tick, the other the newest");
        assert_eq!(plane.streams[1].ticks_skipped, 4);
    }
}
