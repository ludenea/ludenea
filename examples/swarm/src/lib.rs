//! The runtime's test game: 10,000 animated sprites on deterministic paths and four player
//! sprites moved by local players. It exercises the renderer (interpolation, layers, an
//! atlas), the runner and the device assignment, and it is a project: `ludenea.toml` names
//! its scene.

mod assets;

use ludenea::draw::Color;
use ludenea::math::{clamp, sin_cos};
use ludenea::prelude::*;
use serde::{Deserialize, Serialize};

pub use assets::{BUG_SHEET, BUG_TEXTURE, generated_files};

ludenea::worker_entry!(app);

/// The local players the game draws a sprite for.
pub const PLAYERS: usize = 4;
/// Distance a player covers in one tick at full deflection, in world units.
const PLAYER_SPEED: f32 = 0.2;
/// The players stay inside the camera: half of 64 by 36 units, minus the sprite's radius.
const PLAYER_LIMIT: Vec2 = Vec2::new(31.5, 17.5);
/// Axes within this absolute value are noise, as for `RawInput::is_idle`.
const DEAD_ZONE: f32 = 0.2;
/// Gamepad API standard mapping: d-pad buttons.
const DPAD_UP: usize = 12;
const DPAD_DOWN: usize = 13;
const DPAD_LEFT: usize = 14;
const DPAD_RIGHT: usize = 15;
/// What a hidden player sprite is tinted: fully transparent.
const HIDDEN: Color = Color(0x0000_0000);

#[derive(Serialize, Deserialize, Schema, Default, Clone)]
pub struct SwarmInput {
    /// The direction asked for, of length 1 at most.
    pub direction: Vec2,
}

/// A sprite circling `center` at `radius`, `speed` radians per tick, from `phase`.
#[derive(Component, Serialize, Deserialize, Clone)]
pub struct Wanderer {
    pub center: Vec2,
    pub radius: f32,
    pub phase: f32,
    pub speed: f32,
}

/// A sprite moved by local player `index`; hidden while the session has fewer players.
#[derive(Component, Serialize, Deserialize, Clone)]
pub struct Player {
    pub index: u8,
}

pub struct SwarmPlugin;

impl Plugin for SwarmPlugin {
    fn build(&self, app: &mut App) {
        app.input::<SwarmInput>()
            .input_mapping(map_raw_input)
            .component::<Wanderer>()
            .component::<Player>()
            .render_layer("background", 0.5)
            .render_layer("world", 1.0)
            .system(Sim, wander)
            .system(Sim, move_players);
    }
}

pub fn app() -> App {
    let mut app = App::new();
    app.plugin(RenderPlugin).plugin(SwarmPlugin);
    app
}

/// The color of player `index`, `0xRRGGBBAA`.
pub(crate) fn player_color(index: u8) -> Color {
    Color(match index % 4 {
        0 => 0xFF6060FF,
        1 => 0x60A0FFFF,
        2 => 0x60E070FF,
        _ => 0xFFD040FF,
    })
}

/// Arrows and WASD, gamepad 0's left stick and d-pad; the sum is limited to length 1.
fn map_raw_input(raw: &RawInput, _player: usize) -> SwarmInput {
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
    SwarmInput { direction }
}

/// Puts every wanderer on its circle at the current tick.
fn wander(time: Res<Time>, mut wanderers: Query<(&Wanderer, &mut Pose)>) {
    // Exact for ticks below 2^24 (about 77 hours at 60 ticks per second).
    let tick = time.tick as f32;
    for (wanderer, pose) in wanderers.iter_mut() {
        let angle = wanderer.phase + wanderer.speed * tick;
        let (sin, cos) = sin_cos(angle);
        pose.position = wanderer.center + Vec2::new(cos, sin) * wanderer.radius;
        pose.rotation = Rot::from_angle(angle);
    }
}

/// Moves the sprite of each present player and hides the others.
fn move_players(
    inputs: Runtime<Inputs<SwarmInput>>,
    mut players: Query<(&Player, &mut Pose, &mut Sprite)>,
) {
    for (player, pose, sprite) in players.iter_mut() {
        let index = usize::from(player.index);
        if index >= inputs.len() {
            sprite.tint = HIDDEN;
            continue;
        }
        sprite.tint = player_color(player.index);
        let step = inputs.player(index).direction * PLAYER_SPEED;
        pose.position = Vec2::new(
            clamp(pose.position.x + step.x, -PLAYER_LIMIT.x, PLAYER_LIMIT.x),
            clamp(pose.position.y + step.y, -PLAYER_LIMIT.y, PLAYER_LIMIT.y),
        );
    }
}
