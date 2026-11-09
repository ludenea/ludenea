//! Transactions over the reference workload's state, their verdicts and their
//! inverse. Throwaway, never product code.
//!
//! A transaction is an ordered list of elements: a kick (the toy spike's
//! command), a field set, the add or the remove of a component, a spawn, a
//! despawn. It is validated as a whole against the state at the input fixed
//! point, every element seeing the effect of the accepted elements before it,
//! and applied in order only if every element is accepted; otherwise nothing
//! is applied and every element carries a verdict. The inverse, computed at
//! application from the values overwritten, is itself a list of elements.
//!
//! The reference workload has a fixed set of components, so the component a
//! transaction adds or removes is a spike-local `label` (one integer per
//! entity) kept beside the state. The slot table has a fixed size, so a spawn
//! takes the lowest dead slot, as the workload's own spawns do, and is
//! rejected when none is free.
//!
//! One function, [`transact`], is shared by the world and the sequential
//! oracle so that both decide alike; what the spike checks independently is
//! what it observes of the state before and after.

use reference_workload::{Fnv1a, Grid, Parameters, SEQUENTIAL_PHASES, SPEED_LIMIT, Size, State, Timing, WORLD_SIDE};

use crate::wire::{Reader, put_f32, put_u8, put_u32};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    pub index: u32,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub mass: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    X,
    Y,
    Vx,
    Vy,
    Mass,
}

impl Field {
    pub const ALL: [Field; 5] = [Field::X, Field::Y, Field::Vx, Field::Vy, Field::Mass];

    fn tag(self) -> u8 {
        self as u8
    }

