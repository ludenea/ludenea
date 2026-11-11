//! The scene asset: the component schema of the fixture, an entity prototype,
//! the five kinds of override, the canonical payload with its content hash,
//! the import function with its refusals by name, the ordered application of
//! a list to a payload, and the payload store. Throwaway, never product code.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use reference_workload::{Fnv1a, fnv1a};

// The components in the order of their stable names, each with its fields in
// declaration order: the canonical traversal of a prototype.
pub const INSTANCING: u8 = 0;
pub const LABEL: u8 = 1;
pub const MASS: u8 = 2;
pub const POSITION: u8 = 3;
pub const REFERENCE: u8 = 4;
pub const VELOCITY: u8 = 5;
pub const COMPONENT_COUNT: u8 = 6;
pub const COMPONENT_NAMES: [&str; 6] = ["instancing", "label", "mass", "position", "reference", "velocity"];
pub const FIELD_NAMES: [&[&str]; 6] = [&["asset", "anchor_x", "anchor_y"], &["value"], &["value"], &["x", "y"], &["ups", "down", "local"], &["x", "y"]];
const FIELD_OFFSET: [usize; 6] = [0, 3, 4, 5, 7, 10];
pub const FIELDS: usize = 12;
pub const MAX_COMPONENT_FIELDS: usize = 3;
/// The run-time range of local identifiers has the high bit set; an
/// identifier an override carries lies in the authored range.
pub const RUNTIME_RANGE: u64 = 1 << 63;

pub const fn bit(component: u8) -> u8 {
    1 << component
}

pub fn field_count(component: u8) -> usize {
    FIELD_NAMES[component as usize].len()
}

pub fn float_bits(value: f32) -> u64 {
    u64::from(value.to_bits())
}

pub fn bits_float(bits: u64) -> f32 {
    f32::from_bits(bits as u32)
}

/// An entity prototype: its local identifier, its parent's (zero for a root
/// of its scene), and its components. A field holds 64 bits (an `f32` in the
/// low half); the fields of an absent component are zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proto {
    pub local: u64,
    pub parent: u64,
    pub mask: u8,
    pub fields: [u64; FIELDS],
}

impl Proto {
    pub fn new(local: u64, parent: u64) -> Proto {
        Proto { local, parent, mask: 0, fields: [0; FIELDS] }
    }

    pub fn has(&self, component: u8) -> bool {
        self.mask & bit(component) != 0
    }

    pub fn get(&self, component: u8, field: u8) -> u64 {
        self.fields[FIELD_OFFSET[component as usize] + field as usize]
    }

    pub fn set(&mut self, component: u8, field: u8, value: u64) {
        self.fields[FIELD_OFFSET[component as usize] + field as usize] = value;
    }

    pub fn add(&mut self, component: u8, values: &[u64]) {
        self.mask |= bit(component);
        for (field, value) in values.iter().take(field_count(component)).enumerate() {
            self.set(component, field as u8, *value);
        }
    }

    pub fn with(mut self, component: u8, values: &[u64]) -> Proto {
        self.add(component, values);
        self
    }

    pub fn remove(&mut self, component: u8) {
        self.mask &= !bit(component);
        for field in 0..field_count(component) {
            self.set(component, field as u8, 0);
        }
    }

    pub fn values(&self, component: u8) -> [u64; MAX_COMPONENT_FIELDS] {
        let mut values = [0; MAX_COMPONENT_FIELDS];
        for (field, value) in values.iter_mut().enumerate().take(field_count(component)) {
            *value = self.get(component, field as u8);
        }
        values
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.local.to_le_bytes());
        out.extend_from_slice(&self.parent.to_le_bytes());
        out.push(self.mask);
        for component in 0..COMPONENT_COUNT {
            if self.has(component) {
                for field in 0..field_count(component) {
                    out.extend_from_slice(&self.get(component, field as u8).to_le_bytes());
                }
            }
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Option<Proto> {
        let mut proto = Proto::new(reader.u64()?, reader.u64()?);
        proto.mask = reader.u8()?;
        if proto.mask >> COMPONENT_COUNT != 0 {
            return None;
        }
        for component in 0..COMPONENT_COUNT {
            if proto.has(component) {
                for field in 0..field_count(component) {
                    proto.set(component, field as u8, reader.u64()?);
                }
            }
        }
        Some(proto)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    fn u64(&mut self) -> Option<u64> {
        let bytes = self.bytes.get(self.at..self.at + 8)?;
        self.at += 8;
        Some(u64::from_le_bytes(bytes.try_into().expect("eight bytes")))
    }
}

// ---------------------------------------------------------------------------
// Overrides
// ---------------------------------------------------------------------------

/// The five kinds of override, declared in the fixed order of application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    RemoveEntity,
    /// The added entity, a member of the child instance under the local
    /// identifier the override carries (its target).
    AddEntity(Proto),
    RemoveComponent(u8),
    AddComponent(u8, [u64; MAX_COMPONENT_FIELDS]),
    SetField(u8, u8, u64),
}

/// One override: a target named by a path of local identifiers through the
/// nested instances (the instancing entity at each level below the child)
/// down to the entity, and what changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    pub path: Vec<u64>,
    pub target: u64,
    pub change: Change,
}

pub const KIND_NAMES: [&str; 5] = ["entity removed", "entity added", "component removed", "component added", "field value"];
/// The fixed order: entities removed, entities added, components removed,
/// components added, field values.
pub const STATED_ORDER: [u8; 5] = [0, 1, 2, 3, 4];

impl Override {
    pub fn new(path: &[u64], target: u64, change: Change) -> Override {
        Override { path: path.to_vec(), target, change }
    }

