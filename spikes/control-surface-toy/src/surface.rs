//! The control surface over the toy world. Throwaway, never product code.
//!
//! One contract, described as data (`describe`), with one domain command (a
//! kick on a set of entities), one query (the state of entities by index) and
//! two events (a tick completed with its hash; a command applied with the
//! outcome per entity), served to sessions of two profiles: stepped, where
//! the client owns the step call and observes in the same round trip, and
//! paced, where a host steps the world and a client's commands apply at the
//! next tick boundary. Commands are elements of a tick's ordered input record,
//! written to the input log as canonical bytes; the log replays on a fresh
//! world. Sessions survive a disconnection by their token and resume their
//! events from a cursor; a retried command with a known idempotency key is
//! answered from the record and applied once.
//!
//! The same typed operations serve the in-process surface and, through
//! `handle`, the message surface: a request as bytes in a fixed envelope, the
//! command and query parameters as name-tagged fields decoded against the
//! description, an unknown command or field rejected with a structured error
//! that names it.

use std::collections::{HashMap, VecDeque};
use std::fmt;

use crate::contract::{
    Component, DecodeError, Description, EnumType, Event, Field, Global, Handle, Operation, Reader, StructType, Type,
    Value, Writer, decode_value, encode_value,
};
use crate::world::{Kick, KickOutcome, World, WorldConfig};

pub const PROTOCOL: u32 = 1;
/// How many events the world keeps for sessions to resume from.
pub const EVENT_RETENTION: usize = 4_096;

// ---------------------------------------------------------------------------
// The description
// ---------------------------------------------------------------------------

fn field(name: &str, ty: Type, doc: &str) -> Field {
    Field { name: name.to_string(), ty, default: None, doc: doc.to_string() }
}

fn structure(name: &str, version: u32, fields: Vec<Field>) -> StructType {
    StructType { name: name.to_string(), version, fields }
}

/// The contract of the toy at a schema version. Version 2 adds `scale` to the
/// kick command with a default of 1: the one compatible change.
pub fn describe(schema_version: u32) -> Description {
    let mut kick_fields = vec![
        field("entities", Type::Sequence(Box::new(Type::Handle)), "the entities to kick, by handle"),
        field("delta_vx", Type::F32, "added to the velocity along x, then clamped to the speed limit"),
        field("delta_vy", Type::F32, "added to the velocity along y, then clamped to the speed limit"),
    ];
    if schema_version >= 2 {
        kick_fields.push(Field {
            name: "scale".to_string(),
            ty: Type::F32,
            default: Some(Value::F32(1.0)),
            doc: "multiplies both deltas; added by the compatible change, 1 when absent".to_string(),
        });
    }
    Description {
        engine: "ludenea spike".to_string(),
        schema_version,
        structs: vec![
            structure("position", 1, vec![field("x", Type::F32, ""), field("y", Type::F32, "")]),
            structure("velocity", 1, vec![field("vx", Type::F32, ""), field("vy", Type::F32, "")]),
            structure("mass", 1, vec![field("mass", Type::F32, "one of 1, 2, 4")]),
            structure("kick_parameters", schema_version.min(2), kick_fields),
            structure("kick_queued", 1, vec![field("tick", Type::U32, "the tick boundary the command applies at")]),
            structure(
                "kick_outcome",
                1,
                vec![field("handle", Type::Handle, ""), field("outcome", Type::Enum("kick_outcome_kind".to_string()), "")],
            ),
            structure("entity_state_parameters", 1, vec![field("indices", Type::Sequence(Box::new(Type::U32)), "the identifier indices to read")]),
            structure(
                "entity_row",
                1,
                vec![
                    field("handle", Type::Handle, "the current handle at that index"),
                    field("present", Type::Bool, "whether an entity lives at that index; the values below are 0 otherwise"),
                    field("x", Type::F32, ""),
                    field("y", Type::F32, ""),
                    field("vx", Type::F32, ""),
                    field("vy", Type::F32, ""),
                    field("mass", Type::F32, ""),
                ],
            ),
            structure("entity_rows", 1, vec![field("tick", Type::U32, "the tick boundary the rows were read at"), field("rows", Type::Sequence(Box::new(Type::Struct("entity_row".to_string()))), "")]),
            structure("tick_completed_event", 1, vec![field("tick", Type::U32, ""), field("hash", Type::U64, "the hash of the state after the tick")]),
            structure(
                "command_applied_event",
                1,
                vec![
                    field("key", Type::U64, "the command's idempotency key"),
                    field("tick", Type::U32, "the tick it applied at"),
                    field("outcomes", Type::Sequence(Box::new(Type::Struct("kick_outcome".to_string()))), "one per entity, in the command's order"),
                ],
            ),
        ],
        enums: vec![EnumType {
            name: "kick_outcome_kind".to_string(),
            version: 1,
            variants: vec!["applied".to_string(), "stale_handle".to_string(), "unknown_index".to_string()],
        }],
        components: ["position", "velocity", "mass"].iter().map(|name| Component { name: name.to_string(), operations: vec!["set".to_string()] }).collect(),
        globals: vec![Global { name: "tick".to_string(), ty: Type::U32, doc: "the last completed tick".to_string() }],
        commands: vec![Operation {
            name: "kick".to_string(),
            version: schema_version.min(2),
            parameters: "kick_parameters".to_string(),
            result: "kick_queued".to_string(),
            doc: "a velocity impulse on a set of entities, applied at the next tick boundary".to_string(),
        }],
        queries: vec![Operation {
            name: "entity_state".to_string(),
            version: 1,
            parameters: "entity_state_parameters".to_string(),
            result: "entity_rows".to_string(),
            doc: "the state of the entities at the given indices, at the current tick boundary".to_string(),
        }],
        events: vec![
            Event { name: "tick_completed".to_string(), version: 1, payload: "tick_completed_event".to_string(), doc: String::new() },
            Event { name: "command_applied".to_string(), version: 1, payload: "command_applied_event".to_string(), doc: String::new() },
        ],
    }
}

