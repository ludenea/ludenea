//! The render slices of a world and the extraction that fills them: a copy
//! of marked columns at the fixed point, into one of two buffers, and nothing
//! else. Throwaway, never product code.
//!
//! The reference workload's entities carry no visual component, so the
//! visual model's column is derived here from the slot and the mass, as the
//! extraction measurement derived its sprite handle. The tile layers are not
//! in the workload's state either: their written blocks live in a stand-in
//! for domain state that this capability advances itself, by one tile write
//! per tick, a function of the tick alone.

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use reference_workload::{SplitMix64, State, chunk_range};
use skeleton_core::Extraction;

/// The visual column: 0 for a dead slot, otherwise a kind and an asset.
pub const VISUAL_NONE: u32 = 0;
pub const KIND_SPRITE: u32 = 1;
pub const KIND_MESH: u32 = 2;

pub fn visual_of(kind: u32, asset: u32) -> u32 {
    kind << 8 | asset
}

pub fn kind_of(visual: u32) -> u32 {
    visual >> 8
}

pub fn asset_of(visual: u32) -> u32 {
    visual & 0xff
}

/// The visual component the model gives a slot: six slots of eight carry a
/// sprite chosen by the mass, two a mesh chosen by the slot.
fn visual_component(slot: usize, mass: f32) -> u32 {
    let by_mass = if mass == 1.0 {
        0
    } else if mass == 2.0 {
        1
    } else {
        2
    };
    if slot % 8 < 6 { visual_of(KIND_SPRITE, by_mass) } else { visual_of(KIND_MESH, (slot / 8 % 2) as u32) }
}

#[derive(Clone, Default)]
pub struct EntityColumns {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub visual: Vec<u32>,
    pub generation: Vec<u32>,
}

/// A block of a tile layer that gameplay has written: its coordinate in the
/// layer's grid of blocks, its version, and its cells (a tile index in the
/// low 14 bits, two flip bits above), row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenBlock {
    pub coordinate: u32,
    pub version: u32,
    pub cells: Vec<u16>,
}

#[derive(Clone, Default)]
pub struct RenderSlice {
    /// The tick the slice was extracted after; 0 before any extraction.
    pub tick: u32,
    pub entities: EntityColumns,
    /// Per tile layer, its written blocks in the order they were first written.
    pub layers: Vec<Vec<WrittenBlock>>,
}

/// The slots a storage chunk holds, as the reference workload has it. The
/// written blocks of a layer are child entities stored in chunks of this
/// many, and change detection is a flag per chunk.
pub const CHUNK_SLOTS: usize = 256;

/// A tile layer's grid asset: a square of cells, a pure function of the
/// layer's index.
pub struct TileGrid {
    pub side_cells: u32,
    pub cells: Vec<u16>,
}

/// The finalizer of SplitMix64: a fixed integer hash.
pub fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The empty cell: nothing is drawn for it.
pub const TILE_EMPTY: u16 = 0x3fff;
pub const TILE_COUNT: u16 = 16;

impl TileGrid {
    /// Layer 0 is full; the layers above it hold a tile in one cell of four.
    pub fn new(layer: u32, side_cells: u32) -> TileGrid {
        let cells = (0..u64::from(side_cells) * u64::from(side_cells))
            .map(|cell| {
                let hash = mix(cell ^ u64::from(layer) << 40);
                if layer > 0 && hash >> 20 & 3 != 0 { TILE_EMPTY } else { (hash % u64::from(TILE_COUNT)) as u16 | ((hash >> 8 & 3) as u16) << 14 }
            })
            .collect();
        TileGrid { side_cells, cells }
    }

    /// The cells of one block, row by row, as the canonical part holds them.
    pub fn block(&self, block_side: u32, coordinate: u32) -> Vec<u16> {
        let blocks_per_side = self.side_cells / block_side;
        let (block_x, block_y) = (coordinate % blocks_per_side, coordinate / blocks_per_side);
        let mut cells = Vec::with_capacity((block_side * block_side) as usize);
        for row in 0..block_side {
            let start = ((block_y * block_side + row) * self.side_cells + block_x * block_side) as usize;
            cells.extend_from_slice(&self.cells[start..start + block_side as usize]);
        }
        cells
    }
}

