//! Prepare: from two render slices, a view set and the caches, the sorted
//! list of what each view draws, and the content hash of a frame. Throwaway,
//! never product code.
//!
//! Every value that reaches a list or a byte of the per-draw and per-view
//! data is computed here, with the arithmetic the caller names: the math
//! layer or the platform's library (`crate::math`). Everything else is sums,
//! products, divisions and square roots, written once, in a fixed order.
//!
//! The rules that keep timing out of a frame's content are applied here: a
//! view's list is written in parts, one per chunk of the render slice, and
//! merged in chunk order; every sort is a total order that ends with the
//! entity's identifier and the index of the item inside it; no list is built
//! by iterating a hash table.

use std::cmp::Ordering;
use std::marker::PhantomData;

use reference_workload::{Fnv1a, TIME_STEP};
use render_device_baseline::frames::DrawConstants;
use render_device_baseline::scene::{SAMPLER_NEAREST, TEXTURE_WHITE};

use crate::math::Arithmetic;
use crate::slices::{EntityColumns, KIND_MESH, KIND_SPRITE, VISUAL_NONE, asset_of, kind_of};
use crate::visual::{BOUND_RADIUS, MESH_SAMPLER, MESH_SIZES, MESH_TEXTURE, Range, SPRITE_SIZES, SPRITES, SharedGeometry, TILE_SAMPLER, TILE_TEXTURE};

pub const VIEW_COUNT: usize = 4;
/// The side of a view's image, in pixels.
pub const VIEW_SIDE: u32 = 128;
/// A mesh is drawn at its first level from this many pixels across, and one
/// level lower for each halving.
const LEVEL_PIXELS: f32 = 16.0;
/// How far inside a level's range an interactive frame keeps the level the
/// frame before it drew: the declared history of the level of detail.
const LEVEL_HYSTERESIS: f32 = 0.25;
/// The level a slot has in the history when no frame has drawn it: the reset content.
pub const NO_LEVEL: u8 = 255;

/// A 4 by 4 matrix stored column by column: the element of column `c` and
/// row `r` at `c * 4 + r`.
pub type Matrix = [f32; 16];

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut product = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[column * 4 + k];
            }
            product[column * 4 + row] = sum;
        }
    }
    product
}

fn transform(matrix: &Matrix, point: [f32; 3]) -> [f32; 4] {
    let mut result = [0.0; 4];
    for (row, value) in result.iter_mut().enumerate() {
        *value = matrix[row] * point[0] + matrix[4 + row] * point[1] + matrix[8 + row] * point[2] + matrix[12 + row];
    }
    result
}

fn subtract(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let length = dot(a, a).sqrt();
    [a[0] / length, a[1] / length, a[2] / length]
}

/// One view of the view set, with what prepare derives from it once.
#[derive(Clone, Debug)]
pub struct View {
    pub index: u32,
    pub perspective: bool,
    pub view_projection: Matrix,
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    /// Pixels per world unit, at a clip `w` of one.
    pub pixel_scale: f32,
    /// Clip units per world unit across the view, at a clip `w` of one.
    pub clip_scale: f32,
    pub near: f32,
    /// A rule of content: the view keeps no mesh.
    pub keeps_meshes: bool,
    /// A rule of content: the view keeps what lies within this distance of its eye, on the ground.
    pub content_radius: Option<f32>,
}

fn orthographic(index: u32, center: [f32; 2], half_extent: f32, (sine, cosine): (f32, f32), keeps_meshes: bool) -> View {
    let mut matrix = [0.0; 16];
    (matrix[0], matrix[1]) = (cosine / half_extent, -sine / half_extent);
    (matrix[4], matrix[5]) = (sine / half_extent, cosine / half_extent);
    matrix[10] = -1.0 / 128.0;
    matrix[12] = -(cosine * center[0] + sine * center[1]) / half_extent;
    matrix[13] = -(cosine * center[1] - sine * center[0]) / half_extent;
    (matrix[14], matrix[15]) = (0.5, 1.0);
    View {
        index,
        perspective: false,
        view_projection: matrix,
        eye: [center[0], center[1], 64.0],
        right: [cosine, sine, 0.0],
        up: [-sine, cosine, 0.0],
        pixel_scale: VIEW_SIDE as f32 / 2.0 / half_extent,
        clip_scale: 1.0 / half_extent,
        near: 0.0,
        keeps_meshes,
        content_radius: None,
    }
}

