//! Frames on Direct3D 12: the adapter of [`crate::frames::FrameDevice`].
//!
//! - A target is a 4-sample color resource and a 4-sample depth resource with
//!   their views, and a single-sample image of the same format. Between two
//!   passes the image rests in two read states at once (pixel shader resource
//!   and copy source), so that a later pass samples it and a read-back copies
//!   it with no transition of their own. A pass moves it to the resolve
//!   destination state in its opening list and back in its closing list.
//! - The draw's constants are 22 root constants visible to both stages. The
//!   five textures of a pass are five consecutive descriptors of one
//!   shader-visible heap that lives as long as the device: a pass takes a
//!   table of it when it opens, and the table returns to the heap with the
//!   submission, or when the pass is discarded. The three samplers are a
//!   table of a sampler heap made once.
//! - Each part is a command list on an allocator of its own, which sets the
//!   render targets, the viewport, the scissor, the root signature, the heaps
//!   and the tables itself. `close_pass` records two more lists: the opening
//!   one (the image's transition and the clears) and the closing one (the
//!   resolve and the transitions back). `submit` executes, for each pass, the
//!   opening list, the parts in their order and the closing list, then one
//!   list with the copies to the read-back buffers, and signals the fence
//!   that `finished` reads.
//! - What a submission owned is freed by `take_images`: its tables return to
//!   the heap, its read-back buffers are released, and its lists and their
//!   allocators are reset and recorded again by the passes that follow, a
//!   reset being how the interface reclaims the memory of a command list.
//! - `remove_device` waits for the queue to finish what it was given, then
//!   calls `ID3D12Device5::RemoveDevice` (see `Gpu::remove` for why it waits
//!   first). On a removed device `finished` and `take_images` return the
//!   removal as an error for every submission, the fence then reading as the
//!   largest value whatever was done; `submit` and the creations fail with
//!   the result of the call that reached the device; `wait_idle` returns the
//!   removal at once, and so the drop of the device returns.
//!
//! Throwaway, never product code.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::ffi::*;
use super::filtering::{self, SamplePattern};
use super::gpu::*;
use super::shaders::Compiler;
use super::validation::Collector;
use crate::backend::DeviceInfo;
use crate::frames::{DRAW_CONSTANT_BYTES, DrawConstants, FrameDevice, FrameDraw, PassDescription, Resource, ShaderPayload, TextureSource};
use crate::scene::{self, Encoding, Image, Texture, Vertex};

const PARAMETER_DRAW_CONSTANTS: u32 = 0;
const PARAMETER_TEXTURES: u32 = 1;
const PARAMETER_SAMPLERS: u32 = 2;
/// The draw's constants counted in 32-bit root constants.
const DRAW_CONSTANT_WORDS: usize = DRAW_CONSTANT_BYTES / 4;
const VERTEX_STRIDE: u32 = std::mem::size_of::<Vertex>() as u32;
/// How many descriptors the table of a pass's textures holds.
const TABLE_DESCRIPTORS: u32 = scene::TEXTURE_COUNT as u32;
/// How many passes may hold a table at once, open or in flight.
const TABLE_COUNT: u32 = 4096;
/// The states a target's image rests in between two passes: a later pass
/// samples it and a read-back copies it without a transition.
const STATE_IMAGE_AT_REST: u32 = STATE_PIXEL_SHADER_RESOURCE | STATE_COPY_SOURCE;

/// The messages of the debug layer that are not collected on a device open
/// for frames: the warning that a render target is cleared to a color other
/// than the one given when it was created (message 820, a note on speed). A
/// target is created before any pass names its clear color, so the warning
/// would follow every pass that clears to another color than the scene's.
pub const MESSAGES_LEFT_OUT: [u32; 1] = [820];

/// Locks a registry. Its entries are whole values that no call leaves half
/// written, so a registry a panicking thread poisoned is used as it stands.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct SampledTexture {
    resource: Com,
    format: u32,
    levels: u32,
}

