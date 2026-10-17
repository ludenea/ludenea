use std::any::TypeId;

use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::{HashTree, Schedule, Snapshot, SnapshotError, World};

use crate::input::{self, Input, InputType};
use crate::{Parallel, Time};

#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error(
        "this app has no input type; register one with `App::input` before stepping with inputs"
    )]
    NoInputType,
    #[error("inputs are not of this app's input type `{expected}`; step with that type")]
    InputTypeMismatch { expected: &'static str },
    #[error("cannot decode the tick inputs: {0}; were they encoded for this app's input type?")]
    InputDecode(String),
    #[error(
        "input log version {found} is not supported (expected {expected}); record it again with this build"
    )]
    InputLogVersion { found: u32, expected: u32 },
    #[error("the input log has no players; record it with at least one player")]
    NoPlayers,
}

/// A built app: the world and its schedule, stepped one fixed tick at a time.
pub struct Simulation {
    pub(crate) world: World,
    schedule: Schedule,
    input: Option<InputType>,
    players: usize,
    /// The world as built, before any step.
    pub(crate) initial: Snapshot,
}

impl Simulation {
    pub(crate) fn new(world: World, schedule: Schedule, input: Option<InputType>) -> Self {
        Self {
            initial: world.snapshot(),
            world,
            schedule,
            input,
            players: 1,
        }
    }

    /// Completed steps, from `Time.tick`.
    pub fn tick(&self) -> u64 {
        self.time().tick
    }

    pub fn players(&self) -> usize {
        self.players
    }

    /// The number of default inputs `step` and `step_encoded(None)` write.
    pub fn set_players(&mut self, players: usize) {
        self.players = players;
    }

    /// Steps with default inputs for every player.
    pub fn step(&mut self) {
        if let Some(input) = &self.input {
            (input.write_defaults)(&mut self.world, self.players);
        }
        self.advance();
    }

    /// Steps with `inputs`, exactly one per player. On error nothing changes.
    pub fn step_with<I: Input>(&mut self, inputs: &[I]) -> Result<(), SimError> {
        let input = self.input.as_ref().ok_or(SimError::NoInputType)?;
        if input.id != TypeId::of::<I>() {
            return Err(SimError::InputTypeMismatch {
                expected: input.name,
            });
        }
        input::check_player_count(self.players, inputs.len())?;
        input::write(&mut self.world, inputs.to_vec());
        self.advance();
        Ok(())
    }

    /// Steps with a postcard `Vec` of the app's input type (one per player), or default inputs for `None`.
    /// On error nothing changes.
    pub fn step_encoded(&mut self, inputs: Option<&[u8]>) -> Result<(), SimError> {
        let Some(bytes) = inputs else {
            self.step();
            return Ok(());
        };
        let input = self.input.as_ref().ok_or(SimError::NoInputType)?;
        (input.write_encoded)(&mut self.world, bytes, self.players)?;
        self.advance();
        Ok(())
    }

    /// The schema of the app's input type, or `None` when it has none.
    pub fn input_schema(&self) -> Option<TypeSchema> {
        self.input.as_ref().map(|input| (input.schema)())
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn hash(&self) -> u64 {
        self.world.hash()
    }

    pub fn hash_tree(&self) -> HashTree {
        self.world.hash_tree()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.world.snapshot()
    }

    /// The snapshot taken right after `App::build`, before any step.
    pub fn initial_snapshot(&self) -> &Snapshot {
        &self.initial
    }

    pub fn restore(&mut self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        self.world.restore(snapshot)
    }

    /// Replaces the `Parallel` runtime resource; results never depend on `threads`.
    pub fn set_threads(&mut self, threads: usize) {
        self.world.insert_runtime(Parallel::new(threads));
    }

    pub fn threads(&self) -> usize {
        self.world
            .runtime::<Parallel>()
            .expect("App::build inserts Parallel")
            .threads()
    }

    /// Runs the schedule on the inputs already written, then ends the tick.
    fn advance(&mut self) {
        self.schedule.run(&mut self.world);
        self.world.clear_events();
        self.time_mut().tick += 1;
    }

    fn time(&self) -> std::cell::Ref<'_, Time> {
        self.world
            .resource::<Time>()
            .expect("App::new registers Time")
    }

    fn time_mut(&mut self) -> &mut Time {
        self.world
            .resource_mut::<Time>()
            .expect("App::new registers Time")
    }
}
