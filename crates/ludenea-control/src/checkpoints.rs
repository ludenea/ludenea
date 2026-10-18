//! Snapshots of a session's past, dense near the head and sparse far behind it, so seeking
//! back costs about as many replayed ticks as the distance sought.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ludenea_ecs::Snapshot;

/// Checkpoints are taken on multiples of this many ticks.
pub const CHECKPOINT_INTERVAL: u64 = 16;
/// How many checkpoints a session keeps across all its branches, beyond those never
/// evicted.
pub const CHECKPOINT_BUDGET: usize = 256;
/// How many checkpoints each distance level keeps.
pub const CHECKPOINTS_PER_LEVEL: usize = 2;

/// Level `k` covers distances `[INTERVAL * 2^k, INTERVAL * 2^(k+1))` from the newest
/// checkpoint and keeps the ticks that are multiples of its spacing, `INTERVAL * 2^k /
/// PER_LEVEL` (at least `INTERVAL`), so at most `CHECKPOINTS_PER_LEVEL` of them. Each
/// level's spacing divides the next one's, so a checkpoint dropped as the head moves on
/// would never have been kept later.
///
/// Snapshots are shared: a branch's run and its ancestors' stored segments hold the same
/// checkpoints of their common past without copying them.
pub struct Checkpoints {
    snapshots: BTreeMap<u64, Arc<Snapshot>>,
    /// Ticks never thinned out: their state cannot be recomputed from the recorded logs.
    pinned: BTreeSet<u64>,
}

impl Checkpoints {
    /// Starts with the snapshot of tick 0, which is never dropped.
    pub fn new(initial: Snapshot) -> Self {
        Self {
            snapshots: BTreeMap::from([(0, Arc::new(initial))]),
            pinned: BTreeSet::new(),
        }
    }

    /// Whether the nearest checkpoint at or before `tick` is a whole interval or more behind
    /// it, as after a truncation far behind the head, where older levels are sparse.
    pub fn is_sparse_at(&self, tick: u64) -> bool {
        tick - self.nearest_at_or_before(tick).0 >= CHECKPOINT_INTERVAL
    }

    pub fn wants(&self, tick: u64) -> bool {
        tick.is_multiple_of(CHECKPOINT_INTERVAL) && !self.snapshots.contains_key(&tick)
    }

    /// Stores `snapshot`, then thins the older checkpoints relative to the newest one.
    pub fn insert(&mut self, tick: u64, snapshot: Snapshot) {
        self.snapshots.insert(tick, Arc::new(snapshot));
        let head = *self.snapshots.keys().next_back().expect("tick 0 is kept");
        self.snapshots.retain(|&kept, _| {
            kept == 0
                || kept == head
                || kept.is_multiple_of(spacing(head - kept))
                || self.pinned.contains(&kept)
        });
    }

    /// Stores `snapshot` at `tick` and keeps it until `truncate_after` drops it.
    // ponytail: pins taken after truncations add up, one snapshot each; drop a pin once the
    // levels around it are dense again if repeated take-overs make checkpoints heavy.
    pub fn pin(&mut self, tick: u64, snapshot: Snapshot) {
        self.pinned.insert(tick);
        self.insert(tick, snapshot);
    }

    pub fn nearest_at_or_before(&self, tick: u64) -> (u64, &Snapshot) {
        let (&at, snapshot) = self
            .snapshots
            .range(..=tick)
            .next_back()
            .expect("tick 0 is kept");
        (at, &**snapshot)
    }

    /// Only the checkpoint of tick 0, shared.
    pub(crate) fn tick_zero(&self) -> Checkpoints {
        Checkpoints {
            snapshots: self
                .snapshots
                .range(..=0)
                .map(|(&t, s)| (t, Arc::clone(s)))
                .collect(),
            pinned: BTreeSet::new(),
        }
    }

    /// Drops every checkpoint after `tick`.
    pub fn truncate_after(&mut self, tick: u64) {
        self.snapshots.split_off(&(tick + 1));
        self.pinned.split_off(&(tick + 1));
    }

