//! The sequential oracle: the same log of commands and arrivals stepped
//! single-threaded by a plain loop over one buffer, with the records, the
//! unloads, the family spawn and the slot allocation written again rather
//! than shared with the graph, and the overrides resolved by another
//! formulation than the ordered application: for every entity, component
//! and field, the outermost list that speaks decides. Throwaway, never
//! product code.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::sync::Arc;

use reference_workload::{CHUNK, chunk_range, integrated, kinetic_energy, repelled_velocity};

use crate::scene::{COMPONENT_COUNT, Change, INSTANCING, Override, Payload, Proto, REFERENCE, Store, bits_float, field_count};
use crate::state::{
    Actual, DYNAMIC, Event, FailReason, InstancePath, Logged, NO_REFERENCE, NONE, ReadCounts, Record, Resolution, SpatialHash, State, bounced_in, content_hash, repulsion_force, resolve, runtime_id,
    slot_of,
};
use crate::world::{WORLD_SIDE, root_origin};

/// How the oracle gives slots to the members of an instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allocation {
    /// The members of the overridden instance, in the order of their local
    /// identifiers: what a spawn of the prepared payload does.
    Payload,
    /// The child's own entities in the order of their local identifiers,
    /// then per list, inner first, the removed entities freed and the added
    /// ones allocated: what the ordered application in the world does.
    InWorld,
}

/// The ancestors of an instance from its top-level one down: each one's
/// payload and the instancing entity the path goes through.
type Chain = Vec<(Arc<Payload>, u64)>;

/// The lists that apply to the members of the instance at the end of
/// `chain`, inner first: at each ancestor, the items of its list on the
/// instancing entity whose path is the rest of the chain.
pub fn lists_at(chain: &[(Arc<Payload>, u64)]) -> Vec<Vec<Override>> {
    let mut lists = Vec::new();
    for depth in (0..chain.len()).rev() {
        let (payload, entity) = &chain[depth];
        let rest: Vec<u64> = chain[depth + 1..].iter().map(|(_, entity)| *entity).collect();
        let items = payload.nested_of(*entity).map(|nested| nested.overrides.iter().filter(|item| item.path == rest).map(|item| Override { path: Vec::new(), ..item.clone() }).collect());
        lists.push(items.unwrap_or_default());
    }
    lists
}

/// The members of an instance by resolution rather than application: an
/// entity exists by the outermost list that removes or adds it, else by the
/// payload; a component by the outermost list, outside the one that settled
/// the entity, that removes or adds it; a field by the outermost list,
/// outside the one that settled the component, that gives it a value.
pub fn resolved_members(payload: &Payload, lists: &[Vec<Override>]) -> BTreeMap<u64, Proto> {
    let mut candidates: BTreeSet<u64> = payload.protos.iter().map(|proto| proto.local).collect();
    candidates.extend(lists.iter().flatten().filter(|item| matches!(item.change, Change::AddEntity(_))).map(|item| item.target));
    let mut members = BTreeMap::new();
    for local in candidates {
        let mut base = payload.proto(local).copied();
        let mut from = 0;
        for depth in (0..lists.len()).rev() {
            match lists[depth].iter().find(|item| item.target == local && matches!(item.change, Change::RemoveEntity | Change::AddEntity(_))).map(|item| &item.change) {
                Some(Change::AddEntity(proto)) => {
                    base = Some(*proto);
                    from = depth + 1;
                    break;
                }
                Some(_) => {
                    base = None;
                    break;
                }
                None => {}
            }
        }
        let Some(base) = base else { continue };
        let mut member = Proto::new(local, base.parent);
        for component in 0..COMPONENT_COUNT {
            let mut held = base.has(component).then(|| base.values(component));
            let mut values_from = from;
            for depth in (from..lists.len()).rev() {
                let settled = lists[depth].iter().find_map(|item| match &item.change {
                    Change::RemoveComponent(c) if item.target == local && *c == component => Some(None),
                    Change::AddComponent(c, values) if item.target == local && *c == component => Some(Some(*values)),
                    _ => None,
                });
                if let Some(settled) = settled {
                    held = settled;
                    values_from = depth + 1;
                    break;
                }
            }
            let Some(mut values) = held else { continue };
            for (field, value) in values.iter_mut().enumerate().take(field_count(component)) {
                for depth in (values_from..lists.len()).rev() {
                    let given = lists[depth].iter().find_map(|item| match item.change {
                        Change::SetField(c, f, given) if item.target == local && c == component && f as usize == field => Some(given),
                        _ => None,
                    });
                    if let Some(given) = given {
                        *value = given;
                        break;
                    }
                }
            }
            member.add(component, &values);
        }
        members.insert(local, member);
    }
    members
}