    fn from_tag(tag: u8) -> Option<Field> {
        Field::ALL.get(tag as usize).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element {
    /// A velocity impulse, added then clamped as the specification clamps.
    Kick { entity: Handle, delta_vx: f32, delta_vy: f32 },
    SetField { entity: Handle, field: Field, value: f32 },
    AddLabel { entity: Handle, value: u32 },
    RemoveLabel { entity: Handle },
    Spawn { body: Body },
    Despawn { entity: Handle },
    /// Revives a despawned entity at its handle with its components: what the
    /// inverse of a despawn is made of. Valid while the slot is dead; when a
    /// later spawn reused the slot and was despawned again, the generation is
    /// rolled back to the handle's.
    Restore { entity: Handle, body: Body, label: Option<u32> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    /// Validate every element and report every rejection, instead of stopping
    /// at the first one. The transaction is rejected as a whole either way.
    pub full_evaluation: bool,
    /// Return the inverse transaction with the result.
    pub want_inverse: bool,
    pub elements: Vec<Element>,
}

impl Transaction {
    pub fn of(elements: Vec<Element>) -> Transaction {
        Transaction { full_evaluation: false, want_inverse: false, elements }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    UnknownEntity,
    StaleHandle,
    InvalidValue,
    LabelAlreadyPresent,
    LabelAbsent,
    NoFreeSlot,
    NotDespawned,
}

impl Rejection {
    const ALL: [Rejection; 7] = [
        Rejection::UnknownEntity,
        Rejection::StaleHandle,
        Rejection::InvalidValue,
        Rejection::LabelAlreadyPresent,
        Rejection::LabelAbsent,
        Rejection::NoFreeSlot,
        Rejection::NotDespawned,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Rejection::UnknownEntity => "unknown_entity",
            Rejection::StaleHandle => "stale_handle",
            Rejection::InvalidValue => "invalid_value",
            Rejection::LabelAlreadyPresent => "label_already_present",
            Rejection::LabelAbsent => "label_absent",
            Rejection::NoFreeSlot => "no_free_slot",
            Rejection::NotDespawned => "not_despawned",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Rejected(Rejection),
    /// After the first rejection, when full evaluation was not asked for.
    NotEvaluated,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub applied: bool,
    pub verdicts: Vec<Verdict>,
    /// The handles of the entities spawned, one per spawn element, in order.
    pub spawned: Vec<Handle>,
    /// The inverse transaction's elements, when asked for and applied.
    pub inverse: Option<Vec<Element>>,
}

impl Outcome {
    pub fn count(&self, wanted: fn(&Verdict) -> bool) -> usize {
        self.verdicts.iter().filter(|verdict| wanted(verdict)).count()
    }

    pub fn accepted(&self) -> usize {
        self.count(|verdict| matches!(verdict, Verdict::Accepted))
    }

    pub fn rejected(&self) -> usize {
        self.count(|verdict| matches!(verdict, Verdict::Rejected(_)))
    }

    pub fn not_evaluated(&self) -> usize {
        self.count(|verdict| matches!(verdict, Verdict::NotEvaluated))
    }
}

/// The spike-local optional component: one integer per entity. A label
/// belongs to the generation it was added to, so the workload's own kills and
/// spawns, which know nothing of it, never hand it to another entity.
#[derive(Clone, Debug)]
pub struct Labels {
    present: Vec<bool>,
    generation: Vec<u32>,
    value: Vec<u32>,
}

impl Labels {
    pub fn new(entity_count: usize) -> Labels {
        Labels { present: vec![false; entity_count], generation: vec![0; entity_count], value: vec![0; entity_count] }
    }

    pub fn get(&self, state: &State, slot: usize) -> Option<u32> {
        (self.present[slot] && state.alive[slot] && self.generation[slot] == state.generation[slot]).then(|| self.value[slot])
    }

    fn set(&mut self, state: &State, slot: usize, value: Option<u32>) {
        self.present[slot] = value.is_some();
        self.generation[slot] = state.generation[slot];
        self.value[slot] = value.unwrap_or(0);
    }

    /// FNV-1a 64 over the slot and the value of every label an alive entity carries.
    pub fn digest(&self, state: &State) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..state.len() {
            if let Some(value) = self.get(state, slot) {
                hasher.write(&(slot as u32).to_le_bytes());
                hasher.write(&value.to_le_bytes());
            }
        }
        hasher.finish()
    }
}

/// The validation's view of the slots a transaction touches: the state as it
/// would be after the accepted elements so far, without touching the state.
#[derive(Clone, Debug)]
pub struct Scratch {
    epoch: u32,
    stamp: Vec<u32>,
    alive: Vec<bool>,
    generation: Vec<u32>,
    label: Vec<bool>,
    dead: Vec<usize>,
    dead_loaded: bool,
}

impl Scratch {
    pub fn new(entity_count: usize) -> Scratch {
        Scratch {
            epoch: 0,
            stamp: vec![0; entity_count],
            alive: vec![false; entity_count],
            generation: vec![0; entity_count],
            label: vec![false; entity_count],
            dead: Vec::new(),
            dead_loaded: false,
        }
    }

    fn begin(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.fill(0);
            self.epoch = 1;
        }
        self.dead_loaded = false;
    }

    fn load(&mut self, state: &State, labels: &Labels, slot: usize) {
        if self.stamp[slot] != self.epoch {
            self.stamp[slot] = self.epoch;
            self.alive[slot] = state.alive[slot];
            self.generation[slot] = state.generation[slot];
            self.label[slot] = labels.get(state, slot).is_some();
        }
    }

    fn load_dead(&mut self, dead: &[usize]) {
        if !self.dead_loaded {
            self.dead.clear();
            self.dead.extend_from_slice(dead);
            self.dead_loaded = true;
        }
    }

    fn entity(&mut self, state: &State, labels: &Labels, entity: Handle) -> Result<usize, Rejection> {
        let slot = entity.index as usize;
        if slot >= state.len() {
            return Err(Rejection::UnknownEntity);
        }
        self.load(state, labels, slot);
        if !self.alive[slot] || self.generation[slot] != entity.generation {
            return Err(Rejection::StaleHandle);
        }
        Ok(slot)
    }
}

fn valid_field(field: Field, value: f32) -> bool {
    match field {
        Field::X | Field::Y => (0.0..WORLD_SIDE).contains(&value),
        Field::Vx | Field::Vy => value.abs() <= SPEED_LIMIT,
        Field::Mass => value > 0.0 && value <= 1_048_576.0,
    }
}

fn valid_body(body: &Body) -> bool {
    valid_field(Field::X, body.x)
        && valid_field(Field::Y, body.y)
        && valid_field(Field::Vx, body.vx)
        && valid_field(Field::Vy, body.vy)
        && valid_field(Field::Mass, body.mass)
}

fn sorted_insert(dead: &mut Vec<usize>, slot: usize) {
    if let Err(at) = dead.binary_search(&slot) {
        dead.insert(at, slot);
    }
}

fn sorted_remove(dead: &mut Vec<usize>, slot: usize) {
    if let Ok(at) = dead.binary_search(&slot) {
        dead.remove(at);
    }
}

fn validate(element: &Element, state: &State, labels: &Labels, dead: &[usize], scratch: &mut Scratch) -> Result<(), Rejection> {
    match *element {
        Element::Kick { entity, delta_vx, delta_vy } => {
            scratch.entity(state, labels, entity)?;
            if !delta_vx.is_finite() || !delta_vy.is_finite() {
                return Err(Rejection::InvalidValue);
            }
        }
        Element::SetField { entity, field, value } => {
            scratch.entity(state, labels, entity)?;
            if !valid_field(field, value) {
                return Err(Rejection::InvalidValue);
            }
        }
        Element::AddLabel { entity, .. } => {
            let slot = scratch.entity(state, labels, entity)?;
            if scratch.label[slot] {
                return Err(Rejection::LabelAlreadyPresent);
            }
            scratch.label[slot] = true;
        }
        Element::RemoveLabel { entity } => {
            let slot = scratch.entity(state, labels, entity)?;
            if !scratch.label[slot] {
                return Err(Rejection::LabelAbsent);
            }
            scratch.label[slot] = false;
        }
        Element::Spawn { body } => {
            if !valid_body(&body) {
                return Err(Rejection::InvalidValue);
            }
            scratch.load_dead(dead);
            if scratch.dead.is_empty() {
                return Err(Rejection::NoFreeSlot);
            }
            let slot = scratch.dead.remove(0);
            scratch.load(state, labels, slot);
            scratch.alive[slot] = true;
            scratch.generation[slot] += 1;
            scratch.label[slot] = false;
        }
        Element::Despawn { entity } => {
            let slot = scratch.entity(state, labels, entity)?;
            scratch.load_dead(dead);
            scratch.alive[slot] = false;
            scratch.label[slot] = false;
            sorted_insert(&mut scratch.dead, slot);
        }
        Element::Restore { entity, body, label } => {
            let slot = entity.index as usize;
            if slot >= state.len() {
                return Err(Rejection::UnknownEntity);
            }
            scratch.load(state, labels, slot);
            if scratch.alive[slot] || scratch.generation[slot] < entity.generation {
                return Err(Rejection::NotDespawned);
            }
            if !valid_body(&body) {
                return Err(Rejection::InvalidValue);
            }
            scratch.load_dead(dead);
            scratch.alive[slot] = true;
            scratch.generation[slot] = entity.generation;
            scratch.label[slot] = label.is_some();
            sorted_remove(&mut scratch.dead, slot);
        }
    }
    Ok(())
}

fn field_of(state: &mut State, field: Field) -> &mut Vec<f32> {
    match field {
        Field::X => &mut state.x,
        Field::Y => &mut state.y,
        Field::Vx => &mut state.vx,
        Field::Vy => &mut state.vy,
        Field::Mass => &mut state.mass,
    }
}

fn body_of(state: &State, slot: usize) -> Body {
    Body { x: state.x[slot], y: state.y[slot], vx: state.vx[slot], vy: state.vy[slot], mass: state.mass[slot] }
}

fn set_body(state: &mut State, slot: usize, body: &Body) {
    state.x[slot] = body.x;
    state.y[slot] = body.y;
    state.vx[slot] = body.vx;
    state.vy[slot] = body.vy;
    state.mass[slot] = body.mass;
}

/// The kick applied as the specification would: added, then clamped.
pub fn kicked_velocity(v: f32, delta: f32) -> f32 {
    (v + delta).clamp(-SPEED_LIMIT, SPEED_LIMIT)
}

struct Application<'a> {
    state: &'a mut State,
    labels: &'a mut Labels,
    dead: &'a mut Vec<usize>,
    touched: &'a mut Vec<usize>,
    spawned: Vec<Handle>,
    inverse: Option<Vec<Element>>,
}

impl Application<'_> {
    fn undo(&mut self, element: Element) {
        if let Some(inverse) = &mut self.inverse {
            inverse.push(element);
        }
    }

    fn apply(&mut self, element: &Element) {
        match *element {
            Element::Kick { entity, delta_vx, delta_vy } => {
                let slot = entity.index as usize;
                self.undo(Element::SetField { entity, field: Field::Vx, value: self.state.vx[slot] });
                self.undo(Element::SetField { entity, field: Field::Vy, value: self.state.vy[slot] });
                self.state.vx[slot] = kicked_velocity(self.state.vx[slot], delta_vx);
                self.state.vy[slot] = kicked_velocity(self.state.vy[slot], delta_vy);
            }
            Element::SetField { entity, field, value } => {
                let slot = entity.index as usize;
                let column = field_of(self.state, field);
                let previous = column[slot];
                column[slot] = value;
                self.undo(Element::SetField { entity, field, value: previous });
                if field == Field::Mass {
                    self.touched.push(slot);
                }
            }
            Element::AddLabel { entity, value } => {
                self.labels.set(self.state, entity.index as usize, Some(value));
                self.undo(Element::RemoveLabel { entity });
            }
            Element::RemoveLabel { entity } => {
                let slot = entity.index as usize;
                let value = self.labels.get(self.state, slot).expect("validated: the label is present");
                self.labels.set(self.state, slot, None);
                self.undo(Element::AddLabel { entity, value });
            }
            Element::Spawn { body } => {
                let slot = self.dead.remove(0);
                set_body(self.state, slot, &body);
                self.state.generation[slot] += 1;
                self.state.alive[slot] = true;
                self.labels.set(self.state, slot, None);
                self.touched.push(slot);
                let entity = Handle { index: slot as u32, generation: self.state.generation[slot] };
                self.spawned.push(entity);
                self.undo(Element::Despawn { entity });
            }
            Element::Despawn { entity } => {
                let slot = entity.index as usize;
                let restore = Element::Restore { entity, body: body_of(self.state, slot), label: self.labels.get(self.state, slot) };
                self.undo(restore);
                self.labels.set(self.state, slot, None);
                self.state.alive[slot] = false;
                sorted_insert(self.dead, slot);
                self.touched.push(slot);
            }
            Element::Restore { entity, body, label } => {
                let slot = entity.index as usize;
                set_body(self.state, slot, &body);
                self.state.generation[slot] = entity.generation;
                self.state.alive[slot] = true;
                self.labels.set(self.state, slot, label);
                sorted_remove(self.dead, slot);
                self.touched.push(slot);
                self.undo(Element::Despawn { entity });
            }
        }
    }
}

/// Validates `transaction` as a whole against the state at the fixed point and
/// applies it in order if every element is accepted. `dead` is the sorted list
/// of dead slots, kept exact; the slots whose liveness, generation or mass
/// change are pushed onto `touched`.
pub fn transact(state: &mut State, labels: &mut Labels, dead: &mut Vec<usize>, touched: &mut Vec<usize>, scratch: &mut Scratch, transaction: &Transaction) -> Outcome {
    scratch.begin();
    let mut verdicts = Vec::with_capacity(transaction.elements.len());
    let mut rejected = false;
    for element in &transaction.elements {
        if rejected && !transaction.full_evaluation {
            verdicts.push(Verdict::NotEvaluated);
            continue;
        }
        match validate(element, state, labels, dead, scratch) {
            Ok(()) => verdicts.push(Verdict::Accepted),
            Err(rejection) => {
                rejected = true;
                verdicts.push(Verdict::Rejected(rejection));
            }
        }
    }
    if rejected {
        return Outcome { applied: false, verdicts, spawned: Vec::new(), inverse: None };
    }
    let inverse = transaction.want_inverse.then(|| Vec::with_capacity(transaction.elements.len()));
    let mut application = Application { state, labels, dead, touched, spawned: Vec::new(), inverse };
    for element in &transaction.elements {
        application.apply(element);
    }
    let Application { spawned, mut inverse, .. } = application;
    if let Some(inverse) = &mut inverse {
        inverse.reverse();
    }
    Outcome { applied: true, verdicts, spawned, inverse }
}

// ---------------------------------------------------------------------------
// Canonical bytes and digests
// ---------------------------------------------------------------------------

/// The bytes the workload's tick hash covers, in its order: what a state
/// stream's frame carries, so that a client recomputes the hash from a frame.
pub fn write_canonical(state: &State, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(state.len() * 25 + 12);
    for slot in 0..state.len() {
        let mut row = [0_u8; 25];
        row[0..4].copy_from_slice(&state.generation[slot].to_le_bytes());
        row[4] = u8::from(state.alive[slot]);
        if state.alive[slot] {
            row[5..9].copy_from_slice(&state.x[slot].to_le_bytes());
            row[9..13].copy_from_slice(&state.y[slot].to_le_bytes());
            row[13..17].copy_from_slice(&state.vx[slot].to_le_bytes());
            row[17..21].copy_from_slice(&state.vy[slot].to_le_bytes());
            row[21..25].copy_from_slice(&state.mass[slot].to_le_bytes());
            out.extend_from_slice(&row);
        } else {
            out.extend_from_slice(&row[..5]);
        }
    }
    out.extend_from_slice(&state.energy.to_le_bytes());
    out.extend_from_slice(&state.rng.state.to_le_bytes());
}

/// One slot as a client reads it from the canonical bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotView {
    pub generation: u32,
    pub alive: bool,
    pub body: Body,
}

