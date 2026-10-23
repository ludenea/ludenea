//! The [`Executor`]: applies a [`Session`]'s requests to a [`Simulation`], keeping the saved
//! states a rollback loads and those of reported ticks, and recovering from a game failure.

use ludenea_app::{SavedState, SimError, Simulation, StateError, join_player_inputs};

use crate::session::{Request, Session, SessionError};
use crate::settings::SessionSettings;

/// Why the executor could not apply a request.
#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    /// The app has no input type, so the session's inputs cannot reach it.
    #[error("this app has no input type; register one with `App::input` before running a session")]
    NoInputType,
    /// A `Save` or an `Advance` of `expected` while the simulation stands at `found`: the
    /// requests come from another session, or the simulation was stepped outside the executor.
    #[error(
        "a request for tick {expected} reached a simulation at tick {found}; step the \
         simulation only through the executor of its session"
    )]
    OutOfStep { expected: u64, found: u64 },
    /// No state saved for `tick` is held: never saved, or overwritten by a later tick.
    #[error("no saved state of tick {tick} is held")]
    MissingState { tick: u64 },
    /// A `Release` of `tick`, whose state is held but not kept for a hash report: it was never
    /// saved with `hash: true`, or was released already.
    #[error(
        "the state of tick {tick} is not kept for a hash report, so there is nothing to release"
    )]
    NotKept { tick: u64 },
    /// Every state held is kept for a reported tick, so none is left for the save of `tick`:
    /// the session broke the bound the capacity is sized from.
    #[error(
        "every one of the {capacity} saved states is kept for a reported tick, none is left \
         to save tick {tick}"
    )]
    StatesExhausted { tick: u64, capacity: usize },
    /// The simulation refused a saved state.
    #[error(transparent)]
    State(#[from] StateError),
    /// The session refused a call: a hash it did not ask for, or a failure of a tick it did
    /// not run.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// The game failed while stepping `tick`. A running or leaving session ended with
    /// `EndReason::LocalFailure`, whose end notice tells the other peers; a session that had
    /// ended already, whose landing failed, keeps its reason. With `recovery` `None`, the
    /// simulation stands at the state of `tick`. Otherwise `recovery` is the first step of
    /// the recovery that failed, ending the session or loading that state, and the
    /// simulation is left halfway: the game failure takes precedence, as the cause.
    #[error(
        "the game failed while stepping tick {tick}: {error}{}",
        recovery_failure(recovery)
    )]
    Game {
        tick: u64,
        error: SimError,
        recovery: Option<Box<ExecuteError>>,
    },
}

/// Applies the requests of a [`Session`] to a [`Simulation`], with the inputs of each tick
/// joined in player order as [`join_player_inputs`] joins them.
///
/// It holds at most [`Executor::capacity`] saved states, each tagged with its tick: the
/// `rollback_window + 2` latest a rollback or a landing loads, plus those kept for reported
/// ticks until their `Release`. With `W` the rollback window, `D` the input delay and `I` the
/// hash interval, the hash bounds of the session (`ViolationRule::HashLagging`) keep the
/// reported ticks within `(W + I - 1) + (W + D + I - 1) + I` ticks, one release waiting for
/// the next advance included, so at most `that / I + 2` of them are kept after an advance, and
/// one more while an advance saves a new one before its releases.
pub struct Executor {
    ring: StateRing,
}

impl Executor {
    /// An executor for a session built with `settings`, which sets the simulation's player
    /// count to the match's. The session it executes for must be built from the same
    /// settings: the capacity is sized from their rules, and requests of another session are
    /// refused only as far as the checks of [`Executor::execute`] reach.
    ///
    /// # Errors
    ///
    /// [`ExecuteError::NoInputType`] for an app without an input type.
    pub fn new(
        simulation: &mut Simulation,
        settings: &SessionSettings,
    ) -> Result<Executor, ExecuteError> {
        if simulation.player_input_spec().is_none() {
            return Err(ExecuteError::NoInputType);
        }
        simulation.set_players(settings.identity.players.len());
        let rules = settings.identity.rules;
        let window = u64::from(rules.rollback_window);
        let delay = u64::from(rules.input_delay);
        // Validated settings have a positive interval; `max` keeps an unvalidated one from
        // dividing by zero.
        let interval = u64::from(rules.hash_interval).max(1);
        let span = (window + interval - 1) + (window + delay + interval - 1) + interval;
        let capacity = (window + 2) + (span / interval + 3);
        Ok(Executor {
            ring: StateRing::new(usize::try_from(capacity).unwrap_or(usize::MAX)),
        })
    }

