//! Reference workload schema spike, for the ECS research. Throwaway, never
//! product code.
//!
//! The reference workload's components (position, velocity, mass), its
//! identifiers (index and generation, alive or not) and its globals (energy,
//! the stream state) are described by a schema served as data: stable names,
//! versions, fields, types, defaults and the structural operations each
//! component admits. A client that knows nothing of the storage (`client.rs`
//! imports the contract format only) reads the description at session start,
//! then reads the state after every tick through the description alone and
//! recomputes the workload's tick hash from its view. The spike measures,
//! instead of assuming, whether that hash equals the storage's hash at every
//! tick, what serving and reading the state through the schema costs, and
//! whether a compatible change (a field added to the velocity with a default)
//! leaves the old client reading correctly while a new client reads the field,
//! including for a state saved before the change and loaded after it.

mod client;
mod contract;

use std::time::{Duration, Instant};

use client::{Client, Script, View};
use contract::{Component, DecodeError, Description, Field, Global, Handle, Operation, Reader, StructType, Type, Value, Writer, decode_value};
use reference_workload::{
    DEFAULT_TICK_COUNT, Grid, Json, Parameters, Run, SEQUENTIAL_PHASES, Size, SplitMix64, State, Timing, run_hash,
    run_sequential, tick as workload_tick, write_platform_and_build,
};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-schema";
const OPERATIONS: [&str; 1] = ["set"];
const ENTITY_OPERATIONS: [&str; 2] = ["spawn", "despawn"];

/// The spike-local value of the added field: a function of the index, so that
/// a new client has something to read and the server something to maintain.
fn spin_of(index: usize) -> f32 {
    ((index % 7) as f32) * 0.125
}

/// The description of the reference workload at a schema version. Version 2
/// adds `spin` to the velocity with a default of 0: the one compatible change.
fn describe(schema_version: u32) -> Description {
    let f32_field = |name: &str, doc: &str| Field { name: name.to_string(), ty: Type::F32, default: None, doc: doc.to_string() };
    let mut velocity_fields = vec![f32_field("vx", "velocity along x"), f32_field("vy", "velocity along y")];
    if schema_version >= 2 {
        velocity_fields.push(Field {
            name: "spin".to_string(),
            ty: Type::F32,
            default: Some(Value::F32(0.0)),
            doc: "angular velocity, added by the compatible change; 0 when absent".to_string(),
        });
    }
    let structure = |name: &str, version: u32, fields: Vec<Field>| StructType { name: name.to_string(), version, fields };
    let component = |name: &str| Component { name: name.to_string(), operations: OPERATIONS.iter().map(|o| o.to_string()).collect() };
    Description {
        engine: "ludenea spike".to_string(),
        schema_version,
        structs: vec![
            structure("position", 1, vec![f32_field("x", "abscissa in the world"), f32_field("y", "ordinate in the world")]),
            structure("velocity", schema_version.min(2), velocity_fields),
            structure("mass", 1, vec![f32_field("mass", "one of 1, 2, 4")]),
            structure(
                "identifier_slot",
                1,
                vec![
                    Field { name: "generation".to_string(), ty: Type::U32, default: None, doc: "incremented each time the index is reused".to_string() },
                    Field { name: "alive".to_string(), ty: Type::Bool, default: None, doc: "whether an entity lives at this index".to_string() },
                ],
            ),
            structure(
                "entity",
                1,
                vec![
                    Field { name: "handle".to_string(), ty: Type::Handle, default: None, doc: "index and generation".to_string() },
                    Field { name: "position".to_string(), ty: Type::Struct("position".to_string()), default: None, doc: String::new() },
                    Field { name: "velocity".to_string(), ty: Type::Struct("velocity".to_string()), default: None, doc: String::new() },
                    Field { name: "mass".to_string(), ty: Type::Struct("mass".to_string()), default: None, doc: String::new() },
                ],
            ),
            structure("world_state_parameters", 1, vec![]),
            structure(
                "world_state",
                1,
                vec![
                    Field { name: "tick".to_string(), ty: Type::U32, default: None, doc: "the completed tick".to_string() },
                    Field {
                        name: "identifiers".to_string(),
                        ty: Type::Sequence(Box::new(Type::Struct("identifier_slot".to_string()))),
                        default: None,
                        doc: "one slot per index from 0, the identifier table".to_string(),
                    },
                    Field {
                        name: "entities".to_string(),
                        ty: Type::Sequence(Box::new(Type::Struct("entity".to_string()))),
                        default: None,
                        doc: "the alive entities in canonical order (by index)".to_string(),
                    },
                    Field { name: "energy".to_string(), ty: Type::F32, default: None, doc: "the tick's kinetic energy".to_string() },
                    Field { name: "random_stream".to_string(), ty: Type::U64, default: None, doc: "the state of the global stream".to_string() },
                ],
            ),
        ],
        enums: vec![],
        components: vec![component("position"), component("velocity"), component("mass")],
        globals: vec![
            Global { name: "energy".to_string(), ty: Type::F32, doc: "the tick's kinetic energy".to_string() },
            Global { name: "random_stream".to_string(), ty: Type::U64, doc: "the global stream's state".to_string() },
            Global {
                name: "entity_operations".to_string(),
                ty: Type::Sequence(Box::new(Type::String)),
                doc: format!("the operations on entities: {}", ENTITY_OPERATIONS.join(", ")),
            },
        ],
        commands: vec![],
        queries: vec![Operation {
            name: "world_state".to_string(),
            version: 1,
            parameters: "world_state_parameters".to_string(),
            result: "world_state".to_string(),
            doc: "the whole state at the tick boundary".to_string(),
        }],
        events: vec![],
    }
}