struct VertexBuffer {
    location: u64,
    size: u32,
    /// Held for the address above.
    _buffer: Com,
}

struct Target {
    color: Com,
    resolved: Com,
    /// Held for its view.
    _depth: Com,
    target_view: DescriptorHeap,
    depth_view: DescriptorHeap,
    size: [u32; 2],
    /// Where the image lies in a read-back buffer, and the buffer's size.
    footprint: PlacedFootprint,
    read_back_bytes: u64,
}

struct Pipelines {
    opaque: Com,
    blended: Com,
}

/// Which tables of the heap are free: those returned, then those never taken.
struct TableSlots {
    returned: Vec<u32>,
    never_taken: u32,
}

/// One table of five descriptors, returned to the heap when dropped.
struct Table {
    index: u32,
    slots: Arc<Mutex<TableSlots>>,
}

impl Drop for Table {
    fn drop(&mut self) {
        locked(&self.slots).returned.push(self.index);
    }
}

/// The shader-visible heap the tables of the passes' textures live in.
struct Tables {
    heap: DescriptorHeap,
    slots: Arc<Mutex<TableSlots>>,
}

impl Tables {
    fn take(&self) -> Result<Table, String> {
        let mut slots = locked(&self.slots);
        let index = match slots.returned.pop() {
            Some(index) => index,
            None if slots.never_taken < TABLE_COUNT => {
                slots.never_taken += 1;
                slots.never_taken - 1
            }
            None => return Err(format!("the heap of the passes' textures is full: {TABLE_COUNT} passes are open or in flight")),
        };
        Ok(Table { index, slots: Arc::clone(&self.slots) })
    }

    fn cpu(&self, table: &Table, position: u32) -> CpuHandle {
        self.heap.cpu(table.index * TABLE_DESCRIPTORS + position)
    }

    fn gpu(&self, table: &Table) -> u64 {
        self.heap.gpu(table.index * TABLE_DESCRIPTORS)
    }
}

/// What the lists of a pass are recorded with, copied out of the registries
/// so that no lock is held while a list is recorded.
#[derive(Clone, Copy)]
struct PassInputs {
    color: Shared,
    resolved: Shared,
    target_view: CpuHandle,
    depth_view: CpuHandle,
    size: [u32; 2],
    clear: [f32; 4],
    opaque: Shared,
    blended: Shared,
    textures: u64,
}

/// The lists of a pass, each closed, with its allocator.
enum Lists {
    /// One place per part, filled when the part is recorded.
    Open(Vec<Option<Recorder>>),
    /// The opening list, the parts in their order and the closing list.
    Closed(Vec<Recorder>),
}

struct Pass {
    inputs: PassInputs,
    /// Named by `inputs.textures`.
    table: Table,
    lists: Lists,
}

/// A read-back buffer and the size of the image copied to it.
struct ReadBackImage {
    read_back: ReadBack,
    size: [u32; 2],
}

struct Submission {
    fence: u64,
    /// Every list executed, in the order of its execution.
    recorders: Vec<Recorder>,
    /// Held until the device has read them; returned to the heap when dropped.
    _tables: Vec<Table>,
    images: Vec<ReadBackImage>,
}

pub struct Direct3d12Frames<'a> {
    submissions: Mutex<HashMap<u64, Submission>>,
    passes: Mutex<HashMap<u64, Pass>>,
    /// The lists that are neither recorded nor in flight, each closed, with
    /// its allocator: those of the submissions taken and of the passes
    /// discarded, reset by the next recording.
    idle_recorders: Mutex<Vec<Recorder>>,
    pipelines: Mutex<HashMap<u64, Pipelines>>,
    targets: Mutex<HashMap<u64, Target>>,
    vertex_buffers: Mutex<HashMap<u64, VertexBuffer>>,
    textures: Mutex<HashMap<u64, SampledTexture>>,
    next_identifier: AtomicU64,
    tables: Tables,
    samplers: DescriptorHeap,
    root_signature: Com,
    pattern: SamplePattern,
    info: DeviceInfo,
    compiler: &'a Compiler,
    /// Held while the frame shader is compiled, one thread at a time.
    compiling: Mutex<()>,
    collector: Option<Mutex<Collector>>,
    /// Dropped last: every object above is released before the device.
    gpu: Gpu,
}

