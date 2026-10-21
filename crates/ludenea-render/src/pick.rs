//! What a point on screen hits and what a selection spans, GPU-free, from the same geometry
//! the renderer draws: a sprite's quad (its region's size and pivot, or the 1 by 1
//! placeholder, flipped and turned) and a tilemap's painted cells, each seen through its
//! layer's view. Display only: nothing here feeds back into the simulation.

use std::collections::{BTreeMap, BTreeSet};

use ludenea_draw::frame::{CameraView, RenderFrame, SpriteItem, TilemapItem};
use ludenea_ecs::{AssetId, Entity};
use ludenea_math::Vec2;

use crate::atlas::Region;
use crate::camera::{Size, screen_to_world};

/// A quad's bottom-left corner and size in world units from its pivot, +y up; a flipped
/// axis has a negative size, mirroring the quad around the pivot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Extent {
    pub offset: Vec2,
    pub size: Vec2,
}

/// The quad `item` is drawn as before it is turned: `region size / pixels_per_unit` world
/// units with its pivot on the position, or one world unit centred on it (the placeholder)
/// when `region` is missing; then flipped.
pub(crate) fn sprite_extent(item: &SpriteItem, region: Option<&Region>) -> Extent {
    let mut extent = match region {
        Some(region) => {
            let unit = region.pixels_per_unit as f32;
            let (width, height) = (region.width as f32, region.height as f32);
            let (pivot_x, pivot_y) = (region.pivot.0 as f32, region.pivot.1 as f32);
            Extent {
                // The pivot counts from the top-left, the quad's local +y is up.
                offset: Vec2::new(-pivot_x / unit, -(height - pivot_y) / unit),
                size: Vec2::new(width / unit, height / unit),
            }
        }
        None => Extent {
            offset: Vec2::new(-0.5, -0.5),
            size: Vec2::new(1.0, 1.0),
        },
    };
    if item.flip_x {
        extent.offset.x = -extent.offset.x;
        extent.size.x = -extent.size.x;
    }
    if item.flip_y {
        extent.offset.y = -extent.offset.y;
        extent.size.y = -extent.size.y;
    }
    extent
}

/// The parallax of `layer`: 1 for a layer the frame does not declare, as it is drawn.
pub(crate) fn layer_parallax(frame: &RenderFrame, layer: u16) -> f32 {
    frame
        .layers
        .get(usize::from(layer))
        .map_or(1.0, |info| info.parallax)
}

/// Whether `item` has one tile per cell. Extraction refuses other tilemaps, but a frame from
/// the control protocol may hold one: it is not drawn, and its cells are never walked.
pub(crate) fn well_formed(item: &TilemapItem) -> bool {
    item.tiles.len() as u64 == u64::from(item.width) * u64::from(item.height)
}

/// The tilemaps of `frame` that are drawn: the well-formed ones, and of tilemaps sharing an
/// entity (which only a malformed frame holds), the first well-formed one listed.
pub(crate) fn drawn_tilemaps(frame: &RenderFrame) -> Vec<&TilemapItem> {
    let mut seen = BTreeSet::new();
    frame
        .tilemaps
        .iter()
        .filter(|item| well_formed(item) && seen.insert(item.entity))
        .collect()
}

/// The entities whose drawn sprite or tilemap is under `screen` (pixels from the top-left of
/// a target of `size`, +y down) through `camera`, topmost first: in the reverse of the draw
/// order, layer, then order, then entity, a tilemap drawn before a sprite of the same key.
/// A sprite is hit inside its drawn quad (a sprite whose `(texture, frame)` has no region in
/// `regions` is the 1 by 1 placeholder), a tilemap inside a painted cell. Each entity is
/// listed once, at its topmost hit.
pub fn pick(
    frame: &RenderFrame,
    regions: &BTreeMap<(AssetId, u32), Region>,
    screen: Vec2,
    size: Size,
    camera: CameraView,
) -> Vec<Entity> {
    let seen_at =
        |layer: u16| screen_to_world(screen, size, camera.clone(), layer_parallax(frame, layer));
    // Draw order: tilemaps before sprites on the same key, then as listed.
    let mut hits: Vec<(u16, i32, Entity, usize)> = Vec::new();
    for item in drawn_tilemaps(frame) {
        if painted_cell(item, seen_at(item.layer)) {
            hits.push((item.layer, item.order, item.entity, 0));
        }
    }
    for (index, item) in frame.sprites.iter().enumerate() {
        let region = regions.get(&(item.texture, item.frame));
        if in_sprite(item, sprite_extent(item, region), seen_at(item.layer)) {
            hits.push((item.layer, item.order, item.entity, index + 1));
        }
    }
    hits.sort_unstable();
    let mut listed = BTreeSet::new();
    hits.iter()
        .rev()
        .map(|&(_, _, entity, _)| entity)
        .filter(|entity| listed.insert(*entity))
        .collect()
}

