//! Snapshots of a session's past, dense near the head and sparse far behind it, so seeking
//! back costs about as many replayed ticks as the distance sought.

use std::collections::{BTreeMap, BTreeSet};

use ludenea_ecs::Snapshot;

/// Checkpoints are taken on multiples of this many ticks.
pub const CHECKPOINT_INTERVAL: u64 = 16;
/// How many checkpoints each distance level keeps.
pub const CHECKPOINTS_PER_LEVEL: usize = 2;

/// Level `k` covers distances `[INTERVAL * 2^k, INTERVAL * 2^(k+1))` from the newest
/// checkpoint and keeps the ticks that are multiples of its spacing, `INTERVAL * 2^k /
/// PER_LEVEL` (at least `INTERVAL`), so at most `CHECKPOINTS_PER_LEVEL` of them. Each
/// level's spacing divides the next one's, so a checkpoint dropped as the head moves on
/// would never have been kept later.
pub struct Checkpoints {
    snapshots: BTreeMap<u64, Snapshot>,
    /// Ticks never thinned out: their state cannot be recomputed from the recorded logs.
    pinned: BTreeSet<u64>,
}

impl Checkpoints {
    /// Starts with the snapshot of tick 0, which is never dropped.
    pub fn new(initial: Snapshot) -> Self {
        Self {
            snapshots: BTreeMap::from([(0, initial)]),
            pinned: BTreeSet::new(),
        }
    }

    pub fn wants(&self, tick: u64) -> bool {
        tick.is_multiple_of(CHECKPOINT_INTERVAL) && !self.snapshots.contains_key(&tick)
    }

    /// Stores `snapshot`, then thins the older checkpoints relative to the newest one.
    pub fn insert(&mut self, tick: u64, snapshot: Snapshot) {
        self.snapshots.insert(tick, snapshot);
        let head = *self.snapshots.keys().next_back().expect("tick 0 is kept");
        self.snapshots.retain(|&kept, _| {
            kept == 0
                || kept == head
                || kept.is_multiple_of(spacing(head - kept))
                || self.pinned.contains(&kept)
        });
    }

    /// Stores `snapshot` at `tick` and keeps it until `truncate_after` drops it.
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
        (at, snapshot)
    }

    /// Drops every checkpoint after `tick`.
    pub fn truncate_after(&mut self, tick: u64) {
        self.snapshots.split_off(&(tick + 1));
        self.pinned.split_off(&(tick + 1));
    }

    pub fn ticks(&self) -> Vec<u64> {
        self.snapshots.keys().copied().collect()
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
