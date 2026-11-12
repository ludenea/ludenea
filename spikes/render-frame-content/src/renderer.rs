//! The renderer of one world: refresh, prepare, declare and compile, record.
//! Throwaway, never product code.
//!
//! The stepping loop calls [`WorldRenderer::frame`] between two ticks. It
//! waits for refresh and prepare, which read the render slices, and declares
//! the graph itself; recording is started on the frame pool and not waited
//! for, and the task that ends it places the frame in the hand-off, where
//! the presenter takes it.
//!
//! What the renderer holds between frames is a cache of the frame's inputs
//! (the geometry of the tile blocks, keyed by layer and coordinate) or a
//! declared history (the level of detail each view last drew a mesh at). A
//! reproducible frame reads the history at its reset content and leaves the
//! stored one untouched.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use reference_workload::{Fnv1a, chunk_range};
use render_device_baseline::frames::{DrawConstants, scale_translate, vertex_bytes};
use render_device_baseline::scene::{SAMPLER_NEAREST, TEXTURE_WHITE};

use crate::graph::{GraphBuilder, Read, ResourceId, ResourceKind, Write};
use crate::instance::{Caches, FrameRecord, FrameTimes, Instance, Stamp};
use crate::math::Arithmetic;
use crate::pool::{FramePool, StageTime, Task};
use crate::port::{Handle, MemoryClass, PassSpec, PortDraw, PortError, PortResult, Source};
use crate::prepare::{BlockInfo, ChunkItems, Geometry, Item, NO_LEVEL, VIEW_COUNT, VIEW_SIDE, ViewBuffers, With, block_items, frame_content_hash, frame_time, mask_constants, sort, view_content_hash, view_set};
use crate::slices::{BLOCK_SIDE, CHUNK_SLOTS, LAYER_COUNT, LAYER_SIDE_CELLS, RenderExtraction};
use crate::visual::{CELL_SIZE, SharedGeometry, block_geometry};

/// The side of a world's target: its four views, two by two.
pub const TARGET_SIDE: u32 = VIEW_SIDE * 2;
/// How many parts a view's pass is recorded in when recording is parallel.
pub const SCENE_PARTS: usize = 8;
/// What a transient is filled with in the reference mode before its first
/// pass: a color no pass of the frame clears to.
pub const MARKER: [f32; 4] = [1.0, 0.0, 1.0, 1.0];
/// The clear color of a view's image and of the selection mask.
const VIEW_CLEAR: [f32; 4] = [0.015_625, 0.031_25, 0.062_5, 1.0];
const MASK_CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const BLOCKS_PER_SIDE: u32 = LAYER_SIDE_CELLS / BLOCK_SIDE;

/// The switches the ordinary mode and the reference mode differ by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// A view's pass recorded in parts by the pool's workers at once;
    /// otherwise every pass in one part, by one thread, pass after pass.
    pub parallel_recording: bool,
    pub culling: bool,
    pub remove_unread_passes: bool,
    pub markers: bool,
}

impl Mode {
    pub const ORDINARY: Mode = Mode { parallel_recording: true, culling: true, remove_unread_passes: true, markers: false };
    pub const REFERENCE: Mode = Mode { parallel_recording: false, culling: false, remove_unread_passes: false, markers: true };
}

#[derive(Clone, Copy, Debug)]
pub struct Request {
    /// The frame is drawn between the slices of `tick - 1` and `tick`.
    pub tick: u32,
    pub factor: f32,
    /// Rendered with every history at its reset content, the stored ones untouched.
    pub reproducible: bool,
    pub content_hash: bool,
    pub mode: Mode,
}

struct BlockEntry {
    /// The version of the written block the geometry was built from; none
    /// for a block read from the grid asset.
    version: Option<u32>,
    buffer: Option<Handle>,
    info: BlockInfo,
}

/// One view's list as prepare leaves it.
struct ViewList {
    /// The sorted list, and the positions in it of the items the pass
    /// draws: all of them, or those culling kept.
    buffers: ViewBuffers,
    hash: Option<u64>,
}

/// The buffers a world's frames reuse: the frame's own memory, taken when a
/// frame is prepared and given back when the device has finished it.
type Bin = Arc<Mutex<Vec<ViewBuffers>>>;

thread_local! {
    /// The draws of the part a thread is recording.
    static DRAWS: std::cell::RefCell<Vec<PortDraw>> = const { std::cell::RefCell::new(Vec::new()) };
}

