//! The filtering and blending scene on Direct3D 12: 4-sample sRGB color and
//! float depth targets resolved with `ResolveSubresource`, five textures in a
//! table of shader resource views, three samplers in a sampler heap, the
//! texture and sampler indexes as root constants, the fan of slivers drawn by
//! `ExecuteIndirect`, two timestamps around the pass and a polled read-back.
//! The chunked recordings record one command list per chunk, each on its own
//! allocator, executed in the chunks' order. Throwaway, never product code.

use std::ffi::c_void;
use std::ops::Range;
use std::ptr;
use std::time::Instant;

use super::capabilities::{draw_signature, quality_levels, timestamp_frequency};
use super::ffi::*;
use super::gpu::*;
use super::shaders::Shaders;
use crate::backend::{Recording, SceneRender};
use crate::loader::c_string;
use crate::scene::{self, Draw, Encoding, Image, Scene};

const VERTEX_STRIDE: u32 = 40;
const PARAMETER_DRAW_CONSTANTS: u32 = 0;
const PARAMETER_TEXTURES: u32 = 1;
const PARAMETER_SAMPLERS: u32 = 2;

/// The sample count and quality of the 4-sample targets, and what they mean.
pub struct SamplePattern {
    pub sample: SampleDesc,
    pub description: String,
}

impl SamplePattern {
    /// The standard sample pattern where the device creates both targets
    /// with it, the driver's pattern at quality 0 otherwise.
    pub fn choose(gpu: &Gpu) -> Result<SamplePattern, String> {
        let samples = scene::SAMPLES;
        let color_levels = quality_levels(gpu, FORMAT_R8G8B8A8_UNORM_SRGB, samples)?;
        let depth_levels = quality_levels(gpu, FORMAT_D32_FLOAT, samples)?;
        if color_levels == 0 || depth_levels == 0 {
            return Err(format!("the device reports no quality level at {samples} samples (color {color_levels}, depth {depth_levels})"));
        }
        let standard = SampleDesc { count: samples, quality: STANDARD_MULTISAMPLE_PATTERN };
        let probe = (|| {
            gpu.committed(HEAP_TYPE_DEFAULT, &color_desc([scene::WIDTH, scene::HEIGHT], standard), STATE_RENDER_TARGET, Some(&clear_color(FORMAT_R8G8B8A8_UNORM_SRGB, scene::CLEAR_COLOR)), "standard pattern probe, color")?;
            gpu.committed(HEAP_TYPE_DEFAULT, &depth_desc([scene::WIDTH, scene::HEIGHT], standard), STATE_DEPTH_WRITE, Some(&clear_depth_value(scene::CLEAR_DEPTH)), "standard pattern probe, depth")
        })();
        Ok(match probe {
            Ok(_) => SamplePattern {
                sample: standard,
                description: format!("{samples} samples at the standard sample pattern (sample quality D3D12_STANDARD_MULTISAMPLE_PATTERN), {color_levels} quality levels reported for the color format"),
            },
            Err(error) => SamplePattern {
                sample: SampleDesc { count: samples, quality: 0 },
                description: format!("{samples} samples at quality 0, the driver's own pattern: the standard pattern was refused ({error})"),
            },
        })
    }
}

pub fn color_desc([width, height]: [u32; 2], sample: SampleDesc) -> ResourceDesc {
    texture_desc(width, height, 1, FORMAT_R8G8B8A8_UNORM_SRGB, sample, RESOURCE_FLAG_ALLOW_RENDER_TARGET)
}

pub fn depth_desc([width, height]: [u32; 2], sample: SampleDesc) -> ResourceDesc {
    texture_desc(width, height, 1, FORMAT_D32_FLOAT, sample, RESOURCE_FLAG_ALLOW_DEPTH_STENCIL)
}

/// A timestamp query heap of two queries and the buffer they resolve to.
struct Timestamps {
    heap: Com,
    buffer: Com,
    frequency: u64,
}