    /// The most saved states the executor holds.
    pub fn capacity(&self) -> usize {
        self.ring.capacity
    }

    /// Applies `requests`, which `session` returned, to `simulation` in order:
    ///
    /// - `Save` saves the state, after checking the simulation stands at its tick, and for
    ///   `hash: true` answers the session with the state hash through
    ///   [`Session::report_hash`] at once. Calling this before the session's next
    ///   [`Session::outgoing`], with every request of an advance, is what the session's hash
    ///   lag bounds rely on. A hash the session refuses, which only a session that did not
    ///   return that `Save` does, leaves the state not kept.
    /// - `Load` loads the latest state saved for its tick.
    /// - `Advance` checks the tick, then steps the simulation with the joined inputs.
    /// - `Release` stops keeping the state of its tick.
    ///
    /// When the game fails while stepping a tick, it ends the session with [`Session::fail`]
    /// (an ended session keeps its reason), loads the state of that tick (its `Save` came
    /// first), and returns [`ExecuteError::Game`], which names a failure of either step too.
    /// The requests after it are not applied.
    ///
    /// # Errors
    ///
    /// The first request that cannot be applied stops the others: [`ExecuteError::OutOfStep`],
    /// [`ExecuteError::MissingState`], [`ExecuteError::NotKept`],
    /// [`ExecuteError::StatesExhausted`], [`ExecuteError::State`], [`ExecuteError::Session`]
    /// and [`ExecuteError::Game`].
    pub fn execute(
        &mut self,
        simulation: &mut Simulation,
        session: &mut Session,
        requests: Vec<Request>,
    ) -> Result<(), ExecuteError> {
        for request in requests {
            match request {
                Request::Save { tick, hash } => {
                    in_step(simulation, tick)?;
                    let was_kept = self.ring.save(simulation, tick, hash)?;
                    if hash && let Err(error) = session.report_hash(tick, simulation.hash()) {
                        // The session will send no `Release` for a hash it refused.
                        if !was_kept {
                            self.ring.unkeep(tick);
                        }
                        return Err(error.into());
                    }
                }
                Request::Load { tick } => self.ring.load(simulation, tick)?,
                Request::Advance { tick, inputs } => {
                    in_step(simulation, tick)?;
                    if let Err(error) = simulation.step_encoded(Some(&join_player_inputs(&inputs)))
                    {
                        let ended = session.fail(tick).map_err(ExecuteError::from);
                        let restored = self.ring.load(simulation, tick);
                        return Err(ExecuteError::Game {
                            tick,
                            error,
                            recovery: ended.and(restored).err().map(Box::new),
                        });
                    }
                }
                Request::Release { tick } => self.ring.release(tick)?,
            }
        }
        Ok(())
    }
}

/// The end of the message of [`ExecuteError::Game`] when its recovery failed.
fn recovery_failure(recovery: &Option<Box<ExecuteError>>) -> String {
    recovery
        .as_ref()
        .map(|failure| format!("; recovering from it failed too: {failure}"))
        .unwrap_or_default()
}

fn in_step(simulation: &Simulation, tick: u64) -> Result<(), ExecuteError> {
    if simulation.tick() == tick {
        Ok(())
    } else {
        Err(ExecuteError::OutOfStep {
            expected: tick,
            found: simulation.tick(),
        })
    }
}

/// One saved state and the tick it was saved at.
struct Slot {
    tick: u64,
    /// Saved with `hash: true` and not released yet.
    kept: bool,
    state: SavedState,
}

/// At most `capacity` saved states, allocated as they are first needed and reused afterwards.
struct StateRing {
    slots: Vec<Slot>,
    capacity: usize,
}