    pub fn kind(&self) -> u8 {
        match self.change {
            Change::RemoveEntity => 0,
            Change::AddEntity(_) => 1,
            Change::RemoveComponent(_) => 2,
            Change::AddComponent(..) => 3,
            Change::SetField(..) => 4,
        }
    }

    fn component_and_field(&self) -> (u8, u8) {
        match self.change {
            Change::RemoveEntity | Change::AddEntity(_) => (0, 0),
            Change::RemoveComponent(component) | Change::AddComponent(component, _) => (component, 0),
            Change::SetField(component, field, _) => (component, field),
        }
    }

    /// Within a kind by the target's path, then by the component's stable
    /// name, then by the field's declaration order.
    fn sort_key(&self) -> (u8, Vec<u64>, u8, u8) {
        let mut address = self.path.clone();
        address.push(self.target);
        let (component, field) = self.component_and_field();
        (self.kind(), address, component, field)
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.kind());
        out.push(self.path.len() as u8);
        for element in &self.path {
            out.extend_from_slice(&element.to_le_bytes());
        }
        out.extend_from_slice(&self.target.to_le_bytes());
        match &self.change {
            Change::RemoveEntity => {}
            Change::AddEntity(proto) => proto.encode(out),
            Change::RemoveComponent(component) => out.push(*component),
            Change::AddComponent(component, values) => {
                out.push(*component);
                for value in values.iter().take(field_count(*component)) {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
            Change::SetField(component, field, value) => {
                out.push(*component);
                out.push(*field);
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Option<Override> {
        let kind = reader.u8()?;
        let length = reader.u8()?;
        let mut path = Vec::with_capacity(length as usize);
        for _ in 0..length {
            path.push(reader.u64()?);
        }
        let target = reader.u64()?;
        let component = |reader: &mut Reader<'_>| reader.u8().filter(|c| *c < COMPONENT_COUNT);
        let change = match kind {
            0 => Change::RemoveEntity,
            1 => Change::AddEntity(Proto::decode(reader)?),
            2 => Change::RemoveComponent(component(reader)?),
            3 => {
                let component = component(reader)?;
                let mut values = [0; MAX_COMPONENT_FIELDS];
                for value in values.iter_mut().take(field_count(component)) {
                    *value = reader.u64()?;
                }
                Change::AddComponent(component, values)
            }
            4 => {
                let component = component(reader)?;
                let field = reader.u8().filter(|f| (*f as usize) < field_count(component))?;
                Change::SetField(component, field, reader.u64()?)
            }
            _ => return None,
        };
        Some(Override { path, target, change })
    }
}

/// A list in its canonical order.
pub fn sorted(mut list: Vec<Override>) -> Vec<Override> {
    list.sort_by_key(Override::sort_key);
    list
}

/// The hash of the lists that apply to one instance's members, inner first:
/// with the child payload's content hash, the key a prepared payload is
/// stored under.
pub fn lists_hash(lists: &[Vec<Override>]) -> u64 {
    let mut bytes = Vec::new();
    for list in lists {
        bytes.extend_from_slice(&(list.len() as u64).to_le_bytes());
        for item in list {
            item.encode(&mut bytes);
        }
    }
    fnv1a(&bytes)
}

/// The items of `list` that target the members of the instance itself.
pub fn own_items(list: &[Override]) -> Vec<Override> {
    list.iter().filter(|item| item.path.is_empty()).cloned().collect()
}

/// The lists a nested instance inherits, inner first, their paths relative to
/// it: its parent's list on its instancing entity, then what each list the
/// parent inherited says below that entity.
pub fn descend(nested: &[Override], inherited: &[Vec<Override>], entity: u64) -> Vec<Vec<Override>> {
    let mut lists = vec![nested.to_vec()];
    for list in inherited {
        lists.push(list.iter().filter(|item| item.path.first() == Some(&entity)).map(|item| Override { path: item.path[1..].to_vec(), ..item.clone() }).collect());
    }
    lists
}

/// Why a list could not be applied to a set of members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApplyError {
    pub target: u64,
    pub kind: u8,
}

/// Applies the items of one list that target the members themselves, kind by
/// kind in `order`, the items of a kind in the list's canonical order or its
/// reverse. `STATED_ORDER` forward is the specified application; the other
/// orders exist to measure that a validated list gives one result anyway.
pub fn apply_list(members: &mut BTreeMap<u64, Proto>, list: &[Override], order: &[u8; 5], reversed: bool) -> Result<(), ApplyError> {
    for &kind in order {
        let mut items: Vec<&Override> = list.iter().filter(|item| item.path.is_empty() && item.kind() == kind).collect();
        if reversed {
            items.reverse();
        }
        for item in items {
            let refused = ApplyError { target: item.target, kind };
            match &item.change {
                Change::RemoveEntity => {
                    members.remove(&item.target).ok_or(refused)?;
                }
                Change::AddEntity(proto) => {
                    if members.insert(item.target, *proto).is_some() {
                        return Err(refused);
                    }
                }
                Change::RemoveComponent(component) => {
                    let member = members.get_mut(&item.target).filter(|m| m.has(*component)).ok_or(refused)?;
                    member.remove(*component);
                }
                Change::AddComponent(component, values) => {
                    let member = members.get_mut(&item.target).filter(|m| !m.has(*component)).ok_or(refused)?;
                    member.add(*component, values);
                }
                Change::SetField(component, field, value) => {
                    let member = members.get_mut(&item.target).filter(|m| m.has(*component)).ok_or(refused)?;
                    member.set(*component, *field, *value);
                }
            }
        }
    }
    Ok(())
}

/// The members of an instance: the payload's prototypes with every list
/// applied in the stated order, inner lists first.
pub fn effective(payload: &Payload, lists: &[Vec<Override>]) -> Result<Vec<Proto>, ApplyError> {
    let mut members: BTreeMap<u64, Proto> = payload.protos.iter().map(|proto| (proto.local, *proto)).collect();
    for list in lists {
        apply_list(&mut members, list, &STATED_ORDER, false)?;
    }
    Ok(members.into_values().collect())
}

// ---------------------------------------------------------------------------
// The payload
// ---------------------------------------------------------------------------

/// A nested instance as the parent's payload records it: the instancing
/// entity, the child scene, the content hash of the child payload the
/// overrides were validated against, and the list in canonical order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nested {
    pub entity: u64,
    pub asset: u64,
    pub child_hash: u64,
    pub overrides: Vec<Override>,
}

/// The canonical payload of a scene: the prototypes sorted by local
/// identifier, the nested instances sorted by instancing entity, the bytes
/// and their content hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub asset: u64,
    pub protos: Vec<Proto>,
    pub nested: Vec<Nested>,
    pub bytes: Vec<u8>,
    pub content_hash: u64,
}

impl Payload {
    fn build(asset: u64, mut protos: Vec<Proto>, mut nested: Vec<Nested>) -> Payload {
        protos.sort_by_key(|proto| proto.local);
        nested.sort_by_key(|entry| entry.entity);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&asset.to_le_bytes());
        bytes.extend_from_slice(&(protos.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(nested.len() as u64).to_le_bytes());
        for proto in &protos {
            proto.encode(&mut bytes);
        }
        for entry in &nested {
            bytes.extend_from_slice(&entry.entity.to_le_bytes());
            bytes.extend_from_slice(&entry.asset.to_le_bytes());
            bytes.extend_from_slice(&entry.child_hash.to_le_bytes());
            bytes.extend_from_slice(&(entry.overrides.len() as u64).to_le_bytes());
            for item in &entry.overrides {
                item.encode(&mut bytes);
            }
        }
        let content_hash = fnv1a(&bytes);
        Payload { asset, protos, nested, bytes, content_hash }
    }