fn perspective<A: Arithmetic>(index: u32, eye: [f32; 3], target: [f32; 3], field_of_view: f32, near: f32, far: f32, content_radius: Option<f32>) -> View {
    let forward = normalize(subtract(target, eye));
    let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
    let up = cross(right, forward);
    let focal = 1.0 / A::tan(field_of_view / 2.0);
    let mut view = [0.0; 16];
    for axis in 0..3 {
        (view[axis * 4], view[axis * 4 + 1], view[axis * 4 + 2]) = (right[axis], up[axis], -forward[axis]);
    }
    (view[12], view[13], view[14], view[15]) = (-dot(right, eye), -dot(up, eye), dot(forward, eye), 1.0);
    let mut projection = [0.0; 16];
    (projection[0], projection[5]) = (focal, focal);
    (projection[10], projection[11]) = (far / (near - far), -1.0);
    projection[14] = near * far / (near - far);
    View {
        index,
        perspective: true,
        view_projection: multiply(&projection, &view),
        eye,
        right,
        up,
        pixel_scale: focal * VIEW_SIDE as f32 / 2.0,
        clip_scale: if focal > 1.0 { focal } else { 1.0 },
        near,
        keeps_meshes: true,
        content_radius,
    }
}

/// The frame's time: simulated, computed from the tick, the step and the factor.
pub fn frame_time(tick: u32, factor: f32) -> f32 {
    ((tick - 1) as f32 + factor) * TIME_STEP
}

/// The four views of a world: the whole map from above, a turning close-up
/// that keeps no mesh, a camera in orbit, and a camera near the ground that
/// keeps what lies within a draw distance.
pub fn view_set<A: Arithmetic>(time: f32) -> [View; VIEW_COUNT] {
    let (sine, cosine) = A::sin_cos(time * 0.75);
    let follow = orthographic(1, [512.0 + 256.0 * cosine, 512.0 + 256.0 * sine], 160.0, A::sin_cos(time * 0.5), false);
    let (sine, cosine) = A::sin_cos(time * 0.25 + 0.5);
    let orbit = perspective::<A>(2, [512.0 + 900.0 * cosine, 512.0 + 900.0 * sine, 420.0], [512.0, 512.0, 0.0], 0.9, 4.0, 4096.0, None);
    let (sine, cosine) = A::sin_cos(time);
    let ground = perspective::<A>(3, [96.0, 80.0, 48.0], [512.0 + 128.0 * cosine, 512.0 + 128.0 * sine, 0.0], 1.2, 1.0, 2048.0, Some(640.0));
    [orthographic(0, [512.0, 512.0], 512.0, A::sin_cos(0.0), true), follow, orbit, ground]
}

/// Which vertex buffer an item draws from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    /// The instance's shared geometry.
    Shared,
    /// The cached geometry of a tile block of this world: layer, coordinate.
    Block(u32, u32),
}

/// One item of a view's list.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub blended: bool,
    /// The sort distance: the clip `w` under a perspective, the clip depth otherwise.
    pub depth: f32,
    pub entity: u32,
    pub generation: u32,
    /// The index of the item inside its entity (a block's coordinate).
    pub index: u32,
    pub geometry: Geometry,
    pub range: Range,
    /// The identifiers of the item's assets, less anything of the device.
    pub asset: u64,
    pub constants: DrawConstants,
    /// What culling found: not content.
    pub visible: bool,
    /// Whether the selection mask's pass draws the item too.
    pub selected: bool,
}

/// The total order of a view's list: opaque items front to back, then
/// blended items back to front, then the entity and the index inside it.
#[allow(dead_code, reason = "the order as a comparison, which the sort by integer keys is checked against")]
pub fn order(a: &Item, b: &Item) -> Ordering {
    a.blended
        .cmp(&b.blended)
        .then_with(|| if a.blended { b.depth.total_cmp(&a.depth) } else { a.depth.total_cmp(&b.depth) })
        .then_with(|| (a.entity, a.generation, a.index).cmp(&(b.entity, b.generation, b.index)))
}

