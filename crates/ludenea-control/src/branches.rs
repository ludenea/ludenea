//! The branch tree of a session: every branch other than the current one, stored as the
//! segment it adds after its fork point.
//!
//! A fork point `(fork_tick, fork_edits)` names the tick a branch forks at and how many of
//! the edits its parent recorded at that tick it inherits. A branch's segment is its
//! inputs from `fork_tick` on, its edits at `fork_tick` after the inherited ones and after
//! `fork_tick`, and its hashes and checkpoints after `fork_tick`. The root forks at
//! `(0, 0)`. A branch's run is its ancestors' segments, each cut at the next fork point,
//! followed by its own segment.

use std::collections::BTreeMap;

use ludenea_app::{EditLog, InputLog, TimedEdit};
use serde::{Deserialize, Serialize};

use crate::checkpoints::Checkpoints;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BranchId(pub u32);

impl BranchId {
    pub const ROOT: BranchId = BranchId(0);

    /// Branches are named after their number, counting the root as the first.
    fn default_name(self) -> String {
        format!("Branch {}", self.0 + 1)
    }
}

/// A branch as the timeline shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BranchInfo {
    pub id: BranchId,
    pub name: String,
    pub parent: Option<BranchId>,
    pub fork_tick: u64,
    /// The last tick with a recorded input or edit.
    pub head: u64,
    /// Recorded before a scene edit or a rebuild: resimulated when switched to.
    pub stale: bool,
}

/// What a branch records after its fork point.
pub(crate) struct Segment {
    pub inputs: Vec<Vec<u8>>,
    pub edits: Vec<TimedEdit>,
    pub hashes: Vec<u64>,
    pub checkpoints: Checkpoints,
}

impl Segment {
    /// The part of the run of a branch forked at `(tick, edits)` after its fork point.
    pub fn split(
        (tick, edits): (u64, usize),
        inputs: &InputLog,
        log: &EditLog,
        hashes: &[u64],
        checkpoints: &Checkpoints,
    ) -> Segment {
        let at = |tick: u64| usize::try_from(tick).expect("tick fits in memory");
        // Clamped to the fork tick's edits: a wrong count never swallows later edits.
        let end = log.edits.partition_point(|edit| edit.tick <= tick);
        let first = (log.edits.partition_point(|edit| edit.tick < tick) + edits).min(end);
        Segment {
            inputs: inputs.ticks.get(at(tick)..).unwrap_or_default().to_vec(),
            edits: log.edits[first..].to_vec(),
            hashes: hashes.get(at(tick) + 1..).unwrap_or_default().to_vec(),
            checkpoints: checkpoints.after(tick),
        }
    }

    /// Cuts the run of the parent at the fork point `(tick, edits)` and appends this
    /// segment. A gap before the segment's inputs is padded with `defaults`.
    pub fn append_to(
        &self,
        (tick, edits): (u64, usize),
        inputs: &mut InputLog,
        log: &mut EditLog,
        hashes: &mut Vec<u64>,
        checkpoints: &mut Checkpoints,
        defaults: Option<&[u8]>,
    ) {
        let at = usize::try_from(tick).expect("tick fits in memory");
        inputs.ticks.truncate(at);
        if !self.inputs.is_empty()
            && let Some(defaults) = defaults
        {
            inputs.ticks.resize(at, defaults.to_vec());
        }
        inputs.ticks.extend_from_slice(&self.inputs);
        let first = log.edits.partition_point(|edit| edit.tick < tick);
        let end = log.edits.partition_point(|edit| edit.tick <= tick);
        log.edits.truncate((first + edits).min(end));
        log.edits.extend_from_slice(&self.edits);
        // A parent whose hashes stop before the fork (a failed resimulation) leaves the
        // rest to resimulation.
        if hashes.len() > at {
            hashes.truncate(at + 1);
            hashes.extend_from_slice(&self.hashes);
        }
        checkpoints.truncate_after(tick);
        checkpoints.extend(&self.checkpoints);
    }

    fn head(&self, fork_tick: u64) -> u64 {
        let inputs = (fork_tick + self.inputs.len() as u64).saturating_sub(1);
        let edits = self.edits.last().map_or(0, |edit| edit.tick);
        fork_tick.max(inputs).max(edits)
    }
}

pub(crate) struct Branch {
    pub name: String,
    pub parent: Option<BranchId>,
    pub fork_tick: u64,
    pub fork_edits: usize,
    pub stale: bool,
    /// `None` for the current branch, whose run lives in the session.
    pub segment: Option<Segment>,
}

