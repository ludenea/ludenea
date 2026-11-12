//! Frames on Vulkan: the adapter of [`crate::frames::FrameDevice`]. One
//! render pass serves every target (a 4-sample color attachment, a 4-sample
//! depth attachment and the resolve attachment that is the target's image);
//! the pipelines have their viewport and their scissor set while recording,
//! so that one pair draws to targets of any size; a pass owns one descriptor
//! set for its five textures and the device's three samplers, one primary
//! command buffer, and one secondary command buffer per part, each from its
//! own command pool; a submission hands the passes' primary command buffers
//! to the queue, then one more that copies the targets read back, under one
//! fence that is polled.
//!
//! A target's image rests in the shader-read-only layout. The render pass
//! leaves it there after the resolve, the copy of a read-back moves it to the
//! transfer source layout and back, and a new target is moved there when it
//! is created, so that any pass samples it and any submission copies it
//! without knowing what came before.
//!
//! Vulkan wants the queue, each command pool and each descriptor pool used by
//! one thread at a time. Every submission and the wait for an idle device
//! take the queue's lock, and each pool belongs to one part, one pass or one
//! submission, which the contract gives to one thread at a time. No registry
//! lock is held while a command buffer is recorded. Throwaway, never product
//! code.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::ffi::*;
use super::filtering::{alpha_blend, samplers};
use super::gpu::*;
use crate::backend::DeviceInfo;
use crate::frames::{DRAW_CONSTANT_BYTES, FrameDevice, FrameDraw, PassDescription, Resource, ShaderPayload, TextureSource};
use crate::scene::{CLEAR_DEPTH, Encoding, Image, SAMPLES, TEXTURE_COUNT, Texture};

/// Position, uv and color: 40 bytes.
const VERTEX_STRIDE: u32 = 40;
const COLOR_FORMAT: u32 = FORMAT_R8G8B8A8_SRGB;
const DEPTH_FORMAT: u32 = FORMAT_D32_SFLOAT;
/// The stages that write a target's three attachments.
const ATTACHMENT_STAGES: u32 = PIPELINE_STAGE_EARLY_FRAGMENT_TESTS | PIPELINE_STAGE_LATE_FRAGMENT_TESTS | PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT;
/// How a target's image is used while it rests in the shader-read-only
/// layout: sampled by a pass, or about to be copied for a read-back.
const RESTING_STAGES: u32 = PIPELINE_STAGE_FRAGMENT_SHADER | PIPELINE_STAGE_TRANSFER;
const RESTING_ACCESSES: u32 = ACCESS_SHADER_READ | ACCESS_TRANSFER_READ;
/// The stages that read a draw's constants.
const CONSTANT_STAGES: u32 = STAGE_VERTEX | STAGE_FRAGMENT;
/// A pass's descriptor set: its five textures, then the three samplers.
const SET_BINDINGS: [(u32, u32, u32); 2] = [(DESCRIPTOR_SAMPLED_IMAGE, TEXTURE_COUNT as u32, STAGE_FRAGMENT), (DESCRIPTOR_SAMPLER, 3, STAGE_FRAGMENT)];

/// Numbers the modules the compiler writes, so that two imports at once, on
/// one device or on two, never share a file.
static MODULE_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Where the frame shader's sources are and what compiles them.
#[derive(Clone)]
pub struct ShaderSources {
    /// The command of the GLSL compiler.
    pub compiler: String,
    pub directory: PathBuf,
    /// Where the compiler writes a module before it is read.
    pub work: PathBuf,
}

struct TextureEntry {
    view: Object,
    kept: Kept,
}

struct VertexBufferEntry {
    buffer: Object,
    kept: Kept,
}

struct TargetEntry {
    /// The single-sample image the passes resolve to.
    image: Object,
    view: Object,
    framebuffer: Object,
    extent: [u32; 2],
    kept: Kept,
}

struct PipelinesEntry {
    opaque: Object,
    blended: Object,
    kept: Kept,
}

/// What the recording of a pass names, copied out of the registries when the
/// pass is opened.
#[derive(Clone, Copy)]
struct PassState {
    framebuffer: Object,
    extent: [u32; 2],
    set: Object,
    opaque: Object,
    blended: Object,
}

struct Part {
    command_buffer: CommandBuffer,
    recorded: bool,
}