/// One instance of a family as the sources state it, for the checks of the
/// report: its path below the top-level instance, its payload, the lists
/// that apply to it, and the local identifiers it holds by set arithmetic
/// alone (the payload's, minus what each list removes, plus what it adds).
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub locals: Vec<u64>,
    pub payload: Arc<Payload>,
    pub lists: Vec<Vec<Override>>,
    pub ids: BTreeSet<u64>,
    /// The payload in the store is at another hash than its parent names.
    pub failed: bool,
}

/// The instances of a family of `root`, in the canonical order of their paths.
pub fn family_tree(store: &Store, root: &Arc<Payload>) -> Vec<TreeNode> {
    fn visit(store: &Store, payload: &Arc<Payload>, chain: &Chain, failed: bool, nodes: &mut Vec<TreeNode>) {
        let lists = lists_at(chain);
        let mut ids: BTreeSet<u64> = payload.protos.iter().map(|proto| proto.local).collect();
        for list in &lists {
            for item in list {
                match item.change {
                    Change::RemoveEntity => {
                        ids.remove(&item.target);
                    }
                    Change::AddEntity(_) => {
                        ids.insert(item.target);
                    }
                    _ => {}
                }
            }
        }
        let locals = chain.iter().map(|(_, entity)| *entity).collect();
        if failed {
            nodes.push(TreeNode { locals, payload: payload.clone(), lists, ids: BTreeSet::new(), failed });
            return;
        }
        nodes.push(TreeNode { locals, payload: payload.clone(), lists, ids: ids.clone(), failed });
        for nested in payload.nested.iter().filter(|nested| ids.contains(&nested.entity)) {
            let Some(child) = store.current(nested.asset) else { continue };
            let mut chain = chain.clone();
            chain.push((payload.clone(), nested.entity));
            visit(store, &child, &chain, child.content_hash != nested.child_hash, nodes);
        }
    }
    let mut nodes = Vec::new();
    visit(store, root, &Vec::new(), false, &mut nodes);
    nodes
}

struct OracleRecord {
    index: u32,
    asset: Option<u64>,
    wanted: bool,
    required: bool,
    actual: Actual,
    chain: Chain,
    origin: (f32, f32),
    expected_hash: Option<u64>,
    members: Vec<(u64, u64)>,
}

/// What the oracle computed: the truth of every tick for one log.
#[derive(Clone, Debug, Default)]
pub struct OracleRun {
    pub hashes: Vec<u64>,
    pub content: Vec<u64>,
    pub events: Vec<Event>,
    pub reads: ReadCounts,
    /// Reference reads that disagree with the oracle's own residency.
    pub check_failures: u64,
    /// Log records the oracle's own validation would have answered otherwise.
    pub record_mismatches: u64,
    /// Ticks completed with a required, wanted instance neither loaded nor failed.
    pub required_missing: u64,
    pub spawned_entities: u64,
    pub despawned_entities: u64,
    pub slots: usize,
}

struct Oracle<'a> {
    store: &'a Store,
    allocation: Allocation,
    state: State,
    free: BinaryHeap<Reverse<usize>>,
    records: BTreeMap<InstancePath, OracleRecord>,
    paths: Vec<InstancePath>,
    digests: Vec<u64>,
    available: BTreeMap<u64, Arc<Payload>>,
    tick: u32,
    run: OracleRun,
}