/// One tile layer as the domain holds it: its grid asset, and the blocks
/// gameplay has written, each a child entity of the layer.
pub struct TileLayer {
    pub grid: TileGrid,
    pub block_side: u32,
    written: Vec<WrittenBlock>,
    by_coordinate: BTreeMap<u32, usize>,
    /// The chunks of written blocks a tile write touched since the last extraction.
    changed_chunks: Vec<bool>,
}

impl TileLayer {
    pub fn new(layer: u32, side_cells: u32, block_side: u32) -> TileLayer {
        assert!(side_cells % block_side == 0, "a layer is a whole number of blocks");
        TileLayer { grid: TileGrid::new(layer, side_cells), block_side, written: Vec::new(), by_coordinate: BTreeMap::new(), changed_chunks: Vec::new() }
    }

    /// Writes one tile, as the visual model's command does: the block is
    /// copied from the grid's canonical part when it does not exist yet.
    pub fn write_tile(&mut self, cell_x: u32, cell_y: u32, value: u16) {
        let blocks_per_side = self.grid.side_cells / self.block_side;
        let coordinate = cell_y / self.block_side * blocks_per_side + cell_x / self.block_side;
        let position = match self.by_coordinate.get(&coordinate) {
            Some(&position) => position,
            None => {
                self.written.push(WrittenBlock { coordinate, version: 0, cells: self.grid.block(self.block_side, coordinate) });
                self.by_coordinate.insert(coordinate, self.written.len() - 1);
                self.written.len() - 1
            }
        };
        let block = &mut self.written[position];
        block.cells[(cell_y % self.block_side * self.block_side + cell_x % self.block_side) as usize] = value;
        block.version += 1;
        let chunk = position / CHUNK_SLOTS;
        if self.changed_chunks.len() <= chunk {
            self.changed_chunks.resize(chunk + 1, false);
        }
        self.changed_chunks[chunk] = true;
    }

    /// Copies the written blocks into `target`: a chunk a write touched is
    /// gathered from the domain, another is copied from the previous tick's
    /// slice. Returns the bytes of cells gathered and copied, and clears the flags.
    pub fn extract(&mut self, previous: &[WrittenBlock], target: &mut Vec<WrittenBlock>) -> (u64, u64) {
        let (mut gathered, mut copied) = (0, 0);
        target.truncate(self.written.len());
        for (chunk, positions) in (0..self.written.len()).collect::<Vec<_>>().chunks(CHUNK_SLOTS).enumerate() {
            let changed = self.changed_chunks.get(chunk).copied().unwrap_or(false);
            for &position in positions {
                let from_previous = !changed && position < previous.len();
                let source = if from_previous { &previous[position] } else { &self.written[position] };
                let bytes = source.cells.len() as u64 * 2;
                if from_previous {
                    copied += bytes;
                } else {
                    gathered += bytes;
                }
                if position < target.len() {
                    target[position].clone_from(source);
                } else {
                    target.push(source.clone());
                }
            }
        }
        self.changed_chunks.iter_mut().for_each(|flag| *flag = false);
        (gathered, copied)
    }
}

/// The tile layers of a rendered world: two layers of 128 by 128 cells in
/// blocks of 32 by 32.
pub const LAYER_COUNT: usize = 2;
pub const LAYER_SIDE_CELLS: u32 = 128;
pub const BLOCK_SIDE: u32 = 32;

/// The stand-in for gameplay: at each tick one tile of one layer is written,
/// inside the quarter of the map nearest the origin.
fn write_of_tick(layers: &mut [TileLayer], tick: u32) {
    let mut stream = SplitMix64::new(u64::from(tick) ^ 0x7469_6c65_7772_6974);
    let draw = stream.next();
    let layer = &mut layers[(tick % LAYER_COUNT as u32) as usize];
    let half = u64::from(layer.grid.side_cells / 2);
    layer.write_tile((draw % half) as u32, ((draw >> 16) % half) as u32, (draw >> 32) as u16 % TILE_COUNT);
}