/// The culling test, conservative: an item it rejects touches no pixel.
fn visible(view: &View, clip: [f32; 4], radius: f32) -> bool {
    if view.perspective {
        let reach = radius * (1.0 + view.clip_scale);
        clip[3] + radius >= view.near && clip[0].abs() <= clip[3] + reach && clip[1].abs() <= clip[3] + reach
    } else {
        let reach = 1.0 + radius * view.clip_scale;
        clip[0].abs() <= reach && clip[1].abs() <= reach
    }
}

fn level_of(measure: f32) -> u8 {
    if measure <= 0.0 {
        0
    } else if measure <= 1.0 {
        1
    } else {
        2
    }
}

/// What one chunk of the render slice gives one view.
#[derive(Default)]
pub struct ChunkItems {
    pub items: Vec<Item>,
    /// The level drawn for each slot of the chunk, or `NO_LEVEL`.
    pub levels: Vec<u8>,
}

/// The arithmetic prepare runs with, as a value a task can carry.
#[derive(Clone, Copy)]
pub struct With<A: Arithmetic>(PhantomData<A>);

impl<A: Arithmetic> Default for With<A> {
    fn default() -> With<A> {
        With(PhantomData)
    }
}

impl<A: Arithmetic> With<A> {
    /// The items a view keeps among the slots of one chunk, in slot order.
    /// `history` holds the level each slot was last drawn at in this view,
    /// for an interactive frame; a reproducible frame passes none.
    #[allow(clippy::too_many_arguments, reason = "the inputs of prepare, each named")]
    #[allow(dead_code, reason = "the form the tests call")]
    pub fn chunk(self, view: &View, slots: std::ops::Range<usize>, previous: &EntityColumns, current: &EntityColumns, factor: f32, geometry: &SharedGeometry, history: Option<&[u8]>) -> ChunkItems {
        let mut out = ChunkItems::default();
        self.chunk_into(view, slots, previous, current, factor, geometry, history, &mut out);
        out
    }

    /// As [`With::chunk`], into buffers the caller keeps from frame to frame.
    #[allow(clippy::too_many_arguments, reason = "the inputs of prepare, each named")]
    pub fn chunk_into(self, view: &View, slots: std::ops::Range<usize>, previous: &EntityColumns, current: &EntityColumns, factor: f32, geometry: &SharedGeometry, history: Option<&[u8]>, out: &mut ChunkItems) {
        let ChunkItems { items, levels } = out;
        items.clear();
        levels.clear();
        levels.resize(slots.len(), NO_LEVEL);
        let first_slot = slots.start;
        for slot in slots {
            let visual = current.visual[slot];
            if visual == VISUAL_NONE {
                continue;
            }
            let (kind, asset) = (kind_of(visual), asset_of(visual) as usize);
            if kind == KIND_MESH && !view.keeps_meshes {
                continue;
            }
            // An entity both slices hold is drawn between its two positions; the
            // frame of a named tick, at a factor of one, is drawn from its slice alone.
            let continued = factor < 1.0 && previous.visual.get(slot).is_some_and(|&visual| visual != VISUAL_NONE) && previous.generation[slot] == current.generation[slot];
            let (x, y) = if continued { (previous.x[slot] + (current.x[slot] - previous.x[slot]) * factor, previous.y[slot] + (current.y[slot] - previous.y[slot]) * factor) } else { (current.x[slot], current.y[slot]) };
            if let Some(radius) = view.content_radius {
                let (dx, dy) = (x - view.eye[0], y - view.eye[1]);
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
            }
            let (sine, cosine) = A::sin_cos(A::atan2(current.vy[slot], current.vx[slot]));
            let size = if kind == KIND_SPRITE { SPRITE_SIZES[asset] } else { MESH_SIZES[asset] };
            let mut model = [0.0; 16];
            let center = if kind == KIND_MESH {
                (model[0], model[1]) = (cosine * size, sine * size);
                (model[4], model[5]) = (-sine * size, cosine * size);
                model[10] = size;
                (model[12], model[13], model[14], model[15]) = (x, y, 0.0, 1.0);
                [x, y, size * 0.5]
            } else if view.perspective {
                // A sprite faces a camera in perspective and rolls with its heading.
                for axis in 0..3 {
                    model[axis] = (cosine * view.right[axis] + sine * view.up[axis]) * size;
                    model[4 + axis] = (cosine * view.up[axis] - sine * view.right[axis]) * size;
                }
                model[10] = 1.0;
                (model[12], model[13], model[14], model[15]) = (x, y, size * 0.5, 1.0);
                [x, y, size * 0.5]
            } else {
                (model[0], model[1]) = (cosine * size, sine * size);
                (model[4], model[5]) = (-sine * size, cosine * size);
                model[10] = 1.0;
                (model[12], model[13], model[14], model[15]) = (x, y, 1.0, 1.0);
                [x, y, 1.0]
            };
            let clip = transform(&view.view_projection, center);
            let depth = if view.perspective { clip[3] } else { clip[2] };
            let (range, texture, sampler, blended, level) = if kind == KIND_SPRITE {
                let sprite = SPRITES[asset];
                (geometry.sprite, sprite.texture, sprite.sampler, sprite.blended, 0)
            } else {
                let pixels = if view.perspective { if clip[3] > 0.0 { size * view.pixel_scale / clip[3] } else { 0.0 } } else { size * view.pixel_scale };
                let measure = if pixels > 0.0 { A::log2(LEVEL_PIXELS / pixels) } else { 8.0 };
                let mut level = level_of(measure);
                if let Some(&last) = history.and_then(|history| history.get(slot))
                    && last != NO_LEVEL
                    && last != level
                    && (level_of(measure + LEVEL_HYSTERESIS) == last || level_of(measure - LEVEL_HYSTERESIS) == last)
                {
                    level = last;
                }
                levels[slot - first_slot] = level;
                (geometry.meshes[asset][level as usize], MESH_TEXTURE, MESH_SAMPLER, false, level)
            };
            let generation = current.generation[slot];
            let tint = if kind == KIND_MESH { [1.0; 4] } else { [0.5 + 0.125 * (slot >> 3 & 3) as f32, 0.5 + 0.125 * (generation & 3) as f32, 1.0, if blended { 0.75 } else { 1.0 }] };
            items.push(Item {
                blended,
                depth,
                entity: slot as u32,
                generation,
                index: 0,
                geometry: Geometry::Shared,
                range,
                asset: u64::from(kind) << 16 | (asset as u64) << 8 | u64::from(level),
                constants: DrawConstants { transform: multiply(&view.view_projection, &model), tint, texture, sampler },
                visible: visible(view, clip, BOUND_RADIUS * size),
                selected: slot % 16 == 3,
            });
        }
    }
}