impl SlotView {
    pub fn handle(&self, index: usize) -> Handle {
        Handle { index: index as u32, generation: self.generation }
    }
}

/// Reads the slots back from the canonical bytes; `None` if they are malformed.
pub fn read_canonical(bytes: &[u8]) -> Option<Vec<SlotView>> {
    let mut reader = Reader::new(bytes);
    let mut slots = Vec::new();
    while reader.remaining() > 12 {
        let generation = reader.u32()?;
        let alive = reader.u8()? != 0;
        let mut body = Body { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, mass: 0.0 };
        if alive {
            body = Body { x: reader.f32()?, y: reader.f32()?, vx: reader.f32()?, vy: reader.f32()?, mass: reader.f32()? };
        }
        slots.push(SlotView { generation, alive, body });
    }
    (reader.remaining() == 12).then_some(slots)
}

/// The hash of the component data alone: the tick hash without the
/// generations, which a spawn moves and nothing moves back.
pub fn component_hash(state: &State) -> u64 {
    let mut hasher = Fnv1a::new();
    for slot in 0..state.len() {
        hasher.write(&[u8::from(state.alive[slot])]);
        if state.alive[slot] {
            hasher.write(&state.x[slot].to_le_bytes());
            hasher.write(&state.y[slot].to_le_bytes());
            hasher.write(&state.vx[slot].to_le_bytes());
            hasher.write(&state.vy[slot].to_le_bytes());
            hasher.write(&state.mass[slot].to_le_bytes());
        }
    }
    hasher.write(&state.energy.to_le_bytes());
    hasher.write(&state.rng.state.to_le_bytes());
    hasher.finish()
}