    /// Reads a payload back from its bytes; anything truncated, trailing or
    /// out of the schema is refused.
    pub fn decode(bytes: &[u8]) -> Option<Payload> {
        let mut reader = Reader { bytes, at: 0 };
        let asset = reader.u64()?;
        let proto_count = reader.u64()?;
        let nested_count = reader.u64()?;
        let mut protos = Vec::new();
        for _ in 0..proto_count {
            protos.push(Proto::decode(&mut reader)?);
        }
        let mut nested = Vec::new();
        for _ in 0..nested_count {
            let (entity, child, child_hash, count) = (reader.u64()?, reader.u64()?, reader.u64()?, reader.u64()?);
            let mut overrides = Vec::new();
            for _ in 0..count {
                overrides.push(Override::decode(&mut reader)?);
            }
            nested.push(Nested { entity, asset: child, child_hash, overrides });
        }
        (reader.at == bytes.len()).then(|| Payload { asset, protos, nested, bytes: bytes.to_vec(), content_hash: fnv1a(bytes) })
    }

    pub fn proto(&self, local: u64) -> Option<&Proto> {
        self.protos.binary_search_by_key(&local, |proto| proto.local).ok().map(|at| &self.protos[at])
    }

    pub fn nested_of(&self, entity: u64) -> Option<&Nested> {
        self.nested.binary_search_by_key(&entity, |entry| entry.entity).ok().map(|at| &self.nested[at])
    }

