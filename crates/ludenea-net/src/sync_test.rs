//! SyncTest: catches game code that a rollback would make diverge, in one process. A straight
//! simulation runs next to one that mispredicts and rolls back every tick, and the first tick
//! whose hashes differ is reported with the entities, components and fields that differ.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use ludenea_app::divergence::{
    Cause, DivergenceKind, EntityComparison, EntityDivergence, TickRecorder,
};
use ludenea_app::replay::{ReplayError, play_tick, play_tick_observed};
use ludenea_app::{EditLog, InputLog, SavedState, SimError, Simulation, StateError};
use ludenea_ecs::{Difference, Snapshot};

/// The entities the message of [`SyncTestError::Mismatch`] lists; it counts the others.
pub const LISTED_ENTITIES: usize = 10;

/// What a SyncTest that passed ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncTestReport {
    /// The ticks both simulations ran.
    pub ticks: u64,
    /// The rollbacks the rolled back simulation ran, one per tick.
    pub rollbacks: u64,
}

/// Why a SyncTest failed.
#[derive(Debug, thiserror::Error)]
pub enum SyncTestError {
    /// The window is 0, or longer than the ticks tested.
    #[error(
        "a rollback window of {window} ticks cannot test {ticks} ticks; use a window from 1 \
         to the number of ticks"
    )]
    Window { window: u32, ticks: u64 },
    /// The two simulations do not stand at tick 0 with the same state.
    #[error(
        "both simulations must start at tick 0 with the same state, but the straight one stands \
         at tick {straight_tick} with hash {straight:016x} and the rolled back one at tick \
         {rollback_tick} with hash {rollback:016x}; load the same scene or replay into two \
         simulations of the same app"
    )]
    Start {
        straight_tick: u64,
        straight: u64,
        rollback_tick: u64,
        rollback: u64,
    },
    /// The game failed while either simulation played `tick`.
    #[error("the game failed while SyncTest played tick {tick}: {error}")]
    Game { tick: u64, error: ReplayError },
    /// A state saved during the test does not load back.
    #[error("the state SyncTest saved at tick {tick} does not load back: {error}")]
    State { tick: u64, error: StateError },
    /// The first tick whose hashes differ between the straight simulation and the rolled back
    /// one.
    ///
    /// The comparison is between the straight simulation (`sims[0]` of the comparison, so
    /// `OnlyInCurrent` means alive only in the straight one) and the rolled back one (`sims[1]`,
    /// `OnlyInCompare`). Each side loads its state of `tick - 1` and plays that tick again,
    /// observed, for the causes, then loads its state of `tick` to compare. For game code that
    /// is not deterministic, the tick played again may differ from the one that diverged, so
    /// the causes are candidates, as in the editor's branch report. Both simulations are left
    /// at their state of `tick`.
    #[error("{}", describe_mismatch(*tick, *straight, *rollback, entities, resources, warnings))]
    Mismatch {
        tick: u64,
        /// The hash of the straight simulation at `tick`.
        straight: u64,
        /// The hash of the rolled back simulation at `tick`.
        rollback: u64,
        /// The entities that differ, ordered by entity.
        entities: Vec<EntityDivergence>,
        /// The first differing field of each resource that differs, and of the entity
        /// allocator and each asset when they differ: what the entities do not show. Left is
        /// the straight simulation, right the rolled back one.
        resources: Vec<Difference>,
        /// Problems met while building the report, such as a tick that failed when played
        /// again for its causes.
        warnings: Vec<String>,
        /// The states of `tick`: the straight one, then the rolled back one.
        snapshots: Box<[Snapshot; 2]>,
    },
}