struct Pass {
    state: PassState,
    clear: [f32; 4],
    primary: CommandBuffer,
    parts: Vec<Part>,
    closed: bool,
    /// The descriptor pool and the command pools.
    kept: Kept,
}

struct ReadBack {
    buffer: HostBuffer,
    extent: [u32; 2],
}

struct Submission {
    fence: Object,
    read_backs: Vec<ReadBack>,
    /// The fence, the command pool of the copies and the read-back buffers,
    /// then what each pass owned.
    kept: Vec<Kept>,
}

/// One device open for frames.
pub struct VulkanFrames {
    info: DeviceInfo,
    sources: ShaderSources,
    /// The render pass every target and every pair of pipelines is made for.
    render_pass: Object,
    set_layout: Object,
    pipeline_layout: Object,
    samplers: [Object; 3],
    /// The render pass, the layouts and the samplers.
    kept: Kept,
    next_identifier: AtomicU64,
    /// Held by whichever thread uses the queue.
    queue: Mutex<()>,
    textures: Mutex<HashMap<u64, TextureEntry>>,
    vertex_buffers: Mutex<HashMap<u64, VertexBufferEntry>>,
    targets: Mutex<HashMap<u64, TargetEntry>>,
    pipelines: Mutex<HashMap<u64, PipelinesEntry>>,
    passes: Mutex<HashMap<u64, Pass>>,
    submissions: Mutex<HashMap<u64, Submission>>,
    /// Declared last: dropped once `Drop` has destroyed what was created from it.
    gpu: Gpu,
}

// SAFETY: a Vulkan handle belongs to no thread. What Vulkan wants used by one
// thread at a time is the queue, taken under `queue` by every submission and
// by the wait for an idle device; each command pool and each descriptor pool,
// which belongs to one part, one pass or one submission, given to one thread
// at a time by the contract of `FrameDevice`; and the mapped memory of a host
// buffer, written before the buffer is registered and read by the one thread
// that takes its submission. The rest is behind the registries' locks or is
// never written after `open`.
unsafe impl Send for VulkanFrames {}
// SAFETY: as above.
unsafe impl Sync for VulkanFrames {}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Removes an entry from a registry, whose lock is released on return.
fn take<T>(registry: &Mutex<HashMap<u64, T>>, identifier: u64) -> Option<T> {
    lock(registry).remove(&identifier)
}

/// The words of a SPIR-V module given as bytes.
fn spirv_words(bytes: &[u8], stage: &str) -> Result<Vec<u32>, String> {
    if bytes.len() % 4 != 0 || bytes.len() < 20 {
        return Err(format!("the {stage} stage of the payload is not a SPIR-V module ({} bytes)", bytes.len()));
    }
    Ok(bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect())
}

/// The render pass of every target: clear, draw at 4 samples, resolve.
fn render_pass(scope: &mut Scope) -> Result<Object, String> {
    let attachments = [
        attachment(COLOR_FORMAT, SAMPLES, LOAD_OP_CLEAR, STORE_OP_DONT_CARE, (LAYOUT_UNDEFINED, LAYOUT_COLOR_ATTACHMENT_OPTIMAL)),
        attachment(DEPTH_FORMAT, SAMPLES, LOAD_OP_CLEAR, STORE_OP_DONT_CARE, (LAYOUT_UNDEFINED, LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL)),
        attachment(COLOR_FORMAT, SAMPLE_COUNT_1, LOAD_OP_DONT_CARE, STORE_OP_STORE, (LAYOUT_UNDEFINED, LAYOUT_SHADER_READ_ONLY_OPTIMAL)),
    ];
    // The pass starts after whatever used its attachments before it on the
    // queue: the last pass of the same target, which wrote the three, and
    // the passes and the copies that read the target's image at rest.
    let after_earlier_uses = SubpassDependency {
        src_subpass: SUBPASS_EXTERNAL,
        dst_subpass: 0,
        src_stage_mask: ATTACHMENT_STAGES | RESTING_STAGES,
        dst_stage_mask: ATTACHMENT_STAGES,
        src_access_mask: ACCESS_COLOR_ATTACHMENT_WRITE | ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE,
        dst_access_mask: ACCESS_COLOR_ATTACHMENT_READ | ACCESS_COLOR_ATTACHMENT_WRITE | ACCESS_DEPTH_STENCIL_ATTACHMENT_READ | ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE,
        dependency_flags: 0,
    };
    scope.render_pass(&attachments, &[0], &[2], Some(1), &[after_earlier_uses, after_color_writes(RESTING_STAGES, RESTING_ACCESSES)])
}