// SAFETY: the device, its queue, its fence, its heaps, its resources, its
// pipeline states and its root signature are free-threaded objects of
// Direct3D 12, and releasing any of them is too. A command list and its
// allocator are touched by one thread at a time: a recorder is taken out of
// the idle ones by the thread that records it and stored in its pass once
// closed, and the lists of a pass are executed by `submit` after the pass is
// taken out of its registry. Everything the structure changes is behind a
// mutex, the fence value of `Gpu` and the collector included.
unsafe impl Send for Direct3d12Frames<'_> {}
// SAFETY: as above.
unsafe impl Sync for Direct3d12Frames<'_> {}

impl<'a> Direct3d12Frames<'a> {
    pub fn new(gpu: Gpu, info: DeviceInfo, compiler: &'a Compiler, collector: Option<Collector>) -> Result<Direct3d12Frames<'a>, String> {
        let pattern = SamplePattern::choose(&gpu)?;
        let root_signature = root_signature(&gpu)?;
        let samplers = filtering::create_samplers(&gpu)?;
        let heap = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, TABLE_COUNT * TABLE_DESCRIPTORS, true)?;
        let tables = Tables { heap, slots: Arc::new(Mutex::new(TableSlots { returned: Vec::new(), never_taken: 0 })) };
        Ok(Direct3d12Frames {
            submissions: Mutex::new(HashMap::new()),
            passes: Mutex::new(HashMap::new()),
            idle_recorders: Mutex::new(Vec::new()),
            pipelines: Mutex::new(HashMap::new()),
            targets: Mutex::new(HashMap::new()),
            vertex_buffers: Mutex::new(HashMap::new()),
            textures: Mutex::new(HashMap::new()),
            next_identifier: AtomicU64::new(1),
            tables,
            samplers,
            root_signature,
            pattern,
            info,
            compiler,
            compiling: Mutex::new(()),
            collector: collector.map(Mutex::new),
            gpu,
        })
    }

    /// An identifier no resource, pass or submission of the device has had.
    fn identifier(&self) -> u64 {
        self.next_identifier.fetch_add(1, Ordering::Relaxed)
    }

    /// A list in the recording state with its allocator, both reset: one of
    /// the idle ones where there is one, a new one otherwise.
    fn recorder(&self) -> Result<Recorder, String> {
        let idle = locked(&self.idle_recorders).pop();
        let recorder = match idle {
            Some(recorder) => recorder,
            None => self.gpu.recorder()?,
        };
        recorder.begin()?;
        Ok(recorder)
    }

