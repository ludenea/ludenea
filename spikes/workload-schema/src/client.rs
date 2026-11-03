//! A client that knows nothing of the storage. Throwaway, never product code.
//!
//! It imports the contract format only, never the workload crate: what it
//! knows about the world is the description it reads at session start and its
//! own script, the names of the structures and fields it wants and the byte
//! order of the hash it recomputes. Every field it does not know is skipped by
//! its described type, which is what makes a field added with a default a
//! compatible change for this client.

use crate::contract::{DecodeError, Description, Reader, Type, skip_value};

/// The client's script: version 1 knows the five specified values, version 2
/// also reads the field the compatible change added to the velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Version1,
    Version2,
}

impl Script {
    pub fn name(self) -> &'static str {
        match self {
            Script::Version1 => "version-1",
            Script::Version2 => "version-2",
        }
    }
}

/// A field the script reads, wherever the description puts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Tick,
    Identifiers,
    Entities,
    Energy,
    RandomStream,
    Generation,
    Alive,
    Handle,
    Position,
    Velocity,
    Mass,
    X,
    Y,
    Vx,
    Vy,
    Spin,
    MassValue,
}

/// One field of a structure, in description order: read into a slot, or skipped.
#[derive(Clone, Debug)]
enum Step {
    Read(Slot),
    Skip(Type),
}

#[derive(Clone, Debug, Default)]
struct StructPlan {
    steps: Vec<Step>,
}

/// The values of one entity (or one identifier slot, or the world's globals)
/// as the script reads them.
#[derive(Clone, Copy, Debug, Default)]
struct Scratch {
    tick: u32,
    energy: f32,
    random_stream: u64,
    generation: u32,
    alive: bool,
    handle: (u32, u32),
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    spin: f32,
    mass: f32,
}

/// What one read of the state gives the client.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub tick: u32,
    /// The workload's tick hash, recomputed from the described values.
    pub hash: u64,
    /// The hash of the added field over the alive entities, for the script that reads it.
    pub spin_hash: Option<u64>,
    pub identifier_count: usize,
    pub entity_count: usize,
    /// Whether every entity's handle matched its identifier slot's index and generation.
    pub handles_consistent: bool,
}

#[derive(Debug)]
pub struct Client {
    description: Description,
    script: Script,
    world: StructPlan,
    identifier: StructPlan,
    entity: StructPlan,
    position: StructPlan,
    velocity: StructPlan,
    mass: StructPlan,
}