/// Everything the scene is drawn with, created once per device.
pub struct FilteringScene {
    recorders: Vec<Recorder>,
    timestamps: Option<Timestamps>,
    read_back: ReadBack,
    command_signature: Com,
    opaque: Com,
    blended: Com,
    root_signature: Com,
    samplers: DescriptorHeap,
    texture_views: DescriptorHeap,
    target_views: DescriptorHeap,
    depth_views: DescriptorHeap,
    /// Held for the views of `texture_views`.
    _textures: Vec<Com>,
    indirect_arguments: Com,
    vertex_buffer: Com,
    vertex_bytes: u32,
    /// The width and the height the targets were created with.
    target_size: [u32; 2],
    resolved: Com,
    /// Held for its view in `depth_views`.
    _depth: Com,
    color: Com,
}

/// The objects and handles one chunk is recorded with, shared by the
/// recording threads.
struct ChunkInputs {
    heaps: [Shared; 2],
    root_signature: Shared,
    opaque: Shared,
    blended: Shared,
    command_signature: Shared,
    indirect_arguments: Shared,
    vertex_buffer: Shared,
    vertex_bytes: u32,
    /// The width and the height the targets were created with.
    target_size: [u32; 2],
    textures_table: u64,
    samplers_table: u64,
    target: CpuHandle,
    depth_target: CpuHandle,
    color: Shared,
    resolved: Shared,
    read_back: Shared,
    read_back_footprint: PlacedFootprint,
    timestamps: Option<(Shared, Shared)>,
}