/// The extraction of one world: implements the skeleton's extension point.
pub struct RenderExtraction {
    slices: [RwLock<RenderSlice>; 2],
    layers: RwLock<Vec<TileLayer>>,
    /// What the extraction copied, summed over the ticks: bytes of entity
    /// columns, bytes of tile cells gathered, bytes of tile cells copied.
    pub entity_bytes: AtomicU64,
    pub tile_bytes_gathered: AtomicU64,
    pub tile_bytes_copied: AtomicU64,
}

impl Default for RenderExtraction {
    fn default() -> RenderExtraction {
        RenderExtraction::new()
    }
}

impl RenderExtraction {
    pub fn new() -> RenderExtraction {
        RenderExtraction {
            slices: [RwLock::new(RenderSlice::default()), RwLock::new(RenderSlice::default())],
            layers: RwLock::new((0..LAYER_COUNT as u32).map(|layer| TileLayer::new(layer, LAYER_SIDE_CELLS, BLOCK_SIDE)).collect()),
            entity_bytes: AtomicU64::new(0),
            tile_bytes_gathered: AtomicU64::new(0),
            tile_bytes_copied: AtomicU64::new(0),
        }
    }

    /// Reads the slices of `tick` and of the tick before it. Called between
    /// ticks, when no extraction runs.
    pub fn read<T>(&self, tick: u32, reader: impl FnOnce(&RenderSlice, &RenderSlice) -> T) -> T {
        let current = self.slices[(tick % 2) as usize].read().unwrap();
        let previous = self.slices[((tick + 1) % 2) as usize].read().unwrap();
        assert_eq!(current.tick, tick, "the slice of tick {tick} was not extracted");
        reader(&previous, &current)
    }

    /// The canonical cells of a block no gameplay wrote, from the grid asset.
    pub fn canonical_block(&self, layer: usize, coordinate: u32) -> Vec<u16> {
        self.layers.read().unwrap()[layer].grid.block(BLOCK_SIDE, coordinate)
    }
}

impl Extraction for RenderExtraction {
    fn extract(&self, tick: u32, state: &State) {
        let mut target = self.slices[(tick % 2) as usize].write().unwrap();
        let previous = self.slices[((tick + 1) % 2) as usize].read().unwrap();
        let count = state.len();
        let columns = &mut target.entities;
        if columns.x.len() != count {
            for column in [&mut columns.x, &mut columns.y, &mut columns.vx, &mut columns.vy] {
                column.resize(count, 0.0);
            }
            columns.visual.resize(count, VISUAL_NONE);
            columns.generation.resize(count, 0);
        }
        // Chunk by chunk, each chunk a disjoint range of every column.
        for chunk in 0..state.chunk_count() {
            let range = chunk_range(chunk, count);
            columns.x[range.clone()].copy_from_slice(&state.x[range.clone()]);
            columns.y[range.clone()].copy_from_slice(&state.y[range.clone()]);
            columns.vx[range.clone()].copy_from_slice(&state.vx[range.clone()]);
            columns.vy[range.clone()].copy_from_slice(&state.vy[range.clone()]);
            columns.generation[range.clone()].copy_from_slice(&state.generation[range.clone()]);
            for slot in range {
                columns.visual[slot] = if state.alive[slot] { visual_component(slot, state.mass[slot]) } else { VISUAL_NONE };
            }
        }
        self.entity_bytes.fetch_add(count as u64 * 24, Ordering::Relaxed);

        let mut layers = self.layers.write().unwrap();
        write_of_tick(&mut layers, tick);
        target.layers.resize(layers.len(), Vec::new());
        for (index, layer) in layers.iter_mut().enumerate() {
            let previous_blocks: &[WrittenBlock] = previous.layers.get(index).map_or(&[], Vec::as_slice);
            let (gathered, copied) = layer.extract(previous_blocks, &mut target.layers[index]);
            self.tile_bytes_gathered.fetch_add(gathered, Ordering::Relaxed);
            self.tile_bytes_copied.fetch_add(copied, Ordering::Relaxed);
        }
        target.tick = tick;
    }
}

