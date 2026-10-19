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
    /// The ticks of the edits the branch recorded from its fork point on, ascending.
    pub edit_ticks: Vec<u64>,
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
    /// segment. A gap before the segment's inputs is padded with `defaults`; without them
    /// the game has no input type and any recorded input fails to step, padded or not.
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
        let inputs = fork_tick
            .saturating_add(self.inputs.len() as u64)
            .saturating_sub(1);
        let edits = self.edits.last().map_or(0, |edit| edit.tick);
        fork_tick.max(inputs).max(edits)
    }
}

pub(crate) struct Branch {
    pub name: String,
    pub parent: Option<BranchId>,
    pub fork_tick: u64,
    pub fork_edits: usize,
    /// The last tick the stored segment holds for the current scene and build (with its
    /// edits checked), when a scene edit or a rebuild made the ticks after it stale.
    pub stale: Option<u64>,
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
            stale: None,
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

    /// Stores the segment of the current branch, stale after `stale`, and makes `id`
    /// current, taking its segment and staleness out (`id` may be the current branch).
    pub fn make_current(
        &mut self,
        segment: Segment,
        stale: Option<u64>,
        id: BranchId,
    ) -> (Segment, Option<u64>) {
        let current = self
            .branches
            .get_mut(&self.current)
            .expect("current branch");
        current.segment = Some(segment);
        current.stale = stale;
        self.current = id;
        let target = self.branches.get_mut(&id).expect("known branch");
        let segment = target.segment.take().expect("a stored branch");
        (segment, target.stale.take())
    }

