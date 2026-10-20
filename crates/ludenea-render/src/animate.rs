//! The `animate` system: plays each `SpriteAnimation` on its entity's `Sprite`.

use ludenea_app::Time;
use ludenea_ecs::{Assets, Query, Res};

use crate::components::{Sprite, SpriteAnimation};
use crate::sheet::SpriteSheet;

/// Writes the texture and the frame of the current tick to every animated sprite. A sprite
/// whose sheet is not loaded or has no such clip keeps its last frame; the render frame
/// warns about it.
pub fn animate(
    time: Res<Time>,
    sheets: Assets<SpriteSheet>,
    mut sprites: Query<(&SpriteAnimation, &mut Sprite)>,
) {
    for (animation, sprite) in sprites.iter_mut() {
        let Some(sheet) = sheets.try_get(animation.sheet) else {
            continue;
        };
        if let Ok(frame) = animation.frame(sheet, time.tick) {
            sprite.texture = sheet.texture;
            sprite.frame = frame;
        }
    }
}