    pub fn override_count(&self) -> usize {
        self.nested.iter().map(|entry| entry.overrides.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// The source and the import
// ---------------------------------------------------------------------------

/// A scene source as a structure (the text syntax is not what this spike
/// measures): the entities in the order written, and per instancing entity
/// the override list in the order written, a position in either being what
/// a refusal names.
#[derive(Clone, Debug, Default)]
pub struct Source {
    pub asset: u64,
    pub entities: Vec<Proto>,
    pub overrides: BTreeMap<u64, Vec<Override>>,
}

/// Why the import refuses a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefusalName {
    LocalIdentifierZero,
    LocalIdentifierUsedTwice,
    ChildSceneUnknown,
    ChildPayloadAtAnotherHash,
    OverridesOnAnEntityThatInstancesNothing,
    FieldUnknown,
    InstancingComponentOverridden,
    AddedEntityOutsideTheAuthoredRange,
    OverrideRepeated,
    ValueOnRemovedEntity,
    ComponentOnRemovedEntity,
    EntityAddedUnderRemovedParent,
    OverrideIntoRemovedInstance,
    AddedEntityModifiedByItsOwnList,
    ValueOnRemovedComponent,
    TwoValuesForOneField,
    ComponentAddedAndRemoved,
    AddedEntityUnderHeldIdentifier,
    TargetMissing,
    ParentMissing,
    ComponentMissing,
    ComponentAlreadyHeld,
}

impl RefusalName {
    pub fn words(self) -> &'static str {
        match self {
            RefusalName::LocalIdentifierZero => "a local identifier of zero",
            RefusalName::LocalIdentifierUsedTwice => "a local identifier used twice",
            RefusalName::ChildSceneUnknown => "a child scene the store does not hold",
            RefusalName::ChildPayloadAtAnotherHash => "a nested payload at another content hash than the one its parent names",
            RefusalName::OverridesOnAnEntityThatInstancesNothing => "overrides on an entity that instances nothing",
            RefusalName::FieldUnknown => "a component or a field the schema lacks",
            RefusalName::InstancingComponentOverridden => "an override of the instancing component",
            RefusalName::AddedEntityOutsideTheAuthoredRange => "an added entity outside the authored range of local identifiers",
            RefusalName::OverrideRepeated => "the same override twice",
            RefusalName::ValueOnRemovedEntity => "a field value on an entity the same list removes",
            RefusalName::ComponentOnRemovedEntity => "a component on an entity the same list removes",
            RefusalName::EntityAddedUnderRemovedParent => "an entity added under a parent the same list removes",
            RefusalName::OverrideIntoRemovedInstance => "an override into a nested instance the same list removes",
            RefusalName::AddedEntityModifiedByItsOwnList => "an override on an entity the same list adds",
            RefusalName::ValueOnRemovedComponent => "a field value on a component the same list removes",
            RefusalName::TwoValuesForOneField => "two values for one field",
            RefusalName::ComponentAddedAndRemoved => "a component both added and removed",
            RefusalName::AddedEntityUnderHeldIdentifier => "an added entity under an identifier the child holds",
            RefusalName::TargetMissing => "an override whose target does not exist in the child scene",
            RefusalName::ParentMissing => "an entity added under a parent the child scene does not hold",
            RefusalName::ComponentMissing => "an override on a component its target does not hold",
            RefusalName::ComponentAlreadyHeld => "a component added to an entity that holds it",
        }
    }
}

/// A refusal by name: what is wrong, on which entity of the source (the
/// instancing entity for an override), at which position of its list (of the
/// entities for an entity).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub name: RefusalName,
    pub entity: u64,
    pub position: usize,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (entity {:#018x}, position {})", self.name.words(), self.entity, self.position)
    }
}

pub type Lookup<'a> = &'a dyn Fn(u64) -> Option<Arc<Payload>>;

/// The members of the instance reached from `child` through `path`, as the
/// scenes below hold them: every list of a scene of the subtree applied,
/// inner first, and nothing of the list being validated.
pub fn view(lookup: Lookup<'_>, child: &Arc<Payload>, path: &[u64]) -> Result<BTreeMap<u64, Proto>, RefusalName> {
    let mut payload = child.clone();
    let mut inherited: Vec<Vec<Override>> = Vec::new();
    let members_of = |payload: &Payload, inherited: &[Vec<Override>]| {
        let lists: Vec<Vec<Override>> = inherited.iter().map(|list| own_items(list)).collect();
        effective(payload, &lists).map_err(|_| RefusalName::TargetMissing)
    };
    let mut members = members_of(&payload, &inherited)?;
    for entity in path {
        let nested = members.iter().find(|proto| proto.local == *entity && proto.has(INSTANCING)).and_then(|_| payload.nested_of(*entity)).ok_or(RefusalName::TargetMissing)?;
        let next = lookup(nested.asset).ok_or(RefusalName::ChildSceneUnknown)?;
        if next.content_hash != nested.child_hash {
            return Err(RefusalName::ChildPayloadAtAnotherHash);
        }
        inherited = descend(&nested.overrides, &inherited, *entity);
        payload = next;
        members = members_of(&payload, &inherited)?;
    }
    Ok(members.into_iter().map(|proto| (proto.local, proto)).collect())
}

type Address = (Vec<u64>, u64);

/// The conflicts within one list, which need nothing of the child: every
/// item is checked against the whole list, so that the verdict does not
/// depend on the order written, and the first offending item is named.
fn list_conflict(list: &[Override]) -> Option<(usize, RefusalName)> {
    let address = |item: &Override| -> Address { (item.path.clone(), item.target) };
    let mut removed: BTreeMap<Address, u32> = BTreeMap::new();
    let mut added: BTreeMap<Address, u32> = BTreeMap::new();
    let mut parents: BTreeSet<Address> = BTreeSet::new();
    let mut component_removed: BTreeMap<(Address, u8), u32> = BTreeMap::new();
    let mut component_added: BTreeMap<(Address, u8), u32> = BTreeMap::new();
    let mut values: BTreeMap<(Address, u8, u8), u32> = BTreeMap::new();
    let mut valued_components: BTreeSet<(Address, u8)> = BTreeSet::new();
    let mut valued: BTreeSet<Address> = BTreeSet::new();
    let mut component_changed: BTreeSet<Address> = BTreeSet::new();
    let mut through: BTreeSet<Address> = BTreeSet::new();
    for item in list {
        let at = address(item);
        for depth in 0..item.path.len() {
            through.insert((item.path[..depth].to_vec(), item.path[depth]));
        }
        match &item.change {
            Change::RemoveEntity => *removed.entry(at).or_default() += 1,
            Change::AddEntity(proto) => {
                *added.entry(at).or_default() += 1;
                if proto.parent != 0 {
                    parents.insert((item.path.clone(), proto.parent));
                }
            }
            Change::RemoveComponent(component) => {
                *component_removed.entry((at.clone(), *component)).or_default() += 1;
                component_changed.insert(at);
            }
            Change::AddComponent(component, _) => {
                *component_added.entry((at.clone(), *component)).or_default() += 1;
                component_changed.insert(at);
            }
            Change::SetField(component, field, _) => {
                *values.entry((at.clone(), *component, *field)).or_default() += 1;
                valued_components.insert((at.clone(), *component));
                valued.insert(at);
            }
        }
    }
    for (position, item) in list.iter().enumerate() {
        let at = address(item);
        let into_removed = (0..item.path.len()).any(|depth| removed.contains_key(&(item.path[..depth].to_vec(), item.path[depth])));
        if into_removed {
            return Some((position, RefusalName::OverrideIntoRemovedInstance));
        }
        let name = match &item.change {
            Change::RemoveEntity => {
                if removed[&at] > 1 {
                    Some(RefusalName::OverrideRepeated)
                } else if added.contains_key(&at) {
                    Some(RefusalName::AddedEntityUnderHeldIdentifier)
                } else if valued.contains(&at) {
                    Some(RefusalName::ValueOnRemovedEntity)
                } else if component_changed.contains(&at) {
                    Some(RefusalName::ComponentOnRemovedEntity)
                } else if parents.contains(&at) {
                    Some(RefusalName::EntityAddedUnderRemovedParent)
                } else if through.contains(&at) {
                    Some(RefusalName::OverrideIntoRemovedInstance)
                } else {
                    None
                }
            }
            Change::AddEntity(proto) => {
                if added[&at] > 1 {
                    Some(RefusalName::LocalIdentifierUsedTwice)
                } else if removed.contains_key(&at) {
                    Some(RefusalName::AddedEntityUnderHeldIdentifier)
                } else if valued.contains(&at) || component_changed.contains(&at) {
                    Some(RefusalName::AddedEntityModifiedByItsOwnList)
                } else if proto.parent != 0 && removed.contains_key(&(item.path.clone(), proto.parent)) {
                    Some(RefusalName::EntityAddedUnderRemovedParent)
                } else {
                    None
                }
            }
            Change::RemoveComponent(component) => {
                let key = (at.clone(), *component);
                if removed.contains_key(&at) {
                    Some(RefusalName::ComponentOnRemovedEntity)
                } else if added.contains_key(&at) {
                    Some(RefusalName::AddedEntityModifiedByItsOwnList)
                } else if component_added.contains_key(&key) {
                    Some(RefusalName::ComponentAddedAndRemoved)
                } else if component_removed[&key] > 1 {
                    Some(RefusalName::OverrideRepeated)
                } else if valued_components.contains(&key) {
                    Some(RefusalName::ValueOnRemovedComponent)
                } else {
                    None
                }
            }
            Change::AddComponent(component, _) => {
                let key = (at.clone(), *component);
                if removed.contains_key(&at) {
                    Some(RefusalName::ComponentOnRemovedEntity)
                } else if added.contains_key(&at) {
                    Some(RefusalName::AddedEntityModifiedByItsOwnList)
                } else if component_removed.contains_key(&key) {
                    Some(RefusalName::ComponentAddedAndRemoved)
                } else if component_added[&key] > 1 {
                    Some(RefusalName::OverrideRepeated)
                } else if valued_components.contains(&key) {
                    Some(RefusalName::TwoValuesForOneField)
                } else {
                    None
                }
            }
            Change::SetField(component, field, _) => {
                let key = (at.clone(), *component);
                if removed.contains_key(&at) {
                    Some(RefusalName::ValueOnRemovedEntity)
                } else if added.contains_key(&at) {
                    Some(RefusalName::AddedEntityModifiedByItsOwnList)
                } else if component_removed.contains_key(&key) {
                    Some(RefusalName::ValueOnRemovedComponent)
                } else if component_added.contains_key(&key) || values[&(at.clone(), *component, *field)] > 1 {
                    Some(RefusalName::TwoValuesForOneField)
                } else {
                    None
                }
            }
        };
        if let Some(name) = name {
            return Some((position, name));
        }
    }
    None
}

/// What the schema alone refuses of one item.
fn schema_refusal(item: &Override) -> Option<RefusalName> {
    let known = |component: u8, field: u8| component < COMPONENT_COUNT && (field as usize) < field_count(component);
    match &item.change {
        Change::RemoveEntity => None,
        Change::AddEntity(proto) => {
            if proto.local != item.target || proto.mask >> COMPONENT_COUNT != 0 {
                Some(RefusalName::FieldUnknown)
            } else if proto.has(INSTANCING) {
                Some(RefusalName::InstancingComponentOverridden)
            } else if item.target == 0 || item.target & RUNTIME_RANGE != 0 {
                Some(RefusalName::AddedEntityOutsideTheAuthoredRange)
            } else {
                None
            }
        }
        Change::RemoveComponent(component) | Change::AddComponent(component, _) => {
            if !known(*component, 0) {
                Some(RefusalName::FieldUnknown)
            } else {
                (*component == INSTANCING).then_some(RefusalName::InstancingComponentOverridden)
            }
        }
        Change::SetField(component, field, _) => {
            if !known(*component, *field) {
                Some(RefusalName::FieldUnknown)
            } else {
                (*component == INSTANCING).then_some(RefusalName::InstancingComponentOverridden)
            }
        }
    }
}

/// Validates one list against the schema, against itself, then against the
/// child scene as the scenes below it hold it.
fn validate_list(lookup: Lookup<'_>, child: &Arc<Payload>, entity: u64, list: &[Override]) -> Result<(), Refusal> {
    let refuse = |position: usize, name: RefusalName| Refusal { name, entity, position };
    for (position, item) in list.iter().enumerate() {
        if let Some(name) = schema_refusal(item) {
            return Err(refuse(position, name));
        }
    }
    if let Some((position, name)) = list_conflict(list) {
        return Err(refuse(position, name));
    }
    let mut views: BTreeMap<Vec<u64>, Result<BTreeMap<u64, Proto>, RefusalName>> = BTreeMap::new();
    for (position, item) in list.iter().enumerate() {
        let members = views.entry(item.path.clone()).or_insert_with(|| view(lookup, child, &item.path)).as_ref().map_err(|name| refuse(position, *name))?;
        let target = members.get(&item.target);
        let name = match &item.change {
            Change::RemoveEntity => target.is_none().then_some(RefusalName::TargetMissing),
            Change::AddEntity(proto) => {
                if target.is_some() {
                    Some(RefusalName::AddedEntityUnderHeldIdentifier)
                } else {
                    (proto.parent != 0 && !members.contains_key(&proto.parent)).then_some(RefusalName::ParentMissing)
                }
            }
            Change::RemoveComponent(component) | Change::SetField(component, _, _) => match target {
                None => Some(RefusalName::TargetMissing),
                Some(proto) => (!proto.has(*component)).then_some(RefusalName::ComponentMissing),
            },
            Change::AddComponent(component, _) => match target {
                None => Some(RefusalName::TargetMissing),
                Some(proto) => proto.has(*component).then_some(RefusalName::ComponentAlreadyHeld),
            },
        };
        if let Some(name) = name {
            return Err(refuse(position, name));
        }
    }
    Ok(())
}

/// The import action as a function of the source and of the payloads of the
/// scenes it nests: the canonical payload, or a refusal by name.
pub fn import(source: &Source, lookup: Lookup<'_>) -> Result<Payload, Refusal> {
    let mut seen = BTreeSet::new();
    for (position, proto) in source.entities.iter().enumerate() {
        if proto.local == 0 {
            return Err(Refusal { name: RefusalName::LocalIdentifierZero, entity: 0, position });
        }
        if !seen.insert(proto.local) {
            return Err(Refusal { name: RefusalName::LocalIdentifierUsedTwice, entity: proto.local, position });
        }
    }
    if let Some(entity) = source.overrides.keys().find(|entity| !source.entities.iter().any(|proto| proto.local == **entity && proto.has(INSTANCING))) {
        return Err(Refusal { name: RefusalName::OverridesOnAnEntityThatInstancesNothing, entity: *entity, position: 0 });
    }
    let mut nested = Vec::new();
    for (position, proto) in source.entities.iter().enumerate().filter(|(_, proto)| proto.has(INSTANCING)) {
        let asset = proto.get(INSTANCING, 0);
        let child = lookup(asset).ok_or(Refusal { name: RefusalName::ChildSceneUnknown, entity: proto.local, position })?;
        let list = source.overrides.get(&proto.local).cloned().unwrap_or_default();
        validate_list(lookup, &child, proto.local, &list)?;
        nested.push(Nested { entity: proto.local, asset, child_hash: child.content_hash, overrides: sorted(list) });
    }
    Ok(Payload::build(source.asset, source.entities.clone(), nested))
}

// ---------------------------------------------------------------------------
// The payload store
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StoreInner {
    versions: BTreeMap<(u64, u64), Arc<Payload>>,
    current: BTreeMap<u64, Arc<Payload>>,
    dependencies: BTreeMap<u64, BTreeSet<u64>>,
    imports: u64,
}

/// The payloads of a process, shared read-only by its worlds: every version
/// imported, the current one per scene, and the scenes each one nests to any
/// depth, which the import records.
#[derive(Default)]
pub struct Store {
    inner: Mutex<StoreInner>,
}

impl Store {
    pub fn new() -> Arc<Store> {
        Arc::new(Store::default())
    }