// ---------------------------------------------------------------------------
// Errors as data
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceError {
    pub code: String,
    pub message: String,
    pub context: Vec<(String, String)>,
}

impl SurfaceError {
    pub fn new(code: &str, message: impl Into<String>) -> SurfaceError {
        SurfaceError { code: code.to_string(), message: message.into(), context: Vec::new() }
    }

    pub fn with(mut self, key: &str, value: impl ToString) -> SurfaceError {
        self.context.push((key.to_string(), value.to_string()));
        self
    }
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        for (key, value) in &self.context {
            write!(f, "; {key}={value}")?;
        }
        Ok(())
    }
}

impl From<DecodeError> for SurfaceError {
    fn from(error: DecodeError) -> SurfaceError {
        SurfaceError::new("malformed_message", error.0)
    }
}

// ---------------------------------------------------------------------------
// Sessions, events, the input log
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Stepped,
    Paced,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Profile::Stepped => "stepped",
            Profile::Paced => "paced",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEvent {
    pub seq: u64,
    pub tick: u32,
    pub name: String,
    pub payload: Vec<u8>,
}

struct Session {
    token: u64,
    profile: Profile,
    subscriptions: Vec<String>,
    connected: bool,
    /// The idempotency keys seen, with the tick their command was queued for.
    seen: HashMap<u64, u32>,
}

/// A command accepted into the next tick's record.
struct PendingCommand {
    session: u32,
    key: u64,
    name: String,
    /// The parameters as canonical bytes by the parameter structure, defaults filled.
    parameters: Vec<u8>,
    kicks: Vec<Kick>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedCommand {
    pub session: u32,
    pub key: u64,
    pub name: String,
    pub parameters: Vec<u8>,
}

/// The per-tick input log: one record per tick, the commands in record order.
#[derive(Clone, Debug, Default)]
pub struct InputLog {
    pub records: Vec<(u32, Vec<LoggedCommand>)>,
}

impl InputLog {
    pub fn bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(self.records.len() as u32);
        for (tick, commands) in &self.records {
            w.u32(*tick);
            w.u32(commands.len() as u32);
            for c in commands {
                w.u32(c.session);
                w.u64(c.key);
                w.string(&c.name);
                w.u32(c.parameters.len() as u32);
                w.bytes(&c.parameters);
            }
        }
        w.finish()
    }

    #[cfg(test)]
    pub fn decode(bytes: &[u8]) -> Result<InputLog, DecodeError> {
        let mut r = Reader::new(bytes);
        let count = r.u32()? as usize;
        let mut records = Vec::with_capacity(count);
        for _ in 0..count {
            let tick = r.u32()?;
            let n = r.u32()? as usize;
            let mut commands = Vec::with_capacity(n);
            for _ in 0..n {
                let session = r.u32()?;
                let key = r.u64()?;
                let name = r.string()?;
                let len = r.u32()? as usize;
                let parameters = r.take(len)?.to_vec();
                commands.push(LoggedCommand { session, key, name, parameters });
            }
            records.push((tick, commands));
        }
        r.finish()?;
        Ok(InputLog { records })
    }