/// What a client observes of the state at a boundary without reading it all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Digests {
    /// The workload's tick hash of the state as it is now.
    pub full: u64,
    pub components: u64,
    pub labels: u64,
}

pub fn digests(state: &State, labels: &Labels) -> Digests {
    Digests { full: state.hash(), components: component_hash(state), labels: labels.digest(state) }
}

// ---------------------------------------------------------------------------
// Encodings
// ---------------------------------------------------------------------------

fn put_handle(out: &mut Vec<u8>, handle: Handle) {
    put_u32(out, handle.index);
    put_u32(out, handle.generation);
}

fn put_body(out: &mut Vec<u8>, body: &Body) {
    for value in [body.x, body.y, body.vx, body.vy, body.mass] {
        put_f32(out, value);
    }
}

fn read_handle(reader: &mut Reader) -> Option<Handle> {
    Some(Handle { index: reader.u32()?, generation: reader.u32()? })
}

fn read_body(reader: &mut Reader) -> Option<Body> {
    Some(Body { x: reader.f32()?, y: reader.f32()?, vx: reader.f32()?, vy: reader.f32()?, mass: reader.f32()? })
}

fn put_elements(out: &mut Vec<u8>, elements: &[Element]) {
    put_u32(out, elements.len() as u32);
    for element in elements {
        match element {
            Element::Kick { entity, delta_vx, delta_vy } => {
                put_u8(out, 0);
                put_handle(out, *entity);
                put_f32(out, *delta_vx);
                put_f32(out, *delta_vy);
            }
            Element::SetField { entity, field, value } => {
                put_u8(out, 1);
                put_handle(out, *entity);
                put_u8(out, field.tag());
                put_f32(out, *value);
            }
            Element::AddLabel { entity, value } => {
                put_u8(out, 2);
                put_handle(out, *entity);
                put_u32(out, *value);
            }
            Element::RemoveLabel { entity } => {
                put_u8(out, 3);
                put_handle(out, *entity);
            }
            Element::Spawn { body } => {
                put_u8(out, 4);
                put_body(out, body);
            }
            Element::Despawn { entity } => {
                put_u8(out, 5);
                put_handle(out, *entity);
            }
            Element::Restore { entity, body, label } => {
                put_u8(out, 6);
                put_handle(out, *entity);
                put_body(out, body);
                put_u8(out, u8::from(label.is_some()));
                put_u32(out, label.unwrap_or(0));
            }
        }
    }
}