impl VulkanFrames {
    /// Creates what every frame of the device shares: the samplers, the
    /// layouts and the render pass.
    pub fn open(gpu: Gpu, info: DeviceInfo, sources: ShaderSources) -> Result<VulkanFrames, String> {
        if gpu.core_features[feature::SHADER_SAMPLED_IMAGE_ARRAY_DYNAMIC_INDEXING] == 0 {
            return Err("the device does not offer shaderSampledImageArrayDynamicIndexing, which indexing the array of five textures by a draw's constant needs".to_string());
        }
        let queue = Mutex::new(());
        let mut scope = Scope::on_shared_device(&gpu, &queue);
        let samplers = samplers(&mut scope)?;
        let set_layout = scope.descriptor_set_layout(&SET_BINDINGS)?;
        let constants = [PushConstantRange { stage_flags: CONSTANT_STAGES, offset: 0, size: DRAW_CONSTANT_BYTES as u32 }];
        let pipeline_layout = scope.pipeline_layout(&[set_layout], &constants)?;
        let render_pass = render_pass(&mut scope)?;
        let kept = scope.keep();
        Ok(VulkanFrames {
            info,
            sources,
            render_pass,
            set_layout,
            pipeline_layout,
            samplers,
            kept,
            next_identifier: AtomicU64::new(1),
            queue,
            textures: Mutex::default(),
            vertex_buffers: Mutex::default(),
            targets: Mutex::default(),
            pipelines: Mutex::default(),
            passes: Mutex::default(),
            submissions: Mutex::default(),
            gpu,
        })
    }