/// The server: the reference workload's storage (identifier-indexed slots as
/// structures of arrays), stepped by the sequential reference, with a spin
/// column when the schema version has the added field. The client never sees
/// this type: it is served the bytes of `encode_state` and nothing else.
struct Server {
    schema_version: u32,
    description: Description,
    state: State,
    grid: Grid,
    spin: Vec<f32>,
    /// The generations before the last tick, to find the slots it respawned.
    previous_generation: Vec<u32>,
    tick: u32,
    wander: bool,
    timing: Timing,
}

impl Server {
    fn new(parameters: &Parameters, schema_version: u32) -> Server {
        let state = State::initialize(parameters);
        let spin = if schema_version >= 2 { (0..state.len()).map(spin_of).collect() } else { Vec::new() };
        Server {
            schema_version,
            description: describe(schema_version),
            previous_generation: state.generation.clone(),
            state,
            grid: Grid::new(),
            spin,
            tick: 0,
            wander: parameters.wander,
            timing: Timing::new(&SEQUENTIAL_PHASES),
        }
    }

    fn description_bytes(&self) -> Vec<u8> {
        self.description.encode()
    }

    /// One tick of the sequential reference; a respawned entity gets the spin
    /// of its index, since the fixture has no other source for it.
    fn step(&mut self) {
        self.tick += 1;
        workload_tick(&mut self.state, &mut self.grid, self.tick, self.wander, &mut self.timing);
        if self.schema_version >= 2 {
            for slot in 0..self.state.len() {
                if self.state.generation[slot] != self.previous_generation[slot] {
                    // A respawned slot gets its index's spin; a loaded default
                    // stays until the slot turns over.
                    self.spin[slot] = spin_of(slot);
                }
            }
        }
        self.previous_generation.copy_from_slice(&self.state.generation);
    }

    fn storage_hash(&self) -> u64 {
        self.state.hash()
    }

    /// The hash of the spin column over the alive entities in canonical order,
    /// for the comparison with the new client's reading.
    fn spin_hash(&self) -> u64 {
        let mut hasher = reference_workload::Fnv1a::new();
        for slot in 0..self.state.len() {
            if self.state.alive[slot] {
                hasher.write(&self.spin[slot].to_le_bytes());
            }
        }
        hasher.finish()
    }

