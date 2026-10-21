use ludenea::physics;
use ludenea::prelude::*;
use serde::{Deserialize, Serialize};

ludenea::worker_entry!(app);

/// What the player does during one tick.
#[derive(Serialize, Deserialize, Schema, Default, Clone)]
pub struct GameInput {
    /// Direction held on the arrows, WASD, or gamepad 0.
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

/// Sticks within this distance of the center read as released.
const DEAD_ZONE: f32 = 0.2;
// Button indices of the standard gamepad layout.
const DPAD_UP: usize = 12;
const DPAD_DOWN: usize = 13;
const DPAD_LEFT: usize = 14;
const DPAD_RIGHT: usize = 15;

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

/// Arrows and WASD, gamepad 0's left stick and d-pad; the sum is limited to length 1.
fn map_raw_input(raw: &RawInput, _player: usize) -> GameInput {
    let key = |code: &str| f32::from(u8::from(raw.keys.contains(code)));
    let mut direction = Vec2::new(
        key("ArrowRight") + key("KeyD") - key("ArrowLeft") - key("KeyA"),
        key("ArrowUp") + key("KeyW") - key("ArrowDown") - key("KeyS"),
    );
    if let Some(pad) = raw.gamepads.first() {
        let axis = |index: usize| {
            pad.axes
                .get(index)
                .copied()
                .filter(|value| value.abs() > DEAD_ZONE)
                .unwrap_or(0.0)
        };
        let button = |index: usize| {
            f32::from(u8::from(
                pad.buttons.get(index).is_some_and(|&value| value > 0.5),
            ))
        };
        // The stick's vertical axis points down.
        direction.x += axis(0) + button(DPAD_RIGHT) - button(DPAD_LEFT);
        direction.y += -axis(1) + button(DPAD_UP) - button(DPAD_DOWN);
    }
    if direction.length_squared() > 1.0 {
        direction = direction.normalize();
    }
    GameInput { direction }
}

fn apply_settings(settings: Assets<Settings>, mut balls: Query<(&Ball, &mut Body)>) {
    for (ball, body) in balls.iter_mut() {
        body.gravity_scale = settings.get(ball.settings).gravity_scale;
    }
}