    /// A scope for what one call creates: kept when the call succeeds,
    /// destroyed at once when it fails.
    fn scope(&self) -> Scope<'_> {
        Scope::on_shared_device(&self.gpu, &self.queue)
    }

    fn identifier(&self) -> u64 {
        self.next_identifier.fetch_add(1, Ordering::Relaxed)
    }

    /// The view each of a pass's five sources is sampled through.
    fn source_views(&self, sources: &[TextureSource; TEXTURE_COUNT]) -> Result<[Object; TEXTURE_COUNT], String> {
        let mut views = [0; TEXTURE_COUNT];
        for (view, source) in views.iter_mut().zip(sources) {
            *view = match *source {
                TextureSource::Texture(texture) => lock(&self.textures).get(&texture).map(|entry| entry.view).ok_or_else(|| format!("no texture {texture}"))?,
                TextureSource::Target(target) => lock(&self.targets).get(&target).map(|entry| entry.view).ok_or_else(|| format!("no target {target}"))?,
            };
        }
        Ok(views)
    }

    /// The buffer of each draw, looked up once for each run of draws that
    /// name the same one.
    fn vertex_buffers_of(&self, draws: &[FrameDraw]) -> Result<Vec<Object>, String> {
        let registry = lock(&self.vertex_buffers);
        let mut buffers = Vec::with_capacity(draws.len());
        let mut last: Option<(u64, Object)> = None;
        for draw in draws {
            let buffer = match last {
                Some((identifier, buffer)) if identifier == draw.vertex_buffer => buffer,
                _ => registry.get(&draw.vertex_buffer).map(|entry| entry.buffer).ok_or_else(|| format!("no vertex buffer {}", draw.vertex_buffer))?,
            };
            last = Some((draw.vertex_buffer, buffer));
            buffers.push(buffer);
        }
        Ok(buffers)
    }

    /// Records one part into its secondary command buffer, which continues
    /// the render pass of the pass's target.
    fn record_draws(&self, command_buffer: Handle, state: &PassState, draws: &[FrameDraw], buffers: &[Object]) -> Result<(), String> {
        let functions = &self.gpu.functions;
        let inheritance = CommandBufferInheritanceInfo {
            s_type: structure::COMMAND_BUFFER_INHERITANCE_INFO,
            p_next: ptr::null(),
            render_pass: self.render_pass,
            subpass: 0,
            framebuffer: state.framebuffer,
            occlusion_query_enable: 0,
            query_flags: 0,
            pipeline_statistics: 0,
        };
        begin(functions, command_buffer, COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT | COMMAND_BUFFER_USAGE_RENDER_PASS_CONTINUE, Some(&inheritance))?;
        let viewport = full_viewport(state.extent[0], state.extent[1]);
        let scissor = Rect2D { offset: [0, 0], extent: state.extent };
        let offset = 0_u64;
        // SAFETY: the command buffer is recording and continues the render
        // pass; its pool is used by this thread alone; every object belongs
        // to the device and the pointers live for each call. A secondary
        // command buffer inherits no viewport, so each part sets its own.
        unsafe {
            (functions.cmd_set_viewport)(command_buffer, 0, 1, &viewport);
            (functions.cmd_set_scissor)(command_buffer, 0, 1, &scissor);
            (functions.cmd_bind_descriptor_sets)(command_buffer, BIND_POINT_GRAPHICS, self.pipeline_layout, 0, 1, &state.set, 0, ptr::null());
            let mut bound_blended = None;
            let mut bound_buffer = None;
            for (draw, buffer) in draws.iter().zip(buffers) {
                if bound_blended != Some(draw.blended) {
                    (functions.cmd_bind_pipeline)(command_buffer, BIND_POINT_GRAPHICS, if draw.blended { state.blended } else { state.opaque });
                    bound_blended = Some(draw.blended);
                }
                if bound_buffer != Some(*buffer) {
                    (functions.cmd_bind_vertex_buffers)(command_buffer, 0, 1, buffer, &offset);
                    bound_buffer = Some(*buffer);
                }
                let constants = draw.constants.bytes();
                (functions.cmd_push_constants)(command_buffer, self.pipeline_layout, CONSTANT_STAGES, 0, DRAW_CONSTANT_BYTES as u32, constants.as_ptr() as *const c_void);
                (functions.cmd_draw)(command_buffer, draw.vertex_count, 1, draw.first_vertex, 0);
            }
        }
        end(functions, command_buffer)
    }

    /// Records a pass's primary command buffer: the render pass with its
    /// clear, the parts executed in their order, the resolve at its end.
    fn record_primary(&self, primary: Handle, state: &PassState, clear: [f32; 4], parts: &[Handle]) -> Result<(), String> {
        let functions = &self.gpu.functions;
        begin(functions, primary, COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT, None)?;
        let clears = [clear, [CLEAR_DEPTH, 0.0, 0.0, 0.0], [0.0; 4]];
        let contents = if parts.is_empty() { SUBPASS_CONTENTS_INLINE } else { SUBPASS_CONTENTS_SECONDARY_COMMAND_BUFFERS };
        begin_render_pass(functions, primary, self.render_pass, state.framebuffer, state.extent, &clears, contents);
        // SAFETY: the primary command buffer is recording inside the render
        // pass and its pool is used by this thread alone; every part has been
        // recorded and ended, and was begun for this render pass and framebuffer.
        unsafe {
            if !parts.is_empty() {
                (functions.cmd_execute_commands)(primary, parts.len() as u32, parts.as_ptr());
            }
            (functions.cmd_end_render_pass)(primary);
        }
        end(functions, primary)
    }

    /// Records the copy of each target read back into a buffer of its own:
    /// the image leaves its resting layout for the copy and returns to it.
    fn record_copies(&self, scope: &mut Scope, command_buffer: Handle, targets: &[(Object, [u32; 2])]) -> Result<Vec<ReadBack>, String> {
        let functions = &self.gpu.functions;
        begin(functions, command_buffer, COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT, None)?;
        let mut read_backs = Vec::with_capacity(targets.len());
        for &(image, extent) in targets {
            let buffer = scope.host_buffer(extent[0] as usize * extent[1] as usize * 4, BUFFER_USAGE_TRANSFER_DST)?;
            image_barrier(functions, command_buffer, image, ASPECT_COLOR, 1, (LAYOUT_SHADER_READ_ONLY_OPTIMAL, LAYOUT_TRANSFER_SRC_OPTIMAL), (ACCESS_NONE, ACCESS_TRANSFER_READ), (RESTING_STAGES, PIPELINE_STAGE_TRANSFER));
            copy_to_host(functions, command_buffer, image, extent[0], extent[1], buffer.buffer);
            image_barrier(functions, command_buffer, image, ASPECT_COLOR, 1, (LAYOUT_TRANSFER_SRC_OPTIMAL, LAYOUT_SHADER_READ_ONLY_OPTIMAL), (ACCESS_NONE, RESTING_ACCESSES), (PIPELINE_STAGE_TRANSFER, RESTING_STAGES));
            read_backs.push(ReadBack { buffer, extent });
        }
        end(functions, command_buffer)?;
        Ok(read_backs)
    }

    /// Hands command buffers to the queue in their order, under one fence.
    fn submit_to_queue(&self, command_buffers: &[Handle], fence: Object) -> Result<(), String> {
        let submit = SubmitInfo {
            s_type: structure::SUBMIT_INFO,
            p_next: ptr::null(),
            wait_semaphore_count: 0,
            wait_semaphores: ptr::null(),
            wait_dst_stage_mask: ptr::null(),
            command_buffer_count: command_buffers.len() as u32,
            command_buffers: command_buffers.as_ptr(),
            signal_semaphore_count: 0,
            signal_semaphores: ptr::null(),
        };
        let _queue = lock(&self.queue);
        // SAFETY: every command buffer has been ended, the fence is
        // unsignaled, and the lock makes this thread the only user of the queue.
        check(unsafe { (self.gpu.functions.queue_submit)(self.gpu.queue, 1, &submit, fence) }, "vkQueueSubmit")
    }

    fn signaled(&self, fence: Object) -> Result<bool, String> {
        // SAFETY: the fence belongs to this device; the call does not block.
        match unsafe { (self.gpu.functions.get_fence_status)(self.gpu.device, fence) } {
            SUCCESS => Ok(true),
            NOT_READY => Ok(false),
            other => check(other, "vkGetFenceStatus").map(|()| false),
        }
    }
}