    /// The view of each run of draws that share a vertex buffer, with the run.
    fn vertex_runs<'draws>(&self, draws: &'draws [FrameDraw]) -> Result<Vec<(VertexBufferView, &'draws [FrameDraw])>, String> {
        let buffers = locked(&self.vertex_buffers);
        draws
            .chunk_by(|one, next| one.vertex_buffer == next.vertex_buffer)
            .map(|run| {
                let buffer = buffers.get(&run[0].vertex_buffer).ok_or_else(|| format!("record_part: no vertex buffer {}", run[0].vertex_buffer))?;
                Ok((VertexBufferView { location: buffer.location, size: buffer.size, stride: VERTEX_STRIDE }, run))
            })
            .collect()
    }

    /// Sets what every part of a pass draws with, which no list inherits
    /// from the one executed before it.
    fn set_part_state(&self, list: Object, inputs: &PassInputs) {
        set_descriptor_heaps(list, &[self.tables.heap.heap.raw(), self.samplers.heap.raw()]);
        set_graphics_root_signature(list, self.root_signature.raw());
        set_graphics_table(list, PARAMETER_TEXTURES, inputs.textures);
        set_graphics_table(list, PARAMETER_SAMPLERS, self.samplers.gpu(0));
        set_viewport(list, 0, inputs.size[0], inputs.size[1]);
        set_render_targets(list, &[inputs.target_view], Some(inputs.depth_view));
        set_triangle_list(list);
    }

    /// The read-back buffers of the targets named and the list that copies
    /// each target's image to its buffer; no list where no target is named.
    fn record_read_back(&self, read_back: &[u64]) -> Result<(Vec<ReadBackImage>, Option<Recorder>), String> {
        if read_back.is_empty() {
            return Ok((Vec::new(), None));
        }
        let sources: Vec<(Shared, PlacedFootprint, u64, [u32; 2])> = {
            let targets = locked(&self.targets);
            let source = |identifier: &u64| {
                let target = targets.get(identifier).ok_or_else(|| format!("submit: no target {identifier} to read back"))?;
                Ok((Shared(target.resolved.raw()), target.footprint, target.read_back_bytes, target.size))
            };
            read_back.iter().map(source).collect::<Result<_, String>>()?
        };
        let recorder = self.recorder()?;
        let mut images = Vec::with_capacity(sources.len());
        for (resolved, footprint, bytes, size) in sources {
            let read_back = ReadBack { buffer: self.gpu.buffer(HEAP_TYPE_READBACK, bytes, RESOURCE_FLAG_NONE, "frame read-back")?, footprint, size: bytes };
            copy_texture_to_read_back(recorder.list.raw(), resolved.0, &read_back);
            images.push(ReadBackImage { read_back, size });
        }
        close(recorder.list.raw())?;
        Ok((images, Some(recorder)))
    }

    /// Takes the passes out of their registry, each with its table and its
    /// lists in the order of their execution; none is taken unless all are
    /// closed.
    fn take_closed(&self, passes: &[u64]) -> Result<Vec<(Table, Vec<Recorder>)>, String> {
        let mut registry = locked(&self.passes);
        for (position, identifier) in passes.iter().enumerate() {
            match registry.get(identifier).map(|pass| &pass.lists) {
                None => return Err(format!("submit: no pass {identifier}")),
                Some(Lists::Open(_)) => return Err(format!("submit: the pass {identifier} is not closed")),
                Some(Lists::Closed(_)) if passes[..position].contains(identifier) => return Err(format!("submit: the pass {identifier} is named twice")),
                Some(Lists::Closed(_)) => {}
            }
        }
        let mut taken = Vec::with_capacity(passes.len());
        for identifier in passes {
            match registry.remove(identifier) {
                Some(Pass { table, lists: Lists::Closed(lists), .. }) => taken.push((table, lists)),
                // Checked above under this same lock.
                _ => return Err(format!("submit: the pass {identifier} is no longer closed")),
            }
        }
        Ok(taken)
    }
}

impl Drop for Direct3d12Frames<'_> {
    fn drop(&mut self) {
        // The queue may still execute the lists of submissions that were not
        // taken; what is left is released only once it has drained. A failure
        // here has no caller to report to, and releasing anyway is the only
        // option.
        let _ = self.gpu.wait_idle();
    }
}

/// The 22 root constants of the draws, the table of a pass's five textures
/// and the table of the three samplers.
fn root_signature(gpu: &Gpu) -> Result<Com, String> {
    let textures = [DescriptorRange { kind: DESCRIPTOR_RANGE_TYPE_SRV, count: TABLE_DESCRIPTORS, base_register: 0, space: 0, offset_from_table_start: 0 }];
    let samplers = [DescriptorRange { kind: DESCRIPTOR_RANGE_TYPE_SAMPLER, count: 3, base_register: 0, space: 0, offset_from_table_start: 0 }];
    let parameters = [
        RootParameter::constants(0, DRAW_CONSTANT_WORDS as u32, SHADER_VISIBILITY_ALL),
        RootParameter::table(&textures, SHADER_VISIBILITY_PIXEL),
        RootParameter::table(&samplers, SHADER_VISIBILITY_PIXEL),
    ];
    gpu.root_signature(&parameters, &[], ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, "frames")
}