/// The world box (min, max) of the sprites and tilemaps of `entities` drawn on layers of
/// parallax 1: each sprite's turned quad and each tilemap's whole grid. A layer of another
/// parallax moves with the camera, so it has no single world box and is left out. `None`
/// when none of them is drawn on such a layer.
pub fn bounds(
    frame: &RenderFrame,
    regions: &BTreeMap<(AssetId, u32), Region>,
    entities: &BTreeSet<Entity>,
) -> Option<(Vec2, Vec2)> {
    let in_world = |entity: Entity, layer: u16| {
        entities.contains(&entity) && layer_parallax(frame, layer) == 1.0
    };
    let sprite_corners = frame
        .sprites
        .iter()
        .filter(|item| in_world(item.entity, item.layer))
        .flat_map(|item| {
            let extent = sprite_extent(item, regions.get(&(item.texture, item.frame)));
            let (low, high) = (extent.offset, extent.offset + extent.size);
            [
                Vec2::new(low.x, low.y),
                Vec2::new(high.x, low.y),
                Vec2::new(low.x, high.y),
                Vec2::new(high.x, high.y),
            ]
            // Turned as the shader turns it: along `(cos, sin)` and `(-sin, cos)`.
            .map(|local| item.position + item.rotation.rotate(local))
        });
    let tilemap_corners = drawn_tilemaps(frame)
        .into_iter()
        .filter(|item| in_world(item.entity, item.layer))
        .flat_map(|item| {
            let span = Vec2::new(
                item.width as f32 * item.cell_size.x,
                item.height as f32 * item.cell_size.y,
            );
            [item.position, item.position + span]
        });
    sprite_corners
        .chain(tilemap_corners)
        .fold(None, |bounds, corner| match bounds {
            None => Some((corner, corner)),
            Some((min, max)) => Some((min.min(corner), max.max(corner))),
        })
}

/// Whether `point` lies in the sprite's quad of `extent`, turned and placed as it is drawn.
fn in_sprite(item: &SpriteItem, extent: Extent, point: Vec2) -> bool {
    let rotation = item.rotation;
    let length_squared = rotation.cos * rotation.cos + rotation.sin * rotation.sin;
    // A rotation of length 0 collapses the quad onto its position: nothing to hit.
    if length_squared == 0.0 {
        return false;
    }
    // The inverse of the shader's turn, which also scales by the rotation's length.
    let local = rotation.inv_rotate(point - item.position) * (1.0 / length_squared);
    let within = |value: f32, offset: f32, size: f32| {
        let (low, high) = if size < 0.0 {
            (offset + size, offset)
        } else {
            (offset, offset + size)
        };
        low <= value && value <= high
    };
    within(local.x, extent.offset.x, extent.size.x)
        && within(local.y, extent.offset.y, extent.size.y)
}

/// Whether `point` lies in a painted cell of `item`, a drawn tilemap: cell `(column, row)`
/// spans `column` to `column + 1` cells right of its position and `row` to `row + 1` up.
fn painted_cell(item: &TilemapItem, point: Vec2) -> bool {
    let local = point - item.position;
    let column = (local.x / item.cell_size.x).floor();
    let row = (local.y / item.cell_size.y).floor();
    // Out of the grid, or a cell size of 0 (an infinite or undefined quotient).
    let inside = |cell: f32, count: u32| cell >= 0.0 && cell < count as f32;
    if !(inside(column, item.width) && inside(row, item.height)) {
        return false;
    }
    // Both are whole numbers within the grid, which `well_formed` sizes by `tiles`.
    let cell = row as u64 * u64::from(item.width) + column as u64;
    usize::try_from(cell)
        .ok()
        .and_then(|cell| item.tiles.get(cell))
        .is_some_and(Option::is_some)
}