impl Oracle<'_> {
    fn record_index(&mut self, path: &InstancePath) -> u32 {
        if let Some(record) = self.records.get(path) {
            return record.index;
        }
        let index = self.paths.len() as u32;
        self.paths.push(path.clone());
        self.digests.push(path.digest());
        self.records.insert(
            path.clone(),
            OracleRecord { index, asset: None, wanted: false, required: false, actual: Actual::Absent, chain: Vec::new(), origin: (0.0, 0.0), expected_hash: None, members: Vec::new() },
        );
        index
    }

    fn allocate(&mut self) -> usize {
        match self.free.pop() {
            Some(Reverse(slot)) => slot,
            None => self.state.push_slot(),
        }
    }

    fn input(&mut self, logged: &Logged) {
        let accepted = match logged.record {
            Record::Load { root, asset, required } => {
                let path = InstancePath::root(root);
                let free = self.records.get(&path).is_none_or(|record| !record.wanted && record.asset.is_none_or(|held| held == asset));
                let accepted = free && self.store.current(asset).is_some();
                if accepted {
                    self.record_index(&path);
                    let record = self.records.get_mut(&path).expect("just created");
                    (record.asset, record.wanted, record.required, record.origin) = (Some(asset), true, required, root_origin(root));
                }
                accepted
            }
            Record::Unload { root } => match self.records.get_mut(&InstancePath::root(root)) {
                Some(record) if record.wanted => {
                    record.wanted = false;
                    true
                }
                _ => false,
            },
            Record::Arrival { asset, content_hash } => match self.store.version(asset, content_hash) {
                Some(payload) => {
                    self.available.insert(asset, payload);
                    true
                }
                None => false,
            },
        };
        if accepted != logged.accepted || logged.tick != self.tick {
            self.run.record_mismatches += 1;
        }
    }

    fn passes(&mut self, spatial: &mut SpatialHash) {
        let state = &mut self.state;
        let n = state.len();
        spatial.build(&state.spatial, &state.x, &state.y);
        // The reference system reads the positions before they move and the
        // tables as the last fixed point left them.
        for i in 0..n {
            if !state.alive[i] {
                continue;
            }
            if !state.has(i, REFERENCE) {
                state.observed[i] = NO_REFERENCE;
                continue;
            }
            let target = state.reference_record[i];
            let local = state.reference_local[i];
            let record = self.paths.get(target as usize).map(|path| &self.records[path]);
            let resolution = match record {
                Some(record) => resolve(state, record.actual == Actual::Loaded, &record.members, target, local),
                None => Resolution::Absent,
            };
            let expected = record.is_some_and(|record| record.actual == Actual::Loaded && record.members.iter().any(|member| member.0 == local));
            if matches!(resolution, Resolution::Present(..)) != expected || resolution == Resolution::Stale {
                self.run.check_failures += 1;
            }
            state.observed[i] = self.run.reads.record(resolution);
        }
        for i in 0..n {
            if state.alive[i] && state.mask[i] & DYNAMIC == DYNAMIC {
                let (fx, fy) = repulsion_force(&state.x, &state.y, spatial, i);
                state.vx[i] = repelled_velocity(state.vx[i], fx, state.mass[i]);
                state.vy[i] = repelled_velocity(state.vy[i], fy, state.mass[i]);
            }
        }
        let mut energy = 0.0_f32;
        for chunk in 0..n.div_ceil(CHUNK) {
            let mut sum = 0.0_f32;
            for i in chunk_range(chunk, n) {
                if state.alive[i] && state.mask[i] & DYNAMIC == DYNAMIC {
                    let (x, vx) = bounced_in(integrated(state.x[i], state.vx[i]), state.vx[i], WORLD_SIDE);
                    let (y, vy) = bounced_in(integrated(state.y[i], state.vy[i]), state.vy[i], WORLD_SIDE);
                    (state.x[i], state.y[i], state.vx[i], state.vy[i]) = (x, y, vx, vy);
                    sum += kinetic_energy(vx, vy, state.mass[i]);
                }
            }
            energy += sum;
        }
        state.energy = energy;
    }

    fn structural(&mut self) {
        let unloading: Vec<InstancePath> = self.records.iter().filter(|(path, record)| path.locals.is_empty() && !record.wanted && record.actual != Actual::Absent).map(|(path, _)| path.clone()).collect();
        for top in unloading {
            let subtree: Vec<InstancePath> = self.records.range(top.clone()..).take_while(|(path, _)| path.starts_with(&top)).map(|(path, _)| path.clone()).collect();
            for path in subtree.iter().rev() {
                let record = self.records.get_mut(path).expect("listed");
                if record.actual == Actual::Loaded {
                    for (_, id) in &record.members {
                        self.state.free_slot(slot_of(*id));
                        self.free.push(Reverse(slot_of(*id)));
                    }
                    self.run.despawned_entities += record.members.len() as u64;
                    self.run.events.push(Event::Despawned { tick: self.tick, path: path.clone(), members: record.members.len() as u32 });
                }
                record.members.clear();
                record.actual = Actual::Absent;
                if *path != top {
                    record.wanted = false;
                }
            }
        }
        let order: Vec<InstancePath> = self.records.keys().cloned().collect();
        for path in &order {
            self.spawn(path);
        }
        self.run.required_missing += self.records.values().filter(|record| record.wanted && record.required && !matches!(record.actual, Actual::Loaded | Actual::Failed(_))).count() as u64;
    }

    fn spawn(&mut self, path: &InstancePath) {
        let record = &self.records[path];
        if !record.wanted || matches!(record.actual, Actual::Loaded | Actual::Failed(_)) {
            return;
        }
        let Some(asset) = record.asset else { return };
        if let Some((_, above)) = path.locals.split_last()
            && self.records[&InstancePath { root: path.root, locals: above.to_vec() }].actual != Actual::Loaded
        {
            return;
        }
        let Some(payload) = self.available.get(&asset).cloned() else {
            self.records.get_mut(path).expect("held").actual = Actual::Requested;
            return;
        };
        if record.expected_hash.is_some_and(|hash| hash != payload.content_hash) {
            self.records.get_mut(path).expect("held").actual = Actual::Failed(FailReason::AnotherContentHash);
            self.run.events.push(Event::Failed { tick: self.tick, path: path.clone(), reason: FailReason::AnotherContentHash });
            return;
        }
        let (index, origin, required, chain) = (record.index, record.origin, record.required, record.chain.clone());
        let lists = lists_at(&chain);
        let resolved = resolved_members(&payload, &lists);
        let slots: BTreeMap<u64, usize> = match self.allocation {
            Allocation::Payload => resolved.keys().map(|local| (*local, self.allocate())).collect(),
            Allocation::InWorld => {
                let mut slots: BTreeMap<u64, usize> = payload.protos.iter().map(|proto| (proto.local, self.allocate())).collect();
                for list in &lists {
                    for item in list.iter().filter(|item| item.change == Change::RemoveEntity) {
                        let slot = slots.remove(&item.target).expect("a validated list removes what the instance holds");
                        self.state.generation[slot] += 1;
                        self.free.push(Reverse(slot));
                    }
                    for item in list.iter().filter(|item| matches!(item.change, Change::AddEntity(_))) {
                        slots.insert(item.target, self.allocate());
                    }
                }
                slots
            }
        };
        let mut members = Vec::with_capacity(resolved.len());
        for (local, proto) in &resolved {
            let reference = match proto.has(REFERENCE).then(|| path.relative(proto.get(REFERENCE, 0), proto.get(REFERENCE, 1))).flatten() {
                Some(target) => self.record_index(&target),
                None => NONE,
            };
            let slot = slots[local];
            self.state.write_proto(slot, index, proto, origin, reference);
            members.push((*local, runtime_id(slot, self.state.generation[slot])));
        }
        self.run.spawned_entities += members.len() as u64;
        let digest = self.state.members_digest(&members, &self.digests);
        self.run.events.push(Event::Spawned { tick: self.tick, path: path.clone(), members: members.len() as u32, digest });
        let record = self.records.get_mut(path).expect("held");
        record.members = members;
        record.actual = Actual::Loaded;
        for (local, proto) in resolved.iter().filter(|(_, proto)| proto.has(INSTANCING)) {
            let Some(nested) = payload.nested_of(*local) else { continue };
            let child = path.child(*local);
            self.record_index(&child);
            let record = self.records.get_mut(&child).expect("just created");
            record.asset = Some(proto.get(INSTANCING, 0));
            record.origin = (origin.0 + bits_float(proto.get(INSTANCING, 1)), origin.1 + bits_float(proto.get(INSTANCING, 2)));
            record.expected_hash = Some(nested.child_hash);
            record.chain = chain.clone();
            record.chain.push((payload.clone(), *local));
            record.wanted = true;
            record.required = required;
            self.spawn(&child);
        }
    }

    fn hash(&mut self) {
        self.run.hashes.push(self.state.hash(&self.digests));
        let records = self.records.iter().map(|(path, record)| (path, record.actual, &record.members[..]));
        self.run.content.push(content_hash(&self.state, &self.digests, records));
    }
}