    pub fn command_count(&self) -> usize {
        self.records.iter().map(|(_, c)| c.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct StepResult {
    pub tick: u32,
    pub hash: u64,
    pub answers: Vec<(String, Vec<u8>)>,
    pub events: Vec<StoredEvent>,
}

pub struct Server {
    pub world: World,
    description: Description,
    description_bytes: Vec<u8>,
    sessions: Vec<Session>,
    pending: Vec<PendingCommand>,
    pub input_log: InputLog,
    events: VecDeque<StoredEvent>,
    next_seq: u64,
    pub log_bytes_written: usize,
}

impl Server {
    pub fn new(config: WorldConfig, schema_version: u32) -> Server {
        Server::over(World::new(config), schema_version)
    }

    pub fn over(world: World, schema_version: u32) -> Server {
        let description = describe(schema_version);
        let description_bytes = description.encode();
        Server {
            world,
            description,
            description_bytes,
            sessions: Vec::new(),
            pending: Vec::new(),
            input_log: InputLog::default(),
            events: VecDeque::new(),
            next_seq: 1,
            log_bytes_written: 0,
        }
    }

    #[cfg(test)]
    pub fn description(&self) -> &Description {
        &self.description
    }

    pub fn description_bytes(&self) -> &[u8] {
        &self.description_bytes
    }

    pub fn tick(&self) -> u32 {
        self.world.tick()
    }

    // --- sessions ---------------------------------------------------------

    pub fn open_session(&mut self, profile: Profile, subscriptions: Vec<String>) -> Result<(u32, u64), SurfaceError> {
        for name in &subscriptions {
            if self.description.event(name).is_none() {
                return Err(SurfaceError::new("unknown_event", format!("no event named `{name}`")).with("event", name));
            }
        }
        let id = self.sessions.len() as u32;
        // The token identifies the session across connections; its secrecy is
        // the security model's subsystem question, not this toy's.
        let token = 0x5e55_1000_0000_0000 ^ (u64::from(id) << 8) ^ u64::from(self.world.tick());
        self.sessions.push(Session { token, profile, subscriptions, connected: true, seen: HashMap::new() });
        Ok((id, token))
    }

    fn session(&self, id: u32) -> Result<&Session, SurfaceError> {
        self.sessions.get(id as usize).ok_or_else(|| SurfaceError::new("unknown_session", format!("no session {id}")).with("session", id))
    }

    /// Reconnects a session by its token; the events after `cursor` are
    /// returned by the caller's next poll. A cursor older than the retained
    /// events is a structured error.
    pub fn resume(&mut self, token: u64, cursor: u64) -> Result<u32, SurfaceError> {
        let Some(id) = self.sessions.iter().position(|s| s.token == token) else {
            return Err(SurfaceError::new("unknown_session_token", "no session has this token"));
        };
        self.check_cursor(cursor)?;
        self.sessions[id].connected = true;
        Ok(id as u32)
    }

    pub fn close(&mut self, id: u32) -> Result<(), SurfaceError> {
        self.session(id)?;
        self.sessions[id as usize].connected = false;
        Ok(())
    }

    fn check_cursor(&self, cursor: u64) -> Result<(), SurfaceError> {
        if let Some(oldest) = self.events.front()
            && cursor + 1 < oldest.seq
        {
            return Err(SurfaceError::new("events_lost", "the events after this cursor are no longer retained")
                .with("cursor", cursor)
                .with("oldest_retained", oldest.seq));
        }
        Ok(())
    }

    // --- commands ---------------------------------------------------------

    /// Accepts a command into the next tick's record. Name-tagged `fields` are
    /// completed with the parameter structure's defaults; an unknown command, an
    /// unknown field, a missing field without a default or a value of another
    /// type is a structured error naming it. A known idempotency key returns
    /// the tick recorded for it without queueing again.
    pub fn command(&mut self, session: u32, key: u64, name: &str, fields: &[(String, Value)]) -> Result<u32, SurfaceError> {
        self.session(session)?;
        if let Some(&tick) = self.sessions[session as usize].seen.get(&key) {
            return Ok(tick);
        }
        let Some(operation) = self.description.command(name) else {
            return Err(SurfaceError::new("unknown_command", format!("no command named `{name}`")).with("command", name));
        };
        let parameters = self.description.structure(&operation.parameters)?.clone();
        let values = complete_fields(&self.description, &parameters, fields)?;
        let kicks = match name {
            "kick" => kicks_from(&parameters, &values)?,
            _ => Vec::new(),
        };
        let mut w = Writer::new();
        encode_value(&mut w, &Value::Struct(values), &Type::Struct(parameters.name.clone()), &self.description)?;
        let tick = self.world.tick() + 1;
        self.pending.push(PendingCommand { session, key, name: name.to_string(), parameters: w.finish(), kicks });
        self.sessions[session as usize].seen.insert(key, tick);
        Ok(tick)
    }

    // --- stepping ---------------------------------------------------------

    /// A stepped session advances the world by `ticks`, the pending commands
    /// applying at the first tick's boundary, and observes in the same call:
    /// the events after `cursor` for its subscriptions and the answers to its
    /// queries at the new tick boundary.
    pub fn step(&mut self, session: u32, ticks: u32, cursor: u64, queries: &[(String, Vec<(String, Value)>)]) -> Result<StepResult, SurfaceError> {
        let s = self.session(session)?;
        if s.profile != Profile::Stepped {
            return Err(SurfaceError::new("wrong_profile", "only a stepped session steps the world").with("profile", s.profile.name()));
        }
        if ticks == 0 {
            return Err(SurfaceError::new("invalid_argument", "a step runs at least one tick").with("ticks", 0));
        }
        self.advance(ticks)?;
        let mut answers = Vec::with_capacity(queries.len());
        for (name, fields) in queries {
            answers.push((name.clone(), self.query(session, name, fields)?));
        }
        let events = self.events_after(session, cursor)?;
        Ok(StepResult { tick: self.world.tick(), hash: self.world.hash(), answers, events })
    }

    /// The paced host's step: one tick, the pending commands applying at it.
    pub fn tick_once(&mut self) -> Result<(), SurfaceError> {
        self.advance(1)
    }

    fn advance(&mut self, ticks: u32) -> Result<(), SurfaceError> {
        let from = self.world.tick();
        let pending = std::mem::take(&mut self.pending);
        let mut records = vec![Vec::new(); ticks as usize];
        records[0] = pending.iter().flat_map(|c| c.kicks.iter().copied()).collect();
        let outcomes = self.world.step(records).map_err(|failed| SurfaceError::new("world_failed", failed.to_string()).with("tick", failed.0.tick))?;
        // The input log: the first tick's record holds the commands, the others are empty.
        let first_tick = from + 1;
        let logged: Vec<LoggedCommand> =
            pending.iter().map(|c| LoggedCommand { session: c.session, key: c.key, name: c.name.clone(), parameters: c.parameters.clone() }).collect();
        self.log_bytes_written += 8 + logged.iter().map(|c| 4 + 8 + 4 + c.name.len() + 4 + c.parameters.len()).sum::<usize>() + 8 * (ticks as usize - 1);
        self.input_log.records.push((first_tick, logged));
        for tick in first_tick + 1..=from + ticks {
            self.input_log.records.push((tick, Vec::new()));
        }
        // The events: the commands applied at the first tick, in record order,
        // then every tick completed with its hash.
        let mut cursor = 0;
        for c in &pending {
            let count = c.kicks.len();
            let slice = &outcomes[0][cursor..cursor + count];
            cursor += count;
            let mut payload = Writer::new();
            payload.u64(c.key);
            payload.u32(first_tick);
            payload.u32(count as u32);
            for (kick, outcome) in c.kicks.iter().zip(slice) {
                payload.handle(Handle { index: kick.index, generation: kick.generation });
                payload.u32(match outcome {
                    KickOutcome::Applied => 0,
                    KickOutcome::StaleHandle => 1,
                    KickOutcome::UnknownIndex => 2,
                });
            }
            self.push_event(first_tick, "command_applied", payload.finish());
        }
        for tick in first_tick..=from + ticks {
            let mut payload = Writer::new();
            payload.u32(tick);
            payload.u64(self.world.tick_hash(tick).expect("a completed tick has a hash"));
            self.push_event(tick, "tick_completed", payload.finish());
        }
        Ok(())
    }

    fn push_event(&mut self, tick: u32, name: &str, payload: Vec<u8>) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.events.push_back(StoredEvent { seq, tick, name: name.to_string(), payload });
        while self.events.len() > EVENT_RETENTION {
            self.events.pop_front();
        }
    }

    // --- queries and events ----------------------------------------------

    /// Answers a query at the current tick boundary, as the result structure's
    /// canonical bytes.
    pub fn query(&mut self, session: u32, name: &str, fields: &[(String, Value)]) -> Result<Vec<u8>, SurfaceError> {
        self.session(session)?;
        let Some(operation) = self.description.query(name) else {
            return Err(SurfaceError::new("unknown_query", format!("no query named `{name}`")).with("query", name));
        };
        let parameters = self.description.structure(&operation.parameters)?.clone();
        let values = complete_fields(&self.description, &parameters, fields)?;
        match name {
            "entity_state" => {
                let indices = match field_value(&parameters, &values, "indices") {
                    Some(Value::Sequence(items)) => items.iter().filter_map(|v| if let Value::U32(i) = v { Some(*i) } else { None }).collect::<Vec<u32>>(),
                    _ => return Err(SurfaceError::new("invalid_argument", "`indices` must be a sequence of u32")),
                };
                let state = self.world.state();
                let mut w = Writer::with_capacity(8 + indices.len() * 29);
                w.u32(self.world.tick());
                w.u32(indices.len() as u32);
                for index in indices {
                    let slot = index as usize;
                    if slot < state.len() && state.alive[slot] {
                        w.handle(Handle { index, generation: state.generation[slot] });
                        w.bool(true);
                        for value in [state.x[slot], state.y[slot], state.vx[slot], state.vy[slot], state.mass[slot]] {
                            w.f32(value);
                        }
                    } else {
                        w.handle(Handle { index, generation: if slot < state.len() { state.generation[slot] } else { 0 } });
                        w.bool(false);
                        for _ in 0..5 {
                            w.f32(0.0);
                        }
                    }
                }
                Ok(w.finish())
            }
            _ => Err(SurfaceError::new("unknown_query", format!("no query named `{name}`")).with("query", name)),
        }
    }

    /// The events after `cursor` for the session's subscriptions.
    pub fn poll(&mut self, session: u32, cursor: u64) -> Result<Vec<StoredEvent>, SurfaceError> {
        self.session(session)?;
        self.events_after(session, cursor)
    }

    fn events_after(&self, session: u32, cursor: u64) -> Result<Vec<StoredEvent>, SurfaceError> {
        self.check_cursor(cursor)?;
        let subscriptions = &self.sessions[session as usize].subscriptions;
        Ok(self.events.iter().filter(|e| e.seq > cursor && subscriptions.iter().any(|s| *s == e.name)).cloned().collect())
    }

    #[cfg(test)]
    pub fn session_connected(&self, id: u32) -> bool {
        self.sessions.get(id as usize).is_some_and(|s| s.connected)
    }

    // --- the message surface ----------------------------------------------

    /// One request as bytes, one response as bytes: the envelope of the
    /// out-of-process surface, shared by the in-process message endpoint.
    pub fn handle(&mut self, request: &[u8]) -> Vec<u8> {
        match self.dispatch(request) {
            Ok(body) => {
                let mut w = Writer::with_capacity(1 + body.len());
                w.u8(0);
                w.bytes(&body);
                w.finish()
            }
            Err(error) => {
                let mut w = Writer::new();
                w.u8(1);
                w.string(&error.code);
                w.string(&error.message);
                w.u32(error.context.len() as u32);
                for (key, value) in &error.context {
                    w.string(key);
                    w.string(value);
                }
                w.finish()
            }
        }
    }

    fn dispatch(&mut self, request: &[u8]) -> Result<Vec<u8>, SurfaceError> {
        let mut r = Reader::new(request);
        let protocol = r.u32()?;
        if protocol != PROTOCOL {
            return Err(SurfaceError::new("unknown_protocol", format!("protocol {protocol}, this server speaks {PROTOCOL}")).with("protocol", protocol));
        }
        let kind = r.u8()?;
        let mut w = Writer::new();
        match kind {
            0 => {
                r.finish()?;
                w.bytes(&self.description_bytes);
            }
            1 => {
                let profile = match r.u8()? {
                    0 => Profile::Stepped,
                    1 => Profile::Paced,
                    other => return Err(SurfaceError::new("invalid_argument", format!("profile {other}"))),
                };
                let count = r.u32()? as usize;
                let mut subscriptions = Vec::with_capacity(count);
                for _ in 0..count {
                    subscriptions.push(r.string()?);
                }
                r.finish()?;
                let (id, token) = self.open_session(profile, subscriptions)?;
                w.u32(id);
                w.u64(token);
            }
            2 => {
                let token = r.u64()?;
                let cursor = r.u64()?;
                r.finish()?;
                let id = self.resume(token, cursor)?;
                w.u32(id);
            }
            3 => {
                let session = r.u32()?;
                let key = r.u64()?;
                let name = r.string()?;
                let fields = self.decode_tagged_fields(&mut r, &name, true)?;
                r.finish()?;
                let tick = self.command(session, key, &name, &fields)?;
                w.u32(tick);
            }
            4 => {
                let session = r.u32()?;
                let ticks = r.u32()?;
                let cursor = r.u64()?;
                let count = r.u32()? as usize;
                let mut queries = Vec::with_capacity(count);
                for _ in 0..count {
                    let name = r.string()?;
                    let fields = self.decode_tagged_fields(&mut r, &name, false)?;
                    queries.push((name, fields));
                }
                r.finish()?;
                let result = self.step(session, ticks, cursor, &queries)?;
                w.u32(result.tick);
                w.u64(result.hash);
                w.u32(result.answers.len() as u32);
                for (name, bytes) in &result.answers {
                    w.string(name);
                    w.u32(bytes.len() as u32);
                    w.bytes(bytes);
                }
                write_events(&mut w, &result.events);
            }
            5 => {
                let session = r.u32()?;
                let name = r.string()?;
                let fields = self.decode_tagged_fields(&mut r, &name, false)?;
                r.finish()?;
                let bytes = self.query(session, &name, &fields)?;
                w.u32(bytes.len() as u32);
                w.bytes(&bytes);
            }
            6 => {
                let session = r.u32()?;
                let cursor = r.u64()?;
                r.finish()?;
                let events = self.poll(session, cursor)?;
                w.u32(self.world.tick());
                write_events(&mut w, &events);
            }
            7 => {
                let session = r.u32()?;
                r.finish()?;
                self.close(session)?;
            }
            other => return Err(SurfaceError::new("unknown_request", format!("request kind {other}")).with("kind", other)),
        }
        Ok(w.finish())
    }

    /// Name-tagged fields of a command's or query's parameters: a count, then
    /// each field's name and its value by the described type. An unknown
    /// field name stops the decoding with an error that names it, since its
    /// value's length is unknown.
    fn decode_tagged_fields(&self, r: &mut Reader<'_>, operation: &str, command: bool) -> Result<Vec<(String, Value)>, SurfaceError> {
        let found = if command { self.description.command(operation) } else { self.description.query(operation) };
        let Some(op) = found else {
            let (code, kind) = if command { ("unknown_command", "command") } else { ("unknown_query", "query") };
            return Err(SurfaceError::new(code, format!("no {kind} named `{operation}`")).with(kind, operation));
        };
        let parameters = self.description.structure(&op.parameters)?;
        let count = r.u32()? as usize;
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            let name = r.string()?;
            let Some(field) = parameters.fields.iter().find(|f| f.name == name) else {
                return Err(SurfaceError::new("unknown_field", format!("`{}` has no field named `{name}`", parameters.name))
                    .with("field", &name)
                    .with("structure", &parameters.name));
            };
            let value = decode_value(r, &field.ty, &self.description)?;
            fields.push((name, value));
        }
        Ok(fields)
    }
}

fn write_events(w: &mut Writer, events: &[StoredEvent]) {
    w.u32(events.len() as u32);
    for e in events {
        w.u64(e.seq);
        w.u32(e.tick);
        w.string(&e.name);
        w.u32(e.payload.len() as u32);
        w.bytes(&e.payload);
    }
}

/// Reads the events of a step or poll response, as `write_events` wrote them.
pub fn read_events(r: &mut Reader<'_>) -> Result<Vec<StoredEvent>, DecodeError> {
    let count = r.u32()? as usize;
    let mut events = Vec::with_capacity(count);
    for _ in 0..count {
        let seq = r.u64()?;
        let tick = r.u32()?;
        let name = r.string()?;
        let len = r.u32()? as usize;
        let payload = r.take(len)?.to_vec();
        events.push(StoredEvent { seq, tick, name, payload });
    }
    Ok(events)
}

/// The values of a structure in description order from name-tagged fields:
/// a field the client did not send takes its default, a field without one is
/// a structured error naming it, a value of another type too.
pub fn complete_fields(d: &Description, s: &StructType, fields: &[(String, Value)]) -> Result<Vec<Value>, SurfaceError> {
    for (name, _) in fields {
        if !s.fields.iter().any(|f| f.name == *name) {
            return Err(SurfaceError::new("unknown_field", format!("`{}` has no field named `{name}`", s.name)).with("field", name).with("structure", &s.name));
        }
    }
    let mut values = Vec::with_capacity(s.fields.len());
    for f in &s.fields {
        let value = match fields.iter().find(|(name, _)| *name == f.name) {
            Some((_, value)) => value.clone(),
            None => match &f.default {
                Some(default) => default.clone(),
                None => return Err(SurfaceError::new("missing_field", format!("`{}` requires the field `{}`", s.name, f.name)).with("field", &f.name).with("structure", &s.name)),
            },
        };
        // A type check by a throwaway encoding.
        let mut probe = Writer::new();
        encode_value(&mut probe, &value, &f.ty, d)
            .map_err(|e| SurfaceError::new("wrong_type", e.0).with("field", &f.name).with("structure", &s.name))?;
        values.push(value);
    }
    Ok(values)
}

fn field_value<'a>(s: &StructType, values: &'a [Value], name: &str) -> Option<&'a Value> {
    s.fields.iter().position(|f| f.name == name).and_then(|i| values.get(i))
}