/// What one tile write costs the extraction of a layer's written blocks, at
/// one block size and one population of written blocks.
pub struct TileExtractionCost {
    pub block_side: u32,
    pub prior_writes: u32,
    pub written_blocks: u64,
    pub chunks: u64,
    pub block_bytes: u64,
    pub gathered_bytes_per_tick: f64,
    pub copied_bytes_per_tick: f64,
}

/// A layer of 512 by 512 cells in which `prior_writes` tiles were written,
/// spread over a square of `spread_cells` cells a side, then one tile a tick
/// for `ticks` ticks in the same square, each tick followed by the extraction.
pub fn tile_extraction_cost(block_side: u32, prior_writes: u32, spread_cells: u32, ticks: u32) -> TileExtractionCost {
    let mut layer = TileLayer::new(0, 512, block_side);
    let mut stream = SplitMix64::new(0x626c_6f63_6b73);
    let mut write = |layer: &mut TileLayer| {
        let draw = stream.next();
        layer.write_tile((draw % u64::from(spread_cells)) as u32, (draw >> 20) as u32 % spread_cells, (draw >> 40) as u16 % TILE_COUNT);
    };
    for _ in 0..prior_writes {
        write(&mut layer);
    }
    let mut slices = [Vec::new(), Vec::new()];
    // Two extractions bring both slices up to date before the measured ticks.
    for tick in 0..2_usize {
        let (previous, target) = if tick % 2 == 0 { (slices[1].clone(), &mut slices[0]) } else { (slices[0].clone(), &mut slices[1]) };
        layer.extract(&previous, target);
    }
    let (mut gathered, mut copied) = (0, 0);
    for tick in 0..ticks as usize {
        write(&mut layer);
        let (first, second) = slices.split_at_mut(1);
        let (previous, target) = if tick % 2 == 0 { (&second[0], &mut first[0]) } else { (&first[0], &mut second[0]) };
        let (tick_gathered, tick_copied) = layer.extract(previous, target);
        gathered += tick_gathered;
        copied += tick_copied;
        debug_assert!(target.iter().eq(layer.written.iter()), "a slice holds the written blocks as the domain has them");
    }
    TileExtractionCost {
        block_side,
        prior_writes,
        written_blocks: layer.written.len() as u64,
        chunks: layer.written.len().div_ceil(CHUNK_SLOTS) as u64,
        block_bytes: u64::from(block_side * block_side) * 2,
        gathered_bytes_per_tick: gathered as f64 / f64::from(ticks),
        copied_bytes_per_tick: copied as f64 / f64::from(ticks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skeleton_core::{Capabilities, Size, World, WorldConfig};
    use std::sync::Arc;

    #[test]
    fn a_step_leaves_the_slices_of_its_last_two_ticks() {
        let extraction = Arc::new(RenderExtraction::new());
        let mut world = World::new(WorldConfig::new(Size::Small, 2), Capabilities { extraction: Some(extraction.clone()) });
        world.step(5).unwrap();
        extraction.read(5, |previous, current| {
            assert_eq!((previous.tick, current.tick), (4, 5));
            assert_eq!(current.entities.x.len(), Size::Small.entity_count());
            assert!(current.entities.visual.iter().all(|&visual| visual != VISUAL_NONE));
            let written: usize = current.layers.iter().map(Vec::len).sum();
            assert!((1..=5).contains(&written), "five ticks wrote five tiles into {written} blocks");
        });
    }

    #[test]
    fn a_slice_holds_the_written_blocks_whichever_way_a_chunk_reached_it() {
        for block_side in [16, 32, 64] {
            let cost = tile_extraction_cost(block_side, 4_000, 512, 50);
            assert!(cost.written_blocks > 0 && cost.gathered_bytes_per_tick > 0.0);
            assert!(cost.gathered_bytes_per_tick + cost.copied_bytes_per_tick >= (cost.written_blocks - 50) as f64 * cost.block_bytes as f64);
        }
    }

    #[test]
    fn a_canonical_block_is_the_grid_read_block_by_block() {
        let grid = TileGrid::new(1, 64);
        let block = grid.block(32, 3);
        assert_eq!(block[0], grid.cells[32 * 64 + 32]);
        assert_eq!(block[32 * 32 - 1], grid.cells[64 * 64 - 1]);
    }
}