    /// The `world_state` value at the tick boundary, as canonical bytes in the
    /// description's field order. Hand-encoded for speed; the self-check
    /// verifies that it equals the generic encoding of the same value.
    fn encode_state(&self) -> Vec<u8> {
        let n = self.state.len();
        let alive = self.state.alive.iter().filter(|&&a| a).count();
        let row = 8 + 8 + 8 + 4 + if self.schema_version >= 2 { 4 } else { 0 };
        let mut w = Writer::with_capacity(4 + 4 + n * 5 + 4 + alive * row + 4 + 8);
        w.u32(self.tick);
        w.u32(n as u32);
        for slot in 0..n {
            w.u32(self.state.generation[slot]);
            w.bool(self.state.alive[slot]);
        }
        w.u32(alive as u32);
        for slot in 0..n {
            if self.state.alive[slot] {
                w.handle(Handle { index: slot as u32, generation: self.state.generation[slot] });
                w.f32(self.state.x[slot]);
                w.f32(self.state.y[slot]);
                w.f32(self.state.vx[slot]);
                w.f32(self.state.vy[slot]);
                if self.schema_version >= 2 {
                    w.f32(self.spin[slot]);
                }
                w.f32(self.state.mass[slot]);
            }
        }
        w.f32(self.state.energy);
        w.u64(self.state.rng.state);
        w.finish()
    }

    /// The same state as a value tree, for the self-check of the hand encoder.
    #[cfg(test)]
    fn state_value(&self) -> Value {
        let n = self.state.len();
        let identifiers = (0..n).map(|slot| Value::Struct(vec![Value::U32(self.state.generation[slot]), Value::Bool(self.state.alive[slot])])).collect();
        let entities = (0..n)
            .filter(|&slot| self.state.alive[slot])
            .map(|slot| {
                let mut velocity = vec![Value::F32(self.state.vx[slot]), Value::F32(self.state.vy[slot])];
                if self.schema_version >= 2 {
                    velocity.push(Value::F32(self.spin[slot]));
                }
                Value::Struct(vec![
                    Value::Handle(Handle { index: slot as u32, generation: self.state.generation[slot] }),
                    Value::Struct(vec![Value::F32(self.state.x[slot]), Value::F32(self.state.y[slot])]),
                    Value::Struct(velocity),
                    Value::Struct(vec![Value::F32(self.state.mass[slot])]),
                ])
            })
            .collect();
        Value::Struct(vec![
            Value::U32(self.tick),
            Value::Sequence(identifiers),
            Value::Sequence(entities),
            Value::F32(self.state.energy),
            Value::U64(self.state.rng.state),
        ])
    }