/// The kicks a `kick` command's parameters mean: one per entity, the deltas
/// multiplied by the scale when the structure has one.
fn kicks_from(parameters: &StructType, values: &[Value]) -> Result<Vec<Kick>, SurfaceError> {
    let f32_of = |name: &str| match field_value(parameters, values, name) {
        Some(Value::F32(v)) => Ok(*v),
        _ => Err(SurfaceError::new("invalid_argument", format!("`{name}` must be an f32")).with("field", name)),
    };
    let delta_vx = f32_of("delta_vx")?;
    let delta_vy = f32_of("delta_vy")?;
    let scale = if parameters.fields.iter().any(|f| f.name == "scale") { f32_of("scale")? } else { 1.0 };
    let Some(Value::Sequence(entities)) = field_value(parameters, values, "entities") else {
        return Err(SurfaceError::new("invalid_argument", "`entities` must be a sequence of handles").with("field", "entities"));
    };
    let mut kicks = Vec::with_capacity(entities.len());
    for entity in entities {
        let Value::Handle(h) = entity else {
            return Err(SurfaceError::new("invalid_argument", "`entities` must be a sequence of handles").with("field", "entities"));
        };
        kicks.push(Kick { index: h.index, generation: h.generation, delta_vx: delta_vx * scale, delta_vy: delta_vy * scale });
    }
    Ok(kicks)
}

