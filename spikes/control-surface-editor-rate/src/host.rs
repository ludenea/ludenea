//! The paced host and the server side of its editor session. Throwaway,
//! never product code.
//!
//! The stepping thread steps the world one tick per period of the tick rate
//! and, per tick, in this order: admits the commands the session received
//! since the last boundary as the tick's input record, steps, runs the
//! time-sliced sampling system that writes the watched component, extracts
//! the state into its back buffer, and publishes: the buffer is swapped with
//! the shared one and the tick's change bitset is merged into the shared
//! pending bitset, both under one short lock. It serializes nothing.
//!
//! The I/O executor is a second thread with the same period, half a period
//! out of phase: it swaps the shared buffer for its own, takes the pending
//! bitset, and from them builds and delivers the coalesced change set (the
//! current values of the entities whose watched component changed since its
//! last delivery) and the live values (the subscribed fields whose value
//! changed since they were last sent). Three buffers rotate between the two
//! threads, so neither ever waits for the other longer than a swap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use reference_workload::CHUNK;

use crate::client::{self, ClientReport, EditorScript};
use crate::pool::Pool;
use crate::transport::{self, ServerSide};
use crate::wire::{self, ChangeRow, ClientMessage, FullState, LiveField, LiveValue, Names, Rows, TransactionResult};
use crate::world::{Command, Outcome, State, World, WorldConfig};

pub const SESSION: u32 = 1;

// ---------------------------------------------------------------------------
// The watched component and the extracted state
// ---------------------------------------------------------------------------

/// The watched component, `sampled_motion`: the position and the velocity of
/// an entity as a time-sliced system last sampled them, with the change ticks
/// an ECS would keep (one per chunk, always; one per entity, opted in by the
/// subscription). The system samples, at tick `t`, the slots whose index plus
/// `t` is a multiple of the sampling period: every slot once per period, a
/// fraction of one over the period of the entities per tick.
pub struct Watched {
    pub generation: Vec<u32>,
    pub values: [Vec<f32>; 4],
    pub entity_tick: Vec<u32>,
    pub chunk_tick: Vec<u32>,
}

impl Watched {
    pub fn new(entity_count: usize) -> Watched {
        Watched {
            generation: vec![0; entity_count],
            values: std::array::from_fn(|_| vec![0.0; entity_count]),
            entity_tick: vec![0; entity_count],
            chunk_tick: vec![0; entity_count.div_ceil(CHUNK)],
        }
    }

    /// The sampling system, run after tick `tick`. Returns the rows written.
    pub fn sample(&mut self, state: &State, tick: u32, period: u32) -> usize {
        let first = ((period - tick % period) % period) as usize;
        let mut written = 0;
        for slot in (first..state.len()).step_by(period as usize) {
            if state.alive[slot] {
                self.generation[slot] = state.generation[slot];
                self.values[0][slot] = state.x[slot];
                self.values[1][slot] = state.y[slot];
                self.values[2][slot] = state.vx[slot];
                self.values[3][slot] = state.vy[slot];
                self.entity_tick[slot] = tick;
                self.chunk_tick[slot / CHUNK] = tick;
                written += 1;
            }
        }
        written
    }

    /// The change bitset of tick `tick`: one bit per slot whose component was
    /// written at that tick, found through the chunks written at that tick.
    pub fn tick_bits(&self, tick: u32, bits: &mut [u64]) {
        bits.fill(0);
        for (chunk, chunk_tick) in self.chunk_tick.iter().enumerate() {
            if *chunk_tick != tick {
                continue;
            }
            let start = chunk * CHUNK;
            let end = (start + CHUNK).min(self.entity_tick.len());
            for slot in start..end {
                if self.entity_tick[slot] == tick {
                    bits[slot / 64] |= 1 << (slot % 64);
                }
            }
        }
    }
}

/// The state of one tick boundary as the extraction hands it to its readers.
pub struct Extracted {
    pub tick: u32,
    pub generation: Vec<u32>,
    pub alive: Vec<bool>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub mass: Vec<f32>,
    pub sampled_generation: Vec<u32>,
    pub sampled: [Vec<f32>; 4],
    pub energy: f32,
    pub random_stream: u64,
}