fn read_elements(reader: &mut Reader) -> Option<Vec<Element>> {
    let count = reader.u32()? as usize;
    if count > reader.remaining() {
        return None;
    }
    let mut elements = Vec::with_capacity(count);
    for _ in 0..count {
        let element = match reader.u8()? {
            0 => Element::Kick { entity: read_handle(reader)?, delta_vx: reader.f32()?, delta_vy: reader.f32()? },
            1 => Element::SetField { entity: read_handle(reader)?, field: Field::from_tag(reader.u8()?)?, value: reader.f32()? },
            2 => Element::AddLabel { entity: read_handle(reader)?, value: reader.u32()? },
            3 => Element::RemoveLabel { entity: read_handle(reader)? },
            4 => Element::Spawn { body: read_body(reader)? },
            5 => Element::Despawn { entity: read_handle(reader)? },
            6 => {
                let entity = read_handle(reader)?;
                let body = read_body(reader)?;
                let present = reader.u8()? != 0;
                let value = reader.u32()?;
                Element::Restore { entity, body, label: present.then_some(value) }
            }
            _ => return None,
        };
        elements.push(element);
    }
    Some(elements)
}

impl Transaction {
    /// The canonical bytes: what travels, what the input log keeps and what a
    /// retry is compared with.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.elements.len() * 16);
        put_u8(&mut out, u8::from(self.full_evaluation) | (u8::from(self.want_inverse) << 1));
        put_elements(&mut out, &self.elements);
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Transaction> {
        let mut reader = Reader::new(bytes);
        let flags = reader.u8()?;
        if flags > 3 {
            return None;
        }
        let elements = read_elements(&mut reader)?;
        (reader.remaining() == 0).then_some(Transaction { full_evaluation: flags & 1 != 0, want_inverse: flags & 2 != 0, elements })
    }
}

