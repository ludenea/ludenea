//! The scripted agent. Throwaway, never product code.
//!
//! The agent knows the request envelope, the description's format and its own
//! script: the names of the command, the query and the events it wants and
//! the names of their fields. It reads the description at session start and
//! resolves every name against it; the layout of every value it sends or
//! receives comes from the description, never from a type shared with the
//! server. Over the message surface it encodes requests itself; the typed
//! client calls the same operations in process without encoding, so that the
//! same script measures the surface with and without the message layer.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::contract::{DecodeError, Description, Handle, Reader, StructType, Type, Value, Writer, decode_value};
use crate::surface::{PROTOCOL, Profile, Server, StoredEvent, SurfaceError};
use crate::transport::Endpoint;

/// What a client of the surface can do: the typed operations, with a
/// reconnection for the clients that have a connection to lose.
pub trait Client {
    fn describe(&mut self) -> Result<Vec<u8>, ClientError>;
    fn open(&mut self, profile: Profile, subscriptions: &[&str]) -> Result<(u32, u64), ClientError>;
    fn resume(&mut self, token: u64, cursor: u64) -> Result<u32, ClientError>;
    fn command(&mut self, session: u32, key: u64, name: &str, fields: &[(String, Value)]) -> Result<u32, ClientError>;
    fn step(&mut self, session: u32, ticks: u32, cursor: u64, queries: &[(String, Vec<(String, Value)>)]) -> Result<StepAnswer, ClientError>;
    fn query(&mut self, session: u32, name: &str, fields: &[(String, Value)]) -> Result<Vec<u8>, ClientError>;
    fn poll(&mut self, session: u32, cursor: u64) -> Result<(u32, Vec<StoredEvent>), ClientError>;
    /// Drops the connection without closing the session, then connects again.
    fn reconnect(&mut self) -> Result<(), ClientError>;
    fn traffic(&self) -> (u64, u64);
}

pub struct StepAnswer {
    pub tick: u32,
    pub hash: u64,
    pub answers: Vec<(String, Vec<u8>)>,
    pub events: Vec<StoredEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// A structured error from the server.
    Surface(SurfaceError),
    Transport(String),
    Decode(String),
}

impl From<SurfaceError> for ClientError {
    fn from(error: SurfaceError) -> ClientError {
        ClientError::Surface(error)
    }
}

impl From<io::Error> for ClientError {
    fn from(error: io::Error) -> ClientError {
        ClientError::Transport(error.to_string())
    }
}