impl Extracted {
    pub fn new(n: usize) -> Extracted {
        Extracted {
            tick: 0,
            generation: vec![0; n],
            alive: vec![false; n],
            x: vec![0.0; n],
            y: vec![0.0; n],
            vx: vec![0.0; n],
            vy: vec![0.0; n],
            mass: vec![0.0; n],
            sampled_generation: vec![0; n],
            sampled: std::array::from_fn(|_| vec![0.0; n]),
            energy: 0.0,
            random_stream: 0,
        }
    }

    /// The extraction: every column copied whole, since the buffer it writes
    /// is one or two ticks old.
    pub fn fill(&mut self, state: &State, watched: &Watched, tick: u32) {
        self.tick = tick;
        self.generation.copy_from_slice(&state.generation);
        self.alive.copy_from_slice(&state.alive);
        self.x.copy_from_slice(&state.x);
        self.y.copy_from_slice(&state.y);
        self.vx.copy_from_slice(&state.vx);
        self.vy.copy_from_slice(&state.vy);
        self.mass.copy_from_slice(&state.mass);
        self.sampled_generation.copy_from_slice(&watched.generation);
        for (to, from) in self.sampled.iter_mut().zip(&watched.values) {
            to.copy_from_slice(from);
        }
        self.energy = state.energy;
        self.random_stream = state.rng.state;
    }

    fn live(&self, field: LiveField) -> &[f32] {
        match field {
            LiveField::PositionX => &self.x,
            LiveField::PositionY => &self.y,
            LiveField::VelocityX => &self.vx,
            LiveField::VelocityY => &self.vy,
        }
    }

    fn full_state(&self) -> FullState<'_> {
        FullState {
            tick: self.tick,
            generation: &self.generation,
            alive: &self.alive,
            columns: [&self.x, &self.y, &self.vx, &self.vy, &self.mass, &self.sampled[0], &self.sampled[1], &self.sampled[2], &self.sampled[3]],
            energy: self.energy,
            random_stream: self.random_stream,
        }
    }
}

// ---------------------------------------------------------------------------
// The session's shared state
// ---------------------------------------------------------------------------

struct Publication {
    front: Extracted,
    /// Whether `front` is a tick the I/O executor has not taken yet.
    fresh: bool,
    /// The union of the change bitsets of the ticks published since the I/O
    /// executor last took it.
    pending: Vec<u64>,
    /// The highest request number admitted into a published tick.
    applied_through: u64,
}

pub struct Session {
    /// The commands received since the last tick boundary, with their
    /// request numbers, in arrival order.
    inbox: Mutex<Vec<(u64, Command)>>,
    publication: Mutex<Publication>,
    results: Mutex<Vec<TransactionResult>>,
    close_requested: AtomicBool,
    stepping_done: AtomicBool,
    refused: Mutex<Vec<String>>,
}

impl Session {
    fn new(n: usize) -> Session {
        Session {
            inbox: Mutex::new(Vec::new()),
            publication: Mutex::new(Publication { front: Extracted::new(n), fresh: false, pending: vec![0; n.div_ceil(64)], applied_through: 0 }),
            results: Mutex::new(Vec::new()),
            close_requested: AtomicBool::new(false),
            stepping_done: AtomicBool::new(false),
            refused: Mutex::new(Vec::new()),
        }
    }

    /// A message from the client, decoded on the thread that carried it (the
    /// connection's reader, or the client's own thread in process), never on
    /// the stepping thread.
    pub fn accept(&self, bytes: &[u8]) {
        match wire::decode_client_message(bytes, Names::Strings) {
            Ok(ClientMessage::Command { session: SESSION, request, command }) => self.inbox.lock().unwrap().push((request, command)),
            Ok(ClientMessage::Close { session: SESSION }) => self.close_requested.store(true, Ordering::SeqCst),
            Ok(_) => self.refused.lock().unwrap().push("unknown session".to_string()),
            Err(error) => self.refused.lock().unwrap().push(error.0),
        }
    }
}

