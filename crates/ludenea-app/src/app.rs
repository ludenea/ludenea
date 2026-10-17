use std::any::{TypeId, type_name};
use std::collections::BTreeSet;

use ludenea_debug_draw::DebugDrawFrame;
use ludenea_ecs::{
    BuildError, Component, EcsError, IntoSystemConfig, Resource, ScheduleBuilder, World,
};

use ludenea_ecs::Entity;
use ludenea_math::Vec2;

use crate::input::{Input, InputType, Inputs};
use crate::{Parallel, RawInput, Rng, Simulation, Time};

/// Draws the requested layers of a world into a frame; skips layers not in the set.
pub type DebugDrawFn = fn(&World, &BTreeSet<String>, &mut DebugDrawFrame);
/// The entities under a world point.
pub type PickFn = fn(&World, Vec2) -> Vec<Entity>;

/// The editor hooks an app registered, handed to its `Simulation`.
#[derive(Default)]
pub(crate) struct EditorHooks {
    pub(crate) layers: BTreeSet<&'static str>,
    pub(crate) drawers: Vec<DebugDrawFn>,
    pub(crate) pickers: Vec<PickFn>,
    pub(crate) mapping: Option<RawMapping>,
}

/// Encodes one raw input per player as the postcard bytes of a `Vec` of the mapped input type.
pub(crate) type MapRawFn = Box<dyn Fn(&[RawInput]) -> Vec<u8> + Send + Sync>;

/// A registered raw input mapping, erased like `InputType`.
pub(crate) struct RawMapping {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
    pub(crate) encode: MapRawFn,
}

/// A group of registrations and systems added to an app in one call.
pub trait Plugin {
    fn build(&self, app: &mut App);
}

/// The fixed-tick simulation schedule, the only schedule in this milestone.
pub struct Sim;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("app registration failed: {0}")]
    Ecs(#[from] EcsError),
    #[error("app schedule is invalid: {0}")]
    Build(#[from] BuildError),
    #[error(
        "the input mapping builds `{mapping}` but the app's input type is {input}; map the type given to `App::input`"
    )]
    InputMappingMismatch {
        mapping: &'static str,
        input: String,
    },
}

/// Collects registrations, plugins, and systems, then builds a `Simulation`.
pub struct App {
    world: World,
    schedule: ScheduleBuilder,
    input: Option<InputType>,
    tick_rate: u32,
    hooks: EditorHooks,
    /// Registration errors, reported by `build` so registration calls can chain.
    errors: Vec<EcsError>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An app with `Time` and `Rng::new(0, 0)` registered, ticking at 60 Hz.
    pub fn new() -> Self {
        let mut app = Self {
            world: World::new(),
            schedule: ScheduleBuilder::default(),
            input: None,
            tick_rate: 60,
            hooks: EditorHooks::default(),
            errors: Vec::new(),
        };
        app.resource(Time { tick: 0, dt: 0.0 })
            .resource(Rng::new(0, 0));
        app
    }

    pub fn plugin(&mut self, plugin: impl Plugin) -> &mut Self {
        plugin.build(self);
        self
    }

    pub fn component<T: Component>(&mut self) -> &mut Self {
        if let Err(error) = self.world.register_component::<T>() {
            self.errors.push(error);
        }
        self
    }

    /// Registers `T` with `value`, replacing any earlier value of `T`.
    pub fn resource<T: Resource>(&mut self, value: T) -> &mut Self {
        if let Err(error) = self.world.register_resource(value) {
            self.errors.push(error);
        }
        self
    }

    pub fn event<T: Send + 'static>(&mut self) -> &mut Self {
        self.world.register_event::<T>();
        self
    }

    /// Sets the per-player input type; a later call replaces it.
    pub fn input<I: Input>(&mut self) -> &mut Self {
        self.world.insert_runtime(Inputs {
            players: vec![I::default()],
        });
        self.input = Some(InputType::of::<I>());
        self
    }

    pub fn system<M>(&mut self, _schedule: Sim, system: impl IntoSystemConfig<M>) -> &mut Self {
        self.schedule.add(system);
        self
    }

    /// Registers how raw keyboard and gamepad state becomes the app's input; the `usize`
    /// is the player index. A later call replaces it.
    pub fn input_mapping<I: Input>(&mut self, map: fn(&RawInput, usize) -> I) -> &mut Self {
        self.hooks.mapping = Some(RawMapping {
            id: TypeId::of::<I>(),
            name: type_name::<I>(),
            encode: Box::new(move |players| {
                let inputs: Vec<I> = players
                    .iter()
                    .enumerate()
                    .map(|(index, raw)| map(raw, index))
                    .collect();
                postcard::to_allocvec(&inputs).expect("inputs always encode")
            }),
        });
        self
    }

    /// Registers a debug drawer for `layers`, shown by the editor viewport.
    pub fn debug_draw(&mut self, layers: &[&'static str], draw: DebugDrawFn) -> &mut Self {
        self.hooks.layers.extend(layers);
        self.hooks.drawers.push(draw);
        self
    }

    /// Registers a picker that finds the entities under a world point.
    pub fn picker(&mut self, pick: PickFn) -> &mut Self {
        self.hooks.pickers.push(pick);
        self
    }

    /// Ticks per simulated second; `Time.dt` is its inverse.
    ///
    /// # Panics
    /// If `hertz` is 0.
    pub fn tick_rate(&mut self, hertz: u32) -> &mut Self {
        assert!(hertz > 0, "tick_rate must be at least 1 Hz");
        self.tick_rate = hertz;
        self
    }

    /// Returns the first registration error, or the schedule build error, if any.
    pub fn build(mut self) -> Result<Simulation, AppError> {
        if let Some(error) = self.errors.into_iter().next() {
            return Err(error.into());
        }
        if let Some(mapping) = &self.hooks.mapping
            && self.input.as_ref().map(|input| input.id) != Some(mapping.id)
        {
            return Err(AppError::InputMappingMismatch {
                mapping: mapping.name,
                input: self
                    .input
                    .as_ref()
                    .map_or("missing".to_string(), |input| format!("`{}`", input.name)),
            });
        }
        if let Some(time) = self.world.resource_mut::<Time>() {
            time.dt = 1.0 / self.tick_rate as f32;
        }
        self.world.insert_runtime(Parallel::new(1));
        let schedule = self.schedule.build(&self.world)?;
        Ok(Simulation::new(
            self.world, schedule, self.input, self.hooks,
        ))
    }
}