impl Outcome {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.verdicts.len());
        put_u8(&mut out, u8::from(self.applied));
        put_u32(&mut out, self.verdicts.len() as u32);
        for verdict in &self.verdicts {
            put_u8(
                &mut out,
                match verdict {
                    Verdict::Accepted => 0,
                    Verdict::NotEvaluated => 1,
                    Verdict::Rejected(rejection) => 2 + *rejection as u8,
                },
            );
        }
        put_u32(&mut out, self.spawned.len() as u32);
        for handle in &self.spawned {
            put_handle(&mut out, *handle);
        }
        put_u8(&mut out, u8::from(self.inverse.is_some()));
        if let Some(inverse) = &self.inverse {
            put_elements(&mut out, inverse);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Outcome> {
        let mut reader = Reader::new(bytes);
        let applied = reader.u8()? != 0;
        let count = reader.u32()? as usize;
        let mut verdicts = Vec::with_capacity(count.min(reader.remaining()));
        for _ in 0..count {
            verdicts.push(match reader.u8()? {
                0 => Verdict::Accepted,
                1 => Verdict::NotEvaluated,
                tag => Verdict::Rejected(*Rejection::ALL.get(tag as usize - 2)?),
            });
        }
        let count = reader.u32()? as usize;
        let mut spawned = Vec::with_capacity(count.min(reader.remaining()));
        for _ in 0..count {
            spawned.push(read_handle(&mut reader)?);
        }
        let inverse = if reader.u8()? != 0 { Some(read_elements(&mut reader)?) } else { None };
        (reader.remaining() == 0).then_some(Outcome { applied, verdicts, spawned, inverse })
    }
}

// ---------------------------------------------------------------------------
// The sequential oracle
// ---------------------------------------------------------------------------

/// The reference workload stepped sequentially by the reference crate's
/// systems, taking the same transactions at the same tick boundaries: the
/// ground truth the world's hashes and digests are compared with.
pub struct Oracle {
    state: State,
    grid: Grid,
    labels: Labels,
    dead: Vec<usize>,
    touched: Vec<usize>,
    scratch: Scratch,
    timing: Timing,
    tick: u32,
    wander: bool,
    /// The tick hashes from tick 0.
    pub hashes: Vec<u64>,
}