// ---------------------------------------------------------------------------
// The stepping thread
// ---------------------------------------------------------------------------

fn sleep_until(deadline: Instant) {
    let now = Instant::now();
    if deadline > now {
        thread::sleep(deadline - now);
    }
}

/// What the stepping thread spent on one tick, in nanoseconds, by part. The
/// pacing sleep is in none of them.
#[derive(Clone, Copy, Debug, Default)]
pub struct TickTimes {
    /// Taking the session's received commands as the tick's record.
    pub admission: u32,
    /// The world's step: the tick's systems and its hash.
    pub step: u32,
    /// The sampling system that writes the watched component.
    pub sampling: u32,
    /// The copy of the state into the extraction's back buffer.
    pub extraction: u32,
    /// The tick's change bitset, the swap and the merge under the lock, and
    /// the routing of a transaction's result.
    pub publication: u32,
}

impl TickTimes {
    /// The tick itself: what the host spends without any session.
    pub fn world(&self) -> u32 {
        self.step + self.sampling
    }

    /// What the stepping thread does on behalf of the session.
    pub fn session(&self) -> u32 {
        self.admission + self.publication
    }

    pub fn total(&self) -> u32 {
        self.world() + self.session() + self.extraction
    }
}

fn nanos(from: Instant, to: Instant) -> u32 {
    (to - from).as_nanos().min(u128::from(u32::MAX)) as u32
}

pub struct BareReport {
    pub hashes: Vec<u64>,
    pub times: Vec<TickTimes>,
    pub wall: Duration,
}