    /// Imports a source against the current payloads and makes the result the
    /// scene's current payload, read back from its bytes.
    pub fn import(&self, source: &Source) -> Result<Arc<Payload>, Refusal> {
        let current = self.inner.lock().expect("the store is never poisoned").current.clone();
        let payload = import(source, &|asset| current.get(&asset).cloned())?;
        let payload = Arc::new(Payload::decode(&payload.bytes).expect("a payload reads back from its bytes"));
        let mut inner = self.inner.lock().expect("the store is never poisoned");
        let mut dependencies = BTreeSet::new();
        for entry in &payload.nested {
            dependencies.insert(entry.asset);
            dependencies.extend(inner.dependencies.get(&entry.asset).cloned().unwrap_or_default());
        }
        inner.dependencies.insert(payload.asset, dependencies);
        inner.versions.insert((payload.asset, payload.content_hash), payload.clone());
        inner.current.insert(payload.asset, payload.clone());
        inner.imports += 1;
        Ok(payload)
    }

    pub fn current(&self, asset: u64) -> Option<Arc<Payload>> {
        self.inner.lock().expect("the store is never poisoned").current.get(&asset).cloned()
    }

    pub fn version(&self, asset: u64, content_hash: u64) -> Option<Arc<Payload>> {
        self.inner.lock().expect("the store is never poisoned").versions.get(&(asset, content_hash)).cloned()
    }