impl FrameDevice for VulkanFrames {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn settings(&self) -> String {
        super::settings_of(&self.gpu)
    }

    fn import_shader(&self, variant: u32) -> Result<ShaderPayload, String> {
        if variant > 1 {
            return Err(format!("the frame shader has the variants 0 and 1, not {variant}"));
        }
        let serial = MODULE_SERIAL.fetch_add(1, Ordering::Relaxed);
        let stage = |file: &str, stage: &str, defines: &[&str]| -> Result<Vec<u8>, String> {
            let words = super::compile_glsl(&self.sources.compiler, &self.sources.directory.join(file), &self.sources.work, &format!("frame-{stage}-{serial}"), defines)?;
            Ok(words.iter().flat_map(|word| word.to_le_bytes()).collect())
        };
        let variant = format!("TINT_VARIANT={variant}");
        Ok(ShaderPayload { vertex: stage("frame.vert", "vertex", &[])?, fragment: stage("frame.frag", "fragment", &[&variant])?, form: "SPIR-V, compiled from GLSL by the glslang command-line tool" })
    }

    fn create_pipelines(&self, payload: &ShaderPayload) -> Result<u64, String> {
        let mut scope = self.scope();
        let vertex = scope.shader_module(&spirv_words(&payload.vertex, "vertex")?)?;
        let fragment = scope.shader_module(&spirv_words(&payload.fragment, "fragment")?)?;
        let attributes = [
            VertexInputAttributeDescription { location: 0, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 0 },
            VertexInputAttributeDescription { location: 1, binding: 0, format: FORMAT_R32G32_SFLOAT, offset: 16 },
            VertexInputAttributeDescription { location: 2, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 24 },
        ];
        let mut pipeline = |depth_write: bool, blend: PipelineColorBlendAttachmentState| {
            scope.graphics_pipeline_of_any_size(&GraphicsPipelineDescription {
                vertex,
                fragment,
                vertex_stride: VERTEX_STRIDE,
                attributes: &attributes,
                viewport: full_viewport(0, 0),
                samples: SAMPLES,
                depth_write: Some(depth_write),
                blends: &[blend],
                layout: self.pipeline_layout,
                render_pass: self.render_pass,
            })
        };
        let opaque = pipeline(true, opaque_blend())?;
        let blended = pipeline(false, alpha_blend())?;
        let identifier = self.identifier();
        lock(&self.pipelines).insert(identifier, PipelinesEntry { opaque, blended, kept: scope.keep() });
        Ok(identifier)
    }

