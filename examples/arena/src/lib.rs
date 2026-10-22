//! The arena: local players run and jump as LuizMelo fighters through a town level. One
//! fighter spawns per player present, at that player's `SpawnPoint`; the camera frames them
//! all. The fighters are kinematic bodies moved by raycasts against the level's colliders,
//! and their sprite sheets are the scene's `FighterSheets`, so a scene load reaches them. It
//! is a project: `ludenea.toml` names its scene.

mod camera;
mod fighter;

use ludenea::ecs::AssetId;
use ludenea::physics;
use ludenea::prelude::*;
use ludenea::render::Animate;
use serde::{Deserialize, Serialize};

pub use fighter::{
    FIGHTER_HEIGHT, FIGHTER_RADIUS, GRAVITY, JUMP_SPEED, KILL_Y, MAX_FALL, RUN_SPEED,
};

ludenea::worker_entry!(app);

/// Axes within this absolute value are noise, as for `RawInput::is_idle`.
const DEAD_ZONE: f32 = 0.2;
/// Gamepad API standard mapping: the bottom face button and the horizontal d-pad buttons.
const BUTTON_JUMP: usize = 0;
const DPAD_LEFT: usize = 14;
const DPAD_RIGHT: usize = 15;
/// A gamepad button counts as pressed above this value.
const PRESSED: f32 = 0.5;

/// The identifier in `art/warrior/idle.png.meta`.
pub const WARRIOR_IDLE_TEXTURE: &str = "0000000000000000000000000000a001";
/// The identifier in `warrior-idle.sheet.ron.meta`.
pub const WARRIOR_IDLE_SHEET: &str = "0000000000000000000000000000a101";
/// The identifier in `art/warrior/run.png.meta`.
pub const WARRIOR_RUN_TEXTURE: &str = "0000000000000000000000000000a002";
/// The identifier in `warrior-run.sheet.ron.meta`.
pub const WARRIOR_RUN_SHEET: &str = "0000000000000000000000000000a102";
/// The identifier in `art/warrior/jump.png.meta`.
pub const WARRIOR_JUMP_TEXTURE: &str = "0000000000000000000000000000a003";
/// The identifier in `warrior-jump.sheet.ron.meta`.
pub const WARRIOR_JUMP_SHEET: &str = "0000000000000000000000000000a103";
/// The identifier in `art/warrior/fall.png.meta`.
pub const WARRIOR_FALL_TEXTURE: &str = "0000000000000000000000000000a004";
/// The identifier in `warrior-fall.sheet.ron.meta`.
pub const WARRIOR_FALL_SHEET: &str = "0000000000000000000000000000a104";
/// The identifier in `art/hero/idle.png.meta`.
pub const HERO_IDLE_TEXTURE: &str = "0000000000000000000000000000a005";
/// The identifier in `hero-idle.sheet.ron.meta`.
pub const HERO_IDLE_SHEET: &str = "0000000000000000000000000000a105";
/// The identifier in `art/hero/run.png.meta`.
pub const HERO_RUN_TEXTURE: &str = "0000000000000000000000000000a006";
/// The identifier in `hero-run.sheet.ron.meta`.
pub const HERO_RUN_SHEET: &str = "0000000000000000000000000000a106";
/// The identifier in `art/hero/jump.png.meta`.
pub const HERO_JUMP_TEXTURE: &str = "0000000000000000000000000000a007";
/// The identifier in `hero-jump.sheet.ron.meta`.
pub const HERO_JUMP_SHEET: &str = "0000000000000000000000000000a107";
/// The identifier in `art/hero/fall.png.meta`.
pub const HERO_FALL_TEXTURE: &str = "0000000000000000000000000000a008";
/// The identifier in `hero-fall.sheet.ron.meta`.
pub const HERO_FALL_SHEET: &str = "0000000000000000000000000000a108";
/// The identifier in `art/town/tileset.png.meta`.
pub const TOWN_TILESET_TEXTURE: &str = "0000000000000000000000000000a009";
/// The identifier in `art/town/background.png.meta`.
pub const TOWN_BACKGROUND_TEXTURE: &str = "0000000000000000000000000000a00a";
/// The identifier in `art/town/middleground.png.meta`.
pub const TOWN_MIDDLEGROUND_TEXTURE: &str = "0000000000000000000000000000a00b";

/// One player's input of a tick.
#[derive(Serialize, Deserialize, Schema, Default, Clone, Debug, PartialEq)]
pub struct ArenaInput {
    /// The run asked for, from -1 (left) to 1 (right). Values outside are clamped and a
    /// non-finite one counts as 0, so a hand-written input log cannot break the simulation.
    pub run: f32,
    /// Jumps when the fighter stands on something.
    pub jump: bool,
}

/// Where the fighter of local player `player` appears on the first tick.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SpawnPoint {
    pub player: u8,
}

/// A fighter of local player `player`. `velocity` is the move of the current tick, in units
/// per tick; `facing` is 1 (right) or -1 (left), the sign of the last non-zero run.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Fighter {
    pub player: u8,
    pub velocity: Vec2,
    pub grounded: bool,
    pub facing: i8,
}