    /// The scenes `asset` nests, to any depth.
    pub fn dependencies(&self, asset: u64) -> Vec<u64> {
        self.inner.lock().expect("the store is never poisoned").dependencies.get(&asset).map(|set| set.iter().copied().collect()).unwrap_or_default()
    }

    pub fn assets(&self) -> Vec<Arc<Payload>> {
        self.inner.lock().expect("the store is never poisoned").current.values().cloned().collect()
    }

    pub fn imports(&self) -> u64 {
        self.inner.lock().expect("the store is never poisoned").imports
    }
}

type PreparedMembers = BTreeMap<(u64, u64), Arc<Vec<Proto>>>;

/// The effective members prepared outside the tick, keyed by the child
/// payload's content hash and the hash of the lists that apply to it.
#[derive(Default)]
pub struct Prepared {
    payloads: Mutex<PreparedMembers>,
}

impl Prepared {
    pub fn new() -> Arc<Prepared> {
        Arc::new(Prepared::default())
    }

    pub fn get(&self, key: (u64, u64)) -> Option<Arc<Vec<Proto>>> {
        self.payloads.lock().expect("never poisoned").get(&key).cloned()
    }

    pub fn insert(&self, key: (u64, u64), members: Vec<Proto>) -> Arc<Vec<Proto>> {
        self.payloads.lock().expect("never poisoned").entry(key).or_insert_with(|| Arc::new(members)).clone()
    }