impl Oracle {
    pub fn new(size: Size) -> Oracle {
        let parameters = Parameters::new(size, 0, false);
        let state = State::initialize(&parameters);
        let n = state.len();
        let hashes = vec![state.hash()];
        Oracle {
            state,
            grid: Grid::new(),
            labels: Labels::new(n),
            dead: Vec::new(),
            touched: Vec::new(),
            scratch: Scratch::new(n),
            timing: Timing::new(&SEQUENTIAL_PHASES),
            tick: 0,
            wander: parameters.wander,
            hashes,
        }
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn transact(&mut self, transaction: &Transaction) -> Outcome {
        self.touched.clear();
        transact(&mut self.state, &mut self.labels, &mut self.dead, &mut self.touched, &mut self.scratch, transaction)
    }

    pub fn digests(&self) -> Digests {
        digests(&self.state, &self.labels)
    }

    #[cfg(test)]
    pub fn state(&self) -> &State {
        &self.state
    }

    /// The state, the labels and the sorted dead slots, to measure on copies of them.
    pub fn parts(&self) -> (&State, &Labels, &[usize]) {
        (&self.state, &self.labels, &self.dead)
    }

    pub fn step_to(&mut self, tick: u32) {
        while self.tick < tick {
            self.tick += 1;
            reference_workload::tick(&mut self.state, &mut self.grid, self.tick, self.wander, &mut self.timing);
            self.dead.clear();
            self.dead.extend((0..self.state.len()).filter(|&slot| !self.state.alive[slot]));
            self.hashes.push(self.state.hash());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        state: State,
        labels: Labels,
        dead: Vec<usize>,
        touched: Vec<usize>,
        scratch: Scratch,
    }

    impl Fixture {
        fn new() -> Fixture {
            let state = State::initialize(&Parameters::new(Size::Small, 0, false));
            let n = state.len();
            Fixture { state, labels: Labels::new(n), dead: Vec::new(), touched: Vec::new(), scratch: Scratch::new(n) }
        }

        fn run(&mut self, transaction: &Transaction) -> Outcome {
            transact(&mut self.state, &mut self.labels, &mut self.dead, &mut self.touched, &mut self.scratch, transaction)
        }

        fn digests(&self) -> Digests {
            digests(&self.state, &self.labels)
        }
    }

    fn handle(index: u32) -> Handle {
        Handle { index, generation: 0 }
    }

    fn body() -> Body {
        Body { x: 10.0, y: 20.0, vx: 1.0, vy: -1.0, mass: 2.0 }
    }

    fn mixed() -> Vec<Element> {
        vec![
            Element::SetField { entity: handle(3), field: Field::Vx, value: 5.0 },
            Element::SetField { entity: handle(4), field: Field::Mass, value: 4.0 },
            Element::AddLabel { entity: handle(5), value: 77 },
            Element::Kick { entity: handle(6), delta_vx: 100.0, delta_vy: -3.0 },
            Element::Despawn { entity: handle(7) },
            Element::Despawn { entity: handle(2) },
            Element::Spawn { body: body() },
            Element::AddLabel { entity: handle(8), value: 9 },
            Element::RemoveLabel { entity: handle(8) },
        ]
    }

    #[test]
    fn a_transaction_applies_in_order_and_a_spawn_takes_the_lowest_slot_freed_before_it() {
        let mut fixture = Fixture::new();
        let outcome = fixture.run(&Transaction::of(mixed()));
        assert!(outcome.applied);
        assert_eq!(outcome.accepted(), 9);
        assert_eq!(outcome.spawned, [Handle { index: 2, generation: 1 }]);
        assert_eq!(fixture.state.vx[3], 5.0);
        assert_eq!(fixture.state.mass[4], 4.0);
        assert_eq!(fixture.labels.get(&fixture.state, 5), Some(77));
        assert_eq!(fixture.state.vx[6], SPEED_LIMIT);
        assert!(!fixture.state.alive[7]);
        assert!(fixture.state.alive[2]);
        assert_eq!(body_of(&fixture.state, 2), body());
        assert_eq!(fixture.labels.get(&fixture.state, 8), None);
        assert_eq!(fixture.dead, [7]);
        let mut touched = fixture.touched.clone();
        touched.sort_unstable();
        assert_eq!(touched, [2, 2, 4, 7]);
    }

    #[test]
    fn one_rejected_element_leaves_the_state_untouched_and_the_rest_not_evaluated() {
        let mut fixture = Fixture::new();
        let before = fixture.digests();
        let mut elements = mixed();
        elements.insert(4, Element::SetField { entity: Handle { index: 9, generation: 3 }, field: Field::X, value: 1.0 });
        elements.push(Element::SetField { entity: handle(1), field: Field::X, value: -1.0 });
        let outcome = fixture.run(&Transaction::of(elements.clone()));
        assert!(!outcome.applied);
        assert_eq!((outcome.accepted(), outcome.rejected(), outcome.not_evaluated()), (4, 1, 6));
        assert_eq!(outcome.verdicts[4], Verdict::Rejected(Rejection::StaleHandle));
        assert_eq!(fixture.digests(), before);
        assert!(fixture.dead.is_empty() && fixture.touched.is_empty());

        let full = Transaction { full_evaluation: true, want_inverse: true, elements };
        let outcome = fixture.run(&full);
        assert!(!outcome.applied && outcome.inverse.is_none());
        assert_eq!((outcome.accepted(), outcome.rejected(), outcome.not_evaluated()), (9, 2, 0));
        assert_eq!(outcome.verdicts[10], Verdict::Rejected(Rejection::InvalidValue));
        assert_eq!(fixture.digests(), before);
    }

    #[test]
    fn every_rejection_is_named_and_elements_see_the_elements_before_them() {
        let mut fixture = Fixture::new();
        let elements = vec![
            Element::Despawn { entity: handle(50_000) },
            Element::Spawn { body: body() },
            Element::RemoveLabel { entity: handle(1) },
            Element::AddLabel { entity: handle(1), value: 1 },
            Element::AddLabel { entity: handle(1), value: 2 },
            Element::Despawn { entity: handle(1) },
            Element::SetField { entity: handle(1), field: Field::Vx, value: 0.0 },
            Element::Restore { entity: handle(2), body: body(), label: None },
            Element::Spawn { body: Body { mass: 0.0, ..body() } },
            Element::Spawn { body: body() },
            Element::Spawn { body: body() },
        ];
        let outcome = fixture.run(&Transaction { full_evaluation: true, want_inverse: false, elements });
        let rejections: Vec<Option<&str>> =
            outcome.verdicts.iter().map(|verdict| if let Verdict::Rejected(rejection) = verdict { Some(rejection.code()) } else { None }).collect();
        assert_eq!(
            rejections,
            [
                Some("unknown_entity"),
                Some("no_free_slot"),
                Some("label_absent"),
                None,
                Some("label_already_present"),
                None,
                Some("stale_handle"),
                Some("not_despawned"),
                Some("invalid_value"),
                None,
                Some("no_free_slot"),
            ]
        );
    }

    #[test]
    fn the_inverse_restores_the_state_and_only_a_lone_spawn_leaves_a_generation_behind() {
        let mut fixture = Fixture::new();
        fixture.run(&Transaction::of(vec![Element::AddLabel { entity: handle(7), value: 5 }]));
        let before = fixture.digests();
        let outcome = fixture.run(&Transaction { full_evaluation: false, want_inverse: true, elements: mixed() });
        assert!(outcome.applied);
        assert_ne!(fixture.digests(), before);
        let inverse = outcome.inverse.unwrap();
        assert_eq!(inverse.len(), 10, "a kick is undone by two field sets");
        assert!(fixture.run(&Transaction::of(inverse)).applied);
        assert_eq!(fixture.digests(), before, "despawns come back at their handles with their labels, over a reused slot too");
        assert_eq!(fixture.state.generation[2], 0);
        assert!(fixture.dead.is_empty());

        fixture.run(&Transaction::of(vec![Element::Despawn { entity: handle(2) }]));
        let before = fixture.digests();
        let outcome = fixture.run(&Transaction { full_evaluation: false, want_inverse: true, elements: vec![Element::Spawn { body: body() }] });
        assert_eq!(outcome.inverse, Some(vec![Element::Despawn { entity: Handle { index: 2, generation: 1 } }]));
        assert!(fixture.run(&Transaction::of(outcome.inverse.unwrap())).applied);
        let after = fixture.digests();
        assert_eq!((after.components, after.labels), (before.components, before.labels));
        assert_ne!(after.full, before.full, "the slot keeps the generation the spawn gave it");
        assert_eq!(fixture.state.generation[2], 1);
    }

    #[test]
    fn transactions_and_outcomes_round_trip_through_their_bytes() {
        let mut elements = mixed();
        elements.push(Element::Restore { entity: handle(9), body: body(), label: Some(4) });
        let transaction = Transaction { full_evaluation: true, want_inverse: true, elements };
        let bytes = transaction.encode();
        assert_eq!(Transaction::decode(&bytes), Some(transaction.clone()));
        assert_eq!(Transaction::decode(&bytes[..bytes.len() - 1]), None);
        let mut fixture = Fixture::new();
        let rejected = fixture.run(&transaction);
        assert_eq!(Outcome::decode(&rejected.encode()), Some(rejected));
        let applied = fixture.run(&Transaction { full_evaluation: false, want_inverse: true, elements: mixed() });
        assert_eq!(Outcome::decode(&applied.encode()), Some(applied));
    }

    #[test]
    fn the_canonical_bytes_hash_to_the_tick_hash_and_read_back() {
        let mut oracle = Oracle::new(Size::Small);
        oracle.step_to(40);
        oracle.transact(&Transaction::of(vec![Element::Despawn { entity: handle(11) }]));
        let mut bytes = Vec::new();
        write_canonical(&oracle.state, &mut bytes);
        assert_eq!(reference_workload::fnv1a(&bytes), oracle.state.hash());
        let slots = read_canonical(&bytes).unwrap();
        assert_eq!(slots.len(), oracle.state.len());
        assert!(!slots[11].alive);
        assert_eq!(slots[12].body, body_of(&oracle.state, 12));
        assert_eq!(read_canonical(&bytes[..bytes.len() - 1]), None);
    }

    #[test]
    fn the_oracle_without_transactions_is_the_sequential_reference() {
        let reference = reference_workload::run_sequential(&Parameters::new(Size::Small, 30, false));
        let mut oracle = Oracle::new(Size::Small);
        oracle.step_to(30);
        assert_eq!(oracle.hashes, reference.tick_hashes);
        assert_eq!(oracle.tick(), 30);
    }
}