impl StateRing {
    fn new(capacity: usize) -> Self {
        Self {
            slots: Vec::new(),
            capacity,
        }
    }

    /// Saves `simulation` as the state of `tick`: into the slot already holding `tick`, else
    /// a new slot, else the slot of the lowest tick not kept, as the latest ticks are those a
    /// load needs. Returns whether the state of `tick` was kept before.
    fn save(
        &mut self,
        simulation: &Simulation,
        tick: u64,
        keep: bool,
    ) -> Result<bool, ExecuteError> {
        let index = match self.slots.iter().position(|slot| slot.tick == tick) {
            Some(index) => index,
            None if self.slots.len() < self.capacity => {
                self.slots.push(Slot {
                    tick,
                    kept: false,
                    state: SavedState::default(),
                });
                self.slots.len() - 1
            }
            None => self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| !slot.kept)
                .min_by_key(|(_, slot)| slot.tick)
                .map(|(index, _)| index)
                .ok_or(ExecuteError::StatesExhausted {
                    tick,
                    capacity: self.capacity,
                })?,
        };
        let slot = &mut self.slots[index];
        let was_kept = slot.tick == tick && slot.kept;
        slot.tick = tick;
        slot.kept = was_kept || keep;
        simulation.save_state(&mut slot.state);
        Ok(was_kept)
    }

    /// Stops keeping the state of `tick`, if it is held.
    fn unkeep(&mut self, tick: u64) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.tick == tick) {
            slot.kept = false;
        }
    }

    fn load(&self, simulation: &mut Simulation, tick: u64) -> Result<(), ExecuteError> {
        let slot = self
            .slots
            .iter()
            .find(|slot| slot.tick == tick)
            .ok_or(ExecuteError::MissingState { tick })?;
        Ok(simulation.load_state(&slot.state)?)
    }

    fn release(&mut self, tick: u64) -> Result<(), ExecuteError> {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.tick == tick)
            .ok_or(ExecuteError::MissingState { tick })?;
        if !slot.kept {
            return Err(ExecuteError::NotKept { tick });
        }
        slot.kept = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simulation() -> Simulation {
        let mut app = ludenea_app::App::new();
        app.input::<u8>();
        app.build().unwrap()
    }

    #[test]
    fn kept_states_survive_until_released_and_never_exceed_the_capacity() {
        let mut simulation = simulation();
        let mut ring = StateRing::new(3);
        ring.save(&simulation, 0, true).unwrap();
        // Ticks 1 to 4 share the two slots not kept: the lowest one goes first.
        for tick in 1..=4 {
            simulation.step().unwrap();
            ring.save(&simulation, tick, false).unwrap();
        }
        assert!(ring.load(&mut simulation, 0).is_ok());
        assert!(matches!(
            ring.load(&mut simulation, 2),
            Err(ExecuteError::MissingState { tick: 2 })
        ));
        assert!(ring.load(&mut simulation, 4).is_ok());
        // Keeping every slot leaves none to save into.
        ring.save(&simulation, 3, true).unwrap();
        ring.save(&simulation, 4, true).unwrap();
        assert!(matches!(
            ring.save(&simulation, 5, false),
            Err(ExecuteError::StatesExhausted {
                tick: 5,
                capacity: 3
            })
        ));
        assert_eq!(ring.slots.len(), 3);
        // A release frees its slot for the next save; releasing it again is refused.
        ring.release(0).unwrap();
        assert!(matches!(
            ring.release(0),
            Err(ExecuteError::NotKept { tick: 0 })
        ));
        ring.save(&simulation, 5, false).unwrap();
        assert!(matches!(
            ring.load(&mut simulation, 0),
            Err(ExecuteError::MissingState { tick: 0 })
        ));
    }

    #[test]
    fn a_release_names_whether_the_state_is_held() {
        let simulation = simulation();
        let mut ring = StateRing::new(2);
        ring.save(&simulation, 0, false).unwrap();
        assert!(matches!(
            ring.release(0),
            Err(ExecuteError::NotKept { tick: 0 })
        ));
        assert!(matches!(
            ring.release(1),
            Err(ExecuteError::MissingState { tick: 1 })
        ));
    }
}
