use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};

use ludenea_draw::DebugDrawFrame;
use ludenea_draw::frame::{LayerInfo, RenderFrame};
use ludenea_ecs::{AssetId, EcsError, Entity};
use ludenea_math::Vec2;

use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::{
    HashTree, SavedError, SavedWorld, Schedule, ScheduleObserver, Snapshot, SnapshotError,
    SystemInfo, World, hash_value,
};

use crate::app::EditorHooks;
use crate::input::{self, Input, InputType};
use crate::panic::{GamePanic, catch_panic};
use crate::prefab::LoadedInstance;
use crate::scene::LoadAddress;
use crate::{Parallel, ParallelError, RawInput, Time};

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
    /// Game code panicked. A tick it was running is left halfway: restore a snapshot before
    /// stepping on.
    #[error("{0}")]
    Panicked(Box<GamePanic>),
    /// The asset type is not registered, or has no default value.
    #[error(transparent)]
    AssetDefault(Box<EcsError>),
}

/// The simulation state saved in memory by `Simulation::save_state`, stamped with the
/// schema fingerprint of the simulation that saved it. Saving into the same `SavedState`
/// again reuses its allocations.
#[derive(Default)]
pub struct SavedState {
    /// `None` until a simulation saves into it.
    fingerprint: Option<u64>,
    world: SavedWorld,
}

/// Why `Simulation::load_state` refused a saved state; the simulation is unchanged.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error(
        "nothing was saved into this saved state; save the simulation into it before loading it"
    )]
    Empty,
    #[error(
        "the state was saved by another game build (schema fingerprint {saved:016x}, this simulation has {current:016x}); load it into the simulation that saved it"
    )]
    OtherWorld { saved: u64, current: u64 },
    #[error(transparent)]
    World(#[from] SavedError),
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
    /// See [`Simulation::resolved_scene`].
    pub(crate) resolved_scene: Option<String>,
    /// The file of each asset the last scene load read from a file; not part of the state.
    pub(crate) asset_paths: BTreeMap<AssetId, String>,
}

