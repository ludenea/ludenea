//! Tilemaps, cut into chunks of 32 by 32 cells: each chunk is one instance buffer of the quad
//! pipeline, kept until its tilemap item changes.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use ludenea_draw::frame::TilemapItem;
use ludenea_ecs::{AssetId, Entity};
use wgpu::util::DeviceExt;

use super::sprites::{self, BLANK, Instance, Sample};
use crate::atlas::Region;

/// The side of a chunk, in cells.
pub(super) const CHUNK: u32 = 32;

/// One chunk's instances and the runs of them that sample the same page.
pub(super) struct Chunk {
    pub buffer: wgpu::Buffer,
    pub batches: Vec<(Sample, Range<u32>)>,
}

/// A tilemap's chunks, built from `item`, and the tiles drawn as placeholders.
pub(super) struct Built {
    item: TilemapItem,
    pub chunks: Vec<Chunk>,
    pub missing: Vec<(Entity, AssetId, u32)>,
}

/// The chunks of each tilemap entity, rebuilt only when its item or the atlas changes.
#[derive(Default)]
pub(super) struct Cache(BTreeMap<Entity, Built>);

impl Cache {
    /// Forgets every tilemap: the atlas changed, so their regions did.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Builds the chunks of each item that changed, and forgets the tilemaps not in `items`.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        items: &[TilemapItem],
        regions: &BTreeMap<(AssetId, u32), Region>,
        pages: &[(u32, u32)],
    ) {
        let items: Vec<&TilemapItem> = items.iter().filter(|item| well_formed(item)).collect();
        let present: BTreeSet<Entity> = items.iter().map(|item| item.entity).collect();
        self.0.retain(|entity, _| present.contains(entity));
        for item in items {
            if self
                .0
                .get(&item.entity)
                .is_none_or(|built| built.item != *item)
            {
                self.0
                    .insert(item.entity, build(device, item, regions, pages));
            }
        }
    }

    pub fn get(&self, entity: Entity) -> Option<&Built> {
        self.0.get(&entity)
    }
}

/// Whether `item` has one tile per cell. Extraction refuses other tilemaps, but a frame from
/// the control protocol may hold one: it is not drawn, and its cells are never walked.
fn well_formed(item: &TilemapItem) -> bool {
    item.tiles.len() as u64 == u64::from(item.width) * u64::from(item.height)
}

/// The chunks of `item`, row by row of chunks from the bottom-left. A tile fills its cell; a
/// tile whose region is missing is a magenta cell and is reported. Only the rows `tiles`
/// reaches are walked, and a cell past its end is empty.
fn build(
    device: &wgpu::Device,
    item: &TilemapItem,
    regions: &BTreeMap<(AssetId, u32), Region>,
    pages: &[(u32, u32)],
) -> Built {
    let mut built = Built {
        item: item.clone(),
        chunks: Vec::new(),
        missing: Vec::new(),
    };
    let (cell_width, cell_height) = (item.cell_size.x, item.cell_size.y);
    let size = [cell_width, cell_height];
    // `well_formed` already holds; this keeps the walk bounded by the tiles regardless.
    let rows = match u64::from(item.width) {
        0 => 0,
        width => (item.tiles.len() as u64)
            .div_ceil(width)
            .min(u64::from(item.height)) as u32,
    };
    for first_row in (0..rows).step_by(CHUNK as usize) {
        for first_column in (0..item.width).step_by(CHUNK as usize) {
            // The chunk's corner is the instances' centre, so their offsets stay small.
            let corner = [
                item.position.x + first_column as f32 * cell_width,
                item.position.y + first_row as f32 * cell_height,
            ];
            let mut instances: Vec<Instance> = Vec::new();
            let mut batches: Vec<(Sample, Range<u32>)> = Vec::new();
            for row in first_row..(first_row + CHUNK).min(rows) {
                for column in first_column..(first_column + CHUNK).min(item.width) {
                    let cell = u64::from(row) * u64::from(item.width) + u64::from(column);
                    let Some(&Some(tile)) = usize::try_from(cell)
                        .ok()
                        .and_then(|cell| item.tiles.get(cell))
                    else {
                        continue;
                    };
                    let offset = [
                        (column - first_column) as f32 * cell_width,
                        (row - first_row) as f32 * cell_height,
                    ];
                    let (sample, instance) = match sprites::find(regions, pages, item.tileset, tile)
                    {
                        Some((region, page)) => {
                            let (sample, uv) = sprites::uv(region, page);
                            (sample, sprites::tile(corner, offset, size, uv, false))
                        }
                        None => {
                            built.missing.push((item.entity, item.tileset, tile));
                            let uv = [0.0, 0.0, 1.0, 1.0];
                            (BLANK, sprites::tile(corner, offset, size, uv, true))
                        }
                    };
                    let index = instances.len() as u32;
                    instances.push(instance);
                    match batches.last_mut() {
                        Some((last, run)) if *last == sample => run.end = index + 1,
                        _ => batches.push((sample, index..index + 1)),
                    }
                }
            }
            if instances.is_empty() {
                continue;
            }
            let bytes: Vec<u8> = instances
                .iter()
                .flatten()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            // At most 32 * 32 instances of 72 bytes: far below any buffer limit.
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tilemap chunk"),
                contents: &bytes,
                usage: wgpu::BufferUsages::VERTEX,
            });
            built.chunks.push(Chunk { buffer, batches });
        }
    }
    built
}
