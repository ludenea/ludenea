//! The large table that the rows judged on a magnitude share. Throwaway,
//! never product code.
//!
//! It is the table of the family `gate-cost` of the sheet of the same-harness
//! stage, stated again here so that a mode can take a part of it, give each
//! content a size, add contents of its own and run at a small size: 2 000
//! roots and 8 000 leaves whose identifiers are drawn from the fixed stream
//! of the spikes, in no relation to the names; the first content of the root
//! of rank `i` lists the four leaves of ranks `4i` to `4i + 3`, and a leaf
//! lists nothing; the entities name the sixteen leaves of ranks 0 to 15. The
//! record of tick 1 holds one required want of `source:host` per root, the
//! record of tick 2 the arrivals of every asset, one batch, and the ticks 3
//! to 102 run with an empty record: their median duration is the tick a
//! magnitude is set against.
//!
//! A table of fewer roots is the first roots of the large one and their
//! leaves, with the identifiers and the contents they have there.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use asset_loader::ResidentStore;
use content_store::content_hash;
use prototype_core::table::AssetType::Parameters;
use prototype_core::table::{Arrival, ArrivalQueue, AssetId, Availability, Command, Dependency, Maker, Owner, Policy, Record, RepulsionTable, Rules, part_bytes, part_len};
use prototype_core::world::{Links, Mode, Size, Wait, World, WorldConfig};
use reference_workload::SplitMix64;

use crate::magnitude::median;

/// The roots of the table as the sheets state it.
pub const STATED_ROOTS: usize = 2_000;
pub const LEAVES_OF_A_ROOT: usize = 4;
/// The entities name the first sixteen leaves, in turn.
pub const POOL: usize = 16;
/// The ticks whose median duration a magnitude is set against.
pub const FIRST_TIMED_TICK: u32 = 3;
pub const LAST_TIMED_TICK: u32 = 102;
pub const HOST: Owner = Owner::World(Maker::Source(3));
pub const TOOL: Owner = Owner::World(Maker::Source(5));

/// How large a mode runs: as its sheet states it, or small, which is the
/// size of its self-check and judges nothing.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    /// The size of the reference workload.
    pub size: Size,
    /// The roots of the large table.
    pub roots: usize,
    /// How many times one evaluation is timed for a median.
    pub evaluations: usize,
}

impl Scale {
    pub const STATED: Scale = Scale { size: Size::Default, roots: STATED_ROOTS, evaluations: 100 };
    pub const SMALL: Scale = Scale { size: Size::Small, roots: STATED_ROOTS / 20, evaluations: 10 };

    pub fn of(small: bool) -> Scale {
        if small { Scale::SMALL } else { Scale::STATED }
    }
}

/// The identifiers of the family: the roots, the leaves, the one asset the
/// table does not hold, then the assets a mode adds.
pub struct Identifiers {
    pub roots: Vec<AssetId>,
    pub leaves: Vec<AssetId>,
    /// The asset `G`, outside the table.
    pub gate: AssetId,
    /// The assets a mode adds, drawn after every other so that none moves.
    pub added: Vec<AssetId>,
}

impl Identifiers {
    /// Drawn from the fixed stream of the spikes, in no relation to the
    /// names, in the order the family `gate-cost` draws them: every root of
    /// the stated table, every leaf, the asset outside the table.
    pub fn drawn(added: usize) -> Identifiers {
        let mut stream = SplitMix64::new(0x6173_7365_7420_6964);
        let mut draw = || (u128::from(stream.next()) << 64) | u128::from(stream.next() | 1);
        let roots: Vec<AssetId> = (0..STATED_ROOTS).map(|_| draw()).collect();
        let leaves: Vec<AssetId> = (0..STATED_ROOTS * LEAVES_OF_A_ROOT).map(|_| draw()).collect();
        let gate = draw();
        let added = (0..added).map(|_| draw()).collect();
        Identifiers { roots, leaves, gate, added }
    }

