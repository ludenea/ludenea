//! The grid of scene cells, the payload of a cell (generated from its
//! coordinate and a seed, serialized as canonical bytes with a content hash,
//! so that the import action is a function and nothing is checked in), the
//! payload store shared read-only by the worlds of a process, the scripted
//! path of the point of interest, and the spatial hash and kernels the passes
//! share with the oracle. Throwaway, never product code.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use reference_workload::{INTERACTION_RADIUS, REPULSION_STRENGTH, SplitMix64, fnv1a};

/// The side of a streamed cell in world units. 2 500 entities per cell is the
/// reference workload's density (10 000 entities over 1 024 by 1 024 units).
pub const DEFAULT_CELL_SIDE: f32 = 512.0;
/// The side of a bin of the spatial hash: the workload's spatial hashing cell.
pub const BIN_SIDE: f32 = 16.0;
/// The bytes of one authored entity in a payload: local identifier, position,
/// velocity, mass, reference (target cell and local identifier).
pub const ENTITY_BYTES: usize = 32;
/// The bytes before the entities: the cell coordinate and the entity count.
pub const HEADER_BYTES: usize = 12;
/// The "no target" value of a reference's target cell.
pub const NO_CELL: u32 = u32::MAX;

/// The grid: N by N cells of one side, one payload density, one seed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridConfig {
    pub side_cells: u32,
    pub cell_side: f32,
    pub density: u32,
    pub seed: u64,
}

impl GridConfig {
    pub fn new(side_cells: u32, density: u32, seed: u64) -> GridConfig {
        assert!(side_cells >= 1 && density >= 1);
        GridConfig { side_cells, cell_side: DEFAULT_CELL_SIDE, density, seed }
    }

    pub fn cell_count(&self) -> u32 {
        self.side_cells * self.side_cells
    }

    pub fn world_side(&self) -> f32 {
        self.side_cells as f32 * self.cell_side
    }

    pub fn bins_per_side(&self) -> usize {
        (self.world_side() / BIN_SIDE).ceil() as usize
    }

    /// The instance identity of a cell is its grid coordinate; the index is
    /// its compact form, row by row.
    pub fn cell_index(&self, cx: u32, cy: u32) -> u32 {
        debug_assert!(cx < self.side_cells && cy < self.side_cells);
        cy * self.side_cells + cx
    }

    pub fn coordinate(&self, cell: u32) -> (u32, u32) {
        (cell % self.side_cells, cell / self.side_cells)
    }

    pub fn contains(&self, cx: i64, cy: i64) -> bool {
        cx >= 0 && cy >= 0 && cx < i64::from(self.side_cells) && cy < i64::from(self.side_cells)
    }

    /// The lower corner of a cell's box.
    pub fn origin(&self, cell: u32) -> (f32, f32) {
        let (cx, cy) = self.coordinate(cell);
        (cx as f32 * self.cell_side, cy as f32 * self.cell_side)
    }

    pub fn centre(&self, cell: u32) -> (f32, f32) {
        let (ox, oy) = self.origin(cell);
        (ox + (self.cell_side * 0.5), oy + (self.cell_side * 0.5))
    }

    /// The distance from a point to a cell's centre, in cells.
    pub fn distance_in_cells(&self, cell: u32, x: f32, y: f32) -> f32 {
        let (cx, cy) = self.centre(cell);
        let dx = cx - x;
        let dy = cy - y;
        ((dx * dx) + (dy * dy)).sqrt() / self.cell_side
    }