/// A tile block as the world's cache holds it.
#[derive(Clone, Copy, Debug)]
pub struct BlockInfo {
    pub layer: u32,
    pub coordinate: u32,
    pub vertex_count: u32,
    /// The hash of the block's cells: the identifier of what it draws.
    pub cells_hash: u64,
    /// The center of the block on the ground and the radius that bounds it.
    pub center: [f32; 2],
    pub radius: f32,
}

/// The height each tile layer is drawn at: the first under the entities, the others above.
pub fn layer_height(layer: u32) -> f32 {
    if layer == 0 { -1.0 } else { 20.0 }
}

/// The items a view keeps among a world's tile blocks, in the cache's order.
pub fn block_items(view: &View, blocks: &[BlockInfo]) -> Vec<Item> {
    let mut items = Vec::new();
    for block in blocks {
        if block.vertex_count == 0 {
            continue;
        }
        if let Some(radius) = view.content_radius {
            let (dx, dy) = (block.center[0] - view.eye[0], block.center[1] - view.eye[1]);
            let reach = radius + block.radius;
            if dx * dx + dy * dy > reach * reach {
                continue;
            }
        }
        let height = layer_height(block.layer);
        let mut model = [0.0; 16];
        (model[0], model[5], model[10], model[15]) = (1.0, 1.0, 1.0, 1.0);
        model[14] = height;
        let clip = transform(&view.view_projection, [block.center[0], block.center[1], height]);
        items.push(Item {
            blended: block.layer > 0,
            depth: if view.perspective { clip[3] } else { clip[2] },
            entity: 0x8000_0000 | block.layer,
            generation: 0,
            index: block.coordinate,
            geometry: Geometry::Block(block.layer, block.coordinate),
            range: Range { first_vertex: 0, vertex_count: block.vertex_count },
            asset: block.cells_hash,
            constants: DrawConstants { transform: multiply(&view.view_projection, &model), tint: [1.0; 4], texture: TILE_TEXTURE, sampler: TILE_SAMPLER },
            visible: visible(view, clip, block.radius),
            selected: false,
        });
    }
    items
}