    fn create_texture(&self, texture: &Texture) -> Result<u64, String> {
        if texture.levels.is_empty() {
            return Err(format!("the texture {} has no level", texture.name));
        }
        let format = match texture.encoding {
            Encoding::Srgb => FORMAT_R8G8B8A8_SRGB,
            Encoding::Unorm => FORMAT_R8G8B8A8_UNORM,
        };
        let levels = texture.levels.len() as u32;
        let mut scope = self.scope();
        let image = scope.image(&image_info(format, texture.width, texture.height, levels, SAMPLE_COUNT_1, IMAGE_USAGE_SAMPLED | IMAGE_USAGE_TRANSFER_DST, 0))?;
        // The staging buffer and the command buffer of the upload live in a
        // scope of their own, which ends once the device has finished it.
        self.scope().upload_images(&[ImageUpload { image, width: texture.width, height: texture.height, levels: texture.levels.iter().map(Vec::as_slice).collect() }])?;
        let view = scope.image_view(image, format, ASPECT_COLOR, levels)?;
        let identifier = self.identifier();
        lock(&self.textures).insert(identifier, TextureEntry { view, kept: scope.keep() });
        Ok(identifier)
    }

    fn create_vertex_buffer(&self, bytes: &[u8]) -> Result<u64, String> {
        let mut scope = self.scope();
        let buffer = scope.buffer_with(bytes, BUFFER_USAGE_VERTEX)?.buffer;
        let identifier = self.identifier();
        lock(&self.vertex_buffers).insert(identifier, VertexBufferEntry { buffer, kept: scope.keep() });
        Ok(identifier)
    }

    fn create_target(&self, width: u32, height: u32) -> Result<u64, String> {
        let mut scope = self.scope();
        let color = scope.image(&image_info(COLOR_FORMAT, width, height, 1, SAMPLES, IMAGE_USAGE_COLOR_ATTACHMENT, 0))?;
        let depth = scope.image(&image_info(DEPTH_FORMAT, width, height, 1, SAMPLES, IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT, 0))?;
        let image = scope.image(&image_info(COLOR_FORMAT, width, height, 1, SAMPLE_COUNT_1, IMAGE_USAGE_COLOR_ATTACHMENT | IMAGE_USAGE_SAMPLED | IMAGE_USAGE_TRANSFER_SRC, 0))?;
        let view = scope.image_view(image, COLOR_FORMAT, ASPECT_COLOR, 1)?;
        let views = [scope.image_view(color, COLOR_FORMAT, ASPECT_COLOR, 1)?, scope.image_view(depth, DEPTH_FORMAT, ASPECT_DEPTH, 1)?, view];
        let framebuffer = scope.framebuffer(self.render_pass, &views, width, height)?;
        // A target no pass has rendered to yet is put where a pass that
        // samples it and a submission that copies it expect it.
        self.scope().run_once(|functions, command_buffer| {
            image_barrier(functions, command_buffer, image, ASPECT_COLOR, 1, (LAYOUT_UNDEFINED, LAYOUT_SHADER_READ_ONLY_OPTIMAL), (ACCESS_NONE, RESTING_ACCESSES), (PIPELINE_STAGE_TOP_OF_PIPE, RESTING_STAGES));
        })?;
        let identifier = self.identifier();
        lock(&self.targets).insert(identifier, TargetEntry { image, view, framebuffer, extent: [width, height], kept: scope.keep() });
        Ok(identifier)
    }

    fn destroy(&self, resource: Resource) {
        let kept = match resource {
            Resource::Texture(identifier) => take(&self.textures, identifier).map(|entry| entry.kept),
            Resource::VertexBuffer(identifier) => take(&self.vertex_buffers, identifier).map(|entry| entry.kept),
            Resource::Target(identifier) => take(&self.targets, identifier).map(|entry| entry.kept),
            Resource::Pipelines(identifier) => take(&self.pipelines, identifier).map(|entry| entry.kept),
        };
        if let Some(kept) = kept {
            kept.destroy(&self.gpu);
        }
    }