/// The draw's constants as the 22 root constants, in the order of their bytes.
fn constant_words(constants: &DrawConstants) -> [u32; DRAW_CONSTANT_WORDS] {
    let mut words = [0_u32; DRAW_CONSTANT_WORDS];
    for (word, value) in words.iter_mut().zip(constants.transform.iter().chain(&constants.tint)) {
        *word = value.to_bits();
    }
    let [.., texture, sampler] = &mut words;
    (*texture, *sampler) = (constants.texture, constants.sampler);
    words
}

/// The list executed before the parts of a pass: the image leaves its rest
/// for the resolve, and the color and the depth are cleared.
fn record_opening(list: Object, inputs: &PassInputs) {
    barriers(list, &[transition(inputs.resolved.0, STATE_IMAGE_AT_REST, STATE_RESOLVE_DEST)]);
    clear_target(list, inputs.target_view, inputs.clear);
    clear_depth(list, inputs.depth_view, scene::CLEAR_DEPTH);
}

/// The list executed after the parts of a pass: the resolve, after which the
/// image is back at rest and the color is a render target again.
fn record_closing(list: Object, inputs: &PassInputs) {
    let (color, resolved) = (inputs.color.0, inputs.resolved.0);
    barriers(list, &[transition(color, STATE_RENDER_TARGET, STATE_RESOLVE_SOURCE)]);
    resolve(list, resolved, color, FORMAT_R8G8B8A8_UNORM_SRGB);
    barriers(list, &[transition(color, STATE_RESOLVE_SOURCE, STATE_RENDER_TARGET), transition(resolved, STATE_RESOLVE_DEST, STATE_IMAGE_AT_REST)]);
}