impl FilteringScene {
    pub fn create(gpu: &mut Gpu, shaders: &Shaders, scene: &Scene, sample: SampleDesc) -> Result<FilteringScene, String> {
        let color = gpu.committed(HEAP_TYPE_DEFAULT, &color_desc(scene.target, sample), STATE_RENDER_TARGET, Some(&clear_color(FORMAT_R8G8B8A8_UNORM_SRGB, scene::CLEAR_COLOR)), "4-sample color target")?;
        let depth = gpu.committed(HEAP_TYPE_DEFAULT, &depth_desc(scene.target, sample), STATE_DEPTH_WRITE, Some(&clear_depth_value(scene::CLEAR_DEPTH)), "4-sample depth target")?;
        let resolved_desc = texture_desc(scene.target[0], scene.target[1], 1, FORMAT_R8G8B8A8_UNORM_SRGB, ONE_SAMPLE, RESOURCE_FLAG_NONE);
        let resolved = gpu.committed(HEAP_TYPE_DEFAULT, &resolved_desc, STATE_RESOLVE_DEST, None, "resolved color")?;
        let read_back = gpu.read_back_for(&resolved_desc, "scene read-back")?;

        let target_views = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_RTV, 1, false)?;
        gpu.render_target_view(color.raw(), FORMAT_UNKNOWN, target_views.cpu(0));
        let depth_views = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_DSV, 1, false)?;
        gpu.depth_view(depth.raw(), depth_views.cpu(0));

        let vertex_bytes = scene.vertex_bytes();
        let vertex_buffer = gpu.upload_buffer(vertex_bytes, STATE_VERTEX_AND_CONSTANT_BUFFER, RESOURCE_FLAG_NONE, "scene vertices")?;
        let indirect_arguments = gpu.upload_buffer(scene.indirect_argument_bytes(), STATE_INDIRECT_ARGUMENT, RESOURCE_FLAG_NONE, "indirect arguments")?;
        let (textures, texture_views) = upload_textures(gpu, scene)?;
        let samplers = create_samplers(gpu)?;
        let root_signature = root_signature(gpu)?;
        let opaque = pipeline(gpu, &shaders.scene_vertex, &shaders.scene_pixel, root_signature.raw(), sample, false, "filtering scene")?;
        let blended = pipeline(gpu, &shaders.scene_vertex, &shaders.scene_pixel, root_signature.raw(), sample, true, "filtering scene")?;
        let command_signature = draw_signature(gpu)?;
        let timestamps = match timestamp_frequency(gpu) {
            Ok(frequency) => Some(create_timestamps(gpu, frequency)?),
            Err(_) => None,
        };
        let recorders = (0..scene.chunks.len()).map(|_| gpu.recorder()).collect::<Result<Vec<_>, String>>()?;
        Ok(FilteringScene {
            recorders,
            timestamps,
            read_back,
            command_signature,
            opaque,
            blended,
            root_signature,
            samplers,
            texture_views,
            target_views,
            depth_views,
            _textures: textures,
            indirect_arguments,
            vertex_buffer,
            vertex_bytes: vertex_bytes.len() as u32,
            target_size: scene.target,
            resolved,
            _depth: depth,
            color,
        })
    }

    fn inputs(&self) -> ChunkInputs {
        ChunkInputs {
            heaps: [Shared(self.texture_views.heap.raw()), Shared(self.samplers.heap.raw())],
            root_signature: Shared(self.root_signature.raw()),
            opaque: Shared(self.opaque.raw()),
            blended: Shared(self.blended.raw()),
            command_signature: Shared(self.command_signature.raw()),
            indirect_arguments: Shared(self.indirect_arguments.raw()),
            vertex_buffer: Shared(self.vertex_buffer.raw()),
            vertex_bytes: self.vertex_bytes,
            target_size: self.target_size,
            textures_table: self.texture_views.gpu(0),
            samplers_table: self.samplers.gpu(0),
            target: self.target_views.cpu(0),
            depth_target: self.depth_views.cpu(0),
            color: Shared(self.color.raw()),
            resolved: Shared(self.resolved.raw()),
            read_back: Shared(self.read_back.buffer.raw()),
            read_back_footprint: self.read_back.footprint,
            timestamps: self.timestamps.as_ref().map(|timestamps| (Shared(timestamps.heap.raw()), Shared(timestamps.buffer.raw()))),
        }
    }

    pub fn render(&self, gpu: &mut Gpu, scene: &Scene, recording: Recording) -> Result<SceneRender, String> {
        let inputs = self.inputs();
        let started = Instant::now();
        let lists = match recording {
            Recording::SerialOneBuffer => {
                let recorder = &self.recorders[0];
                record(recorder.allocator.raw(), recorder.list.raw(), &inputs, &scene.draws, true, true)?;
                vec![recorder.list.raw()]
            }
            Recording::ChunkedOneThread => {
                for (position, (chunk, recorder)) in scene.chunks.iter().zip(&self.recorders).enumerate() {
                    let (first, last) = chunk_ends(position, scene.chunks.len());
                    record(recorder.allocator.raw(), recorder.list.raw(), &inputs, &scene.draws[chunk.clone()], first, last)?;
                }
                self.recorders.iter().map(|recorder| recorder.list.raw()).collect()
            }
            Recording::ChunkedFourThreads => {
                record_on_threads(&self.recorders, &inputs, &scene.draws, &scene.chunks)?;
                self.recorders.iter().map(|recorder| recorder.list.raw()).collect()
            }
        };
        let recorded_us = started.elapsed().as_secs_f64() * 1_000_000.0;
        let value = gpu.execute(&lists)?;
        let submitted_us = started.elapsed().as_secs_f64() * 1_000_000.0;
        let readback_polls = gpu.poll(value)?;
        let render_us = started.elapsed().as_secs_f64() * 1_000_000.0;
        let read_started = Instant::now();
        let rgba = tight_rows(&self.read_back, 4)?;
        let readback_us = read_started.elapsed().as_secs_f64() * 1_000_000.0;
        let timestamp_ns = match &self.timestamps {
            Some(timestamps) => Some(elapsed_nanoseconds(timestamps)?),
            None => None,
        };
        Ok(SceneRender { image: Image { width: self.target_size[0], height: self.target_size[1], rgba }, recorded_us, submitted_us, render_us, readback_us, readback_polls, timestamp_ns })
    }
}

fn chunk_ends(position: usize, count: usize) -> (bool, bool) {
    (position == 0, position + 1 == count)
}