/// Runs `ticks` ticks of `inputs` and `edits` twice and compares them at every tick: `other`
/// runs them straight, and `sim` rolls back every tick.
///
/// The caller loads the same scene or replay into both simulations first (`load_replay` for
/// a replay), both built from the same app, and sets their players and threads. For each tick
/// `t`, `other` saves its state, plays `t` and records its hash. `sim` saves its state, plays
/// `t` once with a wrong input, as a misprediction would (the input of `t - 1`, or the
/// default inputs when that equals the real input, without the edits), then loads its state
/// of `max(0, t + 1 - window)` and plays every tick from there to `t` again, saving each
/// state, and compares each hash with the straight one. This catches state kept outside the
/// world, a resimulation that is not deterministic, and state leaking from a mispredicted
/// tick. Both simulations end at tick `ticks` when the test passes.
///
/// # Errors
///
/// [`SyncTestError::Window`] and [`SyncTestError::Start`] before anything runs,
/// [`SyncTestError::Mismatch`] at the first divergent tick, [`SyncTestError::Game`] when the
/// game fails, and [`SyncTestError::State`] when a saved state does not load back.
pub fn sync_test(
    sim: &mut Simulation,
    other: &mut Simulation,
    inputs: &InputLog,
    edits: &EditLog,
    ticks: u64,
    window: u32,
) -> Result<SyncTestReport, SyncTestError> {
    if window == 0 || u64::from(window) > ticks {
        return Err(SyncTestError::Window { window, ticks });
    }
    if other.tick() != 0 || sim.tick() != 0 || other.hash() != sim.hash() {
        return Err(SyncTestError::Start {
            straight_tick: other.tick(),
            straight: other.hash(),
            rollback_tick: sim.tick(),
            rollback: sim.hash(),
        });
    }
    let window = u64::from(window);
    let mut test = Test {
        inputs,
        edits,
        straight_ring: Ring::new(window),
        rollback_ring: Ring::new(window),
        hashes: vec![other.hash()],
    };
    for tick in 0..ticks {
        test.straight_ring.save(other);
        play_tick(other, inputs, edits, tick)
            .map_err(|error| SyncTestError::Game { tick, error })?;
        test.hashes.push(other.hash());

        test.rollback_ring.save(sim);
        sim.step_encoded(mispredicted(sim, inputs, tick).as_deref())
            .map_err(|error| SyncTestError::Game {
                tick,
                error: replay_error(error, tick),
            })?;
        let from = (tick + 1).saturating_sub(window);
        test.rollback_ring.load(sim, from)?;
        for again in from..=tick {
            test.rollback_ring.save(sim);
            play_tick(sim, inputs, edits, again)
                .map_err(|error| SyncTestError::Game { tick: again, error })?;
            let straight = test.hashes[index(again + 1)];
            if sim.hash() != straight {
                // A state that does not load back while reporting is the error to fix first.
                return Err(test
                    .mismatch(other, sim, again + 1, straight)
                    .unwrap_or_else(|error| error));
            }
        }
    }
    Ok(SyncTestReport {
        ticks,
        rollbacks: ticks,
    })
}

/// The input of `tick` a misprediction plays: the input of the tick before, or `None` (the
/// default inputs) at tick 0 or when the input of the tick before equals the real one.
fn mispredicted(sim: &Simulation, inputs: &InputLog, tick: u64) -> Option<Vec<u8>> {
    let encoded = |tick: u64| {
        inputs
            .tick(tick)
            .map(<[u8]>::to_vec)
            .or_else(|| sim.encoded_default_inputs())
    };
    let previous = encoded(tick.checked_sub(1)?);
    (previous != encoded(tick)).then_some(previous).flatten()
}

/// A failure of a mispredicted tick as a [`ReplayError`], as `play_tick` would give it.
fn replay_error(error: SimError, tick: u64) -> ReplayError {
    match error {
        SimError::Panicked(panic) => ReplayError::Panicked {
            panic,
            snapshot: None,
        },
        error => ReplayError::Decode {
            what: format!("mispredicted input of tick {tick}"),
            reason: error.to_string(),
        },
    }
}

/// A tick as an index into the hashes, which hold one per tick played.
fn index(tick: u64) -> usize {
    usize::try_from(tick).unwrap_or(usize::MAX)
}

/// The state of a test in progress.
struct Test<'a> {
    inputs: &'a InputLog,
    edits: &'a EditLog,
    straight_ring: Ring,
    rollback_ring: Ring,
    /// The hash of the straight simulation at every tick played, from tick 0.
    hashes: Vec<u64>,
}

impl Test<'_> {
    /// The report of the first divergent tick `tick`, where `sim` stands, and where `other`
    /// stood or went past. Both simulations end at their state of `tick`.
    fn mismatch(
        &mut self,
        other: &mut Simulation,
        sim: &mut Simulation,
        tick: u64,
        straight: u64,
    ) -> Result<SyncTestError, SyncTestError> {
        let rollback = sim.hash();
        let mut warnings = Vec::new();
        let mut records = [Vec::new(), Vec::new()];
        // The current states are saved too: `tick` for `sim`, the latest tick for `other`.
        self.straight_ring.save(other);
        self.rollback_ring.save(sim);
        let sides = [
            (&mut *other, &self.straight_ring, "straight"),
            (&mut *sim, &self.rollback_ring, "rolled back"),
        ];
        for ((side, ring, name), record) in sides.into_iter().zip(&mut records) {
            ring.load(side, tick - 1)?;
            let mut recorder = TickRecorder::default();
            match play_tick_observed(side, self.inputs, self.edits, tick - 1, &mut recorder) {
                Ok(()) => record.push(recorder.into_record(tick)),
                Err(error) => warnings.push(format!(
                    "the {name} simulation failed playing tick {} again for the causes: {error}",
                    tick - 1
                )),
            }
            ring.load(side, tick)?;
        }
        let mut comparison = EntityComparison::new(other);
        let entities = comparison
            .compare([&*other, &*sim], [&records[0], &records[1]], 0)
            .unwrap_or_else(|error| {
                warnings.push(format!("the entities could not be compared: {error}"));
                Vec::new()
            });
        warnings.extend(comparison.take_warnings());
        let snapshots = Box::new([other.snapshot(), sim.snapshot()]);
        let resources = resource_differences(other, &snapshots).unwrap_or_else(|error| {
            warnings.push(format!("the resources could not be compared: {error}"));
            Vec::new()
        });
        Ok(SyncTestError::Mismatch {
            tick,
            straight,
            rollback,
            entities,
            resources,
            warnings,
            snapshots,
        })
    }
}

