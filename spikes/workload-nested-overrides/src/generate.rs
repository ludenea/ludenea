//! The generated content: sets of four scenes nested three deep (a level
//! nests buildings, a building nests rooms and a vault) with the five kinds
//! of override on every list, the outer lists overriding what the inner ones
//! set, and the conflicts planted for the import to refuse. Everything is a
//! function of a seed, so that no data file is checked in. Throwaway, never
//! product code.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use reference_workload::SplitMix64;

use crate::scene::{Change, INSTANCING, LABEL, MASS, Override, POSITION, Payload, Proto, REFERENCE, RUNTIME_RANGE, Refusal, RefusalName, Source, Store, VELOCITY, float_bits, own_items, view};

pub const ROOM: u64 = 1;
pub const VAULT: u64 = 2;
pub const BUILDING: u64 = 3;
pub const LEVEL: u64 = 4;
const SCENE_NAMES: [&str; 5] = ["", "room", "vault", "building", "level"];
/// The side of the box a scene's entities are authored in; a nested instance
/// is anchored on a quadrant of its parent's box.
const SIDES: [f32; 5] = [0.0, 64.0, 64.0, 128.0, 256.0];

pub fn asset_id(set: u64, scene: u64) -> u64 {
    (set * 16) + scene
}

pub fn asset_name(asset: u64) -> String {
    format!("{}-{}", SCENE_NAMES[(asset % 16) as usize], asset / 16)
}

/// The size of a set of scenes: the entities each scene authors itself, the
/// instances a building and a level nest (four at most, a building's last
/// one being the vault), and the overrides of each kind in every group of a
/// list (a group being the items that target one instance).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sizes {
    pub name: &'static str,
    pub room: usize,
    pub building: usize,
    pub level: usize,
    pub rooms: usize,
    pub buildings: usize,
    pub per_kind: usize,
}

impl Sizes {
    pub const SMALL: Sizes = Sizes { name: "small", room: 40, building: 32, level: 12, rooms: 3, buildings: 2, per_kind: 3 };
    pub const STANDARD: Sizes = Sizes { name: "standard", room: 150, building: 60, level: 40, rooms: 4, buildings: 4, per_kind: 4 };
    pub const DENSE: Sizes = Sizes { name: "dense", room: 600, building: 240, level: 160, rooms: 4, buildings: 4, per_kind: 12 };

    fn plain(&self, scene: u64) -> usize {
        [0, self.room, self.room, self.building, self.level][scene as usize]
    }

    fn nested(&self, scene: u64) -> usize {
        [0, 0, 0, self.rooms, self.buildings][scene as usize]
    }

    fn child(&self, scene: u64, at: usize) -> u64 {
        match scene {
            BUILDING if at + 1 == self.rooms => VAULT,
            BUILDING => ROOM,
            _ => BUILDING,
        }
    }
}

/// The local identifiers of a set, drawn before any scene is built so that a
/// reference may name an entity of a scene built later.
#[derive(Clone, Debug, Default)]
pub struct Identifiers {
    /// Per scene: the entities it authors, the first quarter being the only
    /// ones that parent another entity.
    pub plain: [Vec<u64>; 5],
    /// Per scene: its instancing entities.
    pub nest: [Vec<u64>; 5],
    /// Per scene: the identifiers the overrides add to an instance of it,
    /// the first half for the list of the scene that nests it and the second
    /// for the list of the scene above.
    pub pool: [Vec<u64>; 5],
}

/// A generated set: its sources, as imported into the store.
#[derive(Clone, Debug)]
pub struct SceneSet {
    pub set: u64,
    pub sizes: Sizes,
    pub ids: Identifiers,
    pub sources: BTreeMap<u64, Source>,
    /// Items of an outer list on something the inner list set, added or changed.
    pub outer_on_inner: u64,
}

struct Generator<'a> {
    set: u64,
    seed: u64,
    sizes: &'a Sizes,
    ids: Identifiers,
}

fn pick(rng: &mut SplitMix64, list: &[u64]) -> u64 {
    list[(rng.next() % list.len() as u64) as usize]
}

fn velocity(rng: &mut SplitMix64) -> u64 {
    float_bits((rng.u01() * 8.0) - 4.0)
}

