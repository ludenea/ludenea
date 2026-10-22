//! The arena's game. For now only its art: the fighters' sprite sheets and the town's
//! textures, whose identifiers are the ones in the `.meta` files next to them.

use ludenea::prelude::*;

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

pub fn app() -> App {
    let mut app = App::new();
    app.plugin(RenderPlugin);
    app
}