/// The constants the selection mask's pass draws a selected item with.
pub fn mask_constants(item: &Item) -> DrawConstants {
    DrawConstants { transform: item.constants.transform, tint: [1.0; 4], texture: TEXTURE_WHITE, sampler: SAMPLER_NEAREST }
}

fn write_item(hasher: &mut Fnv1a, item: &Item, constants: &DrawConstants) {
    hasher.write(&item.asset.to_le_bytes());
    hasher.write(&[u8::from(item.blended)]);
    hasher.write(&item.entity.to_le_bytes());
    hasher.write(&item.generation.to_le_bytes());
    hasher.write(&item.index.to_le_bytes());
    hasher.write(&item.range.first_vertex.to_le_bytes());
    hasher.write(&item.range.vertex_count.to_le_bytes());
    hasher.write(&constants.bytes());
}

/// The content of one view: its data, then the passes of its pipeline as
/// declared, each with its sorted list before any culling, each item by its
/// assets, its pipeline's description less the device's shading form, and
/// the bytes of its per-draw data.
pub fn view_content_hash(view: &View, sorted: &[Item]) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.write(&view.index.to_le_bytes());
    hasher.write(b"rgba8 srgb, 4 samples, depth 32 float");
    hasher.write(&VIEW_SIDE.to_le_bytes());
    for value in view.view_projection {
        hasher.write(&value.to_le_bytes());
    }
    hasher.write(b"scene");
    for item in sorted {
        write_item(&mut hasher, item, &item.constants);
    }
    hasher.write(b"selection mask");
    for item in sorted.iter().filter(|item| item.selected) {
        write_item(&mut hasher, item, &mask_constants(item));
    }
    hasher.finish()
}

/// The content hash of a frame: its views in the order of the view set, then
/// the composition of their images onto the target.
pub fn frame_content_hash(time: f32, views: &[u64], composition: &[DrawConstants]) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.write(&time.to_le_bytes());
    for view in views {
        hasher.write(&view.to_le_bytes());
    }
    hasher.write(b"composition");
    for constants in composition {
        hasher.write(&constants.bytes());
    }
    hasher.finish()
}

/// The key of one item in a view's sort: the fields of [`order`], in its
/// order, as integers, then the item's position before the sort.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SortKey {
    blended: bool,
    depth: u32,
    entity: u32,
    generation: u32,
    index: u32,
    position: u32,
}

/// The buffers of one view's list: a frame's own memory, which a world keeps
/// and gives its next frame once the frame that used them is finished.
#[derive(Default)]
pub struct ViewBuffers {
    /// The parts of the list, merged in chunk order.
    pub merged: Vec<Item>,
    pub keys: Vec<SortKey>,
    pub sorted: Vec<Item>,
    /// The positions in `sorted` of the items the view's pass draws.
    pub drawn: Vec<u32>,
    /// The level drawn for each slot, or `NO_LEVEL`.
    pub levels: Vec<u8>,
}