/// Replays an input log on a fresh world of `config`: every record's commands
/// are decoded by the description and applied at their tick. Returns the tick
/// hashes from tick 0.
pub fn replay(log: &InputLog, config: WorldConfig, schema_version: u32) -> Result<Vec<u64>, SurfaceError> {
    let description = describe(schema_version);
    let mut world = World::new(config);
    let mut records: Vec<Vec<Kick>> = Vec::with_capacity(log.records.len());
    for (offset, (tick, commands)) in log.records.iter().enumerate() {
        if *tick != offset as u32 + 1 {
            return Err(SurfaceError::new("invalid_log", format!("record {offset} is for tick {tick}")));
        }
        let mut kicks = Vec::new();
        for c in commands {
            let Some(operation) = description.command(&c.name) else {
                return Err(SurfaceError::new("unknown_command", format!("no command named `{}` in the log", c.name)));
            };
            let parameters = description.structure(&operation.parameters)?;
            let mut r = Reader::new(&c.parameters);
            let Value::Struct(values) = decode_value(&mut r, &Type::Struct(parameters.name.clone()), &description)? else {
                return Err(SurfaceError::new("invalid_log", "a command's parameters are not a structure"));
            };
            r.finish()?;
            if c.name == "kick" {
                kicks.extend(kicks_from(parameters, &values)?);
            }
        }
        records.push(kicks);
    }
    world.step(records).map_err(|failed| SurfaceError::new("world_failed", failed.to_string()))?;
    Ok((0..=world.tick()).map(|t| world.tick_hash(t).expect("in the history")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Size, oracle};

    fn kick_fields(handles: &[Handle], dvx: f32, dvy: f32) -> Vec<(String, Value)> {
        vec![
            ("entities".to_string(), Value::Sequence(handles.iter().map(|h| Value::Handle(*h)).collect())),
            ("delta_vx".to_string(), Value::F32(dvx)),
            ("delta_vy".to_string(), Value::F32(dvy)),
        ]
    }

    fn handles(server: &Server, indices: &[u32]) -> Vec<Handle> {
        let state = server.world.state();
        indices.iter().map(|&i| Handle { index: i, generation: state.generation[i as usize] }).collect()
    }

    #[test]
    fn a_stepped_session_kicks_queries_and_observes_and_the_log_replays() {
        let config = WorldConfig::new(Size::Small, 2);
        let mut server = Server::new(config, 1);
        let (session, _) = server.open_session(Profile::Stepped, vec!["tick_completed".to_string(), "command_applied".to_string()]).unwrap();
        let mut cursor = 0;
        let mut records: Vec<Vec<Kick>> = Vec::new();
        let mut events_seen = 0;
        while server.tick() < 30 {
            let mut record = Vec::new();
            if server.tick() % 10 == 0 {
                let hs = handles(&server, &[0, 1, 2, 999]);
                let tick = server.command(session, u64::from(server.tick()), "kick", &kick_fields(&hs, 4.0, -4.0)).unwrap();
                assert_eq!(tick, server.tick() + 1);
                record = hs.iter().map(|h| Kick { index: h.index, generation: h.generation, delta_vx: 4.0, delta_vy: -4.0 }).collect();
            }
            records.push(record);
            records.extend(std::iter::repeat_n(Vec::new(), 9));
            let queries = vec![("entity_state".to_string(), vec![("indices".to_string(), Value::Sequence(vec![Value::U32(0), Value::U32(5_000)]))])];
            let result = server.step(session, 10, cursor, &queries).unwrap();
            assert_eq!(result.tick, server.tick());
            for e in &result.events {
                assert_eq!(e.seq, cursor + 1, "contiguous");
                cursor = e.seq;
                events_seen += 1;
            }
            let (name, bytes) = &result.answers[0];
            assert_eq!(name, "entity_state");
            let mut r = Reader::new(bytes);
            let rows = decode_value(&mut r, &Type::Struct("entity_rows".to_string()), server.description()).unwrap();
            let Value::Struct(fields) = rows else { panic!() };
            assert_eq!(fields[0], Value::U32(server.tick()));
            let Value::Sequence(rows) = &fields[1] else { panic!() };
            assert_eq!(rows.len(), 2);
            let Value::Struct(row) = &rows[1] else { panic!() };
            assert_eq!(row[1], Value::Bool(false), "index 5 000 is outside the small size");
        }
        assert_eq!(events_seen, 30 + 3);
        let (oracle_hashes, _) = oracle(&config, &records);
        let ours: Vec<u64> = (0..=30).map(|t| server.world.tick_hash(t).unwrap()).collect();
        assert_eq!(ours, oracle_hashes);
        let log = InputLog::decode(&server.input_log.bytes()).unwrap();
        assert_eq!(log.records.len(), 30);
        assert_eq!(log.command_count(), 3);
        assert_eq!(replay(&log, WorldConfig::new(Size::Small, 3), 1).unwrap(), oracle_hashes);
        assert_eq!(server.log_bytes_written + 4, server.input_log.bytes().len(), "the counted log bytes match the encoding");
    }

    #[test]
    fn a_known_key_is_answered_from_the_record_and_applied_once() {
        let mut server = Server::new(WorldConfig::new(Size::Small, 1), 1);
        let (session, _) = server.open_session(Profile::Stepped, vec![]).unwrap();
        let hs = handles(&server, &[0]);
        let first = server.command(session, 7, "kick", &kick_fields(&hs, 1.0, 1.0)).unwrap();
        let again = server.command(session, 7, "kick", &kick_fields(&hs, 1.0, 1.0)).unwrap();
        assert_eq!(first, again);
        assert_eq!(server.pending.len(), 1);
        server.step(session, 1, 0, &[]).unwrap();
        let (oracle_hashes, _) = oracle(&WorldConfig::new(Size::Small, 1), &[vec![Kick { index: 0, generation: 0, delta_vx: 1.0, delta_vy: 1.0 }]]);
        assert_eq!(server.world.hash(), oracle_hashes[1]);
    }

    #[test]
    fn unknown_commands_fields_and_missing_fields_are_structured_errors_and_defaults_fill() {
        let mut server = Server::new(WorldConfig::new(Size::Small, 1), 2);
        let (session, _) = server.open_session(Profile::Stepped, vec![]).unwrap();
        let hs = handles(&server, &[0]);
        let error = server.command(session, 1, "teleport", &kick_fields(&hs, 1.0, 1.0)).unwrap_err();
        assert_eq!(error.code, "unknown_command");
        assert!(error.context.contains(&("command".to_string(), "teleport".to_string())));
        let mut fields = kick_fields(&hs, 1.0, 1.0);
        fields.push(("delta_vz".to_string(), Value::F32(1.0)));
        let error = server.command(session, 2, "kick", &fields).unwrap_err();
        assert_eq!(error.code, "unknown_field");
        assert!(error.context.contains(&("field".to_string(), "delta_vz".to_string())));
        let error = server.command(session, 3, "kick", &kick_fields(&hs, 1.0, 1.0)[..2]).unwrap_err();
        assert_eq!((error.code.as_str(), error.context[0].1.as_str()), ("missing_field", "delta_vy"));
        let error = server.command(session, 4, "kick", &[("entities".to_string(), Value::U32(1)), ("delta_vx".to_string(), Value::F32(0.0)), ("delta_vy".to_string(), Value::F32(0.0))]).unwrap_err();
        assert_eq!(error.code, "wrong_type");
        // The scale defaults to 1: the version 1 fields give the unscaled kicks.
        server.command(session, 5, "kick", &kick_fields(&hs, 2.0, 0.0)).unwrap();
        assert_eq!(server.pending[0].kicks[0].delta_vx, 2.0);
        let mut scaled = kick_fields(&hs, 2.0, 0.0);
        scaled.push(("scale".to_string(), Value::F32(0.5)));
        server.command(session, 6, "kick", &scaled).unwrap();
        assert_eq!(server.pending[1].kicks[0].delta_vx, 1.0);
        let error = server.step(session, 0, 0, &[]).unwrap_err();
        assert_eq!(error.code, "invalid_argument");
        let error = server.query(session, "nothing", &[]).unwrap_err();
        assert_eq!(error.code, "unknown_query");
        assert!(server.open_session(Profile::Paced, vec!["nothing".to_string()]).is_err());
    }

    #[test]
    fn the_message_surface_round_trips_and_names_unknown_fields() {
        let mut server = Server::new(WorldConfig::new(Size::Small, 1), 1);
        let mut w = Writer::new();
        w.u32(PROTOCOL);
        w.u8(0);
        let response = server.handle(&w.finish());
        assert_eq!(response[0], 0);
        assert_eq!(Description::decode(&response[1..]).unwrap(), describe(1));
        let mut w = Writer::new();
        w.u32(PROTOCOL);
        w.u8(1);
        w.u8(0);
        w.u32(1);
        w.string("tick_completed");
        let response = server.handle(&w.finish());
        let mut r = Reader::new(&response[1..]);
        let session = r.u32().unwrap();
        let _token = r.u64().unwrap();
        // A kick with an unknown field name is rejected naming it.
        let mut w = Writer::new();
        w.u32(PROTOCOL);
        w.u8(3);
        w.u32(session);
        w.u64(1);
        w.string("kick");
        w.u32(1);
        w.string("delta_vz");
        w.f32(1.0);
        let response = server.handle(&w.finish());
        assert_eq!(response[0], 1);
        let mut r = Reader::new(&response[1..]);
        assert_eq!(r.string().unwrap(), "unknown_field");
        let message = r.string().unwrap();
        assert!(message.contains("`delta_vz`"), "{message}");
        // A wrong protocol and an unknown request kind are errors too.
        let mut w = Writer::new();
        w.u32(99);
        w.u8(0);
        assert_eq!(server.handle(&w.finish())[0], 1);
        let mut w = Writer::new();
        w.u32(PROTOCOL);
        w.u8(42);
        let response = server.handle(&w.finish());
        let mut r = Reader::new(&response[1..]);
        assert_eq!(r.string().unwrap(), "unknown_request");
    }

    #[test]
    fn a_paced_session_resumes_from_its_cursor_and_an_old_cursor_is_an_error() {
        let mut server = Server::new(WorldConfig::new(Size::Small, 1), 1);
        let (session, token) = server.open_session(Profile::Paced, vec!["tick_completed".to_string()]).unwrap();
        assert_eq!(server.step(session, 1, 0, &[]).unwrap_err().code, "wrong_profile");
        for _ in 0..5 {
            server.tick_once().unwrap();
        }
        let events = server.poll(session, 0).unwrap();
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
        server.close(session).unwrap();
        assert!(!server.session_connected(session));
        for _ in 0..3 {
            server.tick_once().unwrap();
        }
        assert_eq!(server.resume(token, 5).unwrap(), session);
        assert!(server.session_connected(session));
        let events = server.poll(session, 5).unwrap();
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [6, 7, 8]);
        assert_eq!(server.resume(0xdead, 0).unwrap_err().code, "unknown_session_token");
        for _ in 0..EVENT_RETENTION + 10 {
            server.tick_once().unwrap();
        }
        let error = server.poll(session, 8).unwrap_err();
        assert_eq!(error.code, "events_lost");
        assert!(error.context.iter().any(|(k, _)| k == "oldest_retained"));
    }
}
