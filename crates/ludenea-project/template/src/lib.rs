use ludenea::physics;
use ludenea::prelude::*;
use serde::{Deserialize, Serialize};

ludenea::worker_entry!(app);

/// What the player does during one tick.
#[derive(Serialize, Deserialize, Schema, Default, Clone)]
pub struct GameInput {
    /// Direction held on the arrow keys.
    pub direction: Vec2,
}

/// Tuning shared by every ball, in `assets/settings.ron`.
#[derive(Asset, Serialize, Deserialize, Clone)]
pub struct Settings {
    /// Multiplies the world's gravity for each ball.
    pub gravity_scale: f32,
}

/// A ball of `assets/ball.prefab.ron`, tuned by its settings.
#[derive(Component, Serialize, Deserialize, Clone)]
pub struct Ball {
    pub settings: Handle<Settings>,
}

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.input::<GameInput>()
            .input_mapping(map_raw_input)
            .component::<Ball>()
            .asset::<Settings>()
            .system(Sim, apply_settings.before(physics::Step));
    }
}

pub fn app() -> App {
    let mut app = App::new();
    app.plugin(PhysicsPlugin).plugin(GamePlugin);
    app
}

fn map_raw_input(raw: &RawInput, _player: usize) -> GameInput {
    let axis = |negative: &str, positive: &str| {
        f32::from(i8::from(raw.keys.contains(positive)) - i8::from(raw.keys.contains(negative)))
    };
    GameInput {
        direction: Vec2::new(
            axis("ArrowLeft", "ArrowRight"),
            axis("ArrowDown", "ArrowUp"),
        ),
    }
}

fn apply_settings(settings: Assets<Settings>, mut balls: Query<(&Ball, &mut Body)>) {
    for (ball, body) in balls.iter_mut() {
        body.gravity_scale = settings.get(ball.settings).gravity_scale;
    }
}