enum Body {
    Scene { view: usize, color: Write },
    Mask { view: usize, color: Write },
    Composition { sources: [Read; VIEW_COUNT], target: Write },
}

enum Work {
    ScenePart { view: usize, positions: std::ops::Range<usize> },
    Mask { view: usize },
    Composition,
}

struct Planned {
    spec: PassSpec,
    /// One entry per part of the pass.
    parts: Vec<Work>,
}

/// What recording shares between its tasks.
struct Recording {
    instance: Arc<Instance>,
    world: u32,
    number: u64,
    caches: Arc<Caches>,
    lists: Vec<ViewList>,
    blocks: BTreeMap<(u32, u32), Handle>,
    geometry: Arc<SharedGeometry>,
    planned: Vec<Planned>,
    passes: Mutex<Vec<u64>>,
    failed: AtomicBool,
    draws: AtomicU64,
    transients: Vec<(u32, u32, Handle)>,
    bin: Bin,
}

impl Drop for Recording {
    fn drop(&mut self) {
        let mut bin = self.bin.lock().unwrap();
        bin.extend(self.lists.drain(..).map(|list| list.buffers));
    }
}

/// The four quads that compose the views' images onto the target, texel for pixel.
pub fn composition_constants() -> [DrawConstants; VIEW_COUNT] {
    [(-0.5, 0.5), (0.5, 0.5), (-0.5, -0.5), (0.5, -0.5)].map(|(x, y)| DrawConstants { transform: scale_translate(1.0, 1.0, x, y), tint: [1.0; 4], texture: 0, sampler: SAMPLER_NEAREST })
}

impl Recording {
    fn draws_into(&self, work: &Work, draws: &mut Vec<PortDraw>) {
        let item_draw = |item: &Item, constants: DrawConstants, blended: bool| {
            let buffer = match item.geometry {
                Geometry::Shared => Some(self.caches.geometry),
                Geometry::Block(layer, coordinate) => self.blocks.get(&(layer, coordinate)).copied(),
            };
            buffer.map(|buffer| PortDraw { buffer, first_vertex: item.range.first_vertex, vertex_count: item.range.vertex_count, blended, constants })
        };
        match work {
            Work::ScenePart { view, positions } => {
                let list = &self.lists[*view].buffers;
                draws.extend(list.drawn[positions.clone()].iter().map(|&position| &list.sorted[position as usize]).filter_map(|item| item_draw(item, item.constants, item.blended)));
            }
            Work::Mask { view } => {
                let list = &self.lists[*view].buffers;
                draws.extend(list.drawn.iter().map(|&position| &list.sorted[position as usize]).filter(|item| item.selected).filter_map(|item| item_draw(item, mask_constants(item), false)));
            }
            Work::Composition => {
                let quad = self.geometry.sprite;
                draws.extend(
                    composition_constants()
                        .iter()
                        .enumerate()
                        .map(|(view, &constants)| PortDraw { buffer: self.caches.geometry, first_vertex: quad.first_vertex, vertex_count: quad.vertex_count, blended: false, constants: DrawConstants { texture: view as u32, ..constants } }),
                );
            }
        }
    }

    fn open_all(&self) {
        let mut passes = self.passes.lock().unwrap();
        for planned in &self.planned {
            match self.instance.port.open_pass(self.number, &planned.spec) {
                Ok(pass) => passes.push(pass),
                Err(_) => {
                    self.failed.store(true, Ordering::Release);
                    return;
                }
            }
        }
    }

    fn record(&self, pass: usize, part: usize) {
        if self.failed.load(Ordering::Acquire) {
            return;
        }
        let native = self.passes.lock().unwrap()[pass];
        let recorded = DRAWS.with_borrow_mut(|draws| {
            draws.clear();
            self.draws_into(&self.planned[pass].parts[part], draws);
            self.draws.fetch_add(draws.len() as u64, Ordering::Relaxed);
            self.instance.port.record_part(self.number, native, part, draws)
        });
        if recorded.is_err() {
            self.failed.store(true, Ordering::Release);
        }
    }