    fn open_pass(&self, pass: &PassDescription) -> Result<u64, String> {
        let (framebuffer, extent) = lock(&self.targets).get(&pass.target).map(|target| (target.framebuffer, target.extent)).ok_or_else(|| format!("no target {}", pass.target))?;
        let (opaque, blended) = lock(&self.pipelines).get(&pass.pipelines).map(|pipelines| (pipelines.opaque, pipelines.blended)).ok_or_else(|| format!("no pipelines {}", pass.pipelines))?;
        let views = self.source_views(&pass.textures)?;
        let mut scope = self.scope();
        let set = scope.descriptor_set(self.set_layout, &SET_BINDINGS)?;
        scope.write_images(set, 0, DESCRIPTOR_SAMPLED_IMAGE, &views.map(|image_view| DescriptorImageInfo { sampler: 0, image_view, image_layout: LAYOUT_SHADER_READ_ONLY_OPTIMAL }));
        scope.write_images(set, 1, DESCRIPTOR_SAMPLER, &self.samplers.map(|sampler| DescriptorImageInfo { sampler, image_view: 0, image_layout: LAYOUT_UNDEFINED }));
        let primary_pool = scope.command_pool()?;
        let primary = scope.command_buffer(primary_pool, COMMAND_BUFFER_LEVEL_PRIMARY)?;
        let mut parts = Vec::with_capacity(pass.parts);
        for _ in 0..pass.parts {
            let pool = scope.command_pool()?;
            parts.push(Part { command_buffer: scope.command_buffer(pool, COMMAND_BUFFER_LEVEL_SECONDARY)?, recorded: false });
        }
        let state = PassState { framebuffer, extent, set, opaque, blended };
        let identifier = self.identifier();
        lock(&self.passes).insert(identifier, Pass { state, clear: pass.clear, primary, parts, closed: false, kept: scope.keep() });
        Ok(identifier)
    }

    fn record_part(&self, pass: u64, part: usize, draws: &[FrameDraw]) -> Result<(), String> {
        let (state, command_buffer) = {
            let passes = lock(&self.passes);
            let entry = passes.get(&pass).ok_or_else(|| format!("no pass {pass}"))?;
            if entry.closed {
                return Err(format!("the pass {pass} is closed"));
            }
            let slot = entry.parts.get(part).ok_or_else(|| format!("the pass {pass} has {} parts, none at index {part}", entry.parts.len()))?;
            if slot.recorded {
                return Err(format!("the part {part} of the pass {pass} is already recorded"));
            }
            (entry.state, slot.command_buffer)
        };
        let buffers = self.vertex_buffers_of(draws)?;
        self.record_draws(command_buffer.0, &state, draws, &buffers)?;
        let mut passes = lock(&self.passes);
        let entry = passes.get_mut(&pass).ok_or_else(|| format!("the pass {pass} was discarded while its part {part} was recorded"))?;
        entry.parts[part].recorded = true;
        Ok(())
    }

    fn close_pass(&self, pass: u64) -> Result<(), String> {
        let (state, clear, primary, parts) = {
            let passes = lock(&self.passes);
            let entry = passes.get(&pass).ok_or_else(|| format!("no pass {pass}"))?;
            if entry.closed {
                return Err(format!("the pass {pass} is already closed"));
            }
            if let Some(missing) = entry.parts.iter().position(|part| !part.recorded) {
                return Err(format!("the part {missing} of the pass {pass} is not recorded"));
            }
            (entry.state, entry.clear, entry.primary, entry.parts.iter().map(|part| part.command_buffer.0).collect::<Vec<Handle>>())
        };
        self.record_primary(primary.0, &state, clear, &parts)?;
        let mut passes = lock(&self.passes);
        let entry = passes.get_mut(&pass).ok_or_else(|| format!("the pass {pass} was discarded while it was closed"))?;
        entry.closed = true;
        Ok(())
    }

    fn discard_pass(&self, pass: u64) {
        if let Some(entry) = take(&self.passes, pass) {
            entry.kept.destroy(&self.gpu);
        }
    }