impl Generator<'_> {
    fn rng(&self, scene: u64, list: u64) -> SplitMix64 {
        SplitMix64::new(self.seed ^ self.set.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (scene << 32) ^ (list << 40))
    }

    /// An entity of `scene` or an identifier an override adds to it.
    fn target_in(&self, rng: &mut SplitMix64, scene: u64) -> u64 {
        if rng.next().is_multiple_of(4) { pick(rng, &self.ids.pool[scene as usize]) } else { pick(rng, &self.ids.plain[scene as usize]) }
    }

    /// A relative reference from an entity of an instance of `scene`: to its
    /// own instance, to the instance above, to a sibling or to a nested one.
    fn reference(&self, rng: &mut SplitMix64, scene: u64) -> [u64; 3] {
        let nested = |rng: &mut SplitMix64, parent: u64| {
            let at = (rng.next() % self.sizes.nested(parent) as u64) as usize;
            (self.ids.nest[parent as usize][at], self.sizes.child(parent, at))
        };
        match (scene, rng.next() % 4) {
            (ROOM | VAULT, 0 | 1) | (BUILDING, 0) | (LEVEL, 0 | 1) => [0, 0, self.target_in(rng, scene)],
            (ROOM | VAULT, 2) => [1, 0, pick(rng, &self.ids.plain[BUILDING as usize])],
            (ROOM | VAULT, _) => {
                let (entity, child) = nested(rng, BUILDING);
                [1, entity, self.target_in(rng, child)]
            }
            (BUILDING, 1) => [1, 0, pick(rng, &self.ids.plain[LEVEL as usize])],
            (_, _) => {
                let (entity, child) = nested(rng, scene);
                [0, entity, self.target_in(rng, child)]
            }
        }
    }

    fn values(&self, rng: &mut SplitMix64, scene: u64, component: u8) -> [u64; 3] {
        match component {
            LABEL => [rng.next(), 0, 0],
            MASS => [float_bits(rng.mass()), 0, 0],
            POSITION => [float_bits(rng.u01() * SIDES[scene as usize]), float_bits(rng.u01() * SIDES[scene as usize]), 0],
            REFERENCE => self.reference(rng, scene),
            _ => [velocity(rng), velocity(rng), 0],
        }
    }

    fn components(&self, rng: &mut SplitMix64, scene: u64, proto: &mut Proto) {
        proto.add(POSITION, &self.values(rng, scene, POSITION));
        match rng.next() % 10 {
            0..=7 => {
                proto.add(VELOCITY, &self.values(rng, scene, VELOCITY));
                proto.add(MASS, &self.values(rng, scene, MASS));
            }
            8 => proto.add(MASS, &self.values(rng, scene, MASS)),
            _ => {}
        }
        if rng.next().is_multiple_of(3) {
            proto.add(LABEL, &self.values(rng, scene, LABEL));
        }
        if rng.next().is_multiple_of(5) {
            proto.add(REFERENCE, &self.values(rng, scene, REFERENCE));
        }
    }

    /// The entities overrides leave alone: the ones that may parent another.
    fn parents(&self, scene: u64) -> &[u64] {
        let plain = &self.ids.plain[scene as usize];
        &plain[..(plain.len() / 4).max(1)]
    }

    fn entities(&self, scene: u64) -> Vec<Proto> {
        let mut rng = self.rng(scene, 0xff);
        let mut entities = Vec::new();
        for (at, local) in self.ids.plain[scene as usize].iter().enumerate() {
            let parent = if at < self.parents(scene).len() { 0 } else { pick(&mut rng, self.parents(scene)) };
            let mut proto = Proto::new(*local, parent);
            self.components(&mut rng, scene, &mut proto);
            entities.push(proto);
        }
        for (at, local) in self.ids.nest[scene as usize].iter().enumerate() {
            let child = self.sizes.child(scene, at);
            let side = SIDES[child as usize];
            entities.push(Proto::new(*local, 0).with(INSTANCING, &[asset_id(self.set, child), float_bits((at % 2) as f32 * side), float_bits((at / 2) as f32 * side)]));
        }
        entities
    }

    /// The five kinds, `per_kind` items each, on entities of `members` no
    /// other item of the group touches: a validated list by construction.
    fn random_items(&self, rng: &mut SplitMix64, scene: u64, members: &BTreeMap<u64, Proto>, used: &mut BTreeSet<u64>, path: &[u64], pool: &[u64]) -> Vec<Override> {
        let parents = self.parents(scene);
        let mut order: Vec<u64> = members.values().filter(|proto| !proto.has(INSTANCING) && !parents.contains(&proto.local)).map(|proto| proto.local).collect();
        for at in (1..order.len()).rev() {
            order.swap(at, (rng.next() % (at as u64 + 1)) as usize);
        }
        let take = |used: &mut BTreeSet<u64>, wanted: &dyn Fn(&Proto) -> bool| {
            let local = order.iter().copied().find(|local| !used.contains(local) && wanted(&members[local]))?;
            used.insert(local);
            Some(local)
        };
        let mut items = Vec::new();
        for n in 0..self.sizes.per_kind {
            if let Some(target) = take(used, &|_| true) {
                items.push(Override::new(path, target, Change::RemoveEntity));
            }
            let parent = if n % 2 == 0 { 0 } else { pick(rng, parents) };
            let mut added = Proto::new(pool[n], parent);
            self.components(rng, scene, &mut added);
            items.push(Override::new(path, added.local, Change::AddEntity(added)));
            let component = [LABEL, VELOCITY, MASS, REFERENCE][n % 4];
            if let Some(target) = take(used, &|proto| proto.has(component)) {
                items.push(Override::new(path, target, Change::RemoveComponent(component)));
            }
            let component = [LABEL, REFERENCE, VELOCITY, MASS][n % 4];
            if let Some(target) = take(used, &|proto| !proto.has(component)) {
                items.push(Override::new(path, target, Change::AddComponent(component, self.values(rng, scene, component))));
            }
            let (component, field) = [(POSITION, 0), (MASS, 0), (LABEL, 0), (VELOCITY, 1), (REFERENCE, 2), (POSITION, 1)][n % 6];
            if let Some(target) = take(used, &|proto| proto.has(component)) {
                items.push(Override::new(path, target, Change::SetField(component, field, self.values(rng, scene, component)[field as usize])));
            }
        }
        items
    }

    /// What an outer list does to what the inner list did: a value on a field
    /// the inner list set, the removal of an entity it changed and of one it
    /// added, a value on another it added, the removal of a component it
    /// added and the return of one it removed. None is a conflict.
    fn outer_on_inner(&self, rng: &mut SplitMix64, scene: u64, members: &BTreeMap<u64, Proto>, inner: &[Override], used: &mut BTreeSet<u64>, path: &[u64]) -> Vec<Override> {
        let mut done = [false; 6];
        let mut items = Vec::new();
        for item in inner {
            let Some(member) = members.get(&item.target).filter(|_| !used.contains(&item.target)) else { continue };
            let change = match &item.change {
                Change::SetField(component, field, _) if !done[0] && member.has(*component) => {
                    done[0] = true;
                    Change::SetField(*component, *field, self.values(rng, scene, *component)[*field as usize])
                }
                Change::SetField(..) if !done[1] => {
                    done[1] = true;
                    Change::RemoveEntity
                }
                Change::AddEntity(_) if !done[2] => {
                    done[2] = true;
                    Change::RemoveEntity
                }
                Change::AddEntity(_) if !done[3] => {
                    done[3] = true;
                    Change::SetField(POSITION, 0, self.values(rng, scene, POSITION)[0])
                }
                Change::AddComponent(component, _) if !done[4] && member.has(*component) => {
                    done[4] = true;
                    Change::RemoveComponent(*component)
                }
                Change::RemoveComponent(component) if !done[5] && !member.has(*component) => {
                    done[5] = true;
                    Change::AddComponent(*component, self.values(rng, scene, *component))
                }
                _ => continue,
            };
            used.insert(item.target);
            items.push(Override::new(path, item.target, change));
        }
        items
    }

    /// The lists of `scene` on its instancing entities.
    fn lists(&self, store: &Store, scene: u64, outer_on_inner: &mut u64) -> BTreeMap<u64, Vec<Override>> {
        let lookup = |asset: u64| store.current(asset);
        let half = self.sizes.per_kind;
        let mut lists = BTreeMap::new();
        for (at, entity) in self.ids.nest[scene as usize].iter().enumerate() {
            let child_scene = self.sizes.child(scene, at);
            let child = store.current(asset_id(self.set, child_scene)).expect("the nested scene was imported first");
            // A building gives its first two rooms the same list, so that two
            // instances share one prepared payload; every other list is its own.
            let mut rng = self.rng(scene, if scene == BUILDING && at < 2 { 0 } else { at as u64 });
            let members = view(&lookup, &child, &[]).expect("the nested scene is in the store");
            let mut list = Vec::new();
            let mut used = BTreeSet::new();
            // The second building of a level loses its vault: an override that
            // removes an instancing entity removes the instance below it.
            let removed = (scene == LEVEL && at == 1).then(|| *self.ids.nest[BUILDING as usize].last().expect("a building nests"));
            if let Some(vault) = removed {
                list.push(Override::new(&[], vault, Change::RemoveEntity));
            }
            list.extend(self.random_items(&mut rng, child_scene, &members, &mut used, &[], &self.ids.pool[child_scene as usize][..half]));
            for nested in child.nested.iter().filter(|nested| Some(nested.entity) != removed) {
                let below = (nested.asset % 16) as usize;
                let path = [nested.entity];
                let members = view(&lookup, &child, &path).expect("the nested scene is in the store");
                let inner = own_items(&nested.overrides);
                // The last entity the inner list added is left alone by the
                // outer one: an added entity that reaches the world as added.
                let mut used: BTreeSet<u64> = inner.iter().rev().find(|item| matches!(item.change, Change::AddEntity(_))).map(|item| item.target).into_iter().collect();
                let on_inner = self.outer_on_inner(&mut rng, below as u64, &members, &inner, &mut used, &path);
                *outer_on_inner += on_inner.len() as u64;
                list.extend(on_inner);
                list.extend(self.random_items(&mut rng, below as u64, &members, &mut used, &path, &self.ids.pool[below][half..]));
            }
            lists.insert(*entity, list);
        }
        lists
    }
}