    /// Ends the recording: the frame goes to the hand-off, or is dropped if
    /// a call met a lost device or a refused handle.
    fn finish(&self, mut frame: FrameRecord, record: StageTime) {
        let passes = std::mem::take(&mut *self.passes.lock().unwrap());
        let mut failed = self.failed.load(Ordering::Acquire);
        if !failed {
            failed = passes.iter().any(|&pass| self.instance.port.close_pass(pass).is_err());
        }
        if failed {
            self.instance.abandon(self.world, self.number, &passes, &self.transients);
            return;
        }
        frame.passes = passes;
        frame.stamp.draws = self.draws.load(Ordering::Relaxed);
        frame.times.record = record;
        self.instance.place(frame);
    }
}

pub struct WorldRenderer {
    pub id: u32,
    instance: Arc<Instance>,
    extraction: Arc<RenderExtraction>,
    pool: Arc<FramePool>,
    geometry: Arc<SharedGeometry>,
    target: Option<Handle>,
    blocks: BTreeMap<(u32, u32), BlockEntry>,
    /// Per view, the level each slot's mesh was last drawn at.
    histories: Vec<Arc<Vec<u8>>>,
    /// Per view and per chunk of the render slice, the part of the view's
    /// list its prepare task writes: kept from frame to frame.
    parts: Arc<Vec<Mutex<ChunkItems>>>,
    bin: Bin,
    pub blocks_built: u64,
}

fn cells_hash(cells: &[u16]) -> u64 {
    let mut hasher = Fnv1a::new();
    for cell in cells {
        hasher.write(&cell.to_le_bytes());
    }
    hasher.finish()
}

fn add(a: StageTime, b: StageTime) -> StageTime {
    StageTime { wall_us: a.wall_us + b.wall_us, processor_us: a.processor_us + b.processor_us, tasks: a.tasks + b.tasks }
}

impl WorldRenderer {
    pub fn new(id: u32, instance: Arc<Instance>, extraction: Arc<RenderExtraction>, worker_budget: usize) -> WorldRenderer {
        WorldRenderer {
            id,
            instance,
            extraction,
            pool: Arc::new(FramePool::new(worker_budget)),
            geometry: Arc::new(SharedGeometry::build()),
            target: None,
            blocks: BTreeMap::new(),
            histories: vec![Arc::new(Vec::new()); VIEW_COUNT],
            parts: Arc::new(Vec::new()),
            bin: Arc::new(Mutex::new(Vec::new())),
            blocks_built: 0,
        }
    }

    /// Refresh: the geometry of each tile block is built when the block is
    /// first seen and again when its version changed, or when its buffer was
    /// lost with the device.
    fn refresh(&mut self, tick: u32) -> PortResult<StageTime> {
        let port = &self.instance.port;
        // What each block is now: a written block at its version, or the grid's.
        let wanted: Vec<((u32, u32), Option<u32>)> = self.extraction.read(tick, |_, current| {
            let mut wanted = Vec::new();
            for layer in 0..LAYER_COUNT as u32 {
                let written: BTreeMap<u32, u32> = current.layers.get(layer as usize).map(|blocks| blocks.iter().map(|block| (block.coordinate, block.version)).collect()).unwrap_or_default();
                for coordinate in 0..BLOCKS_PER_SIDE * BLOCKS_PER_SIDE {
                    wanted.push(((layer, coordinate), written.get(&coordinate).copied()));
                }
            }
            wanted
        });
        let built: Arc<Mutex<Vec<((u32, u32), PortResult<BlockEntry>)>>> = Arc::new(Mutex::new(Vec::new()));
        let mut tasks: Vec<Task> = Vec::new();
        for (key, version) in wanted {
            let current = self.blocks.get(&key).is_some_and(|entry| entry.version == version && entry.buffer.is_none_or(|buffer| port.is_live(buffer)));
            if current {
                continue;
            }
            let (instance, extraction, built, world) = (Arc::clone(&self.instance), Arc::clone(&self.extraction), Arc::clone(&built), self.id);
            tasks.push(Box::new(move || {
                let (layer, coordinate) = key;
                let cells = match version {
                    Some(_) => extraction.read(tick, |_, current| current.layers[layer as usize].iter().find(|block| block.coordinate == coordinate).map(|block| block.cells.clone())).unwrap_or_default(),
                    None => extraction.canonical_block(layer as usize, coordinate),
                };
                let vertices = block_geometry(&cells, BLOCK_SIDE, BLOCKS_PER_SIDE, coordinate, layer);
                let side = BLOCK_SIDE as f32 * CELL_SIZE;
                let info = BlockInfo {
                    layer,
                    coordinate,
                    vertex_count: vertices.len() as u32,
                    cells_hash: cells_hash(&cells),
                    center: [(coordinate % BLOCKS_PER_SIDE) as f32 * side + side * 0.5, (coordinate / BLOCKS_PER_SIDE) as f32 * side + side * 0.5],
                    radius: side * 0.75,
                };
                let buffer = if vertices.is_empty() { Ok(None) } else { instance.port.create_vertex_buffer(vertex_bytes(&vertices), Some(world)).map(Some) };
                built.lock().unwrap().push((key, buffer.map(|buffer| BlockEntry { version, buffer, info })));
            }));
        }
        let time = self.pool.run(tasks);
        // Refresh is the only writer of the world's caches: the entries are
        // installed here, in the order of their keys.
        let mut built = std::mem::take(&mut *built.lock().unwrap());
        built.sort_by_key(|(key, _)| *key);
        for (key, entry) in built {
            let entry = entry?;
            self.blocks_built += 1;
            if let Some(old) = self.blocks.insert(key, entry)
                && let Some(buffer) = old.buffer
            {
                // A stale handle, after a loss, has nothing left to release.
                let _ = port.release(buffer);
            }
        }
        Ok(time)
    }