    /// The existing neighbours of a cell among the eight around it, in a
    /// fixed order (y offset outer, x offset inner).
    pub fn neighbours(&self, cell: u32) -> Vec<u32> {
        let (cx, cy) = self.coordinate(cell);
        let mut out = Vec::with_capacity(8);
        for b in -1_i64..=1 {
            for a in -1_i64..=1 {
                if a == 0 && b == 0 {
                    continue;
                }
                let (nx, ny) = (i64::from(cx) + a, i64::from(cy) + b);
                if self.contains(nx, ny) {
                    out.push(self.cell_index(nx as u32, ny as u32));
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

/// A full reference: the target instance's identity (a cell) and a local
/// identifier inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FullReference {
    pub cell: u32,
    pub local: u32,
}

/// One authored entity of a cell, as decoded from the payload.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuthoredEntity {
    pub local: u32,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub mass: f32,
    pub reference: Option<FullReference>,
}

/// The canonical bytes of a cell and their content hash.
#[derive(Debug)]
pub struct Payload {
    pub cell: u32,
    pub count: u32,
    pub bytes: Vec<u8>,
    pub content_hash: u64,
}

impl Payload {
    /// The import action: a function of the cell coordinate and the seed.
    /// One entity in ten references an entity of a neighbouring cell, one in
    /// a hundred an entity of its own cell.
    pub fn import(config: &GridConfig, cell: u32) -> Payload {
        let (cx, cy) = config.coordinate(cell);
        let mut salt = Vec::with_capacity(8);
        salt.extend_from_slice(&cx.to_le_bytes());
        salt.extend_from_slice(&cy.to_le_bytes());
        let mut rng = SplitMix64::new(config.seed.wrapping_add(fnv1a(&salt)));
        let neighbours = config.neighbours(cell);
        let (ox, oy) = config.origin(cell);
        let side = config.cell_side;
        let count = config.density;
        let mut bytes = Vec::with_capacity(HEADER_BYTES + count as usize * ENTITY_BYTES);
        bytes.extend_from_slice(&cx.to_le_bytes());
        bytes.extend_from_slice(&cy.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        for local in 0..count {
            let x = inside(ox + (rng.u01() * side), ox, ox + side);
            let y = inside(oy + (rng.u01() * side), oy, oy + side);
            let vx = ((rng.u01() * 2.0) - 1.0) * 32.0;
            let vy = ((rng.u01() * 2.0) - 1.0) * 32.0;
            let mass = rng.mass();
            let draw = rng.next() % 100;
            let reference = if draw < 10 && !neighbours.is_empty() {
                let target = neighbours[(rng.next() % neighbours.len() as u64) as usize];
                Some(FullReference { cell: target, local: (rng.next() % u64::from(count)) as u32 })
            } else if draw == 10 && count > 1 {
                let other = (u64::from(local) + 1 + (rng.next() % u64::from(count - 1))) % u64::from(count);
                Some(FullReference { cell, local: other as u32 })
            } else {
                None
            };
            bytes.extend_from_slice(&local.to_le_bytes());
            for value in [x, y, vx, vy, mass] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            let (target_cell, target_local) = reference.map_or((NO_CELL, 0), |r| (r.cell, r.local));
            bytes.extend_from_slice(&target_cell.to_le_bytes());
            bytes.extend_from_slice(&target_local.to_le_bytes());
        }
        let content_hash = fnv1a(&bytes);
        Payload { cell, count, bytes, content_hash }
    }

    /// Decodes the entity at `local` (entities are stored in local order).
    pub fn entity(&self, local: u32) -> AuthoredEntity {
        let at = HEADER_BYTES + local as usize * ENTITY_BYTES;
        let bytes = &self.bytes[at..at + ENTITY_BYTES];
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().expect("four bytes"));
        let f32_at = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().expect("four bytes"));
        let target_cell = u32_at(24);
        AuthoredEntity {
            local: u32_at(0),
            x: f32_at(4),
            y: f32_at(8),
            vx: f32_at(12),
            vy: f32_at(16),
            mass: f32_at(20),
            reference: (target_cell != NO_CELL).then(|| FullReference { cell: target_cell, local: u32_at(28) }),
        }
    }

    /// Whether the bytes still hash to the announced content hash.
    pub fn verify(&self, announced: u64) -> bool {
        fnv1a(&self.bytes) == announced && announced == self.content_hash
    }
}

/// Keeps a drawn position inside its cell's half-open box, as the boundary
/// pass does (an `f32` sum at the far edge of a large world may round up to
/// the edge itself).
fn inside(p: f32, lo: f32, hi: f32) -> f32 {
    if p >= hi {
        hi - 0.0625
    } else if p < lo {
        lo
    } else {
        p
    }
}

/// The payloads of a grid, imported once and shared read-only across the
/// worlds of a process (the loaders of every world fetch from here).
pub struct PayloadStore {
    config: GridConfig,
    payloads: Mutex<BTreeMap<u32, Arc<Payload>>>,
    imports: AtomicU64,
}

impl PayloadStore {
    pub fn new(config: GridConfig) -> Arc<PayloadStore> {
        Arc::new(PayloadStore { config, payloads: Mutex::new(BTreeMap::new()), imports: AtomicU64::new(0) })
    }

    pub fn config(&self) -> &GridConfig {
        &self.config
    }

    /// The payload of a cell, imported on the first request. The import runs
    /// under the store's lock, so that two loaders asking for one cell at
    /// once import it once (a cell of 2 500 entities imports in about a
    /// tenth of a millisecond).
    pub fn fetch(&self, cell: u32) -> Arc<Payload> {
        let mut payloads = self.payloads.lock().expect("the store is never poisoned");
        payloads
            .entry(cell)
            .or_insert_with(|| {
                self.imports.fetch_add(1, Ordering::Relaxed);
                Arc::new(Payload::import(&self.config, cell))
            })
            .clone()
    }

    /// The payloads currently held.
    pub fn len(&self) -> usize {
        self.payloads.lock().expect("the store is never poisoned").len()
    }

    /// The import actions run so far (one per cell when nothing raced).
    pub fn imports(&self) -> u64 {
        self.imports.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// The point of interest and its path
// ---------------------------------------------------------------------------

/// The streaming source: a position and a velocity in world units per tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointOfInterest {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

/// A scripted path: cell waypoints visited in order at one cell per
/// `ticks_per_cell` ticks, the point moving in a straight line between the
/// centres of consecutive waypoints. Its position is a function of the tick,
/// never integrated, so that every world and the oracle compute the same bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    pub waypoints: Vec<(u32, u32)>,
    pub ticks_per_cell: u32,
}

impl Path {
    /// A path from a start cell through relative moves, each one cell.
    pub fn from_moves(start: (u32, u32), moves: &[(i32, i32)], ticks_per_cell: u32) -> Path {
        assert!(ticks_per_cell >= 1);
        let mut waypoints = vec![start];
        let (mut x, mut y) = (start.0 as i64, start.1 as i64);
        for (dx, dy) in moves {
            x += i64::from(*dx);
            y += i64::from(*dy);
            assert!(x >= 0 && y >= 0, "a path never leaves the grid");
            waypoints.push((x as u32, y as u32));
        }
        Path { waypoints, ticks_per_cell }
    }

    /// The ticks the path takes from its first waypoint to its last.
    pub fn total_ticks(&self) -> u32 {
        (self.waypoints.len() as u32 - 1) * self.ticks_per_cell
    }

    pub fn fits(&self, config: &GridConfig) -> bool {
        self.waypoints.iter().all(|(x, y)| config.contains(i64::from(*x), i64::from(*y)))
    }

    /// The point after `tick` ticks: at rest on the last waypoint once the
    /// path is done.
    pub fn position_at(&self, config: &GridConfig, tick: u32) -> PointOfInterest {
        let segments = self.waypoints.len() as u32 - 1;
        let segment = (tick / self.ticks_per_cell).min(segments);
        if segment >= segments {
            let (x, y) = centre_of(config, self.waypoints[segments as usize]);
            return PointOfInterest { x, y, vx: 0.0, vy: 0.0 };
        }
        let (fx, fy) = centre_of(config, self.waypoints[segment as usize]);
        let (tx, ty) = centre_of(config, self.waypoints[segment as usize + 1]);
        let fraction = (tick % self.ticks_per_cell) as f32 / self.ticks_per_cell as f32;
        PointOfInterest {
            x: fx + ((tx - fx) * fraction),
            y: fy + ((ty - fy) * fraction),
            vx: (tx - fx) / self.ticks_per_cell as f32,
            vy: (ty - fy) / self.ticks_per_cell as f32,
        }
    }
}

fn centre_of(config: &GridConfig, (cx, cy): (u32, u32)) -> (f32, f32) {
    config.centre(config.cell_index(cx, cy))
}

// ---------------------------------------------------------------------------
// The spatial hash and the kernels shared by the passes and the oracle
// ---------------------------------------------------------------------------

/// The workload's cell lists as a counting sort over bins of `BIN_SIDE`
/// covering the whole grid; bin (bx, by) lists the alive slots whose
/// position falls in it, in increasing slot order.
#[derive(Clone, Debug)]
pub struct SpatialHash {
    bins_per_side: usize,
    bin_start: Vec<u32>,
    slots: Vec<u32>,
    cursor: Vec<u32>,
}

impl SpatialHash {
    pub fn new(bins_per_side: usize) -> SpatialHash {
        let bins = bins_per_side * bins_per_side;
        SpatialHash { bins_per_side, bin_start: vec![0; bins + 1], slots: Vec::new(), cursor: vec![0; bins + 1] }
    }

    pub fn bin_of(&self, x: f32, y: f32) -> (usize, usize) {
        let last = self.bins_per_side - 1;
        (((x / BIN_SIDE).floor() as usize).min(last), ((y / BIN_SIDE).floor() as usize).min(last))
    }

    pub fn build(&mut self, alive: &[bool], x: &[f32], y: &[f32]) {
        let bins = self.bins_per_side * self.bins_per_side;
        self.bin_start.clear();
        self.bin_start.resize(bins + 1, 0);
        for slot in 0..alive.len() {
            if alive[slot] {
                let (bx, by) = self.bin_of(x[slot], y[slot]);
                self.bin_start[by * self.bins_per_side + bx + 1] += 1;
            }
        }
        for bin in 0..bins {
            self.bin_start[bin + 1] += self.bin_start[bin];
        }
        self.cursor.clear();
        self.cursor.extend_from_slice(&self.bin_start);
        self.slots.clear();
        self.slots.resize(self.bin_start[bins] as usize, 0);
        for slot in 0..alive.len() {
            if alive[slot] {
                let (bx, by) = self.bin_of(x[slot], y[slot]);
                let bin = by * self.bins_per_side + bx;
                self.slots[self.cursor[bin] as usize] = slot as u32;
                self.cursor[bin] += 1;
            }
        }
    }

    pub fn slots_in(&self, bx: usize, by: usize) -> &[u32] {
        let bin = by * self.bins_per_side + bx;
        &self.slots[self.bin_start[bin] as usize..self.bin_start[bin + 1] as usize]
    }

    pub fn bins_per_side(&self) -> usize {
        self.bins_per_side
    }
}

/// System 2 of the workload for one slot, over this grid's bins: the same
/// operations in the same order as the reference kernel.
pub fn repulsion_force(x: &[f32], y: &[f32], hash: &SpatialHash, i: usize) -> (f32, f32) {
    let (xi, yi) = (x[i], y[i]);
    let (bx, by) = hash.bin_of(xi, yi);
    let side = hash.bins_per_side() as isize;
    let mut fx = 0.0_f32;
    let mut fy = 0.0_f32;
    for b in -1_isize..=1 {
        for a in -1_isize..=1 {
            let (gx, gy) = (bx as isize + a, by as isize + b);
            if gx < 0 || gy < 0 || gx >= side || gy >= side {
                continue;
            }
            for &j in hash.slots_in(gx as usize, gy as usize) {
                let j = j as usize;
                if j == i {
                    continue;
                }
                let dx = x[j] - xi;
                let dy = y[j] - yi;
                let d2 = (dx * dx) + (dy * dy);
                if d2 < INTERACTION_RADIUS * INTERACTION_RADIUS && d2 > 0.0 {
                    let d = d2.sqrt();
                    let s = (REPULSION_STRENGTH * (INTERACTION_RADIUS - d)) / d;
                    fx -= dx * s;
                    fy -= dy * s;
                }
            }
        }
    }
    (fx, fy)
}

/// System 5 of the workload for one axis, inside a cell's box `[lo, hi)`
/// instead of the world: an entity stays in the cell it was authored in.
pub fn bounced_in(mut p: f32, mut v: f32, lo: f32, hi: f32) -> (f32, f32) {
    if p < lo {
        p = (2.0 * lo) - p;
        v = -v;
    }
    if p >= hi {
        p = (2.0 * hi) - p;
        v = -v;
    }
    if p >= hi {
        p = hi - 0.0625;
    }
    if p < lo {
        p = lo;
    }
    (p, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> GridConfig {
        GridConfig::new(3, 50, 7)
    }

    #[test]
    fn the_import_is_a_function_of_the_coordinate_and_the_seed() {
        let a = Payload::import(&grid(), 4);
        let b = Payload::import(&grid(), 4);
        assert_eq!(a.bytes, b.bytes);
        assert_eq!(a.content_hash, b.content_hash);
        assert!(a.verify(a.content_hash));
        assert!(!a.verify(a.content_hash ^ 1));
        let other = Payload::import(&grid(), 5);
        assert_ne!(a.content_hash, other.content_hash);
        let other_seed = Payload::import(&GridConfig::new(3, 50, 8), 4);
        assert_ne!(a.content_hash, other_seed.content_hash);
        assert_eq!(a.bytes.len(), HEADER_BYTES + 50 * ENTITY_BYTES);
    }

    #[test]
    fn entities_lie_inside_their_cell_and_reference_neighbours_or_the_cell() {
        let config = GridConfig::new(4, 2_500, 11);
        for cell in 0..config.cell_count() {
            let payload = Payload::import(&config, cell);
            let (ox, oy) = config.origin(cell);
            let neighbours = config.neighbours(cell);
            let (mut cross, mut own) = (0, 0);
            for local in 0..payload.count {
                let e = payload.entity(local);
                assert_eq!(e.local, local);
                assert!(e.x >= ox && e.x < ox + config.cell_side, "{e:?}");
                assert!(e.y >= oy && e.y < oy + config.cell_side, "{e:?}");
                assert!([1.0, 2.0, 4.0].contains(&e.mass));
                match e.reference {
                    Some(r) if r.cell == cell => {
                        own += 1;
                        assert_ne!(r.local, local);
                    }
                    Some(r) => {
                        cross += 1;
                        assert!(neighbours.contains(&r.cell));
                        assert!(r.local < config.density);
                    }
                    None => {}
                }
            }
            assert!((200..300).contains(&cross), "about one in ten: {cross}");
            assert!((10..45).contains(&own), "about one in a hundred: {own}");
        }
    }

    #[test]
    fn the_store_imports_each_cell_once_and_shares_it() {
        let store = PayloadStore::new(grid());
        let a = store.fetch(2);
        let b = store.fetch(2);
        assert!(Arc::ptr_eq(&a, &b));
        store.fetch(3);
        assert_eq!(store.len(), 2);
        assert_eq!(store.imports(), 2);
    }

    #[test]
    fn the_path_moves_one_cell_per_segment_and_rests_at_the_end() {
        let config = grid();
        let path = Path::from_moves((1, 1), &[(1, 0), (-1, 0)], 10);
        assert_eq!(path.total_ticks(), 20);
        assert!(path.fits(&config));
        let start = path.position_at(&config, 0);
        assert_eq!((start.x, start.y), config.centre(config.cell_index(1, 1)));
        assert_eq!(start.vx, config.cell_side / 10.0);
        let half = path.position_at(&config, 5);
        assert_eq!(half.x, start.x + (config.cell_side * 0.5));
        let turned = path.position_at(&config, 10);
        assert_eq!((turned.x, turned.y), config.centre(config.cell_index(2, 1)));
        assert_eq!(turned.vx, -(config.cell_side / 10.0));
        let end = path.position_at(&config, 20);
        assert_eq!((end.x, end.y, end.vx, end.vy), (start.x, start.y, 0.0, 0.0));
        assert_eq!(path.position_at(&config, 99), end);
        assert!(!Path::from_moves((1, 1), &[(2, 0)], 1).fits(&config));
    }

    #[test]
    fn the_neighbourhood_and_the_distance_follow_the_grid() {
        let config = grid();
        assert_eq!(config.neighbours(4).len(), 8);
        assert_eq!(config.neighbours(0), vec![1, 3, 4]);
        assert_eq!(config.coordinate(config.cell_index(2, 1)), (2, 1));
        let (cx, cy) = config.centre(4);
        assert_eq!(config.distance_in_cells(4, cx, cy), 0.0);
        assert_eq!(config.distance_in_cells(5, cx, cy), 1.0);
        assert_eq!(config.bins_per_side(), 96);
    }

    #[test]
    fn the_bounce_keeps_an_entity_inside_its_cell() {
        assert_eq!(bounced_in(511.5, 2.0, 0.0, 512.0), (511.5, 2.0));
        assert_eq!(bounced_in(512.5, 2.0, 0.0, 512.0), (511.5, -2.0));
        assert_eq!(bounced_in(512.0, 2.0, 0.0, 512.0), (512.0 - 0.0625, -2.0));
        assert_eq!(bounced_in(-0.5, -3.0, 0.0, 512.0), (0.5, 3.0));
        assert_eq!(bounced_in(1023.9, 1.0, 512.0, 1024.0), (1023.9, 1.0));
    }

    #[test]
    fn the_spatial_hash_lists_alive_slots_by_bin_in_slot_order() {
        let mut hash = SpatialHash::new(4);
        let alive = [true, false, true, true];
        let x = [1.0, 1.0, 17.0, 2.0];
        let y = [1.0, 1.0, 1.0, 1.0];
        hash.build(&alive, &x, &y);
        assert_eq!(hash.slots_in(0, 0), &[0, 3]);
        assert_eq!(hash.slots_in(1, 0), &[2]);
        assert!(hash.slots_in(2, 0).is_empty());
        assert_eq!(hash.bin_of(1_000.0, 1_000.0), (3, 3), "clamped to the last bin");
    }
}
