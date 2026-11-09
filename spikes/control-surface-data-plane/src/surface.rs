//! The surface: sessions, the per-session command sequence with the input log
//! as its record, transactions at the input fixed point, the data plane's
//! opening, and the message envelope. Throwaway, never product code.
//!
//! The control surface toy spike measured the contract's discovery; this
//! surface keeps a fixed binary envelope and spends its lines on what the toy
//! left out. A command is a transaction's canonical bytes with the session's
//! next sequence number. In a stepped session the world rests at the input
//! fixed point between two steps, so a command is validated and applied on
//! arrival and its verdicts travel back in the same round trip; in a paced
//! session it waits in the input log for the host's next tick boundary.
//!
//! The server keeps one integer per session, the highest sequence accepted.
//! A command at or below it is a retry, answered from the input log after the
//! check that its bytes equal the logged ones; one that skips ahead is refused.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::commands::{Body, Digests, Handle, Labels, Outcome, Scratch, Transaction, digests, transact, write_canonical};
use crate::data_plane::{Extraction, IoExecutor, Plane, PlaneConfig, PlaneFlags, Policy, Sink, SocketSink, StreamStatus, FRAME_STREAM, STATE_STREAM};
use crate::ring::{DATA_AT, RingWriter};
use crate::shared_memory::Mapping;
use crate::wire::{Reader, put_bytes, put_f32, put_text, put_u8, put_u32, put_u64};
use crate::world::{World, WorldConfig};

pub const PROTOCOL: u32 = 0x4c44_5031;

pub mod op {
    pub const OPEN: u8 = 0;
    pub const STEP: u8 = 1;
    pub const COMMAND: u8 = 2;
    pub const POLL: u8 = 3;
    pub const ENTITY_STATE: u8 = 4;
    pub const FULL_STATE: u8 = 5;
    pub const DIGESTS: u8 = 6;
    pub const OPEN_DATA_PLANE: u8 = 7;
    pub const ATTACH_SECOND_STREAM: u8 = 8;
    pub const RESUME_STREAM: u8 = 9;
    pub const STREAM_STATUS: u8 = 10;
    pub const STATISTICS: u8 = 11;
}

/// How a session's data plane travels.
pub const PLANE_RING: u8 = 0;
pub const PLANE_SECOND_STREAM: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// The client owns the step call.
    Stepped,
    /// A host steps; the client's commands apply at the next tick boundary.
    Paced,
}

/// A structured error: a stable code, a message, a number of context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceError {
    pub code: &'static str,
    pub message: String,
    pub context: u64,
}

fn error(code: &'static str, message: impl Into<String>, context: u64) -> SurfaceError {
    SurfaceError { code, message: message.into(), context }
}

#[derive(Clone, Copy, Debug)]
pub struct ServerConfig {
    pub world: WorldConfig,
    pub plane: PlaneConfig,
    /// The ticks of applied commands the input log keeps.
    pub log_retention: u32,
}

struct LogEntry {
    session: u64,
    sequence: u64,
    bytes: Vec<u8>,
    /// The tick boundary the command was applied at, once it was.
    tick: Option<u32>,
    outcome: Option<Vec<u8>>,
}

/// The input log: every admitted command with its session, its sequence, its
/// canonical bytes and, once applied, its tick and its deterministic result.
/// It is the one record a retry is answered from.
struct InputLog {
    entries: VecDeque<LogEntry>,
    retention: u32,
    pending: usize,
    commands: u64,
    bytes: u64,
}

impl InputLog {
    fn find(&self, session: u64, sequence: u64) -> Option<&LogEntry> {
        self.entries.iter().rev().find(|entry| entry.session == session && entry.sequence == sequence)
    }

    fn admit(&mut self, entry: LogEntry) {
        self.commands += 1;
        self.bytes += entry.bytes.len() as u64 + 20;
        self.pending += usize::from(entry.tick.is_none());
        self.entries.push_back(entry);
    }

    fn trim(&mut self, now: u32) {
        while self.entries.front().is_some_and(|entry| entry.tick.is_some_and(|tick| tick + self.retention < now)) {
            self.entries.pop_front();
        }
    }
}

struct Session {
    token: u64,
    profile: Profile,
    highest_sequence: u64,
    plane: Option<Arc<PlaneFlags>>,
}

/// What a connection can carry beside bytes.
#[derive(Clone, Copy, Debug)]
pub struct Connection {
    pub passes_descriptors: bool,
}