/// Steps the log's schedule sequentially and returns the truth of every tick.
pub fn run(store: &Store, log: &[Logged], ticks: u32, allocation: Allocation) -> OracleRun {
    let mut oracle = Oracle {
        store,
        allocation,
        state: State::default(),
        free: BinaryHeap::new(),
        records: BTreeMap::new(),
        paths: Vec::new(),
        digests: Vec::new(),
        available: BTreeMap::new(),
        tick: 0,
        run: OracleRun::default(),
    };
    let mut spatial = SpatialHash::new(WORLD_SIDE);
    let mut cursor = 0;
    oracle.hash();
    for tick in 1..=ticks {
        oracle.tick = tick;
        while let Some(logged) = log.get(cursor).filter(|logged| logged.tick <= tick) {
            oracle.input(logged);
            cursor += 1;
        }
        oracle.passes(&mut spatial);
        oracle.structural();
        oracle.hash();
    }
    oracle.run.slots = oracle.state.len();
    oracle.run
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{LABEL, MASS, POSITION, Source, effective, float_bits};

    #[test]
    fn the_resolution_gives_what_the_ordered_application_gives() {
        let store = Store::new();
        let entities = vec![
            Proto::new(10, 0).with(POSITION, &[float_bits(1.0), float_bits(2.0)]).with(LABEL, &[7]),
            Proto::new(11, 0).with(MASS, &[float_bits(2.0)]),
            Proto::new(12, 0).with(LABEL, &[1]),
        ];
        let payload = store.import(&Source { asset: 1, entities, overrides: BTreeMap::new() }).unwrap();
        let own = |target, change| Override::new(&[], target, change);
        let inner = crate::scene::sorted(vec![
            own(12, Change::RemoveEntity),
            own(40, Change::AddEntity(Proto::new(40, 10).with(LABEL, &[3]))),
            own(10, Change::SetField(LABEL, 0, 8)),
            own(11, Change::RemoveComponent(MASS)),
            own(11, Change::AddComponent(LABEL, [5, 0, 0])),
        ]);
        let outer = crate::scene::sorted(vec![
            own(10, Change::SetField(LABEL, 0, 9)),
            own(40, Change::SetField(LABEL, 0, 4)),
            own(11, Change::AddComponent(MASS, [float_bits(4.0), 0, 0])),
            own(11, Change::RemoveComponent(LABEL)),
            own(41, Change::AddEntity(Proto::new(41, 0))),
        ]);
        let outermost = crate::scene::sorted(vec![own(41, Change::RemoveEntity), own(11, Change::SetField(MASS, 0, float_bits(1.0))), own(10, Change::RemoveComponent(LABEL))]);
        let lists = vec![inner, outer, outermost];
        let applied = effective(&payload, &lists).unwrap();
        let resolved: Vec<Proto> = resolved_members(&payload, &lists).into_values().collect();
        assert_eq!(resolved, applied);
        assert_eq!(resolved.iter().map(|proto| proto.local).collect::<Vec<_>>(), vec![10, 11, 40]);
        assert_eq!(resolved[1], Proto::new(11, 0).with(MASS, &[float_bits(1.0)]));
        assert_eq!(resolved[2].get(LABEL, 0), 4);
    }
}