impl From<DecodeError> for ClientError {
    fn from(error: DecodeError) -> ClientError {
        ClientError::Decode(error.0)
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Surface(e) => write!(f, "{e}"),
            ClientError::Transport(e) => write!(f, "transport: {e}"),
            ClientError::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The typed client: the in-process surface without encoding
// ---------------------------------------------------------------------------

pub struct TypedClient {
    server: Arc<Mutex<Server>>,
}

impl TypedClient {
    pub fn new(server: Arc<Mutex<Server>>) -> TypedClient {
        TypedClient { server }
    }
}

impl Client for TypedClient {
    fn describe(&mut self) -> Result<Vec<u8>, ClientError> {
        Ok(self.server.lock().unwrap().description_bytes().to_vec())
    }

    fn open(&mut self, profile: Profile, subscriptions: &[&str]) -> Result<(u32, u64), ClientError> {
        Ok(self.server.lock().unwrap().open_session(profile, subscriptions.iter().map(|s| s.to_string()).collect())?)
    }

    fn resume(&mut self, token: u64, cursor: u64) -> Result<u32, ClientError> {
        Ok(self.server.lock().unwrap().resume(token, cursor)?)
    }

    fn command(&mut self, session: u32, key: u64, name: &str, fields: &[(String, Value)]) -> Result<u32, ClientError> {
        Ok(self.server.lock().unwrap().command(session, key, name, fields)?)
    }

    fn step(&mut self, session: u32, ticks: u32, cursor: u64, queries: &[(String, Vec<(String, Value)>)]) -> Result<StepAnswer, ClientError> {
        let result = self.server.lock().unwrap().step(session, ticks, cursor, queries)?;
        Ok(StepAnswer { tick: result.tick, hash: result.hash, answers: result.answers, events: result.events })
    }

    fn query(&mut self, session: u32, name: &str, fields: &[(String, Value)]) -> Result<Vec<u8>, ClientError> {
        Ok(self.server.lock().unwrap().query(session, name, fields)?)
    }

    fn poll(&mut self, session: u32, cursor: u64) -> Result<(u32, Vec<StoredEvent>), ClientError> {
        let mut server = self.server.lock().unwrap();
        let events = server.poll(session, cursor)?;
        Ok((server.tick(), events))
    }

    fn reconnect(&mut self) -> Result<(), ClientError> {
        Ok(())
    }

    fn traffic(&self) -> (u64, u64) {
        (0, 0)
    }
}

// ---------------------------------------------------------------------------
// The message client: requests as bytes over an endpoint
// ---------------------------------------------------------------------------

pub struct MessageClient {
    endpoint: Box<dyn Endpoint>,
    connect: Box<dyn FnMut() -> io::Result<Box<dyn Endpoint>> + Send>,
    description: Option<Description>,
    sent_before: u64,
    received_before: u64,
    pub reconnections: u32,
}

impl MessageClient {
    pub fn new(mut connect: Box<dyn FnMut() -> io::Result<Box<dyn Endpoint>> + Send>) -> io::Result<MessageClient> {
        let endpoint = connect()?;
        Ok(MessageClient { endpoint, connect, description: None, sent_before: 0, received_before: 0, reconnections: 0 })
    }

    fn request(&mut self, kind: u8) -> Writer {
        let mut w = Writer::new();
        w.u32(PROTOCOL);
        w.u8(kind);
        w
    }

    /// Sends a request and splits the response into its body or its error.
    fn call(&mut self, request: Vec<u8>) -> Result<Vec<u8>, ClientError> {
        let response = self.endpoint.call(&request)?;
        let mut r = Reader::new(&response);
        match r.u8()? {
            0 => Ok(response[1..].to_vec()),
            1 => {
                let code = r.string()?;
                let message = r.string()?;
                let count = r.u32()? as usize;
                let mut context = Vec::with_capacity(count);
                for _ in 0..count {
                    context.push((r.string()?, r.string()?));
                }
                Err(ClientError::Surface(SurfaceError { code, message, context }))
            }
            other => Err(ClientError::Decode(format!("response tag {other}"))),
        }
    }

    fn write_fields(&self, w: &mut Writer, operation: &str, command: bool, fields: &[(String, Value)]) -> Result<(), ClientError> {
        let Some(description) = &self.description else {
            return Err(ClientError::Decode("the description was not read yet".to_string()));
        };
        let found = if command { description.command(operation) } else { description.query(operation) };
        let parameters = match found {
            Some(op) => description.structure(&op.parameters)?,
            // An operation the description lacks: the fields are sent with
            // the types their values have, so that the server names it.
            None => &UNKNOWN,
        };
        w.u32(fields.len() as u32);
        for (name, value) in fields {
            w.string(name);
            let ty = match parameters.fields.iter().find(|f| f.name == *name) {
                Some(field) => field.ty.clone(),
                None => type_of(value),
            };
            crate::contract::encode_value(w, value, &ty, description)?;
        }
        Ok(())
    }
}

static UNKNOWN: StructType = StructType { name: String::new(), version: 0, fields: Vec::new() };

/// The type a value has on its face, for a field the description lacks.
fn type_of(value: &Value) -> Type {
    match value {
        Value::Bool(_) => Type::Bool,
        Value::U8(_) => Type::U8,
        Value::U16(_) => Type::U16,
        Value::U32(_) => Type::U32,
        Value::U64(_) => Type::U64,
        Value::I32(_) => Type::I32,
        Value::I64(_) => Type::I64,
        Value::F32(_) => Type::F32,
        Value::F64(_) => Type::F64,
        Value::String(_) => Type::String,
        Value::Handle(_) => Type::Handle,
        Value::Sequence(items) => Type::Sequence(Box::new(items.first().map_or(Type::U8, type_of))),
        Value::Optional(item) => Type::Optional(Box::new(item.as_deref().map_or(Type::U8, type_of))),
        Value::Struct(_) | Value::Enum(_) => Type::U8,
    }
}

impl Client for MessageClient {
    fn describe(&mut self) -> Result<Vec<u8>, ClientError> {
        let request = self.request(0).finish();
        let bytes = self.call(request)?;
        self.description = Some(Description::decode(&bytes)?);
        Ok(bytes)
    }

    fn open(&mut self, profile: Profile, subscriptions: &[&str]) -> Result<(u32, u64), ClientError> {
        let mut w = self.request(1);
        w.u8(match profile {
            Profile::Stepped => 0,
            Profile::Paced => 1,
        });
        w.u32(subscriptions.len() as u32);
        for s in subscriptions {
            w.string(s);
        }
        let body = self.call(w.finish())?;
        let mut r = Reader::new(&body);
        let session = r.u32()?;
        let token = r.u64()?;
        r.finish()?;
        Ok((session, token))
    }

    fn resume(&mut self, token: u64, cursor: u64) -> Result<u32, ClientError> {
        let mut w = self.request(2);
        w.u64(token);
        w.u64(cursor);
        let body = self.call(w.finish())?;
        let mut r = Reader::new(&body);
        let session = r.u32()?;
        r.finish()?;
        Ok(session)
    }

    fn command(&mut self, session: u32, key: u64, name: &str, fields: &[(String, Value)]) -> Result<u32, ClientError> {
        let mut w = self.request(3);
        w.u32(session);
        w.u64(key);
        w.string(name);
        self.write_fields(&mut w, name, true, fields)?;
        let body = self.call(w.finish())?;
        let mut r = Reader::new(&body);
        let tick = r.u32()?;
        r.finish()?;
        Ok(tick)
    }

    fn step(&mut self, session: u32, ticks: u32, cursor: u64, queries: &[(String, Vec<(String, Value)>)]) -> Result<StepAnswer, ClientError> {
        let mut w = self.request(4);
        w.u32(session);
        w.u32(ticks);
        w.u64(cursor);
        w.u32(queries.len() as u32);
        for (name, fields) in queries {
            w.string(name);
            self.write_fields(&mut w, name, false, fields)?;
        }
        let body = self.call(w.finish())?;
        let mut r = Reader::new(&body);
        let tick = r.u32()?;
        let hash = r.u64()?;
        let count = r.u32()? as usize;
        let mut answers = Vec::with_capacity(count);
        for _ in 0..count {
            let name = r.string()?;
            let len = r.u32()? as usize;
            answers.push((name, r.take(len)?.to_vec()));
        }
        let events = crate::surface::read_events(&mut r)?;
        r.finish()?;
        Ok(StepAnswer { tick, hash, answers, events })
    }

    fn query(&mut self, session: u32, name: &str, fields: &[(String, Value)]) -> Result<Vec<u8>, ClientError> {
        let mut w = self.request(5);
        w.u32(session);
        w.string(name);
        self.write_fields(&mut w, name, false, fields)?;
        let body = self.call(w.finish())?;
        let mut r = Reader::new(&body);
        let len = r.u32()? as usize;
        let bytes = r.take(len)?.to_vec();
        r.finish()?;
        Ok(bytes)
    }

    fn poll(&mut self, session: u32, cursor: u64) -> Result<(u32, Vec<StoredEvent>), ClientError> {
        let mut w = self.request(6);
        w.u32(session);
        w.u64(cursor);
        let body = self.call(w.finish())?;
        let mut r = Reader::new(&body);
        let tick = r.u32()?;
        let events = crate::surface::read_events(&mut r)?;
        r.finish()?;
        Ok((tick, events))
    }

    fn reconnect(&mut self) -> Result<(), ClientError> {
        let (sent, received) = self.endpoint.traffic();
        self.sent_before += sent;
        self.received_before += received;
        self.endpoint = (self.connect)()?;
        self.reconnections += 1;
        Ok(())
    }

    fn traffic(&self) -> (u64, u64) {
        let (sent, received) = self.endpoint.traffic();
        (self.sent_before + sent, self.received_before + received)
    }
}

// ---------------------------------------------------------------------------
// The script
// ---------------------------------------------------------------------------

/// The agent's script: what it watches, when it kicks, how it steps.
#[derive(Clone, Debug)]
pub struct Script {
    /// The identifier indices the agent watches and kicks.
    pub watched: Vec<u32>,
    /// A kick at every tick boundary that is a multiple of this.
    pub kick_every: u32,
    pub batch: u32,
    pub total_ticks: u32,
    /// The compatible change's field, set by the version 2 script only.
    pub scale: Option<f32>,
    /// A connection dropped after the command sent at this tick boundary, the
    /// acknowledgement lost, the session resumed and the command retried.
    pub disconnect_at: Option<u32>,
    /// Try an unknown command and an unknown field once, recording the errors.
    pub probe_errors: bool,
}

impl Script {
    pub fn version_1(total_ticks: u32, batch: u32) -> Script {
        Script { watched: (0..16).collect(), kick_every: 10, batch, total_ticks, scale: None, disconnect_at: None, probe_errors: false }
    }
}

/// The agent's decision from what it observed: one impulse toward the center
/// of the world for the watched entities, from their centroid.
pub fn decide(rows: &[Observed]) -> Option<(f32, f32)> {
    let present: Vec<&Observed> = rows.iter().filter(|r| r.present).collect();
    if present.is_empty() {
        return None;
    }
    let (mut sx, mut sy) = (0.0_f32, 0.0_f32);
    for r in &present {
        sx += r.x;
        sy += r.y;
    }
    let n = present.len() as f32;
    let (cx, cy) = (sx / n, sy / n);
    Some((if cx < 512.0 { 8.0 } else { -8.0 }, if cy < 512.0 { 8.0 } else { -8.0 }))
}

/// One row of the entity state query, as the agent reads it by field name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observed {
    pub handle: Handle,
    pub present: bool,
    pub x: f32,
    pub y: f32,
}

/// What the agent reports after its run.
#[derive(Clone, Debug, Default)]
pub struct AgentReport {
    /// The hash of every tick the `tick_completed` events announced, by tick.
    pub tick_hashes: Vec<(u32, u64)>,
    pub events_received: u64,
    pub events_contiguous: bool,
    pub duplicate_events: u64,
    /// Whether every step's hash equalled the hash its last `tick_completed` event announced.
    pub step_hashes_match_events: bool,
    pub commands_sent: u64,
    pub outcomes_applied: u64,
    pub outcomes_stale: u64,
    pub outcomes_unknown: u64,
    pub step_calls: u64,
    pub step_time: Duration,
    pub query_calls: u64,
    pub reconnected: bool,
    pub retried_key_answered_alike: Option<bool>,
    pub description_bytes: usize,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub probed_errors: Vec<SurfaceError>,
    pub final_tick: u32,
    pub error: Option<String>,
}

/// The positions of the fields the agent reads, resolved by name once.
struct Resolved {
    description: Description,
    row_handle: usize,
    row_present: usize,
    row_x: usize,
    row_y: usize,
    rows_field: usize,
    tick_event_tick: usize,
    tick_event_hash: usize,
    applied_outcomes: usize,
    outcome_kind: usize,
    kind_applied: u32,
    kind_stale: u32,
}

fn position(s: &StructType, field: &str) -> Result<usize, ClientError> {
    s.fields.iter().position(|f| f.name == field).ok_or_else(|| ClientError::Decode(format!("`{}` has no field `{field}`", s.name)))
}

fn resolve(description_bytes: &[u8]) -> Result<Resolved, ClientError> {
    let d = Description::decode(description_bytes)?;
    let missing = |what: &str| ClientError::Decode(format!("the description has no {what}"));
    let kick = d.command("kick").ok_or_else(|| missing("command `kick`"))?;
    let kick_parameters = d.structure(&kick.parameters)?;
    for field in ["entities", "delta_vx", "delta_vy"] {
        position(kick_parameters, field)?;
    }
    let query = d.query("entity_state").ok_or_else(|| missing("query `entity_state`"))?;
    position(d.structure(&query.parameters)?, "indices")?;
    let rows = d.structure(&query.result)?;
    let rows_field = position(rows, "rows")?;
    let Type::Sequence(row_type) = &rows.fields[rows_field].ty else { return Err(missing("sequence of rows")) };
    let Type::Struct(row_name) = row_type.as_ref() else { return Err(missing("row structure")) };
    let row = d.structure(row_name)?;
    let tick_event = d.event("tick_completed").ok_or_else(|| missing("event `tick_completed`"))?;
    let tick_payload = d.structure(&tick_event.payload)?;
    let applied_event = d.event("command_applied").ok_or_else(|| missing("event `command_applied`"))?;
    let applied_payload = d.structure(&applied_event.payload)?;
    let applied_outcomes = position(applied_payload, "outcomes")?;
    let Type::Sequence(outcome_type) = &applied_payload.fields[applied_outcomes].ty else { return Err(missing("sequence of outcomes")) };
    let Type::Struct(outcome_name) = outcome_type.as_ref() else { return Err(missing("outcome structure")) };
    let outcome = d.structure(outcome_name)?;
    let outcome_kind = position(outcome, "outcome")?;
    let Type::Enum(kind_name) = &outcome.fields[outcome_kind].ty else { return Err(missing("outcome enumeration")) };
    let kinds = d.enumeration(kind_name)?;
    let variant = |name: &str| kinds.variants.iter().position(|v| v == name).map(|i| i as u32).ok_or_else(|| missing(name));
    Ok(Resolved {
        row_handle: position(row, "handle")?,
        row_present: position(row, "present")?,
        row_x: position(row, "x")?,
        row_y: position(row, "y")?,
        rows_field,
        tick_event_tick: position(tick_payload, "tick")?,
        tick_event_hash: position(tick_payload, "hash")?,
        applied_outcomes,
        outcome_kind,
        kind_applied: variant("applied")?,
        kind_stale: variant("stale_handle")?,
        description: d,
    })
}

impl Resolved {
    fn rows(&self, bytes: &[u8]) -> Result<Vec<Observed>, ClientError> {
        let query = self.description.query("entity_state").expect("resolved");
        let mut r = Reader::new(bytes);
        let value = decode_value(&mut r, &Type::Struct(query.result.clone()), &self.description)?;
        r.finish()?;
        let Value::Struct(fields) = value else { return Err(ClientError::Decode("rows are not a structure".to_string())) };
        let Value::Sequence(rows) = &fields[self.rows_field] else { return Err(ClientError::Decode("rows are not a sequence".to_string())) };
        let mut observed = Vec::with_capacity(rows.len());
        for row in rows {
            let Value::Struct(f) = row else { return Err(ClientError::Decode("a row is not a structure".to_string())) };
            let (Value::Handle(handle), Value::Bool(present), Value::F32(x), Value::F32(y)) = (&f[self.row_handle], &f[self.row_present], &f[self.row_x], &f[self.row_y]) else {
                return Err(ClientError::Decode("a row's fields have unexpected types".to_string()));
            };
            observed.push(Observed { handle: *handle, present: *present, x: *x, y: *y });
        }
        Ok(observed)
    }

