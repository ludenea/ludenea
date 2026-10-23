use std::any::{TypeId, type_name};
use std::collections::BTreeSet;

use ludenea_draw::DebugDrawFrame;
use ludenea_draw::frame::RenderFrame;
use ludenea_ecs::{
    Asset, BuildError, Component, EcsError, Event, IntoSystemConfig, Prefab, Resource,
    ScheduleBuilder, World, check_not_recursive,
};

use ludenea_ecs::Entity;
use ludenea_math::Vec2;

use crate::input::{Input, InputType, Inputs};
use crate::{Parallel, RawInput, Rng, SceneError, Simulation, Time};

/// Draws the requested layers of a world into a frame; skips layers not in the set.
pub type DebugDrawFn = fn(&World, &BTreeSet<String>, &mut DebugDrawFrame);
/// The entities under a world point.
pub type PickFn = fn(&World, Vec2) -> Vec<Entity>;
/// Adds what a world draws to a render frame, such as its sprites or its camera.
pub type ExtractFn = fn(&World, &mut RenderFrame);

/// The editor and render hooks an app registered, handed to its `Simulation`.
#[derive(Default)]
pub(crate) struct EditorHooks {
    pub(crate) debug_layers: BTreeSet<&'static str>,
    /// Render layers and their parallax, in draw order.
    pub(crate) render_layers: Vec<(&'static str, f32)>,
    pub(crate) extractors: Vec<ExtractFn>,
    pub(crate) drawers: Vec<DebugDrawFn>,
    pub(crate) pickers: Vec<PickFn>,
    pub(crate) mapping: Option<RawMapping>,
}

/// Encodes one raw input per player (`None`: the default input) as the postcard bytes of a
/// `Vec` of the mapped input type.
pub(crate) type MapRawFn = Box<dyn Fn(&[Option<RawInput>]) -> Vec<u8> + Send + Sync>;

/// Encodes the input of one player (`None`: the default input) as the postcard bytes of one
/// mapped input, mapped with the given player index.
pub(crate) type MapRawOneFn = Box<dyn Fn(Option<&RawInput>, usize) -> Vec<u8> + Send + Sync>;

/// A registered raw input mapping, erased like `InputType`.
pub(crate) struct RawMapping {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
    pub(crate) encode: MapRawFn,
    pub(crate) encode_one: MapRawOneFn,
}

/// A group of registrations and systems added to an app in one call.
pub trait Plugin {
    fn build(&self, app: &mut App);
}

/// The fixed-tick simulation schedule, the only schedule in this milestone.
pub struct Sim;

/// The most render layers an app declares: `SpriteItem::layer` is a `u16` index.
const MAX_LAYERS: usize = u16::MAX as usize + 1;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("app registration failed: {0}")]
    Ecs(#[from] EcsError),
    #[error("app schedule is invalid: {0}")]
    Build(#[from] BuildError),
    #[error("the render layer `{0}` is declared twice; give each layer its own name")]
    DuplicateLayer(String),
    /// A render frame indexes its layers with a `u16`, so it holds `limit` layers at most.
    #[error(
        "the app declares {count} render layers, past the limit of {limit}; merge some of them"
    )]
    TooManyLayers { count: usize, limit: usize },
    /// A parallax that is NaN or infinite has no position to draw at, and no number to
    /// travel as in a render frame.
    #[error("the render layer `{layer}` has the parallax {parallax}; give it a finite number")]
    NonFiniteParallax { layer: &'static str, parallax: f32 },
    #[error(
        "the input mapping builds `{mapping}` but the app's input type is {input}; map the type given to `App::input`"
    )]
    InputMappingMismatch {
        mapping: &'static str,
        input: String,
    },
    /// The built world cannot be written as the scene a session starts from, or that scene
    /// does not load back.
    #[error("the built world cannot be written as a scene that loads back: {0}")]
    Scene(#[from] SceneError),
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
    /// An app with `Time`, `Rng::new(0, 0)`, and the `Prefab` asset type registered,
    /// ticking at 60 Hz.
    pub fn new() -> Self {
        let tick_rate = 60;
        let mut app = Self {
            world: World::new(),
            schedule: ScheduleBuilder::default(),
            input: None,
            tick_rate,
            hooks: EditorHooks::default(),
            errors: Vec::new(),
        };
        // `build` sets `dt` from the final tick rate; this one already loads back.
        app.resource(Time {
            tick: 0,
            dt: 1.0 / tick_rate as f32,
        })
        .resource(Rng::new(0, 0))
        .asset::<Prefab>();
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

    pub fn asset<T: Asset>(&mut self) -> &mut Self {
        if let Err(error) = self.world.register_asset::<T>() {
            self.errors.push(error);
        }
        self
    }

    /// Registers the asset type `T`, whose new assets start as `T::default()`.
    pub fn asset_with_default<T: Asset + Default>(&mut self) -> &mut Self {
        if let Err(error) = self.world.register_asset_with_default::<T>() {
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

    pub fn event<T: Event>(&mut self) -> &mut Self {
        if let Err(error) = self.world.register_event::<T>() {
            self.errors.push(error);
        }
        self
    }

    /// Sets the per-player input type; a later call replaces it. `build` fails when `I`
    /// holds a type that contains itself.
    pub fn input<I: Input>(&mut self) -> &mut Self {
        if let Err(error) = check_not_recursive::<I>() {
            self.errors.push(error);
        }
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
                    .map(|(index, raw)| raw.as_ref().map_or_else(I::default, |raw| map(raw, index)))
                    .collect();
                postcard::to_allocvec(&inputs).expect("inputs always encode")
            }),
            encode_one: Box::new(move |raw, player| {
                let input = raw.map_or_else(I::default, |raw| map(raw, player));
                postcard::to_allocvec(&input).expect("inputs always encode")
            }),
        });
        self
    }

    /// Registers a debug drawer for `layers`, shown by the editor viewport.
    pub fn debug_draw(&mut self, layers: &[&'static str], draw: DebugDrawFn) -> &mut Self {
        self.hooks.debug_layers.extend(layers);
        self.hooks.drawers.push(draw);
        self
    }

    /// Declares a render layer; layers draw in declaration order. `parallax` 1.0 moves with
    /// the world, 0.0 is fixed to the screen. `build` refuses a name declared twice, and more
    /// than 65,536 layers.
    pub fn render_layer(&mut self, name: &'static str, parallax: f32) -> &mut Self {
        self.hooks.render_layers.push((name, parallax));
        self
    }

    /// Registers an extractor, run by `Simulation::render_frame` in registration order.
    pub fn extractor(&mut self, extract: ExtractFn) -> &mut Self {
        self.hooks.extractors.push(extract);
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
        let (count, limit) = (self.hooks.render_layers.len(), MAX_LAYERS);
        if count > limit {
            return Err(AppError::TooManyLayers { count, limit });
        }
        let mut declared = BTreeSet::new();
        if let Some((name, _)) = self
            .hooks
            .render_layers
            .iter()
            .find(|(name, _)| !declared.insert(*name))
        {
            return Err(AppError::DuplicateLayer(name.to_string()));
        }
        if let Some(&(layer, parallax)) = self
            .hooks
            .render_layers
            .iter()
            .find(|(_, parallax)| !parallax.is_finite())
        {
            return Err(AppError::NonFiniteParallax { layer, parallax });
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
        self.world.insert_runtime(Parallel::single());
        let schedule = self.schedule.build(&self.world)?;
        Ok(Simulation::new(
            self.world,
            schedule,
            self.input,
            self.tick_rate,
            self.hooks,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_app_holds_a_time_that_loads_back() {
        let app = App::new();
        let time = app
            .world
            .resource::<Time>()
            .expect("App::new registers Time")
            .clone();
        let text = ron::to_string(&time).expect("Time encodes");
        let loaded: Result<Time, _> = ron::from_str(&text);
        assert_eq!(loaded.as_ref().ok(), Some(&time), "{loaded:?}");
    }
}