    /// A server at `target_version` loaded from a `world_state` saved under
    /// `source`: the bytes are decoded by the source description, a field the
    /// target has and the source lacks takes the target's default.
    fn load(source: &Description, bytes: &[u8], target_version: u32, wander: bool) -> Result<Server, DecodeError> {
        let target = describe(target_version);
        let mut r = Reader::new(bytes);
        let value = decode_value(&mut r, &Type::Struct("world_state".to_string()), source)?;
        r.finish()?;
        let world = source.structure("world_state")?;
        let field = |name: &str| -> Result<&Value, DecodeError> {
            let Value::Struct(fields) = &value else { return Err(DecodeError("world_state is not a structure".to_string())) };
            match world.fields.iter().position(|f| f.name == name) {
                Some(index) => Ok(&fields[index]),
                None => Err(DecodeError(format!("`{name}` is not in the saved world_state"))),
            }
        };
        let Value::U32(tick) = field("tick")? else { return Err(DecodeError("tick".to_string())) };
        let Value::Sequence(identifiers) = field("identifiers")? else { return Err(DecodeError("identifiers".to_string())) };
        let Value::Sequence(entities) = field("entities")? else { return Err(DecodeError("entities".to_string())) };
        let Value::F32(energy) = field("energy")? else { return Err(DecodeError("energy".to_string())) };
        let Value::U64(random_stream) = field("random_stream")? else { return Err(DecodeError("random_stream".to_string())) };
        let n = identifiers.len();
        let mut state = State {
            generation: vec![0; n],
            alive: vec![false; n],
            x: vec![0.0; n],
            y: vec![0.0; n],
            vx: vec![0.0; n],
            vy: vec![0.0; n],
            mass: vec![0.0; n],
            rng: SplitMix64::new(*random_stream),
            energy: *energy,
        };
        for (slot, identifier) in identifiers.iter().enumerate() {
            let Value::Struct(fields) = identifier else { return Err(DecodeError("identifier_slot".to_string())) };
            let id = source.structure("identifier_slot")?;
            for (f, v) in id.fields.iter().zip(fields) {
                match (f.name.as_str(), v) {
                    ("generation", Value::U32(g)) => state.generation[slot] = *g,
                    ("alive", Value::Bool(a)) => state.alive[slot] = *a,
                    _ => {}
                }
            }
        }
        let spin_default = match target.structure("velocity")?.fields.iter().find(|f| f.name == "spin").and_then(|f| f.default.clone()) {
            Some(Value::F32(v)) => Some(v),
            _ => None,
        };
        let mut spin = if target_version >= 2 { vec![spin_default.unwrap_or(0.0); n] } else { Vec::new() };
        let entity_type = source.structure("entity")?;
        for entity in entities {
            let Value::Struct(fields) = entity else { return Err(DecodeError("entity".to_string())) };
            let mut slot = None;
            for (f, v) in entity_type.fields.iter().zip(fields) {
                if f.name == "handle"
                    && let Value::Handle(h) = v
                {
                    slot = Some(h.index as usize);
                }
            }
            let Some(slot) = slot else { return Err(DecodeError("an entity without a handle".to_string())) };
            for (f, v) in entity_type.fields.iter().zip(fields) {
                let Value::Struct(inner) = v else { continue };
                let Ok(component) = source.structure(match &f.ty {
                    Type::Struct(name) => name,
                    _ => continue,
                }) else {
                    continue;
                };
                for (cf, cv) in component.fields.iter().zip(inner) {
                    match (f.name.as_str(), cf.name.as_str(), cv) {
                        ("position", "x", Value::F32(x)) => state.x[slot] = *x,
                        ("position", "y", Value::F32(y)) => state.y[slot] = *y,
                        ("velocity", "vx", Value::F32(vx)) => state.vx[slot] = *vx,
                        ("velocity", "vy", Value::F32(vy)) => state.vy[slot] = *vy,
                        ("velocity", "spin", Value::F32(s)) if target_version >= 2 => spin[slot] = *s,
                        ("mass", "mass", Value::F32(m)) => state.mass[slot] = *m,
                        _ => {}
                    }
                }
            }
        }
        Ok(Server {
            schema_version: target_version,
            description: target,
            previous_generation: state.generation.clone(),
            state,
            grid: Grid::new(),
            spin,
            tick: *tick,
            wander,
            timing: Timing::new(&SEQUENTIAL_PHASES),
        })
    }
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// One client reading one server at every tick boundary from `from` to `to`.
struct Reading {
    client_hashes: Vec<u64>,
    storage_hashes: Vec<u64>,
    mismatching_ticks: Vec<u32>,
    inconsistent_ticks: u64,
    spin_mismatching_ticks: u64,
    bytes_per_state: usize,
    description_bytes: usize,
    encode_time: Duration,
    read_time: Duration,
    storage_hash_time: Duration,
    stepping_time: Duration,
    last_view: Option<View>,
    error: Option<String>,
}

fn read_every_tick(server: &mut Server, client: &Client, to: u32) -> Reading {
    let mut reading = Reading {
        client_hashes: Vec::new(),
        storage_hashes: Vec::new(),
        mismatching_ticks: Vec::new(),
        inconsistent_ticks: 0,
        spin_mismatching_ticks: 0,
        bytes_per_state: 0,
        description_bytes: server.description_bytes().len(),
        encode_time: Duration::ZERO,
        read_time: Duration::ZERO,
        storage_hash_time: Duration::ZERO,
        stepping_time: Duration::ZERO,
        last_view: None,
        error: None,
    };
    loop {
        let encoding = Instant::now();
        let bytes = server.encode_state();
        reading.encode_time += encoding.elapsed();
        reading.bytes_per_state = bytes.len();
        let hashing = Instant::now();
        let storage_hash = server.storage_hash();
        reading.storage_hash_time += hashing.elapsed();
        let reading_at = Instant::now();
        let view = match client.read(&bytes) {
            Ok(view) => view,
            Err(error) => {
                reading.error = Some(error.0);
                break;
            }
        };
        reading.read_time += reading_at.elapsed();
        reading.storage_hashes.push(storage_hash);
        reading.client_hashes.push(view.hash);
        if view.hash != storage_hash {
            reading.mismatching_ticks.push(server.tick);
        }
        if !view.handles_consistent || view.tick != server.tick {
            reading.inconsistent_ticks += 1;
        }
        if let Some(spin_hash) = view.spin_hash
            && spin_hash != server.spin_hash()
        {
            reading.spin_mismatching_ticks += 1;
        }
        reading.last_view = Some(view);
        if server.tick >= to {
            break;
        }
        let stepping = Instant::now();
        server.step();
        reading.stepping_time += stepping.elapsed();
    }
    reading
}

fn write_reading(out: &mut Json, reading: &Reading, reference: Option<&Run>, ticks_read: u32) {
    let client_run_hash = run_hash(&reading.client_hashes);
    out.u64("ticks_read", u64::from(ticks_read));
    out.hex("client_run_hash", client_run_hash);
    out.hex("storage_run_hash", run_hash(&reading.storage_hashes));
    out.bool("client_hashes_match_storage", reading.mismatching_ticks.is_empty() && reading.error.is_none());
    out.u64("mismatching_ticks", reading.mismatching_ticks.len() as u64);
    out.i64("first_mismatching_tick", reading.mismatching_ticks.first().map_or(-1, |t| i64::from(*t)));
    out.u64("inconsistent_ticks", reading.inconsistent_ticks);
    if let Some(reference) = reference {
        out.hex("reference_run_hash", reference.run_hash);
        let from = reference.tick_hashes.len() - reading.client_hashes.len();
        let divergence = reference_workload::first_divergent_tick(&reading.client_hashes, &reference.tick_hashes[from..]);
        out.bool("client_matches_reference", divergence.is_none());
        out.i64("first_divergent_tick", divergence.map_or(-1, |t| (t + from) as i64));
    }
    if let Some(view) = &reading.last_view {
        out.u64("identifiers", view.identifier_count as u64);
        out.u64("alive_entities", view.entity_count as u64);
        match view.spin_hash {
            Some(hash) => {
                out.hex("client_spin_hash", hash);
                out.bool("spin_matches_storage", reading.spin_mismatching_ticks == 0);
            }
            None => out.string("client_spin_hash", "not read by this script"),
        }
    }
    out.u64("description_bytes", reading.description_bytes as u64);
    out.u64("state_bytes", reading.bytes_per_state as u64);
    let reads = reading.client_hashes.len().max(1) as f64;
    out.decimal("encode_us_per_read", reading.encode_time.as_secs_f64() * 1e6 / reads);
    out.decimal("client_read_us_per_read", reading.read_time.as_secs_f64() * 1e6 / reads);
    out.decimal("storage_hash_us_per_read", reading.storage_hash_time.as_secs_f64() * 1e6 / reads);
    out.decimal("tick_us", reading.stepping_time.as_secs_f64() * 1e6 / f64::from(ticks_read.max(1)));
    if let Some(error) = &reading.error {
        out.string("error", error);
    }
}

fn write_description_summary(out: &mut Json, key: &str, description: &Description) {
    out.key(key);
    out.object_start();
    out.u64("schema_version", u64::from(description.schema_version));
    out.u64("bytes", description.encode().len() as u64);
    out.u64("structures", description.structs.len() as u64);
    out.u64("fields", description.structs.iter().map(|s| s.fields.len() as u64).sum());
    out.u64("fields_with_defaults", description.structs.iter().flat_map(|s| s.fields.iter()).filter(|f| f.default.is_some()).count() as u64);
    out.u64("components", description.components.len() as u64);
    out.u64("globals", description.globals.len() as u64);
    out.u64("queries", description.queries.len() as u64);
    out.key("velocity_fields");
    out.array_start();
    for field in &description.structure("velocity").expect("velocity is described").fields {
        out.object_start();
        out.string("name", &field.name);
        out.string("type", &format!("{:?}", field.ty));
        out.bool("has_default", field.default.is_some());
        out.object_end();
    }
    out.array_end();
    out.object_end();
}

struct Scenario {
    size: Size,
    ticks: u32,
    save_tick: u32,
}

const MEASURED: Scenario = Scenario { size: Size::Default, ticks: DEFAULT_TICK_COUNT, save_tick: 500 };

fn report(out: &mut Json, scenario: &Scenario) {
    let parameters = Parameters::new(scenario.size, scenario.ticks, false);
    let reference = run_sequential(&parameters);
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", scenario.size.name());
    out.u64("ticks", u64::from(scenario.ticks));
    out.hex("reference_run_hash", reference.run_hash);
    out.decimal("reference_ticks_per_second", reference.ticks_per_second());
    write_description_summary(out, "description_v1", &describe(1));
    write_description_summary(out, "description_v2", &describe(2));

    // Every tick read through the schema, by schema version and script.
    out.key("readings");
    out.array_start();
    for (schema_version, script) in [(1, Script::Version1), (2, Script::Version1), (2, Script::Version2)] {
        let mut server = Server::new(&parameters, schema_version);
        let client = Client::new(&server.description_bytes(), script).expect("the script resolves against the description");
        let reading = read_every_tick(&mut server, &client, scenario.ticks);
        out.object_start();
        out.u64("server_schema_version", u64::from(schema_version));
        out.string("client_script", script.name());
        write_reading(out, &reading, Some(&reference), scenario.ticks);
        out.key("operations_seen_by_the_client");
        out.array_start();
        for line in client.operations() {
            out.object_start();
            out.string("component", &line);
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();

    // A state saved under version 1 at the save tick, loaded by a version 2
    // server, read by both scripts to the end.
    out.key("saved_under_v1_loaded_under_v2");
    out.object_start();
    let mut v1 = Server::new(&parameters, 1);
    while v1.tick < scenario.save_tick {
        v1.step();
    }
    let saved = v1.encode_state();
    let source = describe(1);
    out.u64("save_tick", u64::from(scenario.save_tick));
    out.u64("saved_bytes", saved.len() as u64);
    match Server::load(&source, &saved, 2, false) {
        Ok(loaded) => {
            out.bool("loaded", true);
            out.bool("hash_at_load_matches_saved", loaded.storage_hash() == v1.storage_hash());
            out.bool("hash_at_load_matches_reference", loaded.storage_hash() == reference.tick_hashes[scenario.save_tick as usize]);
            out.hex("spin_hash_at_load", loaded.spin_hash());
            out.bool("spin_is_the_default_at_load", loaded.spin.iter().all(|&s| s == 0.0));
            let description_bytes = loaded.description_bytes();
            out.key("by_script");
            out.array_start();
            for script in [Script::Version1, Script::Version2] {
                let mut server = Server::load(&source, &saved, 2, false).expect("loads twice");
                let client = Client::new(&description_bytes, script).expect("the script resolves");
                let reading = read_every_tick(&mut server, &client, scenario.ticks);
                out.object_start();
                out.string("client_script", script.name());
                write_reading(out, &reading, Some(&reference), scenario.ticks - scenario.save_tick);
                out.object_end();
            }
            out.array_end();
        }
        Err(error) => {
            out.bool("loaded", false);
            out.string("error", &error.0);
        }
    }
    out.object_end();

    // The breaking direction: the new script against the old description.
    out.key("new_script_against_old_description");
    out.object_start();
    match Client::new(&describe(1).encode(), Script::Version2) {
        Ok(_) => out.bool("rejected", false),
        Err(error) => {
            out.bool("rejected", true);
            out.string("error", &error.0);
        }
    }
    out.object_end();
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    report(&mut out, &MEASURED);
    out.object_end();
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::encode_value;

    fn small() -> Parameters {
        Parameters::new(Size::Small, 30, false)
    }

    #[test]
    fn the_hand_encoder_equals_the_generic_encoding() {
        for schema_version in [1, 2] {
            let mut server = Server::new(&small(), schema_version);
            for _ in 0..5 {
                server.step();
            }
            // Dead slots at a tick boundary: the specified kills are refilled by
            // the spawns, so two are killed by hand to exercise that path.
            server.state.alive[1] = false;
            server.state.alive[7] = false;
            let mut w = Writer::new();
            encode_value(&mut w, &server.state_value(), &Type::Struct("world_state".to_string()), &server.description).unwrap();
            assert_eq!(server.encode_state(), w.finish(), "schema version {schema_version}");
            let client = Client::new(&server.description_bytes(), Script::Version1).unwrap();
            let view = client.read(&server.encode_state()).unwrap();
            assert_eq!(view.hash, server.storage_hash(), "dead slots hash as the storage does");
            assert_eq!(view.entity_count + 2, view.identifier_count);
            assert!(view.handles_consistent);
        }
    }

    #[test]
    fn the_client_recomputes_the_storage_hash_at_every_tick_for_every_version_and_script() {
        let parameters = small();
        let reference = run_sequential(&parameters);
        for (schema_version, script) in [(1, Script::Version1), (2, Script::Version1), (2, Script::Version2)] {
            let mut server = Server::new(&parameters, schema_version);
            let client = Client::new(&server.description_bytes(), script).unwrap();
            let reading = read_every_tick(&mut server, &client, parameters.tick_count);
            assert_eq!(reading.error, None);
            assert!(reading.mismatching_ticks.is_empty(), "{schema_version} {script:?}");
            assert_eq!(reading.inconsistent_ticks, 0);
            assert_eq!(reading.client_hashes, reference.tick_hashes);
            assert_eq!(reading.spin_mismatching_ticks, 0);
            let view = reading.last_view.unwrap();
            assert_eq!(view.spin_hash.is_some(), script == Script::Version2);
            assert_eq!(view.entity_count, view.identifier_count, "the spawns refill every killed slot at the boundary");
        }
    }

    #[test]
    fn a_state_saved_under_v1_loads_under_v2_with_the_default_and_keeps_the_hashes() {
        let parameters = small();
        let reference = run_sequential(&parameters);
        let mut v1 = Server::new(&parameters, 1);
        for _ in 0..10 {
            v1.step();
        }
        let saved = v1.encode_state();
        let mut loaded = Server::load(&describe(1), &saved, 2, false).unwrap();
        assert_eq!(loaded.tick, 10);
        assert_eq!(loaded.storage_hash(), reference.tick_hashes[10]);
        assert!(loaded.spin.iter().all(|&s| s == 0.0), "the added field takes its default");
        let client = Client::new(&loaded.description_bytes(), Script::Version2).unwrap();
        let reading = read_every_tick(&mut loaded, &client, 30);
        assert_eq!(reading.client_hashes, reference.tick_hashes[10..]);
        assert!(loaded.spin.iter().enumerate().any(|(slot, &s)| s != 0.0 && s == spin_of(slot)), "respawned slots get a spin");
        // A version 2 save loads under version 2 with its spin values.
        let mut v2 = Server::new(&parameters, 2);
        v2.step();
        let reloaded = Server::load(&describe(2), &v2.encode_state(), 2, false).unwrap();
        assert_eq!(reloaded.spin_hash(), v2.spin_hash());
        assert_eq!(reloaded.storage_hash(), v2.storage_hash());
    }

    #[test]
    fn the_new_script_is_refused_by_the_old_description_naming_the_field() {
        let error = Client::new(&describe(1).encode(), Script::Version2).unwrap_err();
        assert!(error.0.contains("`spin`"), "{error}");
        assert!(Client::new(&describe(2).encode(), Script::Version1).is_ok());
        let mut damaged = describe(2);
        damaged.structs.iter_mut().find(|s| s.name == "position").unwrap().fields[0].ty = Type::F64;
        let error = Client::new(&damaged.encode(), Script::Version1).unwrap_err();
        assert!(error.0.contains("`x`") && error.0.contains("F64"), "{error}");
    }

    #[test]
    fn the_description_round_trips_and_the_report_runs_on_the_small_size() {
        for version in [1, 2] {
            let d = describe(version);
            assert_eq!(Description::decode(&d.encode()).unwrap(), d);
        }
        let mut out = Json::new();
        out.object_start();
        report(&mut out, &Scenario { size: Size::Small, ticks: 20, save_tick: 10 });
        out.object_end();
        let json = out.finish();
        assert!(!json.contains("\"client_hashes_match_storage\":false"), "{json}");
        assert!(!json.contains("\"client_matches_reference\":false"), "{json}");
        assert!(!json.contains("\"spin_matches_storage\":false"), "{json}");
        assert!(json.contains("\"loaded\":true") && json.contains("\"spin_is_the_default_at_load\":true"), "{json}");
        assert!(json.contains("\"rejected\":true"), "{json}");
    }
}
