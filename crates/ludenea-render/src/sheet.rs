//! Sprite sheets: a texture's frames grouped into named animation clips, loaded as a data
//! asset so the simulation can play them deterministically.

use std::collections::BTreeMap;

use ludenea_ecs::schema::Schema;
use ludenea_ecs::{Asset, AssetId};
use serde::{Deserialize, Serialize};

use crate::components::SpriteAnimation;

/// The clips of one texture. `texture` is a bare identifier on purpose: a handle would make
/// the simulation load the image. Deserialization rejects what `validate` refuses.
#[derive(Asset, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[asset(name = "ludenea::SpriteSheet")]
#[serde(try_from = "SpriteSheetData", into = "SpriteSheetData")]
pub struct SpriteSheet {
    pub texture: AssetId,
    pub clips: BTreeMap<String, Clip>,
}

/// `count` frames from `first`, each shown for `ticks_per_frame` ticks.
#[derive(Schema, Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct Clip {
    pub first: u32,
    /// At least 1.
    pub count: u32,
    /// At least 1.
    pub ticks_per_frame: u32,
}

/// Why `SpriteSheet::validate` refuses a sheet.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SheetError {
    #[error("clip `{0}`: count must be at least 1")]
    Count(String),
    #[error("clip `{0}`: ticks_per_frame must be at least 1")]
    TicksPerFrame(String),
}

/// Why `SpriteAnimation::frame` has no frame.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AnimationError {
    #[error(
        "animation clip `{clip}` is not in the sprite sheet; its clips: {}",
        quoted(known)
    )]
    UnknownClip { clip: String, known: Vec<String> },
}

impl SpriteSheet {
    pub fn validate(&self) -> Result<(), SheetError> {
        for (name, clip) in &self.clips {
            if clip.count == 0 {
                return Err(SheetError::Count(name.clone()));
            }
            if clip.ticks_per_frame == 0 {
                return Err(SheetError::TicksPerFrame(name.clone()));
            }
        }
        Ok(())
    }
}

impl Clip {
    /// The frame shown `elapsed` ticks after the clip started: it wraps when `looping`,
    /// else holds the last frame. A `count` or `ticks_per_frame` of 0 counts as 1.
    pub fn frame_at(&self, elapsed: u64, looping: bool) -> u32 {
        let count = u64::from(self.count.max(1));
        let shown = elapsed / u64::from(self.ticks_per_frame.max(1));
        let index = if looping {
            shown % count
        } else {
            shown.min(count - 1)
        };
        // `index < count <= u32::MAX`, so the cast is exact.
        self.first.saturating_add(index as u32)
    }
}

impl SpriteAnimation {
    /// The frame of `sheet` this animation shows at `tick`; a tick before `started` shows the
    /// clip's first frame.
    pub fn frame(&self, sheet: &SpriteSheet, tick: u64) -> Result<u32, AnimationError> {
        let clip = sheet
            .clips
            .get(&self.clip)
            .ok_or_else(|| AnimationError::UnknownClip {
                clip: self.clip.clone(),
                known: sheet.clips.keys().cloned().collect(),
            })?;
        Ok(clip.frame_at(tick.saturating_sub(self.started), self.looping))
    }
}

/// `names` as "`a`, `b`", or "none".
pub(crate) fn quoted(names: &[String]) -> String {
    if names.is_empty() {
        return "none".to_string();
    }
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Serialization mirror of `SpriteSheet` so loading can validate.
#[derive(Serialize, Deserialize)]
struct SpriteSheetData {
    texture: AssetId,
    clips: BTreeMap<String, Clip>,
}

impl From<SpriteSheet> for SpriteSheetData {
    fn from(sheet: SpriteSheet) -> Self {
        SpriteSheetData {
            texture: sheet.texture,
            clips: sheet.clips,
        }
    }
}

impl TryFrom<SpriteSheetData> for SpriteSheet {
    type Error = SheetError;

    fn try_from(data: SpriteSheetData) -> Result<Self, SheetError> {
        let sheet = SpriteSheet {
            texture: data.texture,
            clips: data.clips,
        };
        sheet.validate()?;
        Ok(sheet)
    }
}