impl Branch {
    pub fn fork(&self) -> (u64, usize) {
        (self.fork_tick, self.fork_edits)
    }
}

pub(crate) struct BranchTree {
    branches: BTreeMap<BranchId, Branch>,
    current: BranchId,
    /// Identifiers are never reused.
    next: u32,
}

impl Default for BranchTree {
    fn default() -> Self {
        let root = Branch {
            name: BranchId::ROOT.default_name(),
            parent: None,
            fork_tick: 0,
            fork_edits: 0,
            stale: false,
            segment: None,
        };
        BranchTree {
            branches: BTreeMap::from([(BranchId::ROOT, root)]),
            current: BranchId::ROOT,
            next: 1,
        }
    }
}

impl BranchTree {
    pub fn current(&self) -> BranchId {
        self.current
    }

    pub fn get(&self, id: BranchId) -> Option<&Branch> {
        self.branches.get(&id)
    }

    pub fn get_mut(&mut self, id: BranchId) -> Option<&mut Branch> {
        self.branches.get_mut(&id)
    }

    pub fn current_branch(&self) -> &Branch {
        &self.branches[&self.current]
    }

    /// Stores the segment of the current branch and makes `id` current, taking its
    /// segment out.
    pub fn make_current(&mut self, segment: Segment, stale: bool, id: BranchId) -> Option<Segment> {
        let current = self
            .branches
            .get_mut(&self.current)
            .expect("current branch");
        current.segment = Some(segment);
        current.stale = stale;
        self.current = id;
        let target = self.branches.get_mut(&id).expect("known branch");
        target.stale = false;
        target.segment.take()
    }

    /// Stores the segment of the current branch and makes a new child of it, forked at
    /// `fork`, current.
    pub fn fork(&mut self, segment: Segment, stale: bool, (fork_tick, fork_edits): (u64, usize)) {
        let id = BranchId(self.next);
        self.next += 1;
        let child = Branch {
            name: id.default_name(),
            parent: Some(self.current),
            fork_tick,
            fork_edits,
            stale: false,
            segment: None,
        };
        self.branches.insert(id, child);
        self.make_current(segment, stale, id);
    }

    /// The branches from the root down to `id`.
    pub fn path(&self, id: BranchId) -> Vec<BranchId> {
        let mut path = vec![id];
        while let Some(parent) = self.branches[path.last().expect("not empty")].parent {
            path.push(parent);
        }
        path.reverse();
        path
    }

    /// `id` and every branch below it, in identifier order.
    pub fn subtree(&self, id: BranchId) -> Vec<BranchId> {
        self.branches
            .keys()
            .copied()
            .filter(|&branch| self.path(branch).contains(&id))
            .collect()
    }

    pub fn remove(&mut self, ids: &[BranchId]) {
        for id in ids {
            self.branches.remove(id);
        }
    }

    /// After `count` edits were removed at `tick` from the front of that tick's edits in
    /// the current run: the current branch inherits that many fewer at its fork tick.
    pub fn edits_dropped_at(&mut self, tick: u64, count: usize) {
        let branch = self
            .branches
            .get_mut(&self.current)
            .expect("current branch");
        if branch.parent.is_some() && branch.fork_tick == tick {
            branch.fork_edits = branch.fork_edits.saturating_sub(count);
        }
    }

    /// Marks every stored branch stale, dropping its checkpoints of the old state.
    pub fn mark_stale(&mut self) {
        for branch in self.branches.values_mut() {
            if let Some(segment) = &mut branch.segment {
                branch.stale = true;
                segment.checkpoints = Checkpoints::empty();
            }
        }
    }

    pub fn infos(&self, current_head: u64) -> Vec<BranchInfo> {
        self.branches
            .iter()
            .map(|(&id, branch)| BranchInfo {
                id,
                name: branch.name.clone(),
                parent: branch.parent,
                fork_tick: branch.fork_tick,
                head: branch
                    .segment
                    .as_ref()
                    .map_or(current_head, |segment| segment.head(branch.fork_tick)),
                stale: branch.stale,
            })
            .collect()
    }

    /// The checkpoints of the stored branches.
    pub fn checkpoint_count(&self) -> usize {
        self.stored()
            .map(|(_, segment)| segment.checkpoints.len())
            .sum()
    }

    pub fn fork_ticks(&self) -> Vec<u64> {
        self.branches
            .values()
            .map(|branch| branch.fork_tick)
            .collect()
    }