    /// What the part of the root of rank `root` lists: its four leaves, and
    /// `more` leaves after them.
    pub fn listed(&self, root: usize, more: usize) -> Vec<Dependency> {
        let first = root * LEAVES_OF_A_ROOT;
        (first..first + LEAVES_OF_A_ROOT + more).map(|leaf| (self.leaves[leaf], Parameters)).collect()
    }
}

/// How long a part is made.
#[derive(Clone, Copy, Debug)]
pub enum Length {
    /// An opaque payload of that many bytes after what the part lists: the
    /// parts of the family `gate-cost` carry sixteen.
    Payload(usize),
    /// The whole part padded to that many bytes: the size the domain
    /// manifest gives the content.
    Part(usize),
}

/// The contents of a mode: every part made resident, for a scripted
/// availability source, with the size the domain manifest would give each,
/// the length of its part.
#[derive(Default)]
pub struct Contents {
    pub residents: Arc<ResidentStore>,
    sizes: BTreeMap<u64, u64>,
}

impl Contents {
    /// Makes one part resident and returns its hash.
    pub fn resident(&mut self, strength: f32, dependencies: &[Dependency], length: Length, salt: u32) -> u64 {
        let payload = match length {
            Length::Payload(payload) => payload,
            Length::Part(size) => size.checked_sub(part_len(dependencies, 0)).expect("a part is at least as long as what it lists"),
        };
        let bytes = part_bytes(RepulsionTable { strength, radius: 8.0 }, dependencies, payload, salt);
        let hash = content_hash(&bytes);
        self.sizes.insert(hash, bytes.len() as u64);
        self.residents.insert(hash, bytes).expect("a part of the family is valid");
        hash
    }

    /// The size the domain manifest gives a content.
    pub fn size(&self, hash: u64) -> u64 {
        self.sizes[&hash]
    }

    /// The arrival of an asset at a content, naming the size the manifest gives.
    pub fn arrival(&self, asset: AssetId, hash: u64) -> Arrival {
        Arrival::at(asset, hash).sized(self.size(hash))
    }
}

/// The first content of the first roots of the stated table and of their
/// leaves.
pub struct FirstContents {
    pub roots: Vec<u64>,
    pub leaves: Vec<u64>,
}

impl FirstContents {
    /// Made as the family `gate-cost` makes them, each of the length given,
    /// for the first `roots` roots of the stated table and their leaves.
    pub fn of(contents: &mut Contents, identifiers: &Identifiers, roots: usize, length: Length) -> FirstContents {
        let leaves = (0..roots * LEAVES_OF_A_ROOT).map(|leaf| contents.resident(24.0 + (leaf % POOL) as f32, &[], length, leaf as u32)).collect();
        let roots = (0..roots).map(|root| contents.resident(40.0, &identifiers.listed(root, 0), length, root as u32)).collect();
        FirstContents { roots, leaves }
    }
}

/// A record as a world assembles it: its arrivals in identifier order.
pub fn record(tick: u32, commands: Vec<Command>, mut arrivals: Vec<Arrival>) -> Record {
    arrivals.sort_by_key(|arrival| arrival.asset);
    Record { tick, commands, arrivals, events: Vec::new() }
}

/// What a world of the family is made with.
pub struct Plan<'a> {
    pub scale: Scale,
    /// The roots of this world's table: the first ones of the stated table.
    pub roots: usize,
    pub workers: usize,
    pub rules: Rules,
    /// The world's budget for its asset data, where it has one.
    pub budget: Option<u64>,
    /// The scripted records of the ticks after 102, where a mode has some.
    pub script: Vec<Record>,
    pub identifiers: &'a Identifiers,
    pub contents: &'a Contents,
    pub first: &'a FirstContents,
}

/// A world brought to the boundary after tick 102.
pub struct Built {
    pub world: World,
    pub queue: Arc<ArrivalQueue>,
    /// The median duration of the ticks 3 to 102, each with an empty record
    /// and an open gate.
    pub tick: Duration,
    /// Whether, from tick 2 on, the table holds every root and every leaf
    /// of the plan, all available, and nothing else.
    pub as_stated: bool,
}