/// The paced host without any session: the world stepped at the tick rate
/// with its sampling system. With `extract`, the state is also extracted
/// after every tick into three buffers in rotation, as the session's host
/// does, but nothing is published and nobody reads it: the extraction as a
/// renderer alone would need it.
pub fn run_bare(config: WorldConfig, pool: Arc<Pool>, sampling_period: u32, ticks: u32, tick_period: Duration, extract: bool) -> BareReport {
    let mut world = World::new(config, pool);
    let n = world.state().len();
    let mut watched = Watched::new(n);
    let mut buffers: Vec<Extracted> = if extract { (0..3).map(|_| Extracted::new(n)).collect() } else { Vec::new() };
    let mut times = Vec::with_capacity(ticks as usize);
    let start = Instant::now() + tick_period;
    for k in 0..ticks {
        sleep_until(start + tick_period * k);
        let t0 = Instant::now();
        world.step(vec![Vec::new()]).expect("the world steps");
        let t1 = Instant::now();
        watched.sample(world.state(), world.tick(), sampling_period);
        let t2 = Instant::now();
        if extract {
            buffers[k as usize % 3].fill(world.state(), &watched, world.tick());
        }
        let t3 = Instant::now();
        times.push(TickTimes { step: nanos(t0, t1), sampling: nanos(t1, t2), extraction: nanos(t2, t3), ..TickTimes::default() });
    }
    BareReport { hashes: world.tick_hashes(), times, wall: start.elapsed() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    InProcess,
    Loopback,
}

pub struct EditorRun {
    pub config: WorldConfig,
    pub pool: Arc<Pool>,
    pub sampling_period: u32,
    pub transport: Transport,
    pub script: EditorScript,
    /// The period of the tick rate, and of the session's deliveries.
    pub tick_period: Duration,
    /// The host gives up when the session has not closed by this tick.
    pub max_ticks: u32,
}

pub struct EditorReport {
    pub transport: &'static str,
    pub hashes: Vec<u64>,
    /// The input record of every tick, as admitted: what the oracle replays.
    pub log: Vec<Vec<Command>>,
    pub outcomes: Vec<Vec<Outcome>>,
    pub times: Vec<TickTimes>,
    pub wall: Duration,
    pub sampled_rows: u64,
    pub io: IoReport,
    pub client: ClientReport,
    /// Whether the client's copy of the watched component, built from the
    /// change sets alone, equals the host's at the end.
    pub mirror_matches: bool,
    pub refused_messages: Vec<String>,
    pub timed_out: bool,
}

/// The paced host with its editor session, from the first tick to the
/// session's close.
pub fn run_editor(run: &EditorRun) -> std::io::Result<EditorReport> {
    let mut world = World::new(run.config, run.pool.clone());
    let n = world.state().len();
    let mut watched = Watched::new(n);
    let session = Arc::new(Session::new(n));
    let accepting = session.clone();
    let accept: transport::Accept = Arc::new(move |bytes: &[u8]| accepting.accept(bytes));
    let (server_side, client_side) = match run.transport {
        Transport::InProcess => transport::in_process(accept),
        Transport::Loopback => transport::loopback(accept)?,
    };
    let transport = server_side.transport;
    let period = run.tick_period;
    let start = Instant::now() + period;
    let script = run.script.clone();
    let client = thread::Builder::new().name("editor-client".to_string()).spawn(move || client::run(client_side, &script))?;
    let io = {
        let session = session.clone();
        let live = run.script.live.clone();
        thread::Builder::new().name("io-executor".to_string()).spawn(move || io_executor(&session, server_side, n, &live, start + period / 2, period))?
    };

    let mut back = Extracted::new(n);
    let mut bits = vec![0_u64; n.div_ceil(64)];
    let mut log: Vec<Vec<Command>> = Vec::new();
    let mut outcomes: Vec<Vec<Outcome>> = Vec::new();
    let mut times = Vec::new();
    let mut applied_through = 0_u64;
    let mut sampled_rows = 0_u64;
    let mut timed_out = false;
    for k in 0.. {
        sleep_until(start + period * k);
        if session.close_requested.load(Ordering::SeqCst) {
            break;
        }
        if k >= run.max_ticks {
            timed_out = true;
            break;
        }
        let t0 = Instant::now();
        let received = std::mem::take(&mut *session.inbox.lock().unwrap());
        let requests: Vec<u64> = received.iter().map(|(request, _)| *request).collect();
        let record: Vec<Command> = received.into_iter().map(|(_, command)| command).collect();
        let t1 = Instant::now();
        log.push(record.clone());
        let t2 = Instant::now();
        let mut decided = world.step(vec![record]).expect("the world steps");
        let tick = world.tick();
        let t3 = Instant::now();
        sampled_rows += watched.sample(world.state(), tick, run.sampling_period) as u64;
        let t4 = Instant::now();
        back.fill(world.state(), &watched, tick);
        let t5 = Instant::now();
        let decided = decided.pop().expect("one tick, one record of outcomes");
        watched.tick_bits(tick, &mut bits);
        if let Some(last) = requests.last() {
            applied_through = *last;
            let answers: Vec<TransactionResult> = requests
                .iter()
                .zip(&decided)
                .filter(|(_, outcome)| matches!(outcome, Outcome::TransactionApplied { .. } | Outcome::TransactionRejected { .. }))
                .map(|(request, outcome)| TransactionResult { session: SESSION, request: *request, tick, outcome: outcome.clone() })
                .collect();
            if !answers.is_empty() {
                session.results.lock().unwrap().extend(answers);
            }
        }
        {
            let mut publication = session.publication.lock().unwrap();
            std::mem::swap(&mut publication.front, &mut back);
            publication.fresh = true;
            for (pending, tick_bits) in publication.pending.iter_mut().zip(&bits) {
                *pending |= *tick_bits;
            }
            publication.applied_through = applied_through;
        }
        let t6 = Instant::now();
        outcomes.push(decided);
        // The copy of the record into the log is the capture's, not the session's.
        times.push(TickTimes { admission: nanos(t0, t1), step: nanos(t2, t3), sampling: nanos(t3, t4), extraction: nanos(t4, t5), publication: nanos(t5, t6) });
    }
    let wall = start.elapsed();
    session.stepping_done.store(true, Ordering::SeqCst);
    let io = io.join().expect("the I/O executor never panics");
    let client = client.join().expect("the editor client never panics");
    let mirror_matches = (0..n).all(|slot| {
        let expected = (watched.entity_tick[slot] > 0).then(|| ChangeRow {
            index: slot as u32,
            generation: watched.generation[slot],
            values: [watched.values[0][slot], watched.values[1][slot], watched.values[2][slot], watched.values[3][slot]],
        });
        client.mirror[slot].map(|row| (row.index, row.generation, row.values.map(f32::to_bits)))
            == expected.map(|row| (row.index, row.generation, row.values.map(f32::to_bits)))
    });
    let refused_messages = std::mem::take(&mut *session.refused.lock().unwrap());
    Ok(EditorReport { transport, hashes: world.tick_hashes(), log, outcomes, times, wall, sampled_rows, io, client, mirror_matches, refused_messages, timed_out })
}

// ---------------------------------------------------------------------------
// The I/O executor
// ---------------------------------------------------------------------------

/// What one delivery carried.
#[derive(Clone, Copy, Debug)]
pub struct PeriodFacts {
    /// The ticks the change set coalesces.
    pub ticks: u32,
    pub rows: u32,
    /// The change set message as sent: canonical rows, names as strings.
    pub change_set_bytes: u32,
    pub live_values: u32,
    pub live_bytes_strings: u32,
    pub live_bytes_interned: u32,
    /// The executor's time for the period: the take, the two messages built
    /// and delivered.
    pub nanos: u32,
}

#[derive(Default)]
pub struct IoReport {
    pub periods: Vec<PeriodFacts>,
    /// Periods at which no new tick had been published.
    pub idle_periods: u32,
    /// A full-state read of the last tick: canonical rows with string and
    /// interned names, then tagged rows with string and interned names.
    pub full_state_bytes: [usize; 4],
    pub final_tick: u32,
    pub error: Option<String>,
}

fn io_executor(session: &Session, mut server: ServerSide, n: usize, live: &[u32], first: Instant, period: Duration) -> IoReport {
    let mut report = IoReport::default();
    let mut mine = Extracted::new(n);
    let mut bits = vec![0_u64; n.div_ceil(64)];
    let mut rows: Vec<ChangeRow> = Vec::new();
    let mut values: Vec<LiveValue> = Vec::new();
    let mut last_sent: Vec<Option<(u32, u32)>> = vec![None; live.len() * LiveField::ALL.len()];
    let mut message = Vec::new();
    let mut sequence = 0_u64;
    let mut delivered_tick = 0_u32;
    let mut next = first;
    loop {
        sleep_until(next);
        let now = Instant::now();
        while next <= now {
            next += period;
        }
        // Read before the take, so that the last take follows the last tick.
        let done = session.stepping_done.load(Ordering::SeqCst);
        let started = Instant::now();
        let (fresh, applied_through) = {
            let mut publication = session.publication.lock().unwrap();
            let fresh = publication.fresh;
            if fresh {
                std::mem::swap(&mut publication.front, &mut mine);
                publication.fresh = false;
                std::mem::swap(&mut publication.pending, &mut bits);
            }
            (fresh, publication.applied_through)
        };
        let answers = std::mem::take(&mut *session.results.lock().unwrap());
        let delivered: std::io::Result<()> = (|| {
            if fresh {
                rows.clear();
                for (word_index, word) in bits.iter_mut().enumerate() {
                    let mut word = std::mem::take(word);
                    while word != 0 {
                        let slot = word_index * 64 + word.trailing_zeros() as usize;
                        word &= word - 1;
                        rows.push(ChangeRow {
                            index: slot as u32,
                            generation: mine.sampled_generation[slot],
                            values: [mine.sampled[0][slot], mine.sampled[1][slot], mine.sampled[2][slot], mine.sampled[3][slot]],
                        });
                    }
                }
                sequence += 1;
                message.clear();
                wire::encode_change_set(&mut message, SESSION, sequence, delivered_tick, mine.tick, &rows, Rows::Canonical, Names::Strings);
                let change_set_bytes = message.len() as u32;
                server.deliver.deliver(&message)?;

                values.clear();
                for (entity, &index) in live.iter().enumerate() {
                    let generation = mine.generation[index as usize];
                    for (f, field) in LiveField::ALL.into_iter().enumerate() {
                        let value = mine.live(field)[index as usize];
                        let key = Some((generation, value.to_bits()));
                        let sent = &mut last_sent[entity * LiveField::ALL.len() + f];
                        if *sent != key {
                            *sent = key;
                            values.push(LiveValue { index, generation, field, value });
                        }
                    }
                }
                sequence += 1;
                message.clear();
                wire::encode_live_values(&mut message, SESSION, sequence, mine.tick, applied_through, &values, Names::Strings);
                server.deliver.deliver(&message)?;
                report.periods.push(PeriodFacts {
                    ticks: mine.tick - delivered_tick,
                    rows: rows.len() as u32,
                    change_set_bytes,
                    live_values: values.len() as u32,
                    live_bytes_strings: message.len() as u32,
                    live_bytes_interned: wire::live_values_size(&values, Names::Interned) as u32,
                    nanos: 0,
                });
                delivered_tick = mine.tick;
            } else {
                report.idle_periods += 1;
            }
            for answer in &answers {
                message.clear();
                wire::encode_transaction_result(&mut message, answer, Names::Strings);
                server.deliver.deliver(&message)?;
            }
            Ok(())
        })();
        if fresh && let Some(facts) = report.periods.last_mut() {
            facts.nanos = nanos(started, Instant::now());
        }
        if let Err(error) = delivered {
            report.error = Some(format!("delivery failed: {error}"));
            // Without a client the session is over: let the host stop.
            session.close_requested.store(true, Ordering::SeqCst);
            break;
        }
        if done {
            for (slot, (layout, names)) in
                [(Rows::Canonical, Names::Strings), (Rows::Canonical, Names::Interned), (Rows::Tagged, Names::Strings), (Rows::Tagged, Names::Interned)].into_iter().enumerate()
            {
                message.clear();
                wire::encode_full_state(&mut message, SESSION, &mine.full_state(), layout, names);
                report.full_state_bytes[slot] = message.len();
            }
            message.clear();
            wire::encode_end(&mut message, SESSION, delivered_tick);
            if let Err(error) = server.deliver.deliver(&message) {
                report.error = Some(format!("delivery failed: {error}"));
            }
            break;
        }
    }
    report.final_tick = delivered_tick;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Parameters, Size};

    #[test]
    fn the_sampling_system_writes_every_slot_once_per_period_and_the_bitset_names_the_slots_of_the_tick() {
        let state = State::initialize(&Parameters::new(Size::Small, 0, false));
        let n = state.len();
        let mut watched = Watched::new(n);
        let mut bits = vec![0_u64; n.div_ceil(64)];
        let mut seen = vec![0_u32; n];
        for tick in 1..=10 {
            assert_eq!(watched.sample(&state, tick, 10), n / 10);
            watched.tick_bits(tick, &mut bits);
            for slot in 0..n {
                let set = bits[slot / 64] & (1 << (slot % 64)) != 0;
                assert_eq!(set, (slot as u32 + tick) % 10 == 0, "slot {slot} at tick {tick}");
                seen[slot] += u32::from(set);
            }
        }
        assert!(seen.iter().all(|&count| count == 1), "every slot is sampled once per period");
        assert_eq!(watched.values[0], state.x);
        assert_eq!(watched.sample(&state, 11, 1), n, "a period of one samples every slot");
    }

    #[test]
    fn the_extraction_copies_the_state_and_the_watched_component() {
        let state = State::initialize(&Parameters::new(Size::Small, 0, false));
        let mut watched = Watched::new(state.len());
        watched.sample(&state, 1, 1);
        let mut extracted = Extracted::new(state.len());
        extracted.fill(&state, &watched, 1);
        assert_eq!(extracted.tick, 1);
        assert_eq!(extracted.vy, state.vy);
        assert_eq!(extracted.sampled[3], state.vy);
        assert_eq!(extracted.live(LiveField::PositionY), &state.y[..]);
        assert_eq!(extracted.random_stream, state.rng.state);
    }
}