/// What the transport does after sending a reply.
pub enum Handoff {
    None,
    /// Send this descriptor of the session's ring over the Unix domain socket.
    #[cfg(unix)]
    Descriptor(std::os::fd::OwnedFd),
    /// The connection becomes this session's second stream.
    SecondStream(u64),
}

pub struct Reply {
    pub bytes: Vec<u8>,
    pub handoff: Handoff,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandReply {
    pub retry: bool,
    /// The tick boundary the command was applied at; `None` while it waits.
    pub tick: Option<u32>,
    pub outcome: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Statistics {
    pub tick: u32,
    pub stepping_nanos: u64,
    pub extracted_ticks: u64,
    pub extraction_micros: f32,
    pub log_commands: u64,
    pub log_bytes: u64,
    pub retries_answered: u64,
    pub gaps_refused: u64,
    pub altered_retries_refused: u64,
    pub retries_outside_retention: u64,
}

pub struct Server {
    world: World,
    labels: Labels,
    scratch: Scratch,
    sessions: Vec<Session>,
    log: InputLog,
    extraction: Arc<Extraction>,
    io: IoExecutor,
    plane: PlaneConfig,
    counters: Statistics,
}

impl Server {
    pub fn new(config: ServerConfig) -> Server {
        let mut world = World::new(config.world);
        let extraction = Arc::new(Extraction::new(config.plane.retention));
        world.set_extraction(extraction.clone());
        let n = world.state().len();
        Server {
            world,
            labels: Labels::new(n),
            scratch: Scratch::new(n),
            sessions: Vec::new(),
            log: InputLog { entries: VecDeque::new(), retention: config.log_retention, pending: 0, commands: 0, bytes: 0 },
            io: IoExecutor::start(extraction.clone()),
            extraction,
            plane: config.plane,
            counters: Statistics::default(),
        }
    }

    pub fn tick(&self) -> u32 {
        self.world.tick()
    }

    fn session(&mut self, token: u64) -> Result<&mut Session, SurfaceError> {
        self.sessions.iter_mut().find(|session| session.token == token).ok_or_else(|| error("unknown_session", "no session has this token", token))
    }

    pub fn open(&mut self, profile: Profile) -> u64 {
        let token = 0x5e55_0000 + self.sessions.len() as u64 + 1;
        self.sessions.push(Session { token, profile, highest_sequence: 0, plane: None });
        token
    }

    fn ended_streams(&mut self, token: u64) -> Result<u32, SurfaceError> {
        Ok(self.session(token)?.plane.as_ref().map_or(0, |flags| flags.ended.load(Ordering::SeqCst)))
    }

    fn hashes(&self, from: u32) -> Vec<u64> {
        (from + 1..=self.world.tick()).map(|tick| self.world.tick_hash(tick).expect("a completed tick has a hash")).collect()
    }

    fn apply(&mut self, transaction: &Transaction) -> Outcome {
        let (labels, scratch) = (&mut self.labels, &mut self.scratch);
        self.world.at_boundary(|state, dead, touched| transact(state, labels, dead, touched, scratch, transaction))
    }

    fn advance(&mut self, ticks: u32) -> Result<(), SurfaceError> {
        self.world.step(ticks).map_err(|failure| error("world_failed", failure.to_string(), 0))?;
        self.log.trim(self.world.tick());
        Ok(())
    }

    /// A stepped session's step: the ticks' hashes and the streams that ended.
    pub fn step(&mut self, token: u64, ticks: u32) -> Result<(u32, u32, Vec<u64>), SurfaceError> {
        if self.session(token)?.profile != Profile::Stepped {
            return Err(error("wrong_profile", "only a stepped session steps the world", 0));
        }
        let from = self.world.tick();
        self.advance(ticks)?;
        Ok((self.world.tick(), self.ended_streams(token)?, self.hashes(from)))
    }

    /// The host's tick of a paced world: the commands waiting in the input
    /// log are applied at the boundary in admission order, then one tick runs.
    pub fn host_tick(&mut self) -> Result<(), SurfaceError> {
        if self.log.pending > 0 {
            let tick = self.world.tick();
            let first = self.log.entries.len() - self.log.pending;
            for at in first..self.log.entries.len() {
                if self.log.entries[at].tick.is_some() {
                    continue;
                }
                let transaction = Transaction::decode(&self.log.entries[at].bytes).expect("decoded at admission");
                let outcome = self.apply(&transaction).encode();
                let entry = &mut self.log.entries[at];
                entry.tick = Some(tick);
                entry.outcome = Some(outcome);
            }
            self.log.pending = 0;
        }
        self.advance(1)
    }

    pub fn poll(&mut self, token: u64, from: u32) -> Result<(u32, u32, Vec<u64>), SurfaceError> {
        let ended = self.ended_streams(token)?;
        let from = from.min(self.world.tick());
        Ok((self.world.tick(), ended, self.hashes(from)))
    }

    /// Admits the session's next command, answers a retry from the input log,
    /// or refuses a sequence that skips ahead.
    pub fn command(&mut self, token: u64, sequence: u64, bytes: &[u8]) -> Result<CommandReply, SurfaceError> {
        let session = self.session(token)?;
        let (profile, highest) = (session.profile, session.highest_sequence);
        if sequence > highest + 1 {
            self.counters.gaps_refused += 1;
            return Err(error("sequence_gap", format!("sequence {sequence} skips ahead; the next expected is {}", highest + 1), highest + 1));
        }
        if sequence <= highest {
            let Some(entry) = self.log.find(token, sequence) else {
                self.counters.retries_outside_retention += 1;
                return Err(error("outside_retention", format!("sequence {sequence} is older than the retained input log"), sequence));
            };
            if entry.bytes != bytes {
                self.counters.altered_retries_refused += 1;
                return Err(error("altered_retry", format!("sequence {sequence} was admitted with other bytes"), sequence));
            }
            self.counters.retries_answered += 1;
            return Ok(CommandReply { retry: true, tick: entry.tick, outcome: entry.outcome.clone() });
        }
        let transaction = Transaction::decode(bytes).ok_or_else(|| error("malformed_command", "the bytes are not a transaction", sequence))?;
        self.session(token)?.highest_sequence = sequence;
        let (tick, outcome) = match profile {
            Profile::Stepped => (Some(self.world.tick()), Some(self.apply(&transaction).encode())),
            Profile::Paced => (None, None),
        };
        self.log.admit(LogEntry { session: token, sequence, bytes: bytes.to_vec(), tick, outcome: outcome.clone() });
        Ok(CommandReply { retry: false, tick, outcome })
    }

    pub fn entity_state(&self, handles: &[Handle]) -> Vec<Option<Body>> {
        let state = self.world.state();
        handles
            .iter()
            .map(|handle| {
                let slot = handle.index as usize;
                (slot < state.len() && state.alive[slot] && state.generation[slot] == handle.generation)
                    .then(|| Body { x: state.x[slot], y: state.y[slot], vx: state.vx[slot], vy: state.vy[slot], mass: state.mass[slot] })
            })
            .collect()
    }

    pub fn digests(&self) -> Digests {
        digests(self.world.state(), &self.labels)
    }

    pub fn statistics(&self) -> Statistics {
        let (extracted_ticks, extraction_micros) = self.extraction.cost();
        Statistics {
            tick: self.world.tick(),
            stepping_nanos: self.world.stepping().as_nanos() as u64,
            extracted_ticks,
            extraction_micros: extraction_micros as f32,
            log_commands: self.log.commands,
            log_bytes: self.log.bytes,
            ..self.counters
        }
    }

    /// Opens the session's data plane: a ring in a new shared memory object,
    /// or a second stream whose connection the client attaches next.
    fn open_data_plane(&mut self, token: u64, mode: u8, state: Policy, frame: Policy, connection: Connection) -> Result<Reply, SurfaceError> {
        self.session(token)?;
        let policies = [(STATE_STREAM, state), (FRAME_STREAM, frame)];
        let from = self.world.tick() + 1;
        let mut bytes = vec![0];
        let mut handoff = Handoff::None;
        let plane = match mode {
            PLANE_RING => {
                if cfg!(unix) && !connection.passes_descriptors {
                    return Err(error("shared_memory_unavailable", "this transport cannot hand a descriptor over", 0));
                }
                let failed = |failure: std::io::Error| error("shared_memory_failed", failure.to_string(), 0);
                let mapping = Mapping::create(DATA_AT + self.plane.ring_capacity).map_err(failed)?;
                put_u64(&mut bytes, mapping.length() as u64);
                put_text(&mut bytes, mapping.name());
                #[cfg(unix)]
                {
                    handoff = Handoff::Descriptor(crate::shared_memory::duplicate(&mapping).map_err(failed)?);
                }
                Plane::new(token, Sink::Ring(RingWriter::create(mapping).map_err(failed)?), policies, from, &self.plane)
            }
            PLANE_SECOND_STREAM => Plane::new(token, Sink::Unattached, policies, from, &self.plane),
            _ => return Err(error("malformed_request", "unknown data plane mode", u64::from(mode))),
        };
        self.session(token)?.plane = Some(plane.flags());
        self.extraction.enable();
        self.io.add(plane);
        let _ = &mut handoff;
        Ok(Reply { bytes, handoff })
    }

    /// Gives the session's second stream its connection, in non-blocking mode.
    pub fn attach_second_stream(&mut self, token: u64, stream: Box<dyn std::io::Write + Send>) -> bool {
        self.io.attach_socket(token, SocketSink::new(stream))
    }

    pub fn stream_status(&self, token: u64) -> Vec<StreamStatus> {
        self.io.status(token)
    }

    pub fn resume_stream(&mut self, token: u64, stream: u32, cursor: u32) -> Result<(), SurfaceError> {
        self.session(token)?;
        self.io.resume(token, stream, cursor).map_err(|code| error(code, format!("stream {stream} cannot resume at tick {cursor}"), u64::from(cursor)))
    }

    // -----------------------------------------------------------------------
    // The message envelope
    // -----------------------------------------------------------------------

    /// Answers one request message. A reply starts with a status byte: 0 and
    /// the operation's fields, or 1 and a structured error.
    pub fn handle(&mut self, request: &[u8], connection: Connection) -> Reply {
        match self.dispatch(request, connection) {
            Ok(reply) => reply,
            Err(failure) => {
                let mut bytes = vec![1];
                put_text(&mut bytes, failure.code);
                put_text(&mut bytes, &failure.message);
                put_u64(&mut bytes, failure.context);
                Reply { bytes, handoff: Handoff::None }
            }
        }
    }

    fn dispatch(&mut self, request: &[u8], connection: Connection) -> Result<Reply, SurfaceError> {
        let malformed = || error("malformed_request", "the request ends before its fields", 0);
        let mut reader = Reader::new(request);
        if reader.u32() != Some(PROTOCOL) {
            return Err(error("unknown_protocol", "the request does not start with this protocol's number", 0));
        }
        let operation = reader.u8().ok_or_else(malformed)?;
        let mut bytes = vec![0];
        let mut handoff = Handoff::None;
        match operation {
            op::OPEN => {
                let profile = if reader.u8().ok_or_else(malformed)? == 0 { Profile::Stepped } else { Profile::Paced };
                put_u64(&mut bytes, self.open(profile));
                put_u32(&mut bytes, self.world.state().len() as u32);
            }
            op::STEP | op::POLL => {
                let token = reader.u64().ok_or_else(malformed)?;
                let argument = reader.u32().ok_or_else(malformed)?;
                let (tick, ended, hashes) = if operation == op::STEP { self.step(token, argument)? } else { self.poll(token, argument)? };
                put_u32(&mut bytes, tick);
                put_u32(&mut bytes, ended);
                put_u32(&mut bytes, hashes.len() as u32);
                for hash in hashes {
                    put_u64(&mut bytes, hash);
                }
            }
            op::COMMAND => {
                let token = reader.u64().ok_or_else(malformed)?;
                let sequence = reader.u64().ok_or_else(malformed)?;
                let reply = self.command(token, sequence, reader.bytes().ok_or_else(malformed)?)?;
                put_u8(&mut bytes, u8::from(reply.retry));
                put_u8(&mut bytes, u8::from(reply.tick.is_some()));
                put_u32(&mut bytes, reply.tick.unwrap_or(0));
                put_bytes(&mut bytes, reply.outcome.as_deref().unwrap_or(&[]));
            }
            op::ENTITY_STATE => {
                let count = reader.u32().ok_or_else(malformed)? as usize;
                let handles: Option<Vec<Handle>> = (0..count.min(reader.remaining())).map(|_| Some(Handle { index: reader.u32()?, generation: reader.u32()? })).collect();
                let bodies = self.entity_state(&handles.ok_or_else(malformed)?);
                put_u32(&mut bytes, bodies.len() as u32);
                for body in bodies {
                    put_u8(&mut bytes, u8::from(body.is_some()));
                    let body = body.unwrap_or(Body { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, mass: 0.0 });
                    for value in [body.x, body.y, body.vx, body.vy, body.mass] {
                        put_f32(&mut bytes, value);
                    }
                }
            }
            op::FULL_STATE => {
                let mut state = Vec::new();
                write_canonical(self.world.state(), &mut state);
                put_u32(&mut bytes, self.world.tick());
                put_bytes(&mut bytes, &state);
            }
            op::DIGESTS => {
                let digests = self.digests();
                put_u32(&mut bytes, self.world.tick());
                put_u64(&mut bytes, digests.full);
                put_u64(&mut bytes, digests.components);
                put_u64(&mut bytes, digests.labels);
            }
            op::OPEN_DATA_PLANE => {
                let token = reader.u64().ok_or_else(malformed)?;
                let mode = reader.u8().ok_or_else(malformed)?;
                let state = reader.u8().and_then(Policy::from_tag).ok_or_else(malformed)?;
                let frame = reader.u8().and_then(Policy::from_tag).ok_or_else(malformed)?;
                return self.open_data_plane(token, mode, state, frame, connection);
            }
            op::ATTACH_SECOND_STREAM => {
                let token = reader.u64().ok_or_else(malformed)?;
                self.session(token)?;
                handoff = Handoff::SecondStream(token);
            }
            op::RESUME_STREAM => {
                let token = reader.u64().ok_or_else(malformed)?;
                let stream = reader.u32().ok_or_else(malformed)?;
                self.resume_stream(token, stream, reader.u32().ok_or_else(malformed)?)?;
            }
            op::STREAM_STATUS => {
                let streams = self.stream_status(reader.u64().ok_or_else(malformed)?);
                put_u32(&mut bytes, streams.len() as u32);
                for stream in streams {
                    put_u32(&mut bytes, stream.id);
                    put_u8(&mut bytes, stream.policy.tag());
                    put_u32(&mut bytes, stream.cursor);
                    put_text(&mut bytes, stream.ended.unwrap_or(""));
                    for value in [stream.frames_written, stream.ticks_skipped, stream.frames_evicted, stream.frames_refused, stream.pauses, stream.copy_nanos] {
                        put_u64(&mut bytes, value);
                    }
                }
            }
            op::STATISTICS => {
                let statistics = self.statistics();
                put_u32(&mut bytes, statistics.tick);
                put_f32(&mut bytes, statistics.extraction_micros);
                for value in [
                    statistics.stepping_nanos,
                    statistics.extracted_ticks,
                    statistics.log_commands,
                    statistics.log_bytes,
                    statistics.retries_answered,
                    statistics.gaps_refused,
                    statistics.altered_retries_refused,
                    statistics.retries_outside_retention,
                ] {
                    put_u64(&mut bytes, value);
                }
            }
            other => return Err(error("unknown_operation", format!("no operation has the number {other}"), u64::from(other))),
        }
        Ok(Reply { bytes, handoff })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{Element, Field, Oracle, Verdict};
    use crate::world::Size;

    fn server(workers: usize) -> Server {
        let plane = PlaneConfig { ring_capacity: 256 << 10, frame_bytes: 4096, budget: 6, retention: 16 };
        Server::new(ServerConfig { world: WorldConfig::new(Size::Small, workers), plane, log_retention: 20 })
    }

    fn set(index: u32, value: f32) -> Vec<u8> {
        Transaction::of(vec![Element::SetField { entity: Handle { index, generation: 0 }, field: Field::Vx, value }]).encode()
    }

    #[test]
    fn a_stepped_session_applies_commands_at_the_boundary_with_the_oracle_s_hashes() {
        for workers in [1, 3] {
            let mut server = server(workers);
            let mut oracle = Oracle::new(Size::Small);
            let token = server.open(Profile::Stepped);
            let mut hashes = vec![oracle.hashes[0]];
            let mut sequence = 0;
            for round in 0..8_u32 {
                let transaction = Transaction {
                    full_evaluation: false,
                    want_inverse: round % 2 == 0,
                    elements: vec![
                        Element::SetField { entity: Handle { index: 40 + round, generation: 0 }, field: Field::Mass, value: 4.0 },
                        Element::Despawn { entity: Handle { index: 60 + round, generation: 0 } },
                        Element::Spawn { body: Body { x: 5.0, y: 6.0, vx: 1.0, vy: 2.0, mass: 1.0 } },
                        Element::AddLabel { entity: Handle { index: 80 + round, generation: 0 }, value: round },
                    ],
                };
                sequence += 1;
                let reply = server.command(token, sequence, &transaction.encode()).unwrap();
                let expected = oracle.transact(&transaction);
                assert_eq!(reply.tick, Some(round * 7));
                assert_eq!(Outcome::decode(reply.outcome.as_ref().unwrap()), Some(expected));
                assert_eq!(server.digests(), oracle.digests());
                hashes.extend(server.step(token, 7).unwrap().2);
                oracle.step_to(server.tick());
            }
            assert_eq!(hashes, oracle.hashes, "{workers} workers");
            assert_eq!(server.digests(), oracle.digests());
        }
    }

    #[test]
    fn a_retry_is_answered_from_the_log_a_gap_and_an_altered_retry_are_refused() {
        let mut server = server(1);
        let token = server.open(Profile::Stepped);
        let first = server.command(token, 1, &set(3, 5.0)).unwrap();
        assert!(!first.retry && first.tick == Some(0));
        let before = server.digests();
        let retried = server.command(token, 1, &set(3, 5.0)).unwrap();
        assert_eq!(retried, CommandReply { retry: true, ..first.clone() });
        let gap = server.command(token, 3, &set(4, 5.0)).unwrap_err();
        assert_eq!((gap.code, gap.context), ("sequence_gap", 2));
        let altered = server.command(token, 1, &set(3, 6.0)).unwrap_err();
        assert_eq!(altered.code, "altered_retry");
        assert_eq!(server.digests(), before, "none of the three touched the state");
        assert_eq!(server.command(token, 2, &[9, 9]).unwrap_err().code, "malformed_command");
        assert!(server.command(token, 2, &set(4, 5.0)).is_ok(), "a malformed command does not take the sequence");
        server.step(token, 30).unwrap();
        assert_eq!(server.command(token, 1, &set(3, 5.0)).unwrap_err().code, "outside_retention");
        assert_eq!(server.command(77, 1, &set(3, 5.0)).unwrap_err().code, "unknown_session");
        let statistics = server.statistics();
        assert_eq!((statistics.log_commands, statistics.retries_answered, statistics.gaps_refused), (2, 1, 1));
        assert_eq!((statistics.altered_retries_refused, statistics.retries_outside_retention), (1, 1));
    }

    #[test]
    fn a_paced_session_s_commands_wait_for_the_host_and_their_results_come_from_the_log() {
        let mut server = server(2);
        let mut oracle = Oracle::new(Size::Small);
        let token = server.open(Profile::Paced);
        assert_eq!(server.step(token, 1).unwrap_err().code, "wrong_profile");
        for _ in 0..3 {
            server.host_tick().unwrap();
        }
        let bytes = set(3, 5.0);
        let waiting = server.command(token, 1, &bytes).unwrap();
        assert_eq!(waiting, CommandReply { retry: false, tick: None, outcome: None });
        assert_eq!(server.command(token, 1, &bytes).unwrap(), CommandReply { retry: true, tick: None, outcome: None });
        server.host_tick().unwrap();
        let answered = server.command(token, 1, &bytes).unwrap();
        assert_eq!(answered.tick, Some(3));
        let outcome = Outcome::decode(answered.outcome.as_ref().unwrap()).unwrap();
        assert!(outcome.applied && outcome.verdicts == [Verdict::Accepted]);
        oracle.step_to(3);
        oracle.transact(&Transaction::decode(&bytes).unwrap());
        oracle.step_to(4);
        let (tick, ended, hashes) = server.poll(token, 0).unwrap();
        assert_eq!((tick, ended), (4, 0));
        assert_eq!(hashes, oracle.hashes[1..]);
    }

    #[test]
    fn the_envelope_names_what_it_refuses() {
        let mut server = server(1);
        let connection = Connection { passes_descriptors: false };
        let code = |bytes: Vec<u8>| {
            let mut reader = Reader::new(&bytes);
            assert_eq!(reader.u8(), Some(1));
            reader.text().unwrap()
        };
        assert_eq!(code(server.handle(&[1, 2, 3], connection).bytes), "unknown_protocol");
        let mut request = PROTOCOL.to_le_bytes().to_vec();
        request.push(99);
        assert_eq!(code(server.handle(&request, connection).bytes), "unknown_operation");
        *request.last_mut().unwrap() = op::STEP;
        assert_eq!(code(server.handle(&request, connection).bytes), "malformed_request");
        request.extend_from_slice(&5_u64.to_le_bytes());
        request.extend_from_slice(&1_u32.to_le_bytes());
        assert_eq!(code(server.handle(&request, connection).bytes), "unknown_session");
    }
}