/// Brings a world to the boundary after tick 102: the wants of the roots in
/// the record of tick 1, the arrivals of every asset in the record of tick
/// 2, each naming the size the manifest gives, then a hundred ticks with an
/// empty record.
pub fn built(plan: Plan) -> Built {
    let Plan { scale, roots, workers, rules, budget, script, identifiers, contents, first } = plan;
    assert!(script.iter().all(|record| record.tick > LAST_TIMED_TICK), "the ticks 1 to 102 are the family's own");
    let leaves = roots * LEAVES_OF_A_ROOT;
    let links = Links::new(contents.residents.clone());
    let queue = links.queue.clone();
    let mut config = WorldConfig::new(scale.size, workers, Mode::Identifiers, identifiers.leaves[..POOL].to_vec());
    config.rules = rules;
    config.budget = budget;
    let mut world = World::new(config, script, links);
    for root in &identifiers.roots[..roots] {
        world.post(Command::Want { asset: *root, asset_type: Parameters, owner: HOST, policy: Policy::Required });
    }
    let ran = world.step(1, Wait::No).expect("the world does not fail");
    assert_eq!(ran.ticks_ran, 1, "the tick of the wants runs");
    let of_roots = identifiers.roots[..roots].iter().zip(&first.roots);
    let of_leaves = identifiers.leaves[..leaves].iter().zip(&first.leaves);
    let arrivals: Vec<Arrival> = of_roots.chain(of_leaves).map(|(asset, hash)| contents.arrival(*asset, *hash)).collect();
    queue.post(&arrivals);
    let ran = world.step(LAST_TIMED_TICK - 1, Wait::No).expect("the world does not fail");
    assert_eq!(ran.ticks_ran, LAST_TIMED_TICK - 1, "the one batch answers every required asset");
    assert!(ran.rejected.is_empty(), "no arrival of the table is rejected");
    let tick = median_tick(&world, FIRST_TIMED_TICK, LAST_TIMED_TICK);
    let entries = world.entries();
    let as_stated = entries.len() == roots + leaves && entries.iter().all(|entry| matches!(entry.availability, Availability::Available(_)));
    Built { world, queue, tick, as_stated }
}

/// The median duration of the ticks `first` to `last` of a world, each from
/// the end of the tick before to its own end.
pub fn median_tick(world: &World, first: u32, last: u32) -> Duration {
    let mut ticks: Vec<Duration> = world.tick_durations().into_iter().filter(|(tick, _)| (first..=last).contains(tick)).map(|(_, duration)| duration).collect();
    assert_eq!(ticks.len(), (last - first + 1) as usize, "every tick of the range is timed");
    median(&mut ticks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table stated again here is the one of the family `gate-cost`: the
    /// same identifiers and, with the payload that family gives its parts,
    /// the same contents.
    #[test]
    fn the_large_table_is_the_one_of_the_family_gate_cost() {
        let identifiers = Identifiers::drawn(0);
        let mut contents = Contents::default();
        let first = FirstContents::of(&mut contents, &identifiers, STATED_ROOTS, Length::Payload(16));
        let mut stated = Vec::new();
        for (asset, hash) in identifiers.roots.iter().zip(&first.roots).chain(identifiers.leaves.iter().zip(&first.leaves)) {
            stated.extend_from_slice(&asset.to_le_bytes());
            stated.extend_from_slice(&hash.to_le_bytes());
        }
        stated.extend_from_slice(&identifiers.gate.to_le_bytes());
        assert_eq!(content_hash(&stated), crate::gate_cost::fingerprint());
    }

    #[test]
    fn a_part_is_padded_to_the_size_its_family_states_and_an_arrival_names_it() {
        let identifiers = Identifiers::drawn(0);
        let mut contents = Contents::default();
        let root = contents.resident(40.0, &identifiers.listed(0, 1), Length::Part(1_100), 1);
        let leaf = contents.resident(24.0, &[], Length::Part(1_000), 2);
        assert_eq!((contents.size(root), contents.size(leaf)), (1_100, 1_000));
        assert_eq!(contents.arrival(identifiers.leaves[0], leaf).size, 1_000);
    }
}