/// The first differing field of each resource, of the entity allocator and of each asset
/// that differ between `snapshots`: their components are left out, as the entity comparison
/// reports them.
fn resource_differences(
    sim: &Simulation,
    snapshots: &[Snapshot; 2],
) -> Result<Vec<Difference>, ludenea_ecs::SnapshotError> {
    let without_components = |snapshot: &Snapshot| Snapshot {
        components: snapshot
            .components
            .iter()
            .map(|(name, _)| (name.clone(), Vec::new()))
            .collect(),
        ..snapshot.clone()
    };
    let differences = sim.world().diff_snapshots(
        &without_components(&snapshots[0]),
        &without_components(&snapshots[1]),
    )?;
    let mut seen = BTreeSet::new();
    Ok(differences
        .into_iter()
        .filter(|difference| {
            seen.insert((difference.subject.clone(), difference.component.clone()))
        })
        .collect())
}

/// The states of the latest `window + 2` ticks of one simulation, each slot tagged with the
/// tick it holds, if any.
struct Ring {
    slots: Vec<(Option<u64>, SavedState)>,
    capacity: u64,
}

impl Ring {
    fn new(window: u64) -> Ring {
        Ring {
            slots: Vec::new(),
            capacity: window + 2,
        }
    }

    /// Saves `sim` as the state of its tick, over the oldest one once the ring is full.
    fn save(&mut self, sim: &Simulation) {
        let tick = sim.tick();
        let slot = index(tick % self.capacity);
        if slot >= self.slots.len() {
            self.slots
                .resize_with(slot + 1, || (None, SavedState::default()));
        }
        let (saved, state) = &mut self.slots[slot];
        *saved = Some(tick);
        sim.save_state(state);
    }

    /// Loads the state saved for `tick`. The test only loads ticks the ring still holds, so
    /// a tick it does not hold is refused as never saved.
    fn load(&self, sim: &mut Simulation, tick: u64) -> Result<(), SyncTestError> {
        let state = |error| SyncTestError::State { tick, error };
        let (_, saved) = self
            .slots
            .get(index(tick % self.capacity))
            .filter(|(saved, _)| *saved == Some(tick))
            .ok_or_else(|| state(StateError::Empty))?;
        sim.load_state(saved).map_err(state)
    }
}

fn describe_mismatch(
    tick: u64,
    straight: u64,
    rollback: u64,
    entities: &[EntityDivergence],
    resources: &[Difference],
    warnings: &[String],
) -> String {
    let mut text = format!(
        "the rolled back simulation diverges from the straight one at tick {tick}: straight \
         hash {straight:016x}, rolled back hash {rollback:016x}; each value below reads \
         straight != rolled back"
    );
    for divergence in entities.iter().take(LISTED_ENTITIES) {
        let _ = write!(
            text,
            "\n  entity {}: {}",
            divergence.entity,
            kind(divergence)
        );
        let _ = write!(text, "; {}", causes(&divergence.causes));
    }
    if entities.len() > LISTED_ENTITIES {
        let _ = write!(
            text,
            "\n  and {} more entities",
            entities.len() - LISTED_ENTITIES
        );
    }
    for difference in resources {
        let _ = write!(text, "\n  {} {}", difference.subject, field(difference));
    }
    for warning in warnings {
        let _ = write!(text, "\n  warning: {warning}");
    }
    let _ = write!(
        text,
        "\nthe causes are candidates: they come from playing tick {} again, which game code \
         that is not deterministic may play differently",
        tick - 1
    );
    text
}

fn kind(divergence: &EntityDivergence) -> String {
    match (divergence.kind, &divergence.first_difference) {
        (DivergenceKind::Changed, Some(difference)) => field(difference),
        (DivergenceKind::Changed, None) => {
            "changed, its first difference could not be listed".to_string()
        }
        (DivergenceKind::OnlyInCurrent, _) => "alive only in the straight simulation".to_string(),
        (DivergenceKind::OnlyInCompare, _) => {
            "alive only in the rolled back simulation".to_string()
        }
        (DivergenceKind::Replaced, _) => "a different component set in each simulation".to_string(),
    }
}

/// `component.path: left != right`.
fn field(difference: &Difference) -> String {
    let mut text = difference.component.clone();
    for segment in &difference.path {
        if !segment.starts_with('[') {
            text.push('.');
        }
        text.push_str(segment);
    }
    let _ = write!(
        text,
        ": {} != {}",
        difference.left_text, difference.right_text
    );
    text
}

fn causes(causes: &[Cause]) -> String {
    if causes.is_empty() {
        return "no recorded event or command links it to another divergent entity".to_string();
    }
    let named: Vec<String> = causes
        .iter()
        .map(|cause| match cause {
            Cause::Event {
                tick,
                event,
                path,
                other,
            } => format!("event {event} of tick {tick} with entity {other} at {path}"),
            Cause::Command { tick, system, kind } => {
                format!("{kind} command of {system} at tick {tick}")
            }
        })
        .collect();
    format!("causes: {}", named.join(", "))
}