/// The four sheets of one fighter, one clip each, named like the sheet's animation.
#[derive(Serialize, Deserialize, Schema, Clone, Debug, PartialEq)]
pub struct FighterLook {
    pub idle: Handle<SpriteSheet>,
    pub run: Handle<SpriteSheet>,
    pub jump: Handle<SpriteSheet>,
    pub fall: Handle<SpriteSheet>,
}

/// The fighters' sheets: the warrior for player 0, the hero for the others. A scene lists
/// this resource so that loading it loads the sheets, which no entity of the scene holds.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FighterSheets {
    pub warrior: FighterLook,
    pub hero: FighterLook,
}

impl Default for FighterSheets {
    /// The sheets committed in `assets/`.
    fn default() -> Self {
        // The constants are fixed hexadecimal literals; the tests load them all.
        let sheet = |id: &str| {
            Handle::new(asset_id(id).expect("the sheet constants are hexadecimal identifiers"))
        };
        FighterSheets {
            warrior: FighterLook {
                idle: sheet(WARRIOR_IDLE_SHEET),
                run: sheet(WARRIOR_RUN_SHEET),
                jump: sheet(WARRIOR_JUMP_SHEET),
                fall: sheet(WARRIOR_FALL_SHEET),
            },
            hero: FighterLook {
                idle: sheet(HERO_IDLE_SHEET),
                run: sheet(HERO_RUN_SHEET),
                jump: sheet(HERO_JUMP_SHEET),
                fall: sheet(HERO_FALL_SHEET),
            },
        }
    }
}

impl FighterSheets {
    /// The look of player `player`'s fighter.
    pub fn look(&self, player: u8) -> &FighterLook {
        if player == 0 {
            &self.warrior
        } else {
            &self.hero
        }
    }
}

/// The identifier `hexadecimal` spells, as the crate's constants and the `.meta` files write
/// it; `None` when it is not hexadecimal or does not fit 128 bits.
pub fn asset_id(hexadecimal: &str) -> Option<AssetId> {
    u128::from_str_radix(hexadecimal, 16).ok().map(AssetId)
}

pub struct ArenaPlugin;

impl Plugin for ArenaPlugin {
    fn build(&self, app: &mut App) {
        app.input::<ArenaInput>()
            .input_mapping(map_raw_input)
            .component::<SpawnPoint>()
            .component::<Fighter>()
            .resource(FighterSheets::default())
            .render_layer("sky", 0.1)
            .render_layer("town", 0.4)
            .render_layer("tiles", 1.0)
            .render_layer("fighters", 1.0)
            .system(Sim, fighter::spawn_fighters.before(physics::Step))
            // Registration order is the run order of these four after the physics step.
            .system(Sim, fighter::sense_fighters.after(physics::Step))
            .system(Sim, fighter::move_fighters.after(physics::Step))
            .system(
                Sim,
                fighter::animate_fighters
                    .after(physics::Step)
                    .before(Animate),
            )
            .system(Sim, camera::follow_camera.after(physics::Step));
    }
}

pub fn app() -> App {
    let mut app = App::new();
    app.plugin(PhysicsPlugin)
        .plugin(RenderPlugin)
        .plugin(ArenaPlugin);
    app
}

/// Arrows or A and D run, `Space`, `KeyW` or `ArrowUp` jump; on gamepad 0 the left stick and
/// the d-pad run and button 0 jumps. Opposite directions cancel; the run is clamped to
/// [-1, 1].
pub fn map_raw_input(raw: &RawInput, _player: usize) -> ArenaInput {
    let key = |code: &str| f32::from(u8::from(raw.keys.contains(code)));
    let mut run = key("ArrowRight") + key("KeyD") - key("ArrowLeft") - key("KeyA");
    let mut jump = ["Space", "KeyW", "ArrowUp"]
        .iter()
        .any(|code| raw.keys.contains(*code));
    if let Some(pad) = raw.gamepads.first() {
        let pressed = |index: usize| pad.buttons.get(index).is_some_and(|&value| value > PRESSED);
        let stick = pad
            .axes
            .first()
            .copied()
            .filter(|value| value.abs() > DEAD_ZONE)
            .unwrap_or(0.0);
        run += stick + f32::from(u8::from(pressed(DPAD_RIGHT)))
            - f32::from(u8::from(pressed(DPAD_LEFT)));
        jump |= pressed(BUTTON_JUMP);
    }
    ArenaInput {
        run: ludenea::math::clamp(run, -1.0, 1.0),
        jump,
    }
}

/// The input both players of the recorded match give at `tick`: they run apart, jump, cross
/// and come back, so every animation plays. `record-replay` and the hash tests play it.
pub fn scripted_inputs(tick: u64) -> [ArenaInput; 2] {
    let phase = tick % 200;
    let run = if phase < 80 {
        1.0
    } else if phase < 100 {
        0.0
    } else if phase < 180 {
        -1.0
    } else {
        0.0
    };
    let jump = |offset: u64| (tick + offset).is_multiple_of(70);
    [
        ArenaInput { run, jump: jump(0) },
        ArenaInput {
            run: -run,
            jump: jump(35),
        },
    ]
}