/// Records the four chunks on four threads running at once.
fn record_on_threads(recorders: &[Recorder], inputs: &ChunkInputs, draws: &[Draw], chunks: &[Range<usize>]) -> Result<(), String> {
    let results: Vec<Result<(), String>> = std::thread::scope(|threads| {
        let handles: Vec<_> = chunks
            .iter()
            .zip(recorders)
            .enumerate()
            .map(|(position, (chunk, recorder))| {
                let (allocator, list) = (Shared(recorder.allocator.raw()), Shared(recorder.list.raw()));
                let (first, last) = chunk_ends(position, chunks.len());
                let draws = &draws[chunk.clone()];
                threads.spawn(move || {
                    let (allocator, list) = (allocator, list);
                    record(allocator.0, list.0, inputs, draws, first, last)
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap_or_else(|_| Err("a recording thread panicked".to_string()))).collect()
    });
    results.into_iter().collect()
}

/// Records one chunk of draws on its own list: the state every list needs,
/// the clear and the first timestamp in the first chunk, the second
/// timestamp, the resolve and the copies after the last.
fn record(allocator: Object, list: Object, inputs: &ChunkInputs, draws: &[Draw], first: bool, last: bool) -> Result<(), String> {
    // SAFETY: the allocator and the list are live, owned by the scene and
    // recorded by this thread alone; their previous submission completed.
    unsafe {
        check(com!(allocator, slot::allocator::RESET, fn() -> Hresult), "ID3D12CommandAllocator::Reset")?;
        check(com!(list, slot::list::RESET, fn(Object, Object) -> Hresult, allocator, ptr::null_mut()), "ID3D12GraphicsCommandList::Reset")?;
    }
    set_descriptor_heaps(list, &[inputs.heaps[0].0, inputs.heaps[1].0]);
    set_graphics_root_signature(list, inputs.root_signature.0);
    set_graphics_table(list, PARAMETER_TEXTURES, inputs.textures_table);
    set_graphics_table(list, PARAMETER_SAMPLERS, inputs.samplers_table);
    set_viewport(list, 0, inputs.target_size[0], inputs.target_size[1]);
    set_render_targets(list, &[inputs.target], Some(inputs.depth_target));
    set_vertex_buffer(list, inputs.vertex_buffer.0, inputs.vertex_bytes, VERTEX_STRIDE);
    if first {
        if let Some((heap, _)) = inputs.timestamps {
            end_query(list, heap.0, 0);
        }
        clear_target(list, inputs.target, scene::CLEAR_COLOR);
        clear_depth(list, inputs.depth_target, scene::CLEAR_DEPTH);
    }
    // The pipeline is set when it changes only, as on the two other interfaces.
    let mut bound = None;
    for draw in draws {
        if bound != Some(draw.blended) {
            set_pipeline(list, if draw.blended { inputs.blended.0 } else { inputs.opaque.0 });
            bound = Some(draw.blended);
        }
        record_draw(list, inputs, draw);
    }
    if last {
        record_resolve_and_copies(list, inputs);
    }
    close(list)
}

fn record_draw(list: Object, inputs: &ChunkInputs, draw: &Draw) {
    let constants = [draw.texture, draw.sampler];
    // SAFETY: the list is recording with the root signature whose first
    // parameter holds two constants; the indirect arguments buffer is live
    // and in the indirect argument state.
    unsafe {
        com!(list, slot::list::SET_GRAPHICS_ROOT_32BIT_CONSTANTS, fn(u32, u32, *const c_void, u32) -> (), PARAMETER_DRAW_CONSTANTS, 2, constants.as_ptr() as *const c_void, 0);
        if draw.indirect {
            com!(list, slot::list::EXECUTE_INDIRECT, fn(Object, u32, Object, u64, Object, u64) -> (), inputs.command_signature.0, 1, inputs.indirect_arguments.0, 0, ptr::null_mut(), 0);
        }
    }
    if !draw.indirect {
        super::gpu::draw(list, draw.vertex_count, draw.first_vertex);
    }
}

fn record_resolve_and_copies(list: Object, inputs: &ChunkInputs) {
    let (color, resolved) = (inputs.color.0, inputs.resolved.0);
    if let Some((heap, _)) = inputs.timestamps {
        end_query(list, heap.0, 1);
    }
    barriers(list, &[transition(color, STATE_RENDER_TARGET, STATE_RESOLVE_SOURCE)]);
    // SAFETY: the list is recording; the multisampled color is a resolve
    // source and the single-sample texture of the same format a resolve
    // destination.
    unsafe { com!(list, slot::list::RESOLVE_SUBRESOURCE, fn(Object, u32, Object, u32, u32) -> (), resolved, 0, color, 0, FORMAT_R8G8B8A8_UNORM_SRGB) };
    barriers(list, &[transition(resolved, STATE_RESOLVE_DEST, STATE_COPY_SOURCE)]);
    let destination = TextureCopyLocation { resource: inputs.read_back.0, kind: TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, data: CopyLocationData { footprint: inputs.read_back_footprint } };
    let source = TextureCopyLocation { resource: resolved, kind: TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, data: CopyLocationData { subresource: 0 } };
    // SAFETY: the list is recording; both locations name live resources, the
    // read-back buffer sized by the texture's copyable footprint.
    unsafe { com!(list, slot::list::COPY_TEXTURE_REGION, fn(*const TextureCopyLocation, u32, u32, u32, *const TextureCopyLocation, *const c_void) -> (), &destination, 0, 0, 0, &source, ptr::null()) };
    barriers(list, &[transition(resolved, STATE_COPY_SOURCE, STATE_RESOLVE_DEST), transition(color, STATE_RESOLVE_SOURCE, STATE_RENDER_TARGET)]);
    if let Some((heap, buffer)) = inputs.timestamps {
        // SAFETY: the list is recording; the heap holds two timestamp
        // queries and the buffer sixteen bytes in the copy destination state.
        unsafe { com!(list, slot::list::RESOLVE_QUERY_DATA, fn(Object, u32, u32, u32, Object, u64) -> (), heap.0, QUERY_TYPE_TIMESTAMP, 0, 2, buffer.0, 0) };
    }
}

fn end_query(list: Object, heap: Object, index: u32) {
    // SAFETY: the list is recording; the heap is a live timestamp query heap
    // of two queries.
    unsafe { com!(list, slot::list::END_QUERY, fn(Object, u32, u32) -> (), heap, QUERY_TYPE_TIMESTAMP, index) };
}

fn elapsed_nanoseconds(timestamps: &Timestamps) -> Result<u64, String> {
    let bytes = read_buffer(timestamps.buffer.raw(), 16)?;
    let tick = |offset: usize| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap_or([0; 8]));
    let ticks = tick(8).saturating_sub(tick(0));
    Ok((u128::from(ticks) * 1_000_000_000 / u128::from(timestamps.frequency.max(1))) as u64)
}

fn create_timestamps(gpu: &Gpu, frequency: u64) -> Result<Timestamps, String> {
    let desc = QueryHeapDesc { kind: QUERY_HEAP_TYPE_TIMESTAMP, count: 2, node_mask: 0 };
    let mut heap: Object = ptr::null_mut();
    // SAFETY: the device is live; the call writes an ID3D12QueryHeap.
    let result = unsafe { com!(gpu.raw(), slot::device::CREATE_QUERY_HEAP, fn(*const QueryHeapDesc, *const Guid, *mut Object) -> Hresult, &desc, &IID_QUERY_HEAP, &mut heap) };
    let heap = created(result, heap, "ID3D12Device::CreateQueryHeap (timestamps)")?;
    let buffer = gpu.buffer(HEAP_TYPE_READBACK, 16, RESOURCE_FLAG_NONE, "timestamps read-back")?;
    Ok(Timestamps { heap, buffer, frequency })
}

/// The five textures, every level uploaded, and their table of views.
fn upload_textures(gpu: &mut Gpu, scene: &Scene) -> Result<(Vec<Com>, DescriptorHeap), String> {
    let views = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, scene::TEXTURE_COUNT as u32, true)?;
    let mut textures = Vec::with_capacity(scene.textures.len());
    for (index, texture) in scene.textures.iter().enumerate() {
        let format = match texture.encoding {
            Encoding::Srgb => FORMAT_R8G8B8A8_UNORM_SRGB,
            Encoding::Unorm => FORMAT_R8G8B8A8_UNORM,
        };
        let levels = texture.levels.len() as u16;
        let desc = texture_desc(texture.width, texture.height, levels, format, ONE_SAMPLE, RESOURCE_FLAG_NONE);
        let uploaded = gpu.upload_texture(&desc, &texture.levels, 4, STATE_PIXEL_SHADER_RESOURCE, &format!("texture {}", texture.name))?;
        gpu.texture_view(uploaded.raw(), format, u32::from(levels), views.cpu(index as u32));
        textures.push(uploaded);
    }
    Ok((textures, views))
}

fn sampler(filter: u32, address: u32) -> SamplerDesc {
    SamplerDesc {
        filter,
        address_u: address,
        address_v: address,
        address_w: address,
        mip_lod_bias: 0.0,
        max_anisotropy: 1,
        comparison: COMPARISON_NEVER,
        border_color: [0.0; 4],
        min_lod: 0.0,
        max_lod: FLOAT32_MAX,
    }
}

/// Nearest (clamp), linear with the nearest level (clamp) and trilinear
/// (repeat), at the indexes the scene names them by.
pub fn create_samplers(gpu: &Gpu) -> Result<DescriptorHeap, String> {
    let heap = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_SAMPLER, 3, true)?;
    let descs = [
        (scene::SAMPLER_NEAREST, sampler(FILTER_MIN_MAG_MIP_POINT, ADDRESS_MODE_CLAMP)),
        (scene::SAMPLER_LINEAR, sampler(FILTER_MIN_MAG_LINEAR_MIP_POINT, ADDRESS_MODE_CLAMP)),
        (scene::SAMPLER_TRILINEAR, sampler(FILTER_MIN_MAG_MIP_LINEAR, ADDRESS_MODE_WRAP)),
    ];
    for (index, desc) in &descs {
        // SAFETY: the device is live; the handle is a slot of the sampler heap.
        unsafe { com!(gpu.raw(), slot::device::CREATE_SAMPLER, fn(*const SamplerDesc, CpuHandle) -> (), desc, heap.cpu(*index)) };
    }
    Ok(heap)
}