    /// Stores the segment of the current branch, stale after `stale`, and makes a new
    /// child of it, forked at `fork`, current.
    pub fn fork(
        &mut self,
        segment: Segment,
        stale: Option<u64>,
        (fork_tick, fork_edits): (u64, usize),
    ) {
        let id = BranchId(self.next);
        self.next += 1;
        let child = Branch {
            name: id.default_name(),
            parent: Some(self.current),
            fork_tick,
            fork_edits,
            stale: None,
            segment: None,
        };
        let current = self
            .branches
            .get_mut(&self.current)
            .expect("current branch");
        current.segment = Some(segment);
        current.stale = stale;
        self.branches.insert(id, child);
        self.current = id;
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

    /// The children of `id` forked at or after `tick`, and their descendants: the
    /// branches that inherit an edit `id` records at `tick`.
    pub fn forked_at_or_after(&self, id: BranchId, tick: u64) -> Vec<BranchId> {
        self.branches
            .keys()
            .copied()
            .filter(|&branch| {
                let path = self.path(branch);
                let below = path.iter().position(|&b| b == id).map(|at| at + 1);
                below
                    .and_then(|at| path.get(at))
                    .is_some_and(|child| self.branches[child].fork_tick >= tick)
            })
            .collect()
    }

    pub fn names(&self, ids: &[BranchId]) -> Vec<&str> {
        ids.iter()
            .filter_map(|id| Some(self.branches.get(id)?.name.as_str()))
            .collect()
    }

    /// The current branch as `segment` would store it.
    pub fn store_current(&self, segment: &Segment, players: u16) -> StoredBranch {
        StoredBranch::new(self.current, self.current_branch(), Some(segment), players)
    }

    /// Takes the stored branches `ids` out of the tree.
    pub fn take(&mut self, ids: &[BranchId], players: u16) -> Vec<StoredBranch> {
        ids.iter()
            .map(|&id| {
                let branch = self.branches.remove(&id).expect("a branch of the tree");
                assert!(branch.segment.is_some(), "only stored branches are taken");
                StoredBranch::new(id, &branch, branch.segment.as_ref(), players)
            })
            .collect()
    }

    /// Puts back branches `take` took, parents first: all of them, or none on an error.
    pub fn put_back(&mut self, branches: Vec<StoredBranch>) -> Result<(), BranchesError> {
        let mut back: Vec<(BranchId, Branch)> = Vec::new();
        for stored in branches {
            let id = stored.id;
            let branch = stored.into_branch(true)?;
            let parent = branch.parent.ok_or(BranchesError::Tree)?;
            let known = |id| self.branches.contains_key(&id) || back.iter().any(|(b, _)| *b == id);
            if !known(parent) || known(id) {
                return Err(BranchesError::Tree);
            }
            back.push((id, branch));
        }
        self.branches.extend(back);
        Ok(())
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

    /// After the edit at `position` among the edits of `tick` was removed from the current
    /// run: the current branch, and its stored descendants, forked at `tick` that inherited
    /// it inherit one edit fewer. An inherited edit keeps its position in the child's run.
    pub fn edit_removed_at(&mut self, tick: u64, position: usize) {
        let current = self.current;
        let mut inheriting = vec![current];
        // Parents come first in identifier order.
        for (&id, branch) in &mut self.branches {
            let below = branch
                .parent
                .is_some_and(|parent| inheriting.contains(&parent));
            let inherits = (id == current && branch.parent.is_some()) || below;
            if inherits && branch.fork_tick == tick && position < branch.fork_edits {
                branch.fork_edits -= 1;
                if below {
                    inheriting.push(id);
                }
            }
        }
    }

    /// Marks every stored branch stale from its fork tick, dropping its checkpoints of the
    /// old state.
    pub fn mark_stale(&mut self) {
        for branch in self.branches.values_mut() {
            if let Some(segment) = &mut branch.segment {
                branch.stale = Some(branch.fork_tick);
                segment.checkpoints = Checkpoints::empty();
            }
        }
    }

    /// The branches as the timeline shows them; the current one's run is `current_head` and
    /// `current_edits`.
    pub fn infos(&self, current_head: u64, current_edits: &EditLog) -> Vec<BranchInfo> {
        self.branches
            .iter()
            .map(|(&id, branch)| {
                let own = match &branch.segment {
                    Some(segment) => &segment.edits[..],
                    None => {
                        let log = &current_edits.edits;
                        let end = log.partition_point(|edit| edit.tick <= branch.fork_tick);
                        let first = log.partition_point(|edit| edit.tick < branch.fork_tick);
                        &log[(first + branch.fork_edits).min(end)..]
                    }
                };
                let mut edit_ticks: Vec<u64> = own.iter().map(|edit| edit.tick).collect();
                edit_ticks.dedup();
                BranchInfo {
                    id,
                    name: branch.name.clone(),
                    parent: branch.parent,
                    fork_tick: branch.fork_tick,
                    head: branch
                        .segment
                        .as_ref()
                        .map_or(current_head, |segment| segment.head(branch.fork_tick)),
                    stale: branch.stale.is_some(),
                    edit_ticks,
                }
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
            .map(|(&id, branch)| StoredBranch::new(id, branch, branch.segment.as_ref(), players))
            .collect();
        let stored = StoredTree {
            current: self.current,
            next: self.next,
            branches,
        };
        postcard::to_allocvec(&stored).expect("a branch tree always encodes")
    }

    /// Decodes `encode`'s bytes. The current branch's segment stays with the session, whose
    /// run holds `current_edits` and ends at `current_head`. Every fork point must lie on
    /// its parent's run.
    pub fn decode(
        bytes: &[u8],
        current_edits: &EditLog,
        current_head: u64,
    ) -> Result<BranchTree, BranchesError> {
        let stored: StoredTree = postcard::from_bytes(bytes)?;
        let mut branches = BTreeMap::new();
        for branch in stored.branches {
            let id = branch.id;
            branches.insert(id, branch.into_branch(id != stored.current)?);
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
        let tree = BranchTree {
            branches,
            current: stored.current,
            next: next.max(stored.next),
        };
        tree.check_forks(current_edits, current_head)?;
        Ok(tree)
    }

    /// Refuses a fork point past its parent's head or inheriting more edits than the
    /// parent's run records at the fork tick: replaying it would pad the run without bound.
    /// Parents come first in identifier order, so every fork point `edits_at` reads on the
    /// way is checked before.
    fn check_forks(&self, current_edits: &EditLog, current_head: u64) -> Result<(), BranchesError> {
        for branch in self.branches.values() {
            let Some(parent) = branch.parent else {
                if branch.fork() != (0, 0) {
                    return Err(BranchesError::Fork);
                }
                continue;
            };
            let head = match &self.branches[&parent].segment {
                None => current_head,
                Some(segment) => segment.head(self.branches[&parent].fork_tick),
            };
            let edits = self.edits_at(parent, branch.fork_tick, current_edits);
            if branch.fork_tick > head || branch.fork_edits > edits {
                return Err(BranchesError::Fork);
            }
        }
        Ok(())
    }

    /// How many edits the run of `id` records at `tick`: a branch forked after `tick` shares
    /// its parent's, and the current branch's run is `current_edits`.
    fn edits_at(&self, mut id: BranchId, tick: u64, current_edits: &EditLog) -> usize {
        loop {
            let branch = &self.branches[&id];
            let Some(segment) = &branch.segment else {
                return current_edits.at(tick).len();
            };
            match (tick.cmp(&branch.fork_tick), branch.parent) {
                (std::cmp::Ordering::Less, Some(parent)) => id = parent,
                (order, _) => {
                    let own = segment
                        .edits
                        .iter()
                        .filter(|edit| edit.tick == tick)
                        .count();
                    let inherited = if order.is_eq() { branch.fork_edits } else { 0 };
                    return own + inherited;
                }
            }
        }
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
    #[error("a branch forks outside its parent's run")]
    Fork,
    #[error("a branch records an edit outside its run")]
    Run,
}

#[derive(Serialize, Deserialize)]
struct StoredTree {
    current: BranchId,
    next: u32,
    branches: Vec<StoredBranch>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredBranch {
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

impl StoredBranch {
    fn new(id: BranchId, branch: &Branch, segment: Option<&Segment>, players: u16) -> Self {
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
            stale: branch.stale.is_some(),
            inputs: inputs.encode(),
            edits: edits.encode(),
            hashes,
        }
    }

    pub fn id(&self) -> BranchId {
        self.id
    }

    pub fn encode(branches: &[StoredBranch]) -> Vec<u8> {
        postcard::to_allocvec(branches).expect("branches always encode")
    }

    pub fn decode(bytes: &[u8]) -> Result<Vec<StoredBranch>, BranchesError> {
        Ok(postcard::from_bytes(bytes)?)
    }

    /// The names of the branches after the first in `encode`'s bytes, as a `Run` undo
    /// entry removes them; none when they do not decode.
    pub fn names_of_descendants(bytes: &[u8]) -> Vec<String> {
        let branches = StoredBranch::decode(bytes).unwrap_or_default();
        branches
            .into_iter()
            .skip(1)
            .map(|branch| branch.name)
            .collect()
    }

    /// Its segment, and whether it is stale.
    pub fn into_segment(self) -> Result<(Segment, bool), BranchesError> {
        let branch = self.into_branch(true)?;
        Ok((
            branch.segment.expect("a stored segment"),
            branch.stale.is_some(),
        ))
    }

    /// Marks encoded branches stale, after a rebuild: their hashes are of the old build.
    pub fn mark_stale(bytes: &[u8]) -> Result<Vec<u8>, BranchesError> {
        let mut branches = StoredBranch::decode(bytes)?;
        for branch in &mut branches {
            branch.stale = true;
        }
        Ok(StoredBranch::encode(&branches))
    }

    /// The branch, with its segment when `stored` (not the current branch). Its edits lie
    /// at or after its fork tick and before the last tick, and its inputs end within the
    /// tick range, so tick arithmetic on its run never overflows. Hashes do not bound the
    /// edits: `Restore` and a failed resimulation drop hashes and keep the future.
    fn into_branch(self, stored: bool) -> Result<Branch, BranchesError> {
        let fork_tick = self.fork_tick;
        let segment = stored
            .then(|| -> Result<Segment, BranchesError> {
                let inputs = InputLog::decode(&self.inputs)?.ticks;
                let edits = EditLog::decode(&self.edits)?.edits;
                fork_tick
                    .checked_add(inputs.len() as u64)
                    .ok_or(BranchesError::Run)?;
                // The log is in tick order.
                let outside = |edit: Option<&TimedEdit>| {
                    edit.is_some_and(|edit| edit.tick < fork_tick || edit.tick == u64::MAX)
                };
                if outside(edits.first()) || outside(edits.last()) {
                    return Err(BranchesError::Run);
                }
                Ok(Segment {
                    inputs,
                    edits,
                    hashes: self.hashes,
                    checkpoints: Checkpoints::empty(),
                })
            })
            .transpose()?;
        Ok(Branch {
            name: self.name,
            parent: self.parent,
            fork_tick: self.fork_tick,
            fork_edits: usize::try_from(self.fork_edits).map_err(|_| BranchesError::Fork)?,
            stale: self.stale.then_some(self.fork_tick),
            segment,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(id: u32, parent: Option<u32>, (fork_tick, fork_edits): (u64, u64)) -> StoredBranch {
        StoredBranch {
            id: BranchId(id),
            name: BranchId(id).default_name(),
            parent: parent.map(BranchId),
            fork_tick,
            fork_edits,
            stale: false,
            inputs: InputLog::new(1).encode(),
            edits: EditLog::new().encode(),
            hashes: Vec::new(),
        }
    }

    fn decode(branches: Vec<StoredBranch>, current_head: u64) -> Result<BranchTree, BranchesError> {
        let bytes = postcard::to_allocvec(&StoredTree {
            current: BranchId::ROOT,
            next: 3,
            branches,
        })
        .unwrap();
        BranchTree::decode(&bytes, &EditLog::new(), current_head)
    }

    #[test]
    fn decode_refuses_forks_outside_the_parent_run() {
        let root = || stored(0, None, (0, 0));
        assert!(decode(vec![root(), stored(1, Some(0), (40, 0))], 50).is_ok());
        for child in [
            stored(1, Some(0), (u64::MAX, 0)),
            stored(1, Some(0), (51, 0)),
            stored(1, Some(0), (40, u64::MAX)),
            stored(1, Some(0), (40, 1)),
        ] {
            assert!(matches!(
                decode(vec![root(), child], 50),
                Err(BranchesError::Fork)
            ));
        }
        // A grandchild forks on its stored parent's run, which ends at the parent's fork.
        let child = stored(1, Some(0), (40, 0));
        let grandchild = stored(2, Some(1), (41, 0));
        assert!(matches!(
            decode(vec![root(), child, grandchild], 50),
            Err(BranchesError::Fork)
        ));
        assert!(matches!(
            decode(vec![stored(0, None, (3, 0))], 50),
            Err(BranchesError::Fork)
        ));
    }

    #[test]
    fn put_back_puts_back_nothing_when_one_branch_does_not_fit() {
        let mut tree = BranchTree::default();
        let branches = vec![stored(1, Some(0), (0, 0)), stored(2, Some(9), (0, 0))];
        assert!(matches!(tree.put_back(branches), Err(BranchesError::Tree)));
        assert!(tree.get(BranchId(1)).is_none());
        tree.put_back(vec![stored(1, Some(0), (0, 0)), stored(2, Some(1), (0, 0))])
            .unwrap();
        assert!(tree.get(BranchId(2)).is_some());
    }

    #[test]
    fn removing_an_inherited_edit_updates_every_branch_that_inherits_it() {
        let edit = |tick| TimedEdit {
            tick,
            edit: ludenea_app::Edit::SetResource {
                resource: "r".to_string(),
                value: Vec::new(),
            },
        };
        let mut run = EditLog::new();
        run.edits = vec![edit(5), edit(5)];
        let bytes = postcard::to_allocvec(&StoredTree {
            current: BranchId::ROOT,
            next: 4,
            branches: vec![
                stored(0, None, (0, 0)),
                stored(1, Some(0), (5, 2)),
                stored(2, Some(1), (5, 2)),
                // Inherits only the first edit of tick 5.
                stored(3, Some(1), (5, 1)),
            ],
        })
        .unwrap();
        let mut tree = BranchTree::decode(&bytes, &run, 5).unwrap();
        tree.edit_removed_at(5, 1);
        let forks: Vec<usize> = (1..4)
            .map(|id| tree.get(BranchId(id)).unwrap().fork_edits)
            .collect();
        assert_eq!(forks, [1, 1, 1]);
    }
}