    /// Prepare: per view and per chunk of the render slice, then per view the
    /// merge in chunk order, the sort and the content hash.
    fn prepare<A: Arithmetic>(&mut self, request: &Request) -> (Vec<ViewList>, StageTime, StageTime) {
        let time = frame_time(request.tick, request.factor);
        let views = Arc::new(view_set::<A>(time));
        let slots = self.extraction.read(request.tick, |_, current| current.entities.x.len());
        let chunks = slots.div_ceil(CHUNK_SLOTS);
        if self.parts.len() != VIEW_COUNT * chunks {
            self.parts = Arc::new((0..VIEW_COUNT * chunks).map(|_| Mutex::new(ChunkItems::default())).collect());
        }
        let parts = Arc::clone(&self.parts);
        let mut tasks: Vec<Task> = Vec::with_capacity(VIEW_COUNT * chunks);
        for view in 0..VIEW_COUNT {
            let history = if request.reproducible { None } else { Some(Arc::clone(&self.histories[view])) };
            for chunk in 0..chunks {
                let (views, parts, extraction, geometry, history) = (Arc::clone(&views), Arc::clone(&parts), Arc::clone(&self.extraction), Arc::clone(&self.geometry), history.clone());
                let (tick, factor) = (request.tick, request.factor);
                tasks.push(Box::new(move || {
                    let mut part = parts[view * chunks + chunk].lock().unwrap();
                    extraction.read(tick, |previous, current| With::<A>::default().chunk_into(&views[view], chunk_range(chunk, slots), &previous.entities, &current.entities, factor, &geometry, history.as_deref().map(Vec::as_slice), &mut part));
                }));
            }
        }
        let prepare = self.pool.run(tasks);

        let blocks: Arc<Vec<BlockInfo>> = Arc::new(self.blocks.values().map(|entry| entry.info).collect());
        let lists: Arc<Vec<Mutex<Option<ViewList>>>> = Arc::new((0..VIEW_COUNT).map(|_| Mutex::new(None)).collect());
        let (culling, content_hash) = (request.mode.culling, request.content_hash);
        let tasks: Vec<Task> = (0..VIEW_COUNT)
            .map(|view| {
                let (views, parts, blocks, lists) = (Arc::clone(&views), Arc::clone(&parts), Arc::clone(&blocks), Arc::clone(&lists));
                let mut buffers = self.bin.lock().unwrap().pop().unwrap_or_default();
                Box::new(move || {
                    buffers.merged.clear();
                    buffers.levels.clear();
                    // The parts are merged in chunk order, whoever wrote them and when.
                    for chunk in 0..chunks {
                        let part = parts[view * chunks + chunk].lock().unwrap();
                        buffers.merged.extend_from_slice(&part.items);
                        buffers.levels.extend_from_slice(&part.levels);
                    }
                    buffers.merged.extend(block_items(&views[view], &blocks));
                    sort(&mut buffers);
                    buffers.drawn.clear();
                    buffers.drawn.extend(buffers.sorted.iter().enumerate().filter(|(_, item)| !culling || item.visible).map(|(position, _)| position as u32));
                    let hash = content_hash.then(|| view_content_hash(&views[view], &buffers.sorted));
                    *lists[view].lock().unwrap() = Some(ViewList { buffers, hash });
                }) as Task
            })
            .collect();
        let sort = self.pool.run(tasks);
        let lists: Vec<ViewList> = lists.iter().map(|list| list.lock().unwrap().take().expect("every view was sorted")).collect();
        if !request.reproducible {
            for (history, list) in self.histories.iter_mut().zip(&lists) {
                *history = Arc::new(list.buffers.levels.clone());
            }
        }
        (lists, prepare, sort)
    }