    fn submit(&self, passes: &[u64], read_back: &[u64]) -> Result<u64, String> {
        let mut command_buffers: Vec<Handle> = Vec::with_capacity(passes.len() + 1);
        {
            let registry = lock(&self.passes);
            for (index, pass) in passes.iter().enumerate() {
                let entry = registry.get(pass).ok_or_else(|| format!("no pass {pass}"))?;
                if !entry.closed {
                    return Err(format!("the pass {pass} is not closed"));
                }
                if passes[..index].contains(pass) {
                    return Err(format!("the pass {pass} is named twice"));
                }
                command_buffers.push(entry.primary.0);
            }
        }
        let mut targets = Vec::with_capacity(read_back.len());
        {
            let registry = lock(&self.targets);
            for target in read_back {
                targets.push(registry.get(target).map(|entry| (entry.image, entry.extent)).ok_or_else(|| format!("no target {target}"))?);
            }
        }
        let mut scope = self.scope();
        let fence = scope.fence()?;
        let mut read_backs = Vec::new();
        if !targets.is_empty() {
            let pool = scope.command_pool()?;
            let copies = scope.command_buffer(pool, COMMAND_BUFFER_LEVEL_PRIMARY)?.0;
            read_backs = self.record_copies(&mut scope, copies, &targets)?;
            command_buffers.push(copies);
        }
        self.submit_to_queue(&command_buffers, fence)?;
        // The queue has the passes: what they own now belongs to the submission.
        let mut kept = vec![scope.keep()];
        {
            let mut registry = lock(&self.passes);
            kept.extend(passes.iter().filter_map(|pass| registry.remove(pass)).map(|entry| entry.kept));
        }
        let identifier = self.identifier();
        lock(&self.submissions).insert(identifier, Submission { fence, read_backs, kept });
        Ok(identifier)
    }

    fn finished(&self, submission: u64) -> Result<bool, String> {
        let fence = lock(&self.submissions).get(&submission).map(|entry| entry.fence).ok_or_else(|| format!("no submission {submission}"))?;
        self.signaled(fence)
    }

    fn take_images(&self, submission: u64) -> Result<Vec<Image>, String> {
        if !self.finished(submission)? {
            return Err(format!("the submission {submission} is not finished"));
        }
        let entry = take(&self.submissions, submission).ok_or_else(|| format!("no submission {submission}"))?;
        // The copies wrote tight rows, top row first: the bytes are the image.
        let scope = self.scope();
        let images: Result<Vec<Image>, String> = entry.read_backs.iter().map(|read_back| Ok(Image { width: read_back.extent[0], height: read_back.extent[1], rgba: scope.read(&read_back.buffer)? })).collect();
        for kept in entry.kept {
            kept.destroy(&self.gpu);
        }
        images
    }

    fn wait_idle(&self) -> Result<(), String> {
        let _queue = lock(&self.queue);
        // SAFETY: the lock makes this thread the only user of the queue while
        // the device is waited for.
        check(unsafe { (self.gpu.functions.device_wait_idle)(self.gpu.device) }, "vkDeviceWaitIdle")
    }

    fn validation_delivery(&self) -> String {
        super::validation_delivery_of(&self.gpu)
    }

    fn validation_messages(&self) -> Vec<String> {
        self.gpu.sink.as_ref().map(|sink| sink.drain()).unwrap_or_default()
    }
}

impl Drop for VulkanFrames {
    fn drop(&mut self) {
        fn drain<T>(registry: &mut Mutex<HashMap<u64, T>>) -> impl Iterator<Item = T> {
            std::mem::take(registry.get_mut().unwrap_or_else(PoisonError::into_inner)).into_values()
        }
        let gpu = &self.gpu;
        // SAFETY: the value is being dropped, so no other thread uses the
        // queue; once the device is idle nothing below is in use.
        unsafe { (gpu.functions.device_wait_idle)(gpu.device) };
        drain(&mut self.submissions).flat_map(|submission| submission.kept).for_each(|kept| kept.destroy(gpu));
        drain(&mut self.passes).for_each(|pass| pass.kept.destroy(gpu));
        drain(&mut self.pipelines).for_each(|pipelines| pipelines.kept.destroy(gpu));
        drain(&mut self.targets).for_each(|target| target.kept.destroy(gpu));
        drain(&mut self.textures).for_each(|texture| texture.kept.destroy(gpu));
        drain(&mut self.vertex_buffers).for_each(|buffer| buffer.kept.destroy(gpu));
        std::mem::take(&mut self.kept).destroy(gpu);
    }
}