impl FrameDevice for Direct3d12Frames<'_> {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn settings(&self) -> String {
        format!(
            "{}; the three samplers in a shader-visible sampler heap, the five textures of a pass in a table of shader resource views; no environment setting is read by the interface",
            self.pattern.description
        )
    }

    fn import_shader(&self, variant: u32) -> Result<ShaderPayload, String> {
        if variant > 1 {
            return Err(format!("import_shader: the frame shader has the variants 0 and 1, not {variant}"));
        }
        let value = variant.to_string();
        let definitions = [("TINT_VARIANT", value.as_str())];
        let _one_thread_at_a_time = locked(&self.compiling);
        Ok(ShaderPayload {
            vertex: self.compiler.entry("frame.hlsl", "vs_main", "vs_5_1", &definitions)?,
            fragment: self.compiler.entry("frame.hlsl", "ps_main", "ps_5_1", &definitions)?,
            form: "bytecode of the shader model 5.1 targets, compiled by D3DCompile",
        })
    }

    fn create_pipelines(&self, payload: &ShaderPayload) -> Result<u64, String> {
        let pipeline = |blended: bool| filtering::pipeline(&self.gpu, &payload.vertex, &payload.fragment, self.root_signature.raw(), self.pattern.sample, blended, "frames");
        let pipelines = Pipelines { opaque: pipeline(false)?, blended: pipeline(true)? };
        let identifier = self.identifier();
        locked(&self.pipelines).insert(identifier, pipelines);
        Ok(identifier)
    }

    fn create_texture(&self, texture: &Texture) -> Result<u64, String> {
        // The upload reads every level by its size, so a short one is refused here.
        let short = texture.levels.iter().enumerate().any(|(level, bytes)| {
            let side = |full: u32| full.checked_shr(level as u32).unwrap_or(0).max(1) as usize;
            bytes.len() < side(texture.width) * side(texture.height) * 4
        });
        if texture.levels.is_empty() || short {
            return Err(format!("create_texture: the texture {} has no level, or a level shorter than its size asks", texture.name));
        }
        let format = match texture.encoding {
            Encoding::Srgb => FORMAT_R8G8B8A8_UNORM_SRGB,
            Encoding::Unorm => FORMAT_R8G8B8A8_UNORM,
        };
        let levels = texture.levels.len() as u16;
        let desc = texture_desc(texture.width, texture.height, levels, format, ONE_SAMPLE, RESOURCE_FLAG_NONE);
        let resource = self.gpu.upload_texture(&desc, &texture.levels, 4, STATE_PIXEL_SHADER_RESOURCE, &format!("frame texture {}", texture.name))?;
        let identifier = self.identifier();
        locked(&self.textures).insert(identifier, SampledTexture { resource, format, levels: u32::from(levels) });
        Ok(identifier)
    }

    fn create_vertex_buffer(&self, bytes: &[u8]) -> Result<u64, String> {
        let size = u32::try_from(bytes.len()).map_err(|_| format!("create_vertex_buffer: {} bytes are more than a vertex buffer view names", bytes.len()))?;
        let buffer = self.gpu.upload_buffer(bytes, STATE_VERTEX_AND_CONSTANT_BUFFER, RESOURCE_FLAG_NONE, "frame vertices")?;
        let identifier = self.identifier();
        locked(&self.vertex_buffers).insert(identifier, VertexBuffer { location: gpu_address(buffer.raw()), size, _buffer: buffer });
        Ok(identifier)
    }

    fn create_target(&self, width: u32, height: u32) -> Result<u64, String> {
        let (size, sample) = ([width, height], self.pattern.sample);
        // The scene's clear color is the one a target can be told of here; a
        // pass that clears to another is slower on some devices, not wrong.
        let color_clear = clear_color(FORMAT_R8G8B8A8_UNORM_SRGB, scene::CLEAR_COLOR);
        let color = self.gpu.committed(HEAP_TYPE_DEFAULT, &filtering::color_desc(size, sample), STATE_RENDER_TARGET, Some(&color_clear), "frame target, 4-sample color")?;
        let depth = self.gpu.committed(HEAP_TYPE_DEFAULT, &filtering::depth_desc(size, sample), STATE_DEPTH_WRITE, Some(&clear_depth_value(scene::CLEAR_DEPTH)), "frame target, 4-sample depth")?;
        let resolved_desc = texture_desc(width, height, 1, FORMAT_R8G8B8A8_UNORM_SRGB, ONE_SAMPLE, RESOURCE_FLAG_NONE);
        let resolved = self.gpu.committed(HEAP_TYPE_DEFAULT, &resolved_desc, STATE_IMAGE_AT_REST, None, "frame target, image")?;
        let (footprints, _, read_back_bytes) = self.gpu.footprints(&resolved_desc, 1);
        let target_view = self.gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_RTV, 1, false)?;
        self.gpu.render_target_view(color.raw(), FORMAT_UNKNOWN, target_view.cpu(0));
        let depth_view = self.gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_DSV, 1, false)?;
        self.gpu.depth_view(depth.raw(), depth_view.cpu(0));
        let identifier = self.identifier();
        locked(&self.targets).insert(identifier, Target { color, resolved, _depth: depth, target_view, depth_view, size, footprint: footprints[0], read_back_bytes });
        Ok(identifier)
    }

    fn destroy(&self, resource: Resource) {
        match resource {
            Resource::Texture(identifier) => drop(locked(&self.textures).remove(&identifier)),
            Resource::VertexBuffer(identifier) => drop(locked(&self.vertex_buffers).remove(&identifier)),
            Resource::Target(identifier) => drop(locked(&self.targets).remove(&identifier)),
            Resource::Pipelines(identifier) => drop(locked(&self.pipelines).remove(&identifier)),
        }
    }

    fn open_pass(&self, pass: &PassDescription) -> Result<u64, String> {
        let table = self.tables.take()?;
        let (opaque, blended) = {
            let pipelines = locked(&self.pipelines);
            let pair = pipelines.get(&pass.pipelines).ok_or_else(|| format!("open_pass: no pair of pipelines {}", pass.pipelines))?;
            (Shared(pair.opaque.raw()), Shared(pair.blended.raw()))
        };
        for (position, source) in pass.textures.iter().enumerate() {
            let descriptor = self.tables.cpu(&table, position as u32);
            match *source {
                TextureSource::Texture(identifier) => {
                    let textures = locked(&self.textures);
                    let texture = textures.get(&identifier).ok_or_else(|| format!("open_pass: no texture {identifier}"))?;
                    self.gpu.texture_view(texture.resource.raw(), texture.format, texture.levels, descriptor);
                }
                TextureSource::Target(identifier) => {
                    let targets = locked(&self.targets);
                    let target = targets.get(&identifier).ok_or_else(|| format!("open_pass: no target {identifier} to sample"))?;
                    self.gpu.texture_view(target.resolved.raw(), FORMAT_R8G8B8A8_UNORM_SRGB, 1, descriptor);
                }
            }
        }
        let inputs = {
            let targets = locked(&self.targets);
            let target = targets.get(&pass.target).ok_or_else(|| format!("open_pass: no target {}", pass.target))?;
            PassInputs {
                color: Shared(target.color.raw()),
                resolved: Shared(target.resolved.raw()),
                target_view: target.target_view.cpu(0),
                depth_view: target.depth_view.cpu(0),
                size: target.size,
                clear: pass.clear,
                opaque,
                blended,
                textures: self.tables.gpu(&table),
            }
        };
        let identifier = self.identifier();
        locked(&self.passes).insert(identifier, Pass { inputs, table, lists: Lists::Open((0..pass.parts).map(|_| None).collect()) });
        Ok(identifier)
    }

    fn record_part(&self, pass: u64, part: usize, draws: &[FrameDraw]) -> Result<(), String> {
        let inputs = {
            let passes = locked(&self.passes);
            let entry = passes.get(&pass).ok_or_else(|| format!("record_part: no pass {pass}"))?;
            match &entry.lists {
                Lists::Closed(_) => return Err(format!("record_part: the pass {pass} is closed")),
                Lists::Open(parts) => match parts.get(part) {
                    None => return Err(format!("record_part: the pass {pass} has {} parts, none at index {part}", parts.len())),
                    Some(Some(_)) => return Err(format!("record_part: the part {part} of the pass {pass} is already recorded")),
                    Some(None) => entry.inputs,
                },
            }
        };
        let runs = self.vertex_runs(draws)?;
        let recorder = self.recorder()?;
        let list = recorder.list.raw();
        self.set_part_state(list, &inputs);
        // The pipeline is set when it changes only, as the vertex buffer is.
        let mut bound = None;
        for (view, run) in &runs {
            set_vertex_view(list, view);
            for draw in *run {
                if bound != Some(draw.blended) {
                    set_pipeline(list, if draw.blended { inputs.blended.0 } else { inputs.opaque.0 });
                    bound = Some(draw.blended);
                }
                set_graphics_constants(list, PARAMETER_DRAW_CONSTANTS, &constant_words(&draw.constants));
                super::gpu::draw(list, draw.vertex_count, draw.first_vertex);
            }
        }
        close(list)?;
        match locked(&self.passes).get_mut(&pass).map(|entry| &mut entry.lists) {
            Some(Lists::Open(parts)) if matches!(parts.get(part), Some(None)) => {
                parts[part] = Some(recorder);
                Ok(())
            }
            _ => Err(format!("record_part: the pass {pass} was closed or discarded, or its part {part} recorded twice, while the part was recorded")),
        }
    }

    fn close_pass(&self, pass: u64) -> Result<(), String> {
        let inputs = locked(&self.passes).get(&pass).map(|entry| entry.inputs).ok_or_else(|| format!("close_pass: no pass {pass}"))?;
        let (opening, closing) = (self.recorder()?, self.recorder()?);
        record_opening(opening.list.raw(), &inputs);
        close(opening.list.raw())?;
        record_closing(closing.list.raw(), &inputs);
        close(closing.list.raw())?;
        let mut passes = locked(&self.passes);
        let entry = passes.get_mut(&pass).ok_or_else(|| format!("close_pass: the pass {pass} was discarded while it was closed"))?;
        let Lists::Open(parts) = &mut entry.lists else {
            return Err(format!("close_pass: the pass {pass} is already closed"));
        };
        if let Some(missing) = parts.iter().position(Option::is_none) {
            return Err(format!("close_pass: the part {missing} of the pass {pass} is not recorded"));
        }
        let lists = std::iter::once(opening).chain(parts.drain(..).flatten()).chain(std::iter::once(closing)).collect();
        entry.lists = Lists::Closed(lists);
        Ok(())
    }

    fn discard_pass(&self, pass: u64) {
        let Some(Pass { lists, .. }) = locked(&self.passes).remove(&pass) else {
            return;
        };
        // Its lists are closed and were never executed: they are recorded again.
        let recorders: Vec<Recorder> = match lists {
            Lists::Open(parts) => parts.into_iter().flatten().collect(),
            Lists::Closed(lists) => lists,
        };
        locked(&self.idle_recorders).extend(recorders);
    }

    fn submit(&self, passes: &[u64], read_back: &[u64]) -> Result<u64, String> {
        let (images, copies) = self.record_read_back(read_back)?;
        let (mut recorders, mut tables) = (Vec::new(), Vec::new());
        for (table, lists) in self.take_closed(passes)? {
            tables.push(table);
            recorders.extend(lists);
        }
        recorders.extend(copies);
        let lists: Vec<Object> = recorders.iter().map(|recorder| recorder.list.raw()).collect();
        let fence = if lists.is_empty() { self.gpu.signal()? } else { self.gpu.execute(&lists)? };
        let identifier = self.identifier();
        locked(&self.submissions).insert(identifier, Submission { fence, recorders, _tables: tables, images });
        Ok(identifier)
    }

    fn finished(&self, submission: u64) -> Result<bool, String> {
        let fence = locked(&self.submissions).get(&submission).map(|entry| entry.fence).ok_or_else(|| format!("finished: no submission {submission}"))?;
        self.gpu.reached(fence)
    }

    fn take_images(&self, submission: u64) -> Result<Vec<Image>, String> {
        let Submission { recorders, images, .. } = {
            let mut submissions = locked(&self.submissions);
            let fence = submissions.get(&submission).map(|entry| entry.fence).ok_or_else(|| format!("take_images: no submission {submission}"))?;
            if !self.gpu.reached(fence)? {
                return Err(format!("take_images: the submission {submission} is not finished"));
            }
            submissions.remove(&submission).ok_or_else(|| format!("take_images: no submission {submission}"))?
        };
        // The device has executed the lists: they are recorded again.
        locked(&self.idle_recorders).extend(recorders);
        let read = |image: &ReadBackImage| {
            let (read_back, [width, height]) = (&image.read_back, image.size);
            let bytes = read_buffer(read_back.buffer.raw(), read_back.size)?;
            Ok(Image::from_rows(width, height, read_back.footprint.row_pitch as usize, &bytes[read_back.footprint.offset as usize..]))
        };
        images.iter().map(read).collect()
    }

    fn wait_idle(&self) -> Result<(), String> {
        self.gpu.wait_idle()
    }

    fn remove_device(&self) -> Result<String, String> {
        self.gpu.remove()?;
        Ok("ID3D12Device5::RemoveDevice, on the interface the device gave when queried for it, once the queue had finished the work it was given".to_string())
    }

    fn removed_reason(&self) -> Option<String> {
        self.gpu.removed_reason()
    }

    fn validation_delivery(&self) -> String {
        match &self.collector {
            Some(collector) => format!(
                "{}; the warning that a render target is cleared to a color other than the one given at its creation (message 820) is left out, a pass naming its own clear color",
                locked(collector).delivery()
            ),
            None => "the debug layer is not enabled on this device".to_string(),
        }
    }

    fn validation_messages(&self) -> Vec<String> {
        self.collector.as_ref().map(|collector| locked(collector).drain()).unwrap_or_default()
    }
}
