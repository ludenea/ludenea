use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};

use ludenea_debug_draw::DebugDrawFrame;
use ludenea_ecs::Entity;
use ludenea_math::Vec2;

use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::{
    HashTree, Schedule, ScheduleObserver, Snapshot, SnapshotError, SystemInfo, World, hash_value,
};

use crate::app::EditorHooks;
use crate::input::{self, Input, InputType};
use crate::prefab::LoadedInstance;
use crate::scene::LoadAddress;
use crate::{Parallel, RawInput, Time};

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
    #[error(
        "edit log version {found} is not supported (expected {expected}); record it again with this build"
    )]
    EditLogVersion { found: u32, expected: u32 },
    #[error("cannot decode the edit log: {0}; was it written by this build?")]
    EditLogDecode(String),
    #[error("this app has no raw input mapping; register one with `App::input_mapping`")]
    NoInputMapping,
    #[error("the input log has no players; record it with at least one player")]
    NoPlayers,
}

/// A built app: the world and its schedule, stepped one fixed tick at a time.
pub struct Simulation {
    pub(crate) world: World,
    schedule: Schedule,
    input: Option<InputType>,
    players: usize,
    tick_rate: u32,
    hooks: EditorHooks,
    /// The world as built, before any step.
    pub(crate) initial: Snapshot,
    /// Where each entity of the last loaded scene came from; not part of the state.
    pub(crate) load_addresses: BTreeMap<Entity, LoadAddress>,
    /// The prefab instances of the last loaded scene; not part of the state.
    pub(crate) instances: Vec<LoadedInstance>,
}

impl Simulation {
    pub(crate) fn new(
        world: World,
        schedule: Schedule,
        input: Option<InputType>,
        tick_rate: u32,
        hooks: EditorHooks,
    ) -> Self {
        Self {
            initial: world.snapshot(),
            world,
            schedule,
            input,
            players: 1,
            tick_rate,
            hooks,
            load_addresses: BTreeMap::new(),
            instances: Vec::new(),
        }
    }

    /// Completed steps, from `Time.tick`.
    pub fn tick(&self) -> u64 {
        self.time().tick
    }

    /// Ticks per simulated second, as set by `App::tick_rate`.
    pub fn tick_rate(&self) -> u32 {
        self.tick_rate
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
        self.write_defaults();
        self.advance(None);
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
        self.advance(None);
        Ok(())
    }

    /// Steps with a postcard `Vec` of the app's input type (one per player), or default inputs for `None`.
    /// On error nothing changes.
    pub fn step_encoded(&mut self, inputs: Option<&[u8]>) -> Result<(), SimError> {
        self.write_inputs(inputs)?;
        self.advance(None);
        Ok(())
    }

    /// Like `step_encoded`, reporting each system, its commands, and the events of the tick
    /// to `observer`. Observing never changes the result: hashes equal those of `step_encoded`.
    pub fn step_observed(
        &mut self,
        inputs: Option<&[u8]>,
        observer: &mut dyn ScheduleObserver,
    ) -> Result<(), SimError> {
        self.write_inputs(inputs)?;
        self.advance(Some(observer));
        Ok(())
    }

    /// What each system reads and writes, in run order.
    pub fn systems(&self) -> Vec<SystemInfo> {
        self.schedule.systems(&self.world)
    }

    /// What `step_encoded(None)` replays as: default inputs for every player, encoded, or
    /// `None` when the app has no input type.
    pub fn encoded_default_inputs(&self) -> Option<Vec<u8>> {
        self.input
            .as_ref()
            .map(|input| (input.encode_defaults)(self.players))
    }

    /// Maps one raw input per player to the postcard bytes `step_encoded` takes.
    pub fn map_raw_input(&self, players: &[RawInput]) -> Result<Vec<u8>, SimError> {
        let mapping = self
            .hooks
            .mapping
            .as_ref()
            .ok_or(SimError::NoInputMapping)?;
        input::check_player_count(self.players, players.len())?;
        Ok((mapping.encode)(players))
    }

    /// The schema of the app's input type, or `None` when it has none.
    pub fn input_schema(&self) -> Option<TypeSchema> {
        self.input.as_ref().map(|input| (input.schema)())
    }

    /// The schema of every registered asset type, by stable name.
    pub fn asset_schemas(&self) -> Vec<(&'static str, TypeSchema)> {
        self.world.asset_schemas()
    }

    /// The debug draw layers the app registered, sorted.
    pub fn debug_layers(&self) -> Vec<&'static str> {
        self.hooks.layers.iter().copied().collect()
    }

    /// What every registered drawer draws for the requested layers.
    pub fn debug_draw(&self, layers: &BTreeSet<String>) -> DebugDrawFrame {
        let mut frame = DebugDrawFrame::default();
        for draw in &self.hooks.drawers {
            draw(&self.world, layers, &mut frame);
        }
        frame
    }

    /// The entities under `point` according to every registered picker, ascending.
    pub fn pick(&self, point: Vec2) -> Vec<Entity> {
        let found: BTreeSet<Entity> = self
            .hooks
            .pickers
            .iter()
            .flat_map(|pick| pick(&self.world, point))
            .collect();
        found.into_iter().collect()
    }

    /// The load address of every entity spawned by the last scene load.
    pub fn load_addresses(&self) -> &BTreeMap<Entity, LoadAddress> {
        &self.load_addresses
    }

    /// The entity the last scene load spawned at `address`.
    pub fn entity_at_address(&self, address: &[u32]) -> Option<Entity> {
        self.load_addresses
            .iter()
            .find(|(_, loaded)| loaded.as_slice() == address)
            .map(|(&entity, _)| entity)
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

    /// Identifies the game build's types: `hash_value` of the component, resource, asset,
    /// event, and input schemas, each list in name order.
    pub fn schema_fingerprint(&self) -> u64 {
        hash_value(&(
            self.world.component_schemas(),
            self.world.resource_schemas(),
            self.world.asset_schemas(),
            self.world.event_schemas(),
            self.input_schema(),
        ))
    }

    /// The world's snapshot, stamped with `schema_fingerprint`.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            fingerprint: self.schema_fingerprint(),
            ..self.world.snapshot()
        }
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

    fn write_defaults(&mut self) {
        if let Some(input) = &self.input {
            (input.write_defaults)(&mut self.world, self.players);
        }
    }

    /// Writes `inputs` (postcard bytes) or the defaults for `None`. On error nothing changes.
    fn write_inputs(&mut self, inputs: Option<&[u8]>) -> Result<(), SimError> {
        let Some(bytes) = inputs else {
            self.write_defaults();
            return Ok(());
        };
        let input = self.input.as_ref().ok_or(SimError::NoInputType)?;
        (input.write_encoded)(&mut self.world, bytes, self.players)
    }

    /// Runs the schedule on the inputs already written, then ends the tick.
    fn advance(&mut self, observer: Option<&mut dyn ScheduleObserver>) {
        match observer {
            Some(observer) => self.schedule.run_observed(&mut self.world, observer),
            None => self.schedule.run(&mut self.world),
        }
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