    /// Prepare alone, for its cost: the two stages' times and the items of
    /// the four views.
    pub fn measure_prepare<A: Arithmetic>(&mut self, request: &Request) -> (StageTime, StageTime, usize) {
        let (lists, prepare, sort) = self.prepare::<A>(request);
        let items = lists.iter().map(|list| list.buffers.sorted.len()).sum();
        self.bin.lock().unwrap().extend(lists.into_iter().map(|list| list.buffers));
        (prepare, sort, items)
    }

    /// Whether any view's history holds a level: whether an interactive
    /// frame would read one.
    pub fn has_history(&self) -> bool {
        self.histories.iter().any(|history| history.iter().any(|&level| level != NO_LEVEL))
    }

    /// Makes one frame of the world. The caller holds the world's right to
    /// start a frame (`Instance::begin` or `try_begin`); the frame's image
    /// arrives on the world's stream once the presenter has served it.
    pub fn frame<A: Arithmetic>(&mut self, request: &Request) -> PortResult<u64> {
        let started = Instant::now();
        let number = self.instance.port.declare_frame();
        match self.stages::<A>(request, number, started) {
            Ok(()) => Ok(number),
            Err(error) => {
                self.instance.abandon(self.id, number, &[], &[]);
                Err(error)
            }
        }
    }