/// Generates the four scenes of a set and imports them, the nested ones
/// first. With `overrides` off every list is empty: the control every entity
/// of which keeps the scene's own content.
pub fn generate(store: &Store, set: u64, seed: u64, sizes: &Sizes, overrides: bool) -> Result<SceneSet, Refusal> {
    let mut rng = SplitMix64::new(seed ^ set.wrapping_mul(0xd6e8_feb8_6659_fd93));
    let mut taken = BTreeSet::new();
    let mut draw = |count: usize| -> Vec<u64> {
        let mut drawn = Vec::with_capacity(count);
        while drawn.len() < count {
            // The authored range: the high bit clear, never zero.
            let local = (rng.next() >> 1).max(1);
            if taken.insert(local) {
                drawn.push(local);
            }
        }
        drawn
    };
    let mut ids = Identifiers::default();
    for scene in [ROOM, VAULT, BUILDING, LEVEL] {
        ids.plain[scene as usize] = draw(sizes.plain(scene));
        ids.nest[scene as usize] = draw(sizes.nested(scene));
        ids.pool[scene as usize] = draw(2 * sizes.per_kind);
    }
    let generator = Generator { set, seed, sizes, ids };
    let mut sources = BTreeMap::new();
    let mut outer_on_inner = 0;
    for scene in [ROOM, VAULT, BUILDING, LEVEL] {
        let lists = if overrides { generator.lists(store, scene, &mut outer_on_inner) } else { BTreeMap::new() };
        let source = Source { asset: asset_id(set, scene), entities: generator.entities(scene), overrides: lists };
        store.import(&source)?;
        sources.insert(scene, source);
    }
    Ok(SceneSet { set, sizes: *sizes, ids: generator.ids, sources, outer_on_inner })
}