    fn payload(&self, event: &StoredEvent) -> Result<Vec<Value>, ClientError> {
        let e = self.description.event(&event.name).ok_or_else(|| ClientError::Decode(format!("unknown event `{}`", event.name)))?;
        let mut r = Reader::new(&event.payload);
        let value = decode_value(&mut r, &Type::Struct(e.payload.clone()), &self.description)?;
        r.finish()?;
        let Value::Struct(fields) = value else { return Err(ClientError::Decode("a payload is not a structure".to_string())) };
        Ok(fields)
    }
}

/// Records an event's content into the report and checks its sequence.
fn take_event(resolved: &Resolved, event: &StoredEvent, cursor: &mut u64, report: &mut AgentReport) -> Result<(), ClientError> {
    if event.seq <= *cursor {
        report.duplicate_events += 1;
    } else if event.seq != *cursor + 1 {
        report.events_contiguous = false;
    }
    *cursor = (*cursor).max(event.seq);
    report.events_received += 1;
    let fields = resolved.payload(event)?;
    match event.name.as_str() {
        "tick_completed" => {
            if let (Value::U32(tick), Value::U64(hash)) = (&fields[resolved.tick_event_tick], &fields[resolved.tick_event_hash]) {
                report.tick_hashes.push((*tick, *hash));
            }
        }
        "command_applied" => {
            if let Value::Sequence(outcomes) = &fields[resolved.applied_outcomes] {
                for outcome in outcomes {
                    if let Value::Struct(f) = outcome
                        && let Value::Enum(kind) = &f[resolved.outcome_kind]
                    {
                        if *kind == resolved.kind_applied {
                            report.outcomes_applied += 1;
                        } else if *kind == resolved.kind_stale {
                            report.outcomes_stale += 1;
                        } else {
                            report.outcomes_unknown += 1;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn kick_fields(rows: &[Observed], delta: (f32, f32), scale: Option<f32>) -> Vec<(String, Value)> {
    let entities = rows.iter().filter(|r| r.present).map(|r| Value::Handle(r.handle)).collect();
    let mut fields = vec![
        ("entities".to_string(), Value::Sequence(entities)),
        ("delta_vx".to_string(), Value::F32(delta.0)),
        ("delta_vy".to_string(), Value::F32(delta.1)),
    ];
    if let Some(scale) = scale {
        fields.push(("scale".to_string(), Value::F32(scale)));
    }
    fields
}

fn indices_fields(watched: &[u32]) -> Vec<(String, Value)> {
    vec![("indices".to_string(), Value::Sequence(watched.iter().map(|i| Value::U32(*i)).collect()))]
}

/// Runs the script through a stepped session.
pub fn run_stepped(client: &mut dyn Client, script: &Script) -> AgentReport {
    let mut report = AgentReport { events_contiguous: true, step_hashes_match_events: true, ..AgentReport::default() };
    if let Err(error) = run_stepped_inner(client, script, &mut report) {
        report.error = Some(error.to_string());
    }
    let (sent, received) = client.traffic();
    report.bytes_sent = sent;
    report.bytes_received = received;
    report
}

fn run_stepped_inner(client: &mut dyn Client, script: &Script, report: &mut AgentReport) -> Result<(), ClientError> {
    let description_bytes = client.describe()?;
    report.description_bytes = description_bytes.len();
    let resolved = resolve(&description_bytes)?;
    let (session, token) = client.open(Profile::Stepped, &["tick_completed", "command_applied"])?;
    let queries = vec![("entity_state".to_string(), indices_fields(&script.watched))];
    let mut rows = resolved.rows(&client.query(session, "entity_state", &indices_fields(&script.watched))?)?;
    report.query_calls += 1;
    let mut cursor = 0_u64;
    let mut key = 1_u64;
    let mut tick = 0_u32;
    if script.probe_errors {
        for (name, fields) in [("teleport", kick_fields(&rows, (1.0, 1.0), None)), ("kick", {
            let mut f = kick_fields(&rows, (1.0, 1.0), None);
            f.push(("delta_vz".to_string(), Value::F32(1.0)));
            f
        })] {
            match client.command(session, 0xffff_0000 + u64::from(name.len() as u32), name, &fields) {
                Err(ClientError::Surface(error)) => report.probed_errors.push(error),
                Err(other) => return Err(other),
                Ok(_) => report.probed_errors.push(SurfaceError::new("accepted", "the server accepted what it should have refused")),
            }
        }
    }
    while tick < script.total_ticks {
        if tick % script.kick_every == 0
            && let Some(delta) = decide(&rows)
        {
            let fields = kick_fields(&rows, delta, script.scale);
            let first = client.command(session, key, "kick", &fields)?;
            report.commands_sent += 1;
            if script.disconnect_at == Some(tick) {
                // The acknowledgement is treated as lost: drop the connection,
                // resume the session, retry the same key.
                client.reconnect()?;
                let resumed = client.resume(token, cursor)?;
                if resumed != session {
                    return Err(ClientError::Decode(format!("resumed as session {resumed}, opened as {session}")));
                }
                let again = client.command(session, key, "kick", &fields)?;
                report.retried_key_answered_alike = Some(again == first);
                report.reconnected = true;
            }
            key += 1;
        }
        let ticks = script.batch.min(script.total_ticks - tick);
        let stepping = Instant::now();
        let answer = client.step(session, ticks, cursor, &queries)?;
        report.step_time += stepping.elapsed();
        report.step_calls += 1;
        for event in &answer.events {
            take_event(&resolved, event, &mut cursor, report)?;
        }
        if report.tick_hashes.last().is_none_or(|&(tick, hash)| tick != answer.tick || hash != answer.hash) {
            report.step_hashes_match_events = false;
        }
        let (_, bytes) = answer.answers.first().ok_or_else(|| ClientError::Decode("no answer to the query".to_string()))?;
        rows = resolved.rows(bytes)?;
        tick = answer.tick;
    }
    report.final_tick = tick;
    Ok(())
}

/// Runs the script through a paced session: the host steps the world; the
/// agent polls, kicks when the tick crosses a multiple of the script's
/// cadence, and drops its connection once for `disconnect_for`, resuming
/// from its cursor. Returns when the world has reached the script's ticks.
pub fn run_paced(client: &mut dyn Client, script: &Script, disconnect_for: Duration) -> AgentReport {
    let mut report = AgentReport { events_contiguous: true, step_hashes_match_events: true, ..AgentReport::default() };
    if let Err(error) = run_paced_inner(client, script, disconnect_for, &mut report) {
        report.error = Some(error.to_string());
    }
    let (sent, received) = client.traffic();
    report.bytes_sent = sent;
    report.bytes_received = received;
    report
}

fn run_paced_inner(client: &mut dyn Client, script: &Script, disconnect_for: Duration, report: &mut AgentReport) -> Result<(), ClientError> {
    let description_bytes = client.describe()?;
    report.description_bytes = description_bytes.len();
    let resolved = resolve(&description_bytes)?;
    let (session, token) = client.open(Profile::Paced, &["tick_completed", "command_applied"])?;
    let mut cursor = 0_u64;
    let mut key = 1_u64;
    let mut last_kick_period = u32::MAX;
    let started = Instant::now();
    let tick = loop {
        let (tick, events) = client.poll(session, cursor)?;
        for event in &events {
            take_event(&resolved, event, &mut cursor, report)?;
        }
        if tick >= script.total_ticks {
            break tick;
        }
        let period = tick / script.kick_every;
        if period != last_kick_period {
            last_kick_period = period;
            let rows = resolved.rows(&client.query(session, "entity_state", &indices_fields(&script.watched))?)?;
            report.query_calls += 1;
            if let Some(delta) = decide(&rows) {
                client.command(session, key, "kick", &kick_fields(&rows, delta, script.scale))?;
                report.commands_sent += 1;
                key += 1;
            }
        }
        if let Some(at) = script.disconnect_at
            && tick >= at
            && !report.reconnected
        {
            client.reconnect()?;
            std::thread::sleep(disconnect_for);
            client.reconnect()?;
            let resumed = client.resume(token, cursor)?;
            if resumed != session {
                return Err(ClientError::Decode(format!("resumed as session {resumed}, opened as {session}")));
            }
            report.reconnected = true;
        }
        std::thread::sleep(Duration::from_micros(500));
        if started.elapsed() > Duration::from_secs(120) {
            return Err(ClientError::Transport("the paced host did not reach the script's ticks in two minutes".to_string()));
        }
    };
    report.final_tick = tick;
    Ok(())
}