    pub fn ticks(&self) -> Vec<u64> {
        self.snapshots.keys().copied().collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// No checkpoint, not even tick 0: only for checkpoints stored apart from a run, which
    /// are never sought from.
    pub(crate) fn empty() -> Self {
        Self {
            snapshots: BTreeMap::new(),
            pinned: BTreeSet::new(),
        }
    }

    /// The checkpoints after `tick`, pins included, sharing their snapshots.
    pub(crate) fn after(&self, tick: u64) -> Checkpoints {
        Checkpoints {
            snapshots: self
                .snapshots
                .range(tick + 1..)
                .map(|(&t, s)| (t, Arc::clone(s)))
                .collect(),
            pinned: self.pinned.range(tick + 1..).copied().collect(),
        }
    }

    /// Adds `other`'s checkpoints as they are, without thinning, sharing their snapshots.
    pub(crate) fn extend(&mut self, other: &Checkpoints) {
        let shared = other.snapshots.iter().map(|(&t, s)| (t, Arc::clone(s)));
        self.snapshots.extend(shared);
        self.pinned.extend(&other.pinned);
    }

    /// The oldest checkpoint the budget may evict: never tick 0, a pinned one, or one at
    /// a tick in `kept`.
    pub(crate) fn oldest_evictable(&self, kept: &[u64]) -> Option<u64> {
        self.snapshots
            .keys()
            .copied()
            .find(|tick| *tick != 0 && !self.pinned.contains(tick) && !kept.contains(tick))
    }

    pub(crate) fn evict(&mut self, tick: u64) {
        self.snapshots.remove(&tick);
    }
}

/// The spacing of the level that `distance` falls in.
fn spacing(distance: u64) -> u64 {
    let level = (distance / CHECKPOINT_INTERVAL).max(1).ilog2();
    ((CHECKPOINT_INTERVAL << level) / CHECKPOINTS_PER_LEVEL as u64).max(CHECKPOINT_INTERVAL)
}

#[cfg(test)]
mod tests {
    use ludenea_ecs::World;

    use super::*;

    fn filled(head: u64) -> Checkpoints {
        let snapshot = World::new().snapshot();
        let mut checkpoints = Checkpoints::new(snapshot.clone());
        for tick in 1..=head {
            if checkpoints.wants(tick) {
                checkpoints.insert(tick, snapshot.clone());
            }
        }
        checkpoints
    }

    #[test]
    fn split_and_composed_checkpoints_share_their_snapshots() {
        let checkpoints = filled(160);
        let mut composed = checkpoints.tick_zero();
        composed.extend(&checkpoints.after(0));
        assert_eq!(composed.ticks(), checkpoints.ticks());
        for (tick, snapshot) in &checkpoints.snapshots {
            assert!(
                Arc::ptr_eq(snapshot, &composed.snapshots[tick]),
                "tick {tick}"
            );
        }
    }

    #[test]
    fn tick_zero_and_head_are_always_kept() {
        for head in [16, 160, 1_600, 16_000] {
            let ticks = filled(head).ticks();
            assert_eq!(ticks.first(), Some(&0));
            assert_eq!(ticks.last(), Some(&head));
        }
    }

    #[test]
    fn count_grows_logarithmically() {
        let head = 100_000;
        // The floor of log2 makes this bound slightly stricter than the real logarithm.
        let levels = (head / CHECKPOINT_INTERVAL).ilog2() as usize + 1;
        let bound = CHECKPOINTS_PER_LEVEL * levels + 2;
        let count = filled(head).ticks().len();
        assert!(count <= bound, "{count} checkpoints, bound {bound}");
    }

    #[test]
    fn seek_cost_near_the_new_head_is_bounded_after_a_truncation() {
        let snapshot = World::new().snapshot();
        let mut checkpoints = filled(100_000);
        let cut = 50_001;
        checkpoints.truncate_after(cut);
        assert!(checkpoints.is_sparse_at(cut));
        checkpoints.pin(cut, snapshot.clone());
        for head in cut + 1..cut + 200 {
            if checkpoints.wants(head) {
                checkpoints.insert(head, snapshot.clone());
            }
            for target in cut..=head {
                let (tick, _) = checkpoints.nearest_at_or_before(target);
                let cost = target - tick;
                assert!(
                    cost <= CHECKPOINT_INTERVAL.max(head - target),
                    "head {head}, target {target}: nearest checkpoint {tick}"
                );
            }
        }
    }

    #[test]
    fn seek_cost_is_bounded_by_distance() {
        for head in [1_000, 4_096, 100_000] {
            let checkpoints = filled(head);
            for target in (0..=head).step_by(7) {
                let (tick, _) = checkpoints.nearest_at_or_before(target);
                assert!(tick <= target);
                let cost = target - tick;
                assert!(
                    cost <= CHECKPOINT_INTERVAL.max(head - target),
                    "head {head}, target {target}: nearest checkpoint {tick}"
                );
            }
        }
    }
}