/// Re-imports the room of a set with one value changed and leaves the
/// building that nests it as it is: the store then holds a child at another
/// content hash than the one its parent's payload names.
pub fn replace_room(store: &Store, set: &SceneSet) -> Arc<Payload> {
    let mut room = set.sources[&ROOM].clone();
    let last = room.entities.len() - 1;
    room.entities[last].set(POSITION, 0, float_bits(1.0));
    store.import(&room).expect("the changed room imports on its own")
}

/// A conflict planted in a source and what the import answered.
#[derive(Clone, Debug)]
pub struct Planted {
    pub what: &'static str,
    pub place: &'static str,
    pub expected: RefusalName,
    /// The refusal, or none when the import accepted the source.
    pub answer: Option<Refusal>,
    /// One of the four conflicts the measurement names.
    pub named: bool,
}

impl Planted {
    pub fn refused_by_name(&self) -> bool {
        self.answer.is_some_and(|refusal| refusal.name == self.expected)
    }
}

/// Plants conflicts in the lists of a set generated in `store` (a store of
/// its own: an import that wrongly succeeds replaces a payload) and records
/// what the import answers, on three places: the building's list on a room,
/// the level's list on a building, and the level's list on a room of that
/// building through a path of two local identifiers.
pub fn plant(store: &Store, set: &SceneSet) -> Vec<Planted> {
    let lookup = |asset: u64| store.current(asset);
    let room_entity = set.ids.nest[BUILDING as usize][0];
    let building_entity = set.ids.nest[LEVEL as usize][0];
    let places: [(&'static str, u64, u64, Vec<u64>); 3] = [
        ("the building's list on a room", BUILDING, room_entity, vec![]),
        ("the level's list on a building", LEVEL, building_entity, vec![]),
        ("the level's list on a room of the building", LEVEL, building_entity, vec![room_entity]),
    ];
    let mut planted = Vec::new();
    for (place, scene, entity, path) in places {
        let source = &set.sources[&scene];
        let child = store.current(source.entities.iter().find(|proto| proto.local == entity).expect("the instancing entity").get(INSTANCING, 0)).expect("imported");
        let members = view(&lookup, &child, &path).expect("the path exists");
        let list = &source.overrides[&entity];
        let touched: BTreeSet<u64> = list.iter().filter(|item| item.path == path).map(|item| item.target).collect();
        let parents: BTreeSet<u64> = members.values().map(|proto| proto.parent).collect();
        let free: Vec<&Proto> = members.values().filter(|proto| !touched.contains(&proto.local) && !parents.contains(&proto.local) && !proto.has(INSTANCING)).collect();
        let plain = free[0].local;
        let labelled = free.iter().find(|proto| proto.has(LABEL)).expect("an untouched entity with a label").local;
        let bare = free.iter().find(|proto| !proto.has(LABEL)).expect("an untouched entity without a label").local;
        let at = |target: u64, change: Change| Override::new(&path, target, change);
        let value = |target: u64, x: f32| at(target, Change::SetField(POSITION, 0, float_bits(x)));
        let mut cases: Vec<(&'static str, RefusalName, bool, Vec<Override>)> = vec![
            ("a value on a removed entity", RefusalName::ValueOnRemovedEntity, true, vec![at(plain, Change::RemoveEntity), value(plain, 1.0)]),
            ("two values for one field", RefusalName::TwoValuesForOneField, true, vec![value(plain, 1.0), value(plain, 2.0)]),
            ("a component both added and removed", RefusalName::ComponentAddedAndRemoved, true, vec![at(labelled, Change::RemoveComponent(LABEL)), at(labelled, Change::AddComponent(LABEL, [1, 0, 0]))]),
            ("an added entity under an identifier the child holds", RefusalName::AddedEntityUnderHeldIdentifier, true, vec![at(plain, Change::AddEntity(Proto::new(plain, 0)))]),
            ("a component on a removed entity", RefusalName::ComponentOnRemovedEntity, false, vec![at(bare, Change::RemoveEntity), at(bare, Change::AddComponent(LABEL, [1, 0, 0]))]),
            ("a value on an entity the child does not hold", RefusalName::TargetMissing, false, vec![value(0x7777_7777, 1.0)]),
            ("a value on a component its target does not hold", RefusalName::ComponentMissing, false, vec![at(bare, Change::SetField(LABEL, 0, 1))]),
            ("a component added to an entity that holds it", RefusalName::ComponentAlreadyHeld, false, vec![at(labelled, Change::AddComponent(LABEL, [1, 0, 0]))]),
            ("an added entity in the run-time range", RefusalName::AddedEntityOutsideTheAuthoredRange, false, vec![at(RUNTIME_RANGE | 9, Change::AddEntity(Proto::new(RUNTIME_RANGE | 9, 0)))]),
            ("an entity added under a parent the same list removes", RefusalName::EntityAddedUnderRemovedParent, false, vec![at(plain, Change::RemoveEntity), at(0x7777_7778, Change::AddEntity(Proto::new(0x7777_7778, plain)))]),
        ];
        if !path.is_empty() {
            // The child as the inner list left it: what that list added is
            // held, what it removed is no target.
            let inner = own_items(&child.nested_of(path[0]).expect("the nested room").overrides);
            let added = inner.iter().find(|item| matches!(item.change, Change::AddEntity(_)) && !touched.contains(&item.target)).expect("the inner list adds an entity the outer one leaves alone").target;
            debug_assert!(members.contains_key(&added));
            let removed = inner.iter().find(|item| item.change == Change::RemoveEntity).expect("the inner list removes an entity").target;
            cases.push(("an added entity under an identifier the inner list added", RefusalName::AddedEntityUnderHeldIdentifier, true, vec![at(added, Change::AddEntity(Proto::new(added, 0)))]));
            cases.push(("a value on an entity the inner list removed", RefusalName::TargetMissing, false, vec![value(removed, 1.0)]));
            cases.push(("an override into a nested instance the same list removes", RefusalName::OverrideIntoRemovedInstance, false, vec![Override::new(&[], path[0], Change::RemoveEntity)]));
        }
        for (what, expected, named, items) in cases {
            let mut source = source.clone();
            source.overrides.get_mut(&entity).expect("the list").extend(items);
            planted.push(Planted { what, place, expected, answer: store.import(&source).err(), named });
        }
    }
    // A local identifier used twice in a source.
    let mut twice = set.sources[&ROOM].clone();
    twice.entities.push(Proto::new(twice.entities[0].local, 0));
    planted.push(Planted { what: "a local identifier used twice", place: "the room's entities", expected: RefusalName::LocalIdentifierUsedTwice, answer: store.import(&twice).err(), named: false });
    // A child whose change removes the target of its parent's override: the
    // parent's next import fails by name. Last, since it replaces the room.
    let building = &set.sources[&BUILDING];
    let target = building.overrides[&room_entity].iter().find(|item| matches!(item.change, Change::SetField(..))).expect("a value in the building's list").target;
    let mut room = set.sources[&ROOM].clone();
    room.entities.retain(|proto| proto.local != target && proto.parent != target);
    store.import(&room).expect("the room without the entity imports on its own");
    planted.push(Planted { what: "a child changed so that the target of its parent's override is gone", place: "the building's list on a room", expected: RefusalName::TargetMissing, answer: store.import(building).err(), named: false });
    planted.push(Planted { what: "a nested payload at another hash than the one its parent names", place: "the level's list on a room of the building", expected: RefusalName::ChildPayloadAtAnotherHash, answer: store.import(&set.sources[&LEVEL]).err(), named: false });
    planted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Payload, effective};

    #[test]
    fn a_set_is_a_function_of_its_seed_and_holds_the_five_kinds_on_every_list() {
        let (first, second) = (Store::new(), Store::new());
        let set = generate(&first, 0, 7, &Sizes::SMALL, true).unwrap();
        generate(&second, 0, 7, &Sizes::SMALL, true).unwrap();
        for scene in [ROOM, VAULT, BUILDING, LEVEL] {
            assert_eq!(first.current(asset_id(0, scene)).unwrap().bytes, second.current(asset_id(0, scene)).unwrap().bytes);
        }
        let other = Store::new();
        generate(&other, 0, 8, &Sizes::SMALL, true).unwrap();
        assert_ne!(first.current(asset_id(0, LEVEL)).unwrap().content_hash, other.current(asset_id(0, LEVEL)).unwrap().content_hash);
        let level = first.current(asset_id(0, LEVEL)).unwrap();
        let building = first.current(asset_id(0, BUILDING)).unwrap();
        assert_eq!((level.nested.len(), building.nested.len()), (2, 3));
        for payload in [&level, &building] {
            for nested in &payload.nested {
                for kind in 0..5 {
                    assert!(nested.overrides.iter().any(|item| item.kind() == kind), "{} lacks kind {kind}", asset_name(payload.asset));
                }
            }
        }
        assert!(level.nested.iter().all(|nested| nested.overrides.iter().any(|item| item.path.len() == 1)), "the level overrides the rooms of its buildings by a path");
        assert!(set.outer_on_inner >= 8, "the outer lists change what the inner ones set: {}", set.outer_on_inner);
        assert_eq!(building.nested[0].overrides, building.nested[1].overrides, "the first two rooms carry one list");
        assert_eq!(first.dependencies(asset_id(0, LEVEL)), vec![asset_id(0, ROOM), asset_id(0, VAULT), asset_id(0, BUILDING)]);
        let plain = Store::new();
        generate(&plain, 0, 7, &Sizes::SMALL, false).unwrap();
        assert_eq!(plain.current(asset_id(0, LEVEL)).unwrap().override_count(), 0);
        let room: Arc<Payload> = first.current(asset_id(0, ROOM)).unwrap();
        assert_eq!(effective(&room, &[]).unwrap().len(), 40);
    }

    #[test]
    fn every_planted_conflict_is_refused_by_its_name() {
        let store = Store::new();
        let set = generate(&store, 0, 7, &Sizes::SMALL, true).unwrap();
        let planted = plant(&store, &set);
        assert_eq!(planted.iter().filter(|case| case.named).count(), 13, "four conflicts on three places and the identifier an inner list added");
        for case in &planted {
            assert!(case.refused_by_name(), "{} in {}: expected {:?}, answered {:?}", case.what, case.place, case.expected, case.answer);
        }
    }
}