fn root_signature(gpu: &Gpu) -> Result<Com, String> {
    let textures = [DescriptorRange { kind: DESCRIPTOR_RANGE_TYPE_SRV, count: scene::TEXTURE_COUNT as u32, base_register: 0, space: 0, offset_from_table_start: 0 }];
    let samplers = [DescriptorRange { kind: DESCRIPTOR_RANGE_TYPE_SAMPLER, count: 3, base_register: 0, space: 0, offset_from_table_start: 0 }];
    let parameters = [
        RootParameter::constants(0, 2, SHADER_VISIBILITY_PIXEL),
        RootParameter::table(&textures, SHADER_VISIBILITY_PIXEL),
        RootParameter::table(&samplers, SHADER_VISIBILITY_PIXEL),
    ];
    gpu.root_signature(&parameters, &[], ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, "filtering scene")
}

/// The opaque or the blended pipeline of the scene's vertex layout and
/// targets, from the bytecode of the two stages; `what` names it in an error.
pub fn pipeline(gpu: &Gpu, vertex: &[u8], pixel: &[u8], root_signature: Object, sample: SampleDesc, blended: bool, what: &str) -> Result<Com, String> {
    let (position, uv, color) = (c_string("POSITION"), c_string("TEXCOORD"), c_string("COLOR"));
    let elements = [input_element(&position, FORMAT_R32G32B32A32_FLOAT, 0), input_element(&uv, FORMAT_R32G32_FLOAT, 16), input_element(&color, FORMAT_R32G32B32A32_FLOAT, 24)];
    let mut desc = default_graphics_desc(root_signature, vertex, pixel);
    desc.input_layout = InputLayoutDesc { elements: elements.as_ptr(), count: elements.len() as u32 };
    set_targets(&mut desc, &[FORMAT_R8G8B8A8_UNORM_SRGB]);
    set_depth(&mut desc, !blended);
    desc.sample = sample;
    desc.rasterizer.multisample_enable = 1;
    if blended {
        let target = &mut desc.blend.targets[0];
        target.blend_enable = 1;
        target.source_blend = BLEND_SRC_ALPHA;
        target.destination_blend = BLEND_INV_SRC_ALPHA;
        target.blend_operation = BLEND_OP_ADD;
        target.source_blend_alpha = BLEND_ONE;
        target.destination_blend_alpha = BLEND_INV_SRC_ALPHA;
        target.blend_operation_alpha = BLEND_OP_ADD;
    }
    gpu.graphics_pipeline(&desc, &format!("{what}, {}", if blended { "blended" } else { "opaque" }))
}