    fn stages<A: Arithmetic>(&mut self, request: &Request, number: u64, started: Instant) -> PortResult<()> {
        let instance = Arc::clone(&self.instance);
        let caches = instance.caches()?;
        let target = match self.target {
            Some(target) if instance.port.is_live(target) => target,
            _ => {
                let target = instance.port.create_target(TARGET_SIDE, TARGET_SIDE, Some(self.id), MemoryClass::Targets)?;
                self.target = Some(target);
                target
            }
        };
        let history = !request.reproducible && self.has_history();
        let refresh = self.refresh(request.tick)?;
        let (lists, prepare, sort) = self.prepare::<A>(request);

        // Declare and compile, on the stepping thread.
        let declaring = Instant::now();
        let mut builder: GraphBuilder<Handle, Body> = GraphBuilder::new();
        let output = builder.import(target, true);
        let mut scenes = [ResourceId::default(); VIEW_COUNT];
        for (view, scene) in scenes.iter_mut().enumerate() {
            builder.pass("scene", |pass| {
                let color = pass.create(VIEW_SIDE, VIEW_SIDE);
                *scene = color.resource();
                Body::Scene { view, color }
            });
            builder.pass("selection mask", |pass| Body::Mask { view, color: pass.create(VIEW_SIDE, VIEW_SIDE) });
        }
        builder.pass("composition", |pass| Body::Composition { sources: scenes.map(|scene| pass.read(scene)), target: pass.write(output) });
        let compiled = builder.compile(request.mode.remove_unread_passes).map_err(PortError::Device)?;
        let declare_us = declaring.elapsed().as_secs_f64() * 1e6;

        // The transients come from the instance's pool, for this graph alone.
        let mut transients = Vec::new();
        let mut handles: BTreeMap<ResourceId, Handle> = BTreeMap::new();
        for (resource, width, height) in compiled.transients() {
            match instance.acquire_transient(width, height) {
                Ok(handle) => {
                    transients.push((width, height, handle));
                    handles.insert(resource, handle);
                }
                Err(error) => {
                    instance.abandon(self.id, 0, &[], &transients);
                    return Err(error);
                }
            }
        }
        let handle_of = |resource: ResourceId| match compiled.kind(resource) {
            ResourceKind::Imported(handle) => handle,
            ResourceKind::Transient { .. } => handles[&resource],
        };
        let asset_sources = caches.textures.map(Source::Texture);
        let mut planned = Vec::new();
        for (position, pass) in compiled.passes.iter().enumerate() {
            if request.mode.markers {
                for &write in &pass.writes {
                    if matches!(compiled.kind(write), ResourceKind::Transient { .. }) && compiled.first_use(write, position) {
                        planned.push(Planned { spec: PassSpec { target: handle_of(write), clear: MARKER, pipelines: caches.pipelines, textures: asset_sources, parts: 0 }, parts: Vec::new() });
                    }
                }
            }
            planned.push(match &pass.body {
                Body::Scene { view, color } => {
                    let drawn = lists[*view].buffers.drawn.len();
                    let part_count = if request.mode.parallel_recording { SCENE_PARTS } else { 1 };
                    let parts: Vec<Work> = (0..part_count).map(|part| Work::ScenePart { view: *view, positions: drawn * part / part_count..drawn * (part + 1) / part_count }).collect();
                    Planned { spec: PassSpec { target: handle_of(color.resource()), clear: VIEW_CLEAR, pipelines: caches.pipelines, textures: asset_sources, parts: parts.len() }, parts }
                }
                Body::Mask { view, color } => Planned { spec: PassSpec { target: handle_of(color.resource()), clear: MASK_CLEAR, pipelines: caches.pipelines, textures: asset_sources, parts: 1 }, parts: vec![Work::Mask { view: *view }] },
                Body::Composition { sources, target } => {
                    let mut textures = [Source::Texture(caches.textures[TEXTURE_WHITE as usize]); 5];
                    for (texture, source) in textures.iter_mut().zip(sources) {
                        *texture = Source::Target(handle_of(source.resource()));
                    }
                    Planned { spec: PassSpec { target: handle_of(target.resource()), clear: MASK_CLEAR, pipelines: caches.pipelines, textures, parts: 1 }, parts: vec![Work::Composition] }
                }
            });
        }

        let content_hash = request.content_hash.then(|| frame_content_hash(frame_time(request.tick, request.factor), &lists.iter().map(|list| list.hash.unwrap_or(0)).collect::<Vec<_>>(), &composition_constants()));

        let frame = FrameRecord {
            world: self.id,
            number,
            passes: Vec::new(),
            target,
            transients: transients.clone(),
            stamp: Stamp { tick: request.tick, factor: request.factor, complete: true, reproducible: request.reproducible, history, shader_variant: caches.shader_variant, draws: 0, passes: planned.len() as u64 },
            content_hash,
            started,
            placed: started,
            times: FrameTimes { refresh, prepare, sort, declare_us, ..FrameTimes::default() },
        };
        let recording = Arc::new(Recording {
            instance,
            world: self.id,
            number,
            caches,
            lists,
            blocks: self.blocks.iter().filter_map(|(key, entry)| entry.buffer.map(|buffer| (*key, buffer))).collect(),
            geometry: Arc::clone(&self.geometry),
            planned,
            passes: Mutex::new(Vec::new()),
            failed: AtomicBool::new(false),
            draws: AtomicU64::new(0),
            bin: Arc::clone(&self.bin),
            transients,
        });

        // Record: not waited for. The passes are opened, then their parts
        // are recorded, at once or in turn, then the frame is handed off.
        let parts: Vec<(usize, usize)> = recording.planned.iter().enumerate().flat_map(|(pass, planned)| (0..planned.parts.len()).map(move |part| (pass, part))).collect();
        if request.mode.parallel_recording {
            let (opening, pool) = (Arc::clone(&recording), Arc::clone(&self.pool));
            let open: Vec<Task> = vec![Box::new(move || opening.open_all())];
            let recording = Arc::clone(&recording);
            self.pool.start(open, move |opened| {
                let tasks: Vec<Task> = parts
                    .into_iter()
                    .map(|(pass, part)| {
                        let recording = Arc::clone(&recording);
                        Box::new(move || recording.record(pass, part)) as Task
                    })
                    .collect();
                pool.start(tasks, move |recorded| recording.finish(frame, add(opened, recorded)));
            });
        } else {
            let working = Arc::clone(&recording);
            let serial: Vec<Task> = vec![Box::new(move || {
                working.open_all();
                for (pass, part) in parts {
                    working.record(pass, part);
                }
            })];
            self.pool.start(serial, move |recorded| recording.finish(frame, recorded));
        }
        Ok(())
    }
}