    pub fn len(&self) -> usize {
        self.payloads.lock().expect("never poisoned").len()
    }

    /// Prepares the members of an instance of `payload` and of every instance
    /// nested below it whose payload is at the hash its parent names; returns
    /// the instances visited.
    pub fn prepare_family(&self, store: &Store, payload: &Arc<Payload>, inherited: &[Vec<Override>]) -> u64 {
        let lists: Vec<Vec<Override>> = inherited.iter().map(|list| own_items(list)).collect();
        let key = (payload.content_hash, lists_hash(&lists));
        let members = match self.get(key) {
            Some(members) => members,
            None => match effective(payload, &lists) {
                Ok(members) => self.insert(key, members),
                Err(_) => return 0,
            },
        };
        let mut visited = 1;
        for proto in members.iter().filter(|proto| proto.has(INSTANCING)) {
            let Some(nested) = payload.nested_of(proto.local) else { continue };
            if let Some(child) = store.current(nested.asset).filter(|child| child.content_hash == nested.child_hash) {
                visited += self.prepare_family(store, &child, &descend(&nested.overrides, inherited, proto.local));
            }
        }
        visited
    }
}

/// A hash over the content hashes of a set of payloads, to compare the
/// imports of two machines in one value.
pub fn payloads_digest(payloads: &[Arc<Payload>]) -> u64 {
    let mut hasher = Fnv1a::new();
    for payload in payloads {
        hasher.write(&payload.asset.to_le_bytes());
        hasher.write(&payload.content_hash.to_le_bytes());
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf() -> Source {
        let entities = vec![
            Proto::new(10, 0).with(POSITION, &[float_bits(1.0), float_bits(2.0)]).with(LABEL, &[7]),
            Proto::new(11, 10).with(POSITION, &[float_bits(3.0), float_bits(4.0)]).with(MASS, &[float_bits(2.0)]),
            Proto::new(12, 0).with(POSITION, &[float_bits(5.0), float_bits(6.0)]),
        ];
        Source { asset: 1, entities, overrides: BTreeMap::new() }
    }

    fn parent(list: Vec<Override>) -> Source {
        let entities = vec![Proto::new(20, 0).with(INSTANCING, &[1, 0, 0]), Proto::new(21, 0).with(LABEL, &[1])];
        Source { asset: 2, entities, overrides: BTreeMap::from([(20, list)]) }
    }

    fn grandparent(list: Vec<Override>) -> Source {
        let entities = vec![Proto::new(30, 0).with(INSTANCING, &[2, 0, 0])];
        Source { asset: 3, entities, overrides: BTreeMap::from([(30, list)]) }
    }

    fn refused(store: &Store, source: &Source) -> RefusalName {
        store.import(source).expect_err("the import refuses").name
    }

    #[test]
    fn a_payload_reads_back_from_its_bytes_and_a_damaged_one_does_not() {
        let store = Store::new();
        store.import(&leaf()).unwrap();
        let list = vec![
            Override::new(&[], 12, Change::SetField(POSITION, 1, float_bits(9.0))),
            Override::new(&[], 40, Change::AddEntity(Proto::new(40, 10).with(LABEL, &[3]))),
            Override::new(&[], 11, Change::RemoveComponent(MASS)),
            Override::new(&[], 10, Change::AddComponent(MASS, [float_bits(4.0), 0, 0])),
        ];
        let payload = store.import(&parent(list)).unwrap();
        let decoded = Payload::decode(&payload.bytes).unwrap();
        assert_eq!(decoded, *payload);
        assert_eq!(payload.nested[0].overrides.iter().map(Override::kind).collect::<Vec<_>>(), vec![1, 2, 3, 4], "the list is kept in the stated order");
        assert_eq!(payload.nested[0].child_hash, store.current(1).unwrap().content_hash);
        assert!(Payload::decode(&payload.bytes[..payload.bytes.len() - 1]).is_none());
        let mut trailing = payload.bytes.clone();
        trailing.push(0);
        assert!(Payload::decode(&trailing).is_none());
        assert_eq!(store.dependencies(2), vec![1]);
    }

    #[test]
    fn the_conflicts_of_one_list_are_refused_by_name() {
        let store = Store::new();
        store.import(&leaf()).unwrap();
        let set = |target, value: f32| Override::new(&[], target, Change::SetField(POSITION, 0, float_bits(value)));
        let remove = |target| Override::new(&[], target, Change::RemoveEntity);
        assert_eq!(refused(&store, &parent(vec![remove(12), set(12, 1.0)])), RefusalName::ValueOnRemovedEntity);
        assert_eq!(refused(&store, &parent(vec![set(12, 1.0), remove(12)])), RefusalName::ValueOnRemovedEntity, "whatever the order written");
        assert_eq!(refused(&store, &parent(vec![set(12, 1.0), set(12, 2.0)])), RefusalName::TwoValuesForOneField);
        let label = [5, 0, 0];
        assert_eq!(refused(&store, &parent(vec![Override::new(&[], 10, Change::RemoveComponent(LABEL)), Override::new(&[], 10, Change::AddComponent(LABEL, label))])), RefusalName::ComponentAddedAndRemoved);
        assert_eq!(refused(&store, &parent(vec![Override::new(&[], 11, Change::AddEntity(Proto::new(11, 0)))])), RefusalName::AddedEntityUnderHeldIdentifier);
        assert_eq!(refused(&store, &parent(vec![remove(12), Override::new(&[], 12, Change::AddComponent(LABEL, label))])), RefusalName::ComponentOnRemovedEntity);
        assert_eq!(refused(&store, &parent(vec![remove(99)])), RefusalName::TargetMissing);
        assert_eq!(refused(&store, &parent(vec![Override::new(&[], 12, Change::AddComponent(LABEL, label)), Override::new(&[], 12, Change::SetField(LABEL, 0, 1))])), RefusalName::TwoValuesForOneField);
        assert_eq!(refused(&store, &parent(vec![Override::new(&[], 10, Change::AddComponent(LABEL, label))])), RefusalName::ComponentAlreadyHeld);
        assert_eq!(refused(&store, &parent(vec![Override::new(&[], 12, Change::SetField(MASS, 0, 1))])), RefusalName::ComponentMissing);
        assert_eq!(refused(&store, &parent(vec![Override::new(&[], RUNTIME_RANGE | 5, Change::AddEntity(Proto::new(RUNTIME_RANGE | 5, 0)))])), RefusalName::AddedEntityOutsideTheAuthoredRange);
        assert_eq!(refused(&store, &parent(vec![remove(10), Override::new(&[], 41, Change::AddEntity(Proto::new(41, 10)))])), RefusalName::EntityAddedUnderRemovedParent);
        let refusal = store.import(&parent(vec![set(11, 1.0), remove(12), set(12, 1.0)])).unwrap_err();
        assert_eq!((refusal.entity, refusal.position), (20, 1), "the instancing entity and the first offending position");
        let mut twice = leaf();
        twice.entities.push(Proto::new(10, 0));
        assert_eq!(refused(&store, &twice), RefusalName::LocalIdentifierUsedTwice);
    }

    #[test]
    fn an_outer_list_is_validated_against_the_child_as_the_inner_list_left_it() {
        let store = Store::new();
        store.import(&leaf()).unwrap();
        let inner = vec![
            Override::new(&[], 12, Change::RemoveEntity),
            Override::new(&[], 40, Change::AddEntity(Proto::new(40, 0).with(LABEL, &[3]))),
            Override::new(&[], 10, Change::SetField(LABEL, 0, 8)),
            Override::new(&[], 11, Change::RemoveComponent(MASS)),
        ];
        store.import(&parent(inner)).unwrap();
        let into = |target, change| Override::new(&[20], target, change);
        // What an outer list may do to what an inner one set is not a conflict.
        let outer = vec![
            into(10, Change::SetField(LABEL, 0, 9)),
            into(40, Change::RemoveEntity),
            into(11, Change::AddComponent(MASS, [float_bits(1.0), 0, 0])),
            into(41, Change::AddEntity(Proto::new(41, 0))),
            Override::new(&[], 21, Change::SetField(LABEL, 0, 2)),
        ];
        let top = store.import(&grandparent(outer)).unwrap();
        let members = view(&|asset| store.current(asset), &top, &[30, 20]).unwrap();
        assert_eq!(members.keys().copied().collect::<Vec<_>>(), vec![10, 11, 41]);
        assert_eq!(members[&10].get(LABEL, 0), 9, "the outer value wins");
        assert!(members[&11].has(MASS) && members[&41].mask == 0);
        // What the inner list removed is no target, and what it added is held.
        assert_eq!(refused(&store, &grandparent(vec![into(12, Change::SetField(POSITION, 0, 0))])), RefusalName::TargetMissing);
        assert_eq!(refused(&store, &grandparent(vec![into(40, Change::AddEntity(Proto::new(40, 0)))])), RefusalName::AddedEntityUnderHeldIdentifier);
        assert_eq!(refused(&store, &grandparent(vec![Override::new(&[], 20, Change::RemoveEntity), into(10, Change::SetField(LABEL, 0, 1))])), RefusalName::OverrideIntoRemovedInstance);
        assert_eq!(refused(&store, &grandparent(vec![Override::new(&[], 20, Change::SetField(INSTANCING, 0, 9))])), RefusalName::InstancingComponentOverridden);
        // A child re-imported under an unchanged parent is seen by the hash.
        let mut changed = leaf();
        changed.entities[0].set(LABEL, 0, 70);
        store.import(&changed).unwrap();
        assert_eq!(refused(&store, &grandparent(vec![into(10, Change::SetField(LABEL, 0, 1))])), RefusalName::ChildPayloadAtAnotherHash);
    }

    #[test]
    fn a_validated_list_gives_one_result_in_every_order_of_kinds() {
        let store = Store::new();
        let payload = store.import(&leaf()).unwrap();
        let list = sorted(vec![
            Override::new(&[], 12, Change::RemoveEntity),
            Override::new(&[], 40, Change::AddEntity(Proto::new(40, 10).with(LABEL, &[3]))),
            Override::new(&[], 10, Change::RemoveComponent(LABEL)),
            Override::new(&[], 10, Change::AddComponent(MASS, [float_bits(4.0), 0, 0])),
            Override::new(&[], 11, Change::SetField(POSITION, 0, float_bits(8.0))),
        ]);
        store.import(&parent(list.clone())).unwrap();
        let stated = effective(&payload, std::slice::from_ref(&list)).unwrap();
        for order in [[4, 3, 2, 1, 0], [2, 0, 4, 1, 3], [1, 4, 0, 3, 2]] {
            let mut members: BTreeMap<u64, Proto> = payload.protos.iter().map(|proto| (proto.local, *proto)).collect();
            apply_list(&mut members, &list, &order, true).unwrap();
            assert_eq!(members.into_values().collect::<Vec<_>>(), stated);
        }
        assert_eq!(stated.iter().map(|proto| proto.local).collect::<Vec<_>>(), vec![10, 11, 40]);
    }
}
