use ludenea_ecs::{
    BuildError, Component, EcsError, IntoSystemConfig, Resource, ScheduleBuilder, World,
};

use crate::input::{Input, InputType, Inputs};
use crate::{Parallel, Rng, Simulation, Time};

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
}

/// Collects registrations, plugins, and systems, then builds a `Simulation`.
pub struct App {
    world: World,
    schedule: ScheduleBuilder,
    input: Option<InputType>,
    tick_rate: u32,
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
        if let Some(time) = self.world.resource_mut::<Time>() {
            time.dt = 1.0 / self.tick_rate as f32;
        }
        self.world.insert_runtime(Parallel::new(1));
        let schedule = self.schedule.build(&self.world)?;
        Ok(Simulation::new(self.world, schedule, self.input))
    }
}