    /// Evicts the oldest evictable checkpoint of the stored branches `from` accepts;
    /// whether one was evicted.
    pub fn evict_oldest(&mut self, from: impl Fn(BranchId) -> bool, fork_ticks: &[u64]) -> bool {
        let oldest = self
            .stored()
            .filter(|&(id, _)| from(id))
            .filter_map(|(id, segment)| {
                Some((segment.checkpoints.oldest_evictable(fork_ticks)?, id))
            })
            .min();
        let Some((tick, id)) = oldest else {
            return false;
        };
        let segment = self.branches.get_mut(&id).and_then(|b| b.segment.as_mut());
        segment.expect("stored branch").checkpoints.evict(tick);
        true
    }

    fn stored(&self) -> impl Iterator<Item = (BranchId, &Segment)> {
        self.branches
            .iter()
            .filter_map(|(&id, branch)| Some((id, branch.segment.as_ref()?)))
    }

    /// Whether the tree has more than the root, or a renamed root.
    pub fn is_branched(&self) -> bool {
        self.branches.len() > 1
            || self.branches[&BranchId::ROOT].name != BranchId::ROOT.default_name()
    }

    /// The tree without checkpoints, for a session export.
    pub fn encode(&self, players: u16) -> Vec<u8> {
        let branches = self
            .branches
            .iter()
            .map(|(&id, branch)| {
                let segment = branch.segment.as_ref();
                let mut inputs = InputLog::new(players);
                let mut edits = EditLog::new();
                let mut hashes = Vec::new();
                if let Some(segment) = segment {
                    inputs.ticks.clone_from(&segment.inputs);
                    edits.edits.clone_from(&segment.edits);
                    hashes.clone_from(&segment.hashes);
                }
                StoredBranch {
                    id,
                    name: branch.name.clone(),
                    parent: branch.parent,
                    fork_tick: branch.fork_tick,
                    fork_edits: branch.fork_edits as u64,
                    stale: branch.stale,
                    inputs: inputs.encode(),
                    edits: edits.encode(),
                    hashes,
                }
            })
            .collect();
        let stored = StoredTree {
            current: self.current,
            next: self.next,
            branches,
        };
        postcard::to_allocvec(&stored).expect("a branch tree always encodes")
    }

    /// Decodes `encode`'s bytes. The current branch's segment stays with the session.
    pub fn decode(bytes: &[u8]) -> Result<BranchTree, BranchesError> {
        let stored: StoredTree = postcard::from_bytes(bytes)?;
        let mut branches = BTreeMap::new();
        for branch in stored.branches {
            let segment = (branch.id != stored.current)
                .then(|| -> Result<Segment, BranchesError> {
                    Ok(Segment {
                        inputs: InputLog::decode(&branch.inputs)?.ticks,
                        edits: EditLog::decode(&branch.edits)?.edits,
                        hashes: branch.hashes,
                        checkpoints: Checkpoints::empty(),
                    })
                })
                .transpose()?;
            let fork_edits = usize::try_from(branch.fork_edits).expect("edits fit in memory");
            branches.insert(
                branch.id,
                Branch {
                    name: branch.name,
                    parent: branch.parent,
                    fork_tick: branch.fork_tick,
                    fork_edits,
                    stale: branch.stale,
                    segment,
                },
            );
        }
        // A parent is always older than its children, so the tree has no cycle.
        let linked = branches.iter().all(|(&id, branch)| match branch.parent {
            None => id == BranchId::ROOT,
            Some(parent) => parent < id && branches.contains_key(&parent),
        });
        let known = |id| branches.contains_key(&id);
        if !known(BranchId::ROOT) || !known(stored.current) || !linked {
            return Err(BranchesError::Tree);
        }
        let next = branches.keys().next_back().map_or(1, |id| id.0 + 1);
        Ok(BranchTree {
            branches,
            current: stored.current,
            next: next.max(stored.next),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BranchesError {
    #[error(transparent)]
    Postcard(#[from] postcard::Error),
    #[error(transparent)]
    Log(#[from] ludenea_app::SimError),
    #[error("a branch names a parent the tree does not have")]
    Tree,
}

#[derive(Serialize, Deserialize)]
struct StoredTree {
    current: BranchId,
    next: u32,
    branches: Vec<StoredBranch>,
}

#[derive(Serialize, Deserialize)]
struct StoredBranch {
    id: BranchId,
    name: String,
    parent: Option<BranchId>,
    fork_tick: u64,
    fork_edits: u64,
    stale: bool,
    /// An encoded input log, so the input log version travels with it.
    inputs: Vec<u8>,
    /// An encoded edit log, likewise.
    edits: Vec<u8>,
    hashes: Vec<u64>,
}