/// FNV-1a 64, the client's own: it shares no code with the storage.
fn fnv1a(hash: u64, bytes: &[u8]) -> u64 {
    let mut hash = hash;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

impl Client {
    /// Reads the description and resolves the script's names against it. A
    /// name the description lacks, or a field of another type than the script
    /// expects, is an error naming the field: the breaking direction.
    pub fn new(description_bytes: &[u8], script: Script) -> Result<Client, DecodeError> {
        let description = Description::decode(description_bytes)?;
        let world = plan(
            &description,
            "world_state",
            &[
                ("tick", Type::U32, Slot::Tick),
                ("identifiers", Type::Sequence(Box::new(Type::Struct("identifier_slot".to_string()))), Slot::Identifiers),
                ("entities", Type::Sequence(Box::new(Type::Struct("entity".to_string()))), Slot::Entities),
                ("energy", Type::F32, Slot::Energy),
                ("random_stream", Type::U64, Slot::RandomStream),
            ],
        )?;
        let identifier = plan(&description, "identifier_slot", &[("generation", Type::U32, Slot::Generation), ("alive", Type::Bool, Slot::Alive)])?;
        let entity = plan(
            &description,
            "entity",
            &[
                ("handle", Type::Handle, Slot::Handle),
                ("position", Type::Struct("position".to_string()), Slot::Position),
                ("velocity", Type::Struct("velocity".to_string()), Slot::Velocity),
                ("mass", Type::Struct("mass".to_string()), Slot::Mass),
            ],
        )?;
        let position = plan(&description, "position", &[("x", Type::F32, Slot::X), ("y", Type::F32, Slot::Y)])?;
        let mut velocity_fields = vec![("vx", Type::F32, Slot::Vx), ("vy", Type::F32, Slot::Vy)];
        if script == Script::Version2 {
            velocity_fields.push(("spin", Type::F32, Slot::Spin));
        }
        let velocity = plan(&description, "velocity", &velocity_fields)?;
        let mass = plan(&description, "mass", &[("mass", Type::F32, Slot::MassValue)])?;
        Ok(Client { description, script, world, identifier, entity, position, velocity, mass })
    }

    /// The structural operations the description says the components admit,
    /// as "component: operation, operation" lines.
    pub fn operations(&self) -> Vec<String> {
        self.description.components.iter().map(|c| format!("{}: {}", c.name, c.operations.join(", "))).collect()
    }

    /// Reads one `world_state` and recomputes the tick hash from it.
    pub fn read(&self, state_bytes: &[u8]) -> Result<View, DecodeError> {
        let mut r = Reader::new(state_bytes);
        let mut scratch = Scratch::default();
        let mut identifiers: Vec<(u32, bool)> = Vec::new();
        let mut entities: Vec<Scratch> = Vec::new();
        for step in &self.world.steps {
            match step {
                Step::Read(Slot::Tick) => scratch.tick = r.u32()?,
                Step::Read(Slot::Energy) => scratch.energy = r.f32()?,
                Step::Read(Slot::RandomStream) => scratch.random_stream = r.u64()?,
                Step::Read(Slot::Identifiers) => {
                    let count = r.u32()? as usize;
                    identifiers.reserve(count);
                    for _ in 0..count {
                        self.read_struct(&self.identifier, &mut r, &mut scratch)?;
                        identifiers.push((scratch.generation, scratch.alive));
                    }
                }
                Step::Read(Slot::Entities) => {
                    let count = r.u32()? as usize;
                    entities.reserve(count);
                    for _ in 0..count {
                        self.read_struct(&self.entity, &mut r, &mut scratch)?;
                        entities.push(scratch);
                    }
                }
                Step::Read(other) => return Err(DecodeError(format!("slot {other:?} is not a world field"))),
                Step::Skip(ty) => skip_value(&mut r, ty, &self.description)?,
            }
        }
        r.finish()?;
        // The hash walks the identifier slots in order; the entities come in
        // canonical order, so the k-th alive slot is the k-th entity.
        let mut hash = FNV_OFFSET_BASIS;
        let mut spin_hash = FNV_OFFSET_BASIS;
        let mut next_entity = 0;
        let mut consistent = true;
        for (index, &(generation, alive)) in identifiers.iter().enumerate() {
            hash = fnv1a(hash, &generation.to_le_bytes());
            hash = fnv1a(hash, &[u8::from(alive)]);
            if alive {
                match entities.get(next_entity) {
                    Some(entity) => {
                        if entity.handle != (index as u32, generation) {
                            consistent = false;
                        }
                        for value in [entity.x, entity.y, entity.vx, entity.vy, entity.mass] {
                            hash = fnv1a(hash, &value.to_le_bytes());
                        }
                        spin_hash = fnv1a(spin_hash, &entity.spin.to_le_bytes());
                    }
                    None => consistent = false,
                }
                next_entity += 1;
            }
        }
        if next_entity != entities.len() {
            consistent = false;
        }
        hash = fnv1a(hash, &scratch.energy.to_le_bytes());
        hash = fnv1a(hash, &scratch.random_stream.to_le_bytes());
        Ok(View {
            tick: scratch.tick,
            hash,
            spin_hash: (self.script == Script::Version2).then_some(spin_hash),
            identifier_count: identifiers.len(),
            entity_count: entities.len(),
            handles_consistent: consistent,
        })
    }

    fn read_struct(&self, plan: &StructPlan, r: &mut Reader<'_>, scratch: &mut Scratch) -> Result<(), DecodeError> {
        for step in &plan.steps {
            match step {
                Step::Read(Slot::Generation) => scratch.generation = r.u32()?,
                Step::Read(Slot::Alive) => scratch.alive = r.bool()?,
                Step::Read(Slot::Handle) => {
                    let handle = r.handle()?;
                    scratch.handle = (handle.index, handle.generation);
                }
                Step::Read(Slot::Position) => self.read_struct(&self.position, r, scratch)?,
                Step::Read(Slot::Velocity) => self.read_struct(&self.velocity, r, scratch)?,
                Step::Read(Slot::Mass) => self.read_struct(&self.mass, r, scratch)?,
                Step::Read(Slot::X) => scratch.x = r.f32()?,
                Step::Read(Slot::Y) => scratch.y = r.f32()?,
                Step::Read(Slot::Vx) => scratch.vx = r.f32()?,
                Step::Read(Slot::Vy) => scratch.vy = r.f32()?,
                Step::Read(Slot::Spin) => scratch.spin = r.f32()?,
                Step::Read(Slot::MassValue) => scratch.mass = r.f32()?,
                Step::Read(other) => return Err(DecodeError(format!("slot {other:?} is not a field of this structure"))),
                Step::Skip(ty) => skip_value(r, ty, &self.description)?,
            }
        }
        Ok(())
    }
}

/// Resolves the fields the script wants in the structure `name`: every field
/// of the description, in its order, becomes a read when the script wants it
/// (with the type the script expects) or a skip by its described type.
fn plan(d: &Description, name: &str, wanted: &[(&str, Type, Slot)]) -> Result<StructPlan, DecodeError> {
    let s = d.structure(name)?;
    let mut steps = Vec::with_capacity(s.fields.len());
    for field in &s.fields {
        match wanted.iter().find(|(wanted_name, _, _)| *wanted_name == field.name) {
            Some((_, ty, slot)) => {
                if *ty != field.ty {
                    return Err(DecodeError(format!("field `{}` of `{name}` has type {:?}, the script expects {ty:?}", field.name, field.ty)));
                }
                steps.push(Step::Read(*slot));
            }
            None => steps.push(Step::Skip(field.ty.clone())),
        }
    }
    for (wanted_name, _, _) in wanted {
        if !s.fields.iter().any(|f| f.name == *wanted_name) {
            return Err(DecodeError(format!("field `{wanted_name}` is not in `{name}`")));
        }
    }
    Ok(StructPlan { steps })
}