/// Sorts `merged` into `sorted` by [`order`], through integer keys: a
/// distance's bits are mapped so that they compare as the distances do.
pub fn sort(buffers: &mut ViewBuffers) {
    buffers.keys.clear();
    buffers.keys.extend(buffers.merged.iter().enumerate().map(|(position, item)| {
        let bits = item.depth.to_bits();
        let ascending = if bits & 0x8000_0000 != 0 { !bits } else { bits | 0x8000_0000 };
        SortKey { blended: item.blended, depth: if item.blended { !ascending } else { ascending }, entity: item.entity, generation: item.generation, index: item.index, position: position as u32 }
    }));
    buffers.keys.sort_unstable();
    buffers.sorted.clear();
    buffers.sorted.extend(buffers.keys.iter().map(|key| buffers.merged[key.position as usize]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{MathLayer, Platform};

    fn columns() -> EntityColumns {
        let count = 64;
        EntityColumns {
            x: (0..count).map(|slot| 100.0 + slot as f32 * 13.0).collect(),
            y: (0..count).map(|slot| 900.0 - slot as f32 * 11.0).collect(),
            vx: (0..count).map(|slot| slot as f32 - 30.0).collect(),
            vy: (0..count).map(|slot| 7.0 - slot as f32 * 0.5).collect(),
            visual: (0..count).map(|slot| if slot == 5 { VISUAL_NONE } else if slot % 8 < 6 { crate::slices::visual_of(KIND_SPRITE, slot as u32 % 3) } else { crate::slices::visual_of(KIND_MESH, slot as u32 / 8 % 2) }).collect(),
            generation: vec![1; count],
        }
    }

    #[test]
    fn the_sort_by_integer_keys_is_the_total_order() {
        let columns = columns();
        let geometry = SharedGeometry::build();
        for view in &view_set::<MathLayer>(frame_time(7, 0.75)) {
            let mut buffers = ViewBuffers { merged: With::<MathLayer>::default().chunk(view, 0..64, &columns, &columns, 0.75, &geometry, None).items, ..ViewBuffers::default() };
            buffers.merged.reverse();
            sort(&mut buffers);
            let mut expected = buffers.merged.clone();
            expected.sort_by(order);
            assert_eq!(view_content_hash(view, &buffers.sorted), view_content_hash(view, &expected));
        }
    }

    #[test]
    fn the_views_keep_what_their_rules_of_content_say() {
        let columns = columns();
        let geometry = SharedGeometry::build();
        let views = view_set::<MathLayer>(frame_time(10, 0.5));
        let kept: Vec<usize> = views.iter().map(|view| With::<MathLayer>::default().chunk(view, 0..64, &columns, &columns, 0.5, &geometry, None).items.len()).collect();
        assert_eq!(kept[0], 63, "the map keeps every living entity");
        assert_eq!(kept[1], 63 - 16, "the close-up keeps no mesh");
        assert_eq!(kept[2], 63);
        assert!(kept[3] < 63, "the camera near the ground keeps what lies within its draw distance");
    }

    #[test]
    fn the_order_is_total_and_independent_of_the_order_of_arrival() {
        let columns = columns();
        let geometry = SharedGeometry::build();
        let view = &view_set::<MathLayer>(frame_time(3, 0.25))[2];
        let mut forward = With::<MathLayer>::default().chunk(view, 0..64, &columns, &columns, 0.25, &geometry, None).items;
        let mut backward: Vec<Item> = forward.iter().rev().copied().collect();
        forward.sort_by(order);
        backward.sort_by(order);
        assert_eq!(view_content_hash(view, &forward), view_content_hash(view, &backward));
        assert!(forward.windows(2).all(|pair| order(&pair[0], &pair[1]) == Ordering::Less));
    }

    #[test]
    fn the_two_arithmetics_agree_on_what_is_kept_and_nearly_on_where() {
        let columns = columns();
        let geometry = SharedGeometry::build();
        let time = frame_time(40, 0.5);
        for (layer, platform) in view_set::<MathLayer>(time).iter().zip(&view_set::<Platform>(time)) {
            let a = With::<MathLayer>::default().chunk(layer, 0..64, &columns, &columns, 0.5, &geometry, None).items;
            let b = With::<Platform>::default().chunk(platform, 0..64, &columns, &columns, 0.5, &geometry, None).items;
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(&b) {
                for (x, y) in a.constants.transform.iter().zip(b.constants.transform) {
                    assert!((x - y).abs() <= 1e-3 * (1.0 + x.abs()), "{x} against {y}");
                }
            }
        }
    }

    #[test]
    fn an_interactive_frame_keeps_a_level_near_its_boundary_and_a_reproducible_one_does_not() {
        let mut columns = columns();
        let geometry = SharedGeometry::build();
        let view = orthographic(0, [512.0, 512.0], 86.0, (0.0, 1.0), true);
        // A mesh of size 10 is 7.44 pixels across in this view: just inside
        // the third level's range, a tenth of a level from the second's.
        columns.visual[6] = crate::slices::visual_of(KIND_MESH, 0);
        let fresh = With::<MathLayer>::default().chunk(&view, 0..64, &columns, &columns, 0.5, &geometry, None);
        assert_eq!(fresh.levels[6], 2);
        let history = vec![1_u8; 64];
        let kept = With::<MathLayer>::default().chunk(&view, 0..64, &columns, &columns, 0.5, &geometry, Some(&history));
        assert_eq!(kept.levels[6], 1);
        let far = vec![0_u8; 64];
        let moved = With::<MathLayer>::default().chunk(&view, 0..64, &columns, &columns, 0.5, &geometry, Some(&far));
        assert_eq!(moved.levels[6], 2);
    }
}