impl Simulation {
    pub(crate) fn new(
        world: World,
        schedule: Schedule,
        input: Option<InputType>,
        tick_rate: u32,
        hooks: EditorHooks,
    ) -> Self {
        let mut simulation = Self {
            initial: world.snapshot(),
            world,
            schedule,
            input,
            players: 1,
            tick_rate,
            hooks,
            load_addresses: BTreeMap::new(),
            instances: Vec::new(),
            resolved_scene: None,
            asset_paths: BTreeMap::new(),
        };
        simulation.initial.fingerprint = simulation.schema_fingerprint();
        simulation
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

    /// Steps with default inputs for every player. When the game panics, returns
    /// `SimError::Panicked` naming the tick and the system, and the tick is left halfway.
    pub fn step(&mut self) -> Result<(), SimError> {
        self.stepping(None, |sim| {
            sim.write_defaults();
            Ok(())
        })
    }

    /// Steps with `inputs`, exactly one per player. On error nothing changes, except after a
    /// panic, as for `step`.
    pub fn step_with<I: Input>(&mut self, inputs: &[I]) -> Result<(), SimError> {
        let input = self.input.as_ref().ok_or(SimError::NoInputType)?;
        if input.id != TypeId::of::<I>() {
            return Err(SimError::InputTypeMismatch {
                expected: input.name,
            });
        }
        input::check_player_count(self.players, inputs.len())?;
        self.stepping(None, |sim| {
            input::write(&mut sim.world, inputs.to_vec());
            Ok(())
        })
    }

    /// Steps with a postcard `Vec` of the app's input type (one per player), or default inputs for `None`.
    /// On error nothing changes, except after a panic, as for `step`.
    pub fn step_encoded(&mut self, inputs: Option<&[u8]>) -> Result<(), SimError> {
        self.stepping(None, |sim| sim.write_inputs(inputs))
    }

    /// Like `step_encoded`, reporting each system, its commands, and the events of the tick
    /// to `observer`. Observing never changes the result: hashes equal those of `step_encoded`.
    pub fn step_observed(
        &mut self,
        inputs: Option<&[u8]>,
        observer: &mut dyn ScheduleObserver,
    ) -> Result<(), SimError> {
        self.stepping(Some(observer), |sim| sim.write_inputs(inputs))
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
        let players: Vec<Option<RawInput>> = players.iter().cloned().map(Some).collect();
        self.map_players(&players)
    }

    /// Like `map_raw_input`, with `None` for a player without a connected device: it
    /// encodes `I::default()`, not the mapping of an idle raw input.
    pub fn map_players(&self, players: &[Option<RawInput>]) -> Result<Vec<u8>, SimError> {
        let mapping = self
            .hooks
            .mapping
            .as_ref()
            .ok_or(SimError::NoInputMapping)?;
        input::check_player_count(self.players, players.len())?;
        self.guarded(|| (mapping.encode)(players))
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
        self.hooks.debug_layers.iter().copied().collect()
    }

    /// What every registered drawer draws for the requested layers.
    pub fn debug_draw(&self, layers: &BTreeSet<String>) -> Result<DebugDrawFrame, SimError> {
        self.guarded(|| self.draw_debug(layers))
    }

    /// What a renderer draws at this tick: the render layers, what every extractor adds in
    /// registration order, and the debug draw of `debug_layers`. Never changes the state.
    pub fn render_frame(&self, debug_layers: &BTreeSet<String>) -> Result<RenderFrame, SimError> {
        self.guarded(|| {
            let mut frame = RenderFrame {
                tick: self.tick(),
                camera: None,
                layers: self
                    .hooks
                    .render_layers
                    .iter()
                    .map(|&(name, parallax)| LayerInfo {
                        name: name.to_string(),
                        parallax,
                    })
                    .collect(),
                sprites: Vec::new(),
                tilemaps: Vec::new(),
                teleported: Vec::new(),
                debug: DebugDrawFrame::default(),
                warnings: Vec::new(),
            };
            for extract in &self.hooks.extractors {
                extract(&self.world, &mut frame);
            }
            frame.debug = self.draw_debug(debug_layers);
            frame
        })
    }

    fn draw_debug(&self, layers: &BTreeSet<String>) -> DebugDrawFrame {
        let mut frame = DebugDrawFrame::default();
        for draw in &self.hooks.drawers {
            draw(&self.world, layers, &mut frame);
        }
        frame
    }

    /// The entities under `point` according to every registered picker, ascending.
    pub fn pick(&self, point: Vec2) -> Result<Vec<Entity>, SimError> {
        self.guarded(|| {
            let found: BTreeSet<Entity> = self
                .hooks
                .pickers
                .iter()
                .flat_map(|pick| pick(&self.world, point))
                .collect();
            found.into_iter().collect()
        })
    }

    /// A new asset of the asset type `type_name` in RON: its default value.
    pub fn default_asset_ron(&self, type_name: &str) -> Result<String, SimError> {
        self.guarded(|| self.world.default_asset_ron(type_name))?
            .map_err(|error| SimError::AssetDefault(Box::new(error)))
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

    /// The snapshot taken right after `App::build`, before any step, stamped with
    /// `schema_fingerprint`.
    pub fn initial_snapshot(&self) -> &Snapshot {
        &self.initial
    }

    pub fn restore(&mut self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        self.world.restore(snapshot)
    }

    /// Saves the simulation state into `into`, reusing its allocations: far cheaper than
    /// `snapshot`, for a rollback that saves every tick.
    pub fn save_state(&self, into: &mut SavedState) {
        into.fingerprint = Some(self.initial.fingerprint);
        self.world.save_into(&mut into.world);
    }

    /// Loads a state `save_state` saved, as `restore` restores a snapshot: queued commands
    /// and events are dropped, runtime resources kept. Refuses a state never saved into or
    /// saved by another simulation, before changing anything.
    pub fn load_state(&mut self, saved: &SavedState) -> Result<(), StateError> {
        let fingerprint = saved.fingerprint.ok_or(StateError::Empty)?;
        if fingerprint != self.initial.fingerprint {
            return Err(StateError::OtherWorld {
                saved: fingerprint,
                current: self.initial.fingerprint,
            });
        }
        Ok(self.world.load(&saved.world)?)
    }

    /// Replaces the `Parallel` runtime resource with a pool of `threads`, from 1 to
    /// [`Parallel::MAX_THREADS`]; results never depend on the count. On error the previous
    /// pool stays.
    pub fn set_threads(&mut self, threads: usize) -> Result<(), ParallelError> {
        self.world.insert_runtime(Parallel::try_new(threads)?);
        Ok(())
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

    /// Checks that `step_encoded(Some(inputs))` would accept `inputs`, without stepping.
    pub(crate) fn check_encoded_inputs(&self, inputs: &[u8]) -> Result<(), SimError> {
        let input = self.input.as_ref().ok_or(SimError::NoInputType)?;
        (input.check_encoded)(inputs, self.players)
    }

    /// Writes the tick's inputs with `write`, runs the schedule, then ends the tick. A panic
    /// of the game comes back as `SimError::Panicked`, naming the system when it was one.
    fn stepping(
        &mut self,
        observer: Option<&mut dyn ScheduleObserver>,
        write: impl FnOnce(&mut Self) -> Result<(), SimError>,
    ) -> Result<(), SimError> {
        let mut scheduled = false;
        let ran = catch_panic(self.tick(), || {
            write(self)?;
            scheduled = true;
            match observer {
                Some(observer) => self.schedule.run_observed(&mut self.world, observer),
                None => self.schedule.run(&mut self.world),
            }
            Ok(())
        });
        match ran {
            Ok(written) => written?,
            Err(mut panic) => {
                panic.system = self.schedule.running().filter(|_| scheduled);
                return Err(SimError::Panicked(self.named(panic)));
            }
        }
        self.world.clear_events();
        self.time_mut().tick += 1;
        Ok(())
    }

    /// `f`, game code run at the current tick, with a panic as `SimError::Panicked`.
    fn guarded<T>(&self, f: impl FnOnce() -> T) -> Result<T, SimError> {
        self.caught(f).map_err(SimError::Panicked)
    }

    /// `f`, game code run at the current tick, with its panic named (see `named`).
    pub(crate) fn caught<T>(&self, f: impl FnOnce() -> T) -> Result<T, Box<GamePanic>> {
        catch_panic(self.tick(), f).map_err(|panic| self.named(panic))
    }

    /// `panic` with each asset the last scene load read from a file named by that file
    /// instead of its identifier.
    pub(crate) fn named(&self, mut panic: Box<GamePanic>) -> Box<GamePanic> {
        for (id, path) in &self.asset_paths {
            panic.message = panic.message.replace(&id.to_string(), path);
        }
        panic
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
