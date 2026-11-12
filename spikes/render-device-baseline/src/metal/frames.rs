//! The adapter of [`crate::frames::FrameDevice`] on Metal: resources kept in
//! registries until they are destroyed, a pass recorded as one command buffer
//! that holds one encoder per part (the encoders of a parallel render command
//! encoder from two parts on), and submissions whose command buffers are
//! committed in order and polled.
//!
//! - **A target** is three textures kept on the device: the color and the
//!   depth at 4 samples, and the image the color is resolved into by the
//!   pass's store action. The image is in the sRGB format itself, so that a
//!   pass samples it as it is and a submission copies it as it is: nothing
//!   moves it from one state to another.
//! - **A pass** makes its command buffer and every encoder when it is opened.
//!   From two parts on, the encoders are those of a parallel render command
//!   encoder, made in the parts' order: that order is the order Metal runs
//!   their commands in, whoever records them and whenever. A pass of no part
//!   is one plain render command encoder, ended at once. A pass of one part
//!   is one plain render command encoder too, left open until the part is
//!   recorded: on the Apple Paravirtual device of the research CI, a parallel
//!   encoder with a single encoder left the target's image as it was, neither
//!   cleared nor drawn, and its command buffer completed without an error.
//! - **A submission** commits the command buffers of its passes in the order
//!   given, then one more that copies each target read back to a buffer in
//!   shared storage. It is finished when the status of every one of them says
//!   so; every status is read at each poll, so that a command buffer the
//!   device failed is an error at once, with what its `NSError` says.
//!
//! Metal returns a command buffer and an encoder autoreleased; each is kept
//! here with a reference of its own, since the call that made it pops its
//! autorelease pool long before the object has served. The command buffers
//! are the default ones, which retain what they name: a resource destroyed
//! here gives its reference back at once, and Metal frees it when the last
//! command buffer that names it is done. Between the opening of a pass and
//! the recording of each of its parts, the pass names its pipelines and its
//! textures by pointers that hold no reference: the contract leaves to the
//! caller that they outlive the frames that name them.
//!
//! A command queue hands out 64 command buffers that are not yet completed
//! and no more, unless it is created for another count, and `commandBuffer`
//! blocks beyond that. Each open pass and each read-back counts for one, so
//! the frames have a queue of their own, created for 1024: `open_pass` and
//! `submit` block beyond that count. Throwaway, never product code.

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::backend::DeviceInfo;
use crate::frames::{DRAW_CONSTANT_BYTES, DrawConstants, FrameDevice, FrameDraw, PassDescription, Resource, ShaderPayload, TextureSource};
use crate::scene::{Image, TEXTURE_COUNT, Texture};

use super::gpu::{self, ColorAttachment, ColorTarget, DepthAttachment, Gpu, PipelineShape, TextureShape};
use super::objc::{Id, Owned, Pool, class_name, present};
use super::scene_pass;
use super::validation::{self, StandardErrorCapture};
use super::values;

/// The frame shader's source under `shaders/`.
pub const FILE: &str = "frame.metal";
const VERTEX_FUNCTION: &str = "frame_vertex";
const FRAGMENT_FUNCTION: &str = "frame_fragment";
const PAYLOAD_FORM: &str = "Metal text that holds both functions, the variant defined on its first line; the system's Metal compiler compiles it when the pipelines are built";
/// The samples of a target's color and depth attachments.
const SAMPLES: usize = 4;
const SAMPLER_COUNT: usize = 3;
const BYTES_PER_PIXEL: usize = 4;
/// The depth every pass clears to.
const CLEAR_DEPTH: f64 = 1.0;
/// The length of the frame shader's structure of constants: the 88 bytes of
/// a draw's constants, rounded up to the 16 bytes its matrix aligns it on.
/// The bytes set for a stage must cover the whole structure.
const CONSTANT_BLOCK_BYTES: usize = 96;
const _: () = assert!(CONSTANT_BLOCK_BYTES >= DRAW_CONSTANT_BYTES);
/// The buffer indexes of the frame shader's vertex function: the vertex
/// buffer, then the draw's constants.
const VERTEX_BUFFER_INDEX: usize = 0;
const VERTEX_CONSTANTS_INDEX: usize = 1;
/// The buffer index of the draw's constants in the fragment function.
const FRAGMENT_CONSTANTS_INDEX: usize = 0;
const READ_BACK: &str = "a read-back of the frames";
/// The command buffers not yet completed that the frames' queue allows, where
/// a queue created without a count allows 64.
const QUEUE_COMMAND_BUFFERS: usize = 1024;

/// An object of Metal that any thread may use: a pipeline, a depth state, a
/// sampler, a texture or a buffer.
struct Shared(Owned);

// SAFETY: Metal's pipeline states, depth states and sampler states are
// immutable once made, and Metal lets any thread bind a texture or a buffer
// to an encoder of its own; their reference counts are atomic. The processor
// writes a texture or a buffer only inside the call that creates it, and
// reads a read-back buffer only once the device has completed the copy.
unsafe impl Send for Shared {}
// SAFETY: as above: every use through a shared reference is one Metal allows
// from several threads at once.
unsafe impl Sync for Shared {}

impl Shared {
    fn id(&self) -> Id {
        self.0.id()
    }
}

/// The device and its queue.
struct SharedGpu(Gpu);

// SAFETY: Metal documents a device and a command queue as safe to use from
// several threads at once; what else a `Gpu` holds is plain data.
unsafe impl Send for SharedGpu {}
// SAFETY: as above.
unsafe impl Sync for SharedGpu {}

/// A command buffer, kept with a reference of its own.
struct CommandBuffer(Owned);

// SAFETY: Metal asks that one thread at a time use a command buffer. One is
// made by the thread that opens a pass or that submits, then left alone
// until it is committed, polled, waited for and released: by whoever holds
// the lock of the registry it is in, or has taken it out of that registry.
unsafe impl Send for CommandBuffer {}

impl CommandBuffer {
    /// A command buffer of the queue, with a reference that outlives the caller's autorelease pool.
    fn create(gpu: &Gpu, what: &str) -> Result<CommandBuffer, String> {
        Ok(CommandBuffer(Owned::retain(gpu.command_buffer(what)?, &format!("commandBuffer for {what}"))?))
    }

    fn id(&self) -> Id {
        self.0.id()
    }
}

/// An encoder kept with a reference of its own, ended before that reference
/// is given back whatever happens: Metal ends the process when an encoder is
/// released while it is open.
struct Encoder {
    encoder: Owned,
    ended: bool,
}

// SAFETY: Metal asks that one thread at a time use an encoder, and makes the
// encoders of a parallel render command encoder to be handed to threads. An
// encoder here, plain or of a parallel encoder, is made by the thread that
// opens the pass and reaches one other thread: the one that takes it out of
// the registry, under its lock, to record a part, or the one that closes or
// discards the pass.
unsafe impl Send for Encoder {}

impl Encoder {
    /// Takes a reference on an encoder Metal just returned autoreleased.
    fn keep(encoder: Id, step: &str) -> Result<Encoder, String> {
        Ok(Encoder { encoder: Owned::retain(encoder, step)?, ended: false })
    }

    fn id(&self) -> Id {
        self.encoder.id()
    }

    fn end(mut self) -> Result<(), String> {
        self.ended = true;
        send!((); self.encoder.id(), "endEncoding")
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if !self.ended {
            // An encoder that does not answer `endEncoding` cannot be ended,
            // and a destructor has nobody to tell.
            let _ = send!((); self.encoder.id(), "endEncoding");
        }
    }
}

/// What every part of a pass binds, as plain object pointers a recording thread copies.
#[derive(Clone, Copy)]
struct Bindings {
    opaque_pipeline: Id,
    blended_pipeline: Id,
    depth_written: Id,
    depth_tested_only: Id,
    samplers: [Id; SAMPLER_COUNT],
    textures: [Id; TEXTURE_COUNT],
    /// The width and the height of the target, for the viewport and the scissor.
    size: [usize; 2],
}

// SAFETY: the pointers name pipelines, depth states, samplers and textures,
// which Metal lets any thread bind. The depth states and the samplers live as
// long as the device; the contract leaves to the caller that the pipelines
// and the textures of a pass outlive its recording, after which the command
// buffer holds references of its own.
unsafe impl Send for Bindings {}

/// A pass being recorded, or recorded and waiting to be submitted. The fields
/// are declared in the order they must be dropped in: the encoder of a part
/// is ended before the parallel encoder, and both before the command buffer
/// is released.
struct Pass {
    /// The encoder of each part, in the parts' order, until the part is recorded.
    parts: Vec<Option<Encoder>>,
    /// The parallel encoder of a pass of two parts or more, until the pass is
    /// closed; none for a pass of no part or of one, whose encoder is plain.
    parallel: Option<Encoder>,
    command_buffer: CommandBuffer,
    bindings: Bindings,
    closed: bool,
}

struct Pipelines {
    opaque: Shared,
    blended: Shared,
}

struct Target {
    /// The color attachment, at 4 samples.
    color: Shared,
    /// The depth attachment, at 4 samples.
    depth: Shared,
    /// The target's image: what the color is resolved into.
    resolved: Shared,
    /// The width and the height.
    size: [u32; 2],
}

/// The copy of one target's image for the processor.
struct ReadBack {
    buffer: Shared,
    /// The width and the height of the image.
    size: [u32; 2],
}

impl ReadBack {
    fn byte_length(size: [u32; 2]) -> usize {
        size[0] as usize * size[1] as usize * BYTES_PER_PIXEL
    }

    /// The image copied, tight rows, top row first: Metal's rows start at the top.
    fn image(&self) -> Result<Image, String> {
        let bytes = gpu::buffer_bytes(&self.buffer.0, ReadBack::byte_length(self.size), READ_BACK)?;
        Ok(Image::from_rows(self.size[0], self.size[1], self.size[0] as usize * BYTES_PER_PIXEL, &bytes))
    }
}

struct Submission {
    /// In the order they were committed: those of the passes, then the one
    /// that copies the targets read back, when there is one.
    command_buffers: Vec<CommandBuffer>,
    read_backs: Vec<ReadBack>,
}

impl Submission {
    /// Whether the device has completed every command buffer, read from their
    /// statuses without waiting. Every status is read, in the order of the
    /// commits: the first command buffer the device failed is an error, with
    /// its place among the others and what its `NSError` says, even while
    /// later ones are still in flight.
    fn finished(&self) -> Result<bool, String> {
        let count = self.command_buffers.len();
        let mut completed = true;
        for (index, command_buffer) in self.command_buffers.iter().enumerate() {
            completed &= gpu::is_completed(command_buffer.id(), "a submission of the frames").map_err(|reason| format!("{reason} (command buffer {} of the {count} committed)", index + 1))?;
        }
        Ok(completed)
    }
}

/// Everything of Metal the device owns. The fields are declared in the order
/// they are released in when the device is dropped: what names other objects
/// first, the device and its queue last.
struct Objects {
    passes: Mutex<HashMap<u64, Pass>>,
    submissions: Mutex<HashMap<u64, Submission>>,
    pipelines: Mutex<HashMap<u64, Pipelines>>,
    textures: Mutex<HashMap<u64, Shared>>,
    vertex_buffers: Mutex<HashMap<u64, Shared>>,
    targets: Mutex<HashMap<u64, Target>>,
    /// Depth test less-or-equal with the depth written: the opaque pipeline's.
    depth_written: Shared,
    /// Depth test less-or-equal with the depth left as it is: the blended pipeline's.
    depth_tested_only: Shared,
    /// Nearest, linear and trilinear, in the order of the sampler indexes.
    samplers: [Shared; SAMPLER_COUNT],
    gpu: SharedGpu,
}

/// A queue of the device that allows `QUEUE_COMMAND_BUFFERS` command buffers
/// not yet completed, owned; nothing where the device does not answer or
/// returns nil.
fn wide_queue(device: Id) -> Option<Owned> {
    let queue = send!(Id; device, "newCommandQueueWithMaxCommandBufferCount:", usize: QUEUE_COMMAND_BUFFERS).ok()?;
    Owned::adopt(queue, "newCommandQueueWithMaxCommandBufferCount:").ok()
}

impl Objects {
    fn create(mut gpu: Gpu) -> Result<Objects, String> {
        // The frames' own queue takes the place of the one the device was
        // opened with, which `newCommandQueue` made and which stays where
        // the device gives no other.
        match wide_queue(gpu.device.id()) {
            Some(queue) => gpu.queue = queue,
            None => gpu::log(&format!("the device gave no queue for {QUEUE_COMMAND_BUFFERS} command buffers: the frames keep the queue newCommandQueue made, which blocks beyond 64 not yet completed")),
        }
        let depth_written = Shared(gpu.depth_state(true)?);
        let depth_tested_only = Shared(gpu.depth_state(false)?);
        let samplers: Vec<Shared> = scene_pass::samplers(&gpu)?.into_iter().map(Shared).collect();
        let samplers: [Shared; SAMPLER_COUNT] = samplers.try_into().map_err(|_| format!("the frames bind {SAMPLER_COUNT} samplers"))?;
        Ok(Objects {
            passes: Mutex::default(),
            submissions: Mutex::default(),
            pipelines: Mutex::default(),
            textures: Mutex::default(),
            vertex_buffers: Mutex::default(),
            targets: Mutex::default(),
            depth_written,
            depth_tested_only,
            samplers,
            gpu: SharedGpu(gpu),
        })
    }
}

/// A registry's lock. A thread that panicked while it held the lock leaves
/// the registry whole: nothing here unwinds between two changes of a registry.
fn lock<T>(registry: &Mutex<T>) -> MutexGuard<'_, T> {
    registry.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Takes a value out of a registry. The lock is released before the value is
/// returned, so that the caller drops it, and gives references back, outside.
fn remove<T>(registry: &Mutex<HashMap<u64, T>>, identifier: u64) -> Option<T> {
    lock(registry).remove(&identifier)
}

/// The frame shader's text for a variant: the source behind a line that
/// defines the variant, which the source's preprocessor conditions read.
fn variant_text(source: &str, variant: u32) -> Result<Vec<u8>, String> {
    if variant > 1 {
        return Err(format!("import_shader: the frame shader has the variants 0 and 1, not {variant}"));
    }
    Ok(format!("#define TINT_VARIANT {variant}\n{source}").into_bytes())
}

/// One half of a payload as the Metal text it must be.
fn stage_text<'a>(bytes: &'a [u8], stage: &str) -> Result<&'a str, String> {
    std::str::from_utf8(bytes).map_err(|error| format!("create_pipelines: the {stage} half of the payload is not Metal text: {error}"))
}

/// The 88 bytes of a draw's constants at the start of the block the frame
/// shader's structure takes; the bytes after them are its padding.
fn constant_block(constants: &DrawConstants) -> [u8; CONSTANT_BLOCK_BYTES] {
    let mut block = [0_u8; CONSTANT_BLOCK_BYTES];
    block[..DRAW_CONSTANT_BYTES].copy_from_slice(&constants.bytes());
    block
}

/// Binds what every draw of a part shares, then issues the draws in their
/// order. `buffers` holds the vertex buffer of each draw.
fn bind_and_draw(encoder: Id, bindings: &Bindings, draws: &[FrameDraw], buffers: &[Id]) -> Result<(), String> {
    let [width, height] = bindings.size;
    send!((); encoder, "setViewport:", values::Viewport: values::Viewport::of(0, 0, width, height))?;
    send!((); encoder, "setScissorRect:", values::ScissorRect: values::ScissorRect { x: 0, y: 0, width, height })?;
    for (index, texture) in bindings.textures.iter().enumerate() {
        send!((); encoder, "setFragmentTexture:atIndex:", Id: *texture, usize: index)?;
    }
    for (index, sampler) in bindings.samplers.iter().enumerate() {
        send!((); encoder, "setFragmentSamplerState:atIndex:", Id: *sampler, usize: index)?;
    }
    // The pipeline with its depth state, and the vertex buffer, are set when
    // they change only.
    let (mut bound_pipeline, mut bound_buffer) = (None, None);
    for (draw, buffer) in draws.iter().zip(buffers) {
        if bound_pipeline != Some(draw.blended) {
            let (pipeline, depth_state) = if draw.blended { (bindings.blended_pipeline, bindings.depth_tested_only) } else { (bindings.opaque_pipeline, bindings.depth_written) };
            send!((); encoder, "setRenderPipelineState:", Id: pipeline)?;
            send!((); encoder, "setDepthStencilState:", Id: depth_state)?;
            bound_pipeline = Some(draw.blended);
        }
        if bound_buffer != Some(draw.vertex_buffer) {
            send!((); encoder, "setVertexBuffer:offset:atIndex:", Id: *buffer, usize: 0, usize: VERTEX_BUFFER_INDEX)?;
            bound_buffer = Some(draw.vertex_buffer);
        }
        // The draw's constants, as bytes Metal copies, for each stage.
        let constants = constant_block(&draw.constants);
        send!((); encoder, "setVertexBytes:length:atIndex:", *const c_void: constants.as_ptr().cast(), usize: constants.len(), usize: VERTEX_CONSTANTS_INDEX)?;
        send!((); encoder, "setFragmentBytes:length:atIndex:", *const c_void: constants.as_ptr().cast(), usize: constants.len(), usize: FRAGMENT_CONSTANTS_INDEX)?;
        send!((); encoder, "drawPrimitives:vertexStart:vertexCount:", usize: values::PRIMITIVE_TYPE_TRIANGLE, usize: draw.first_vertex as usize, usize: draw.vertex_count as usize)?;
    }
    Ok(())
}

/// One Metal device open for frames.
pub struct MetalFrames {
    /// Released by hand, inside an autorelease pool, before the capture ends.
    objects: ManuallyDrop<Objects>,
    info: DeviceInfo,
    settings: String,
    /// The text of the frame shader's source.
    source: String,
    /// One sequence for every resource, pass and submission, so that no
    /// identifier is given twice.
    next_identifier: AtomicU64,
    /// The standard error captured for the validation layer's messages.
    capture: Option<Mutex<StandardErrorCapture>>,
}

impl MetalFrames {
    /// A device open for frames on `gpu`, with the depth states and the
    /// samplers every pass binds. The caller holds an autorelease pool.
    pub fn open(gpu: Gpu, info: DeviceInfo, settings: String, capture: Option<StandardErrorCapture>, source: String) -> Result<MetalFrames, String> {
        let objects = Objects::create(gpu)?;
        Ok(MetalFrames { objects: ManuallyDrop::new(objects), info, settings, source, next_identifier: AtomicU64::new(1), capture: capture.map(Mutex::new) })
    }

    fn gpu(&self) -> &Gpu {
        &self.objects.gpu.0
    }

    fn register<T>(&self, registry: &Mutex<HashMap<u64, T>>, value: T) -> u64 {
        let identifier = self.next_identifier.fetch_add(1, Ordering::Relaxed);
        lock(registry).insert(identifier, value);
        identifier
    }

    /// What a pass samples at each of its five indexes: a texture, or the image of a target.
    fn sampled_textures(&self, sources: &[TextureSource; TEXTURE_COUNT]) -> Result<[Id; TEXTURE_COUNT], String> {
        let mut textures = [ptr::null_mut(); TEXTURE_COUNT];
        for (texture, source) in textures.iter_mut().zip(sources) {
            *texture = match source {
                TextureSource::Texture(identifier) => lock(&self.objects.textures).get(identifier).map(Shared::id).ok_or_else(|| format!("open_pass: there is no texture {identifier}"))?,
                TextureSource::Target(identifier) => lock(&self.objects.targets).get(identifier).map(|target| target.resolved.id()).ok_or_else(|| format!("open_pass: there is no target {identifier} to sample"))?,
            };
        }
        Ok(textures)
    }

    /// The vertex buffer of each draw, looked up before the recording starts
    /// so that the registry's lock is not held while it lasts.
    fn vertex_buffers(&self, draws: &[FrameDraw]) -> Result<Vec<Id>, String> {
        let registry = lock(&self.objects.vertex_buffers);
        let mut last: Option<(u64, Id)> = None;
        let mut buffers = Vec::with_capacity(draws.len());
        for draw in draws {
            let buffer = match last {
                Some((identifier, buffer)) if identifier == draw.vertex_buffer => buffer,
                _ => registry.get(&draw.vertex_buffer).map(Shared::id).ok_or_else(|| format!("record_part: there is no vertex buffer {}", draw.vertex_buffer))?,
            };
            last = Some((draw.vertex_buffer, buffer));
            buffers.push(buffer);
        }
        Ok(buffers)
    }

    /// A command buffer that copies the image of each target to a buffer the
    /// processor reads, in the order given.
    fn encode_read_backs(&self, images: &[(Id, [u32; 2])]) -> Result<(CommandBuffer, Vec<ReadBack>), String> {
        let gpu = self.gpu();
        let command_buffer = CommandBuffer::create(gpu, READ_BACK)?;
        let mut read_backs = Vec::with_capacity(images.len());
        for (image, size) in images {
            let buffer = gpu.empty_buffer(READ_BACK, ReadBack::byte_length(*size))?;
            gpu::encode_copy_to_buffer(command_buffer.id(), *image, buffer.id(), size[0] as usize, size[1] as usize, BYTES_PER_PIXEL)?;
            read_backs.push(ReadBack { buffer: Shared(buffer), size: *size });
        }
        Ok((command_buffer, read_backs))
    }

    /// Takes the passes out of their registry and returns their command
    /// buffers in the order given. A pass that is unknown, named twice or not
    /// closed leaves every pass where it was.
    fn take_closed(&self, passes: &[u64]) -> Result<Vec<CommandBuffer>, String> {
        let mut registry = lock(&self.objects.passes);
        let mut taken: Vec<(u64, Pass)> = Vec::with_capacity(passes.len());
        let mut refused = None;
        for identifier in passes {
            match registry.remove(identifier) {
                Some(pass) if pass.closed => taken.push((*identifier, pass)),
                Some(pass) => {
                    registry.insert(*identifier, pass);
                    refused = Some(format!("submit: the pass {identifier} is not closed"));
                    break;
                }
                None => {
                    refused = Some(format!("submit: there is no pass {identifier}, or it is named twice"));
                    break;
                }
            }
        }
        match refused {
            Some(reason) => {
                registry.extend(taken);
                Err(reason)
            }
            None => Ok(taken.into_iter().map(|(_, pass)| pass.command_buffer).collect()),
        }
    }
}

impl Drop for MetalFrames {
    fn drop(&mut self) {
        let _pool = Pool::push();
        if let Err(reason) = self.wait_idle() {
            gpu::log(&format!("the frames' device is dropped after a submission that failed: {reason}"));
        }
        // SAFETY: the objects are dropped here, once, and the field is not
        // used again.
        unsafe { ManuallyDrop::drop(&mut self.objects) };
    }
}

impl FrameDevice for MetalFrames {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn settings(&self) -> String {
        self.settings.clone()
    }

    fn import_shader(&self, variant: u32) -> Result<ShaderPayload, String> {
        // One text holds both functions: both halves of the payload are that text.
        let text = variant_text(&self.source, variant)?;
        Ok(ShaderPayload { vertex: text.clone(), fragment: text, form: PAYLOAD_FORM })
    }

    fn create_pipelines(&self, payload: &ShaderPayload) -> Result<u64, String> {
        let _pool = Pool::push();
        let gpu = self.gpu();
        let vertex_library = gpu.library(FILE, stage_text(&payload.vertex, "vertex")?)?;
        let vertex = gpu::function(&vertex_library, FILE, VERTEX_FUNCTION)?;
        // The text is compiled once when both halves of the payload are the same.
        let fragment = if payload.fragment == payload.vertex {
            gpu::function(&vertex_library, FILE, FRAGMENT_FUNCTION)?
        } else {
            gpu::function(&gpu.library(FILE, stage_text(&payload.fragment, "fragment")?)?, FILE, FRAGMENT_FUNCTION)?
        };
        let pipeline = |what: &str, blended: bool| {
            let colors = [ColorTarget { format: values::PIXEL_FORMAT_RGBA8_UNORM_SRGB, blended }];
            gpu.render_pipeline(&PipelineShape { what, vertex: &vertex, fragment: &fragment, colors: &colors, depth: true, samples: SAMPLES }).map(Shared)
        };
        let pipelines = Pipelines { opaque: pipeline("the frames' opaque pipeline", false)?, blended: pipeline("the frames' blended pipeline", true)? };
        Ok(self.register(&self.objects.pipelines, pipelines))
    }

    fn create_texture(&self, texture: &Texture) -> Result<u64, String> {
        let _pool = Pool::push();
        let created = Shared(scene_pass::sampled_texture(self.gpu(), texture)?);
        Ok(self.register(&self.objects.textures, created))
    }

    fn create_vertex_buffer(&self, bytes: &[u8]) -> Result<u64, String> {
        let _pool = Pool::push();
        let buffer = Shared(self.gpu().buffer("a vertex buffer of the frames", bytes)?);
        Ok(self.register(&self.objects.vertex_buffers, buffer))
    }

    fn create_target(&self, width: u32, height: u32) -> Result<u64, String> {
        let _pool = Pool::push();
        let texture = |what: &str, format: usize, samples: usize, usage: usize| {
            self.gpu().texture(what, &TextureShape { format, width: width as usize, height: height as usize, levels: 1, samples, usage, private: true }).map(Shared)
        };
        let target = Target {
            color: texture("the 4-sample color of a target", values::PIXEL_FORMAT_RGBA8_UNORM_SRGB, SAMPLES, values::TEXTURE_USAGE_RENDER_TARGET)?,
            depth: texture("the 4-sample depth of a target", values::PIXEL_FORMAT_DEPTH32_FLOAT, SAMPLES, values::TEXTURE_USAGE_RENDER_TARGET)?,
            resolved: texture("the image of a target", values::PIXEL_FORMAT_RGBA8_UNORM_SRGB, 1, values::TEXTURE_USAGE_RENDER_TARGET | values::TEXTURE_USAGE_SHADER_READ)?,
            size: [width, height],
        };
        Ok(self.register(&self.objects.targets, target))
    }

    fn destroy(&self, resource: Resource) {
        let _pool = Pool::push();
        let objects = &*self.objects;
        // The reference is given back here; a command buffer that names the
        // object holds its own until the device has completed it.
        match resource {
            Resource::Texture(identifier) => drop(remove(&objects.textures, identifier)),
            Resource::VertexBuffer(identifier) => drop(remove(&objects.vertex_buffers, identifier)),
            Resource::Target(identifier) => drop(remove(&objects.targets, identifier)),
            Resource::Pipelines(identifier) => drop(remove(&objects.pipelines, identifier)),
        }
    }

    fn open_pass(&self, pass: &PassDescription) -> Result<u64, String> {
        let _pool = Pool::push();
        let objects = &*self.objects;
        let (color, depth, resolved, size) = {
            let targets = lock(&objects.targets);
            let target = targets.get(&pass.target).ok_or_else(|| format!("open_pass: there is no target {}", pass.target))?;
            (target.color.id(), target.depth.id(), target.resolved.id(), target.size.map(|side| side as usize))
        };
        let (opaque_pipeline, blended_pipeline) = {
            let pipelines = lock(&objects.pipelines);
            let pair = pipelines.get(&pass.pipelines).ok_or_else(|| format!("open_pass: there is no pair of pipelines {}", pass.pipelines))?;
            (pair.opaque.id(), pair.blended.id())
        };
        let bindings = Bindings {
            opaque_pipeline,
            blended_pipeline,
            depth_written: objects.depth_written.id(),
            depth_tested_only: objects.depth_tested_only.id(),
            samplers: objects.samplers.each_ref().map(Shared::id),
            textures: self.sampled_textures(&pass.textures)?,
            size,
        };

        // Both 4-sample attachments are cleared, and the color is resolved
        // into the target's image by the store action.
        let [red, green, blue, alpha] = pass.clear.map(f64::from);
        let color = ColorAttachment { texture: color, resolve: Some(resolved), load: values::LOAD_ACTION_CLEAR, clear: values::ClearColor { red, green, blue, alpha } };
        let descriptor = gpu::render_pass(&[color], Some(&DepthAttachment { texture: depth, clear: CLEAR_DEPTH }))?;
        // Declared before the encoders, so that an error ends them before
        // the command buffer is released.
        let command_buffer = CommandBuffer::create(self.gpu(), "a pass of the frames")?;
        let (parallel, parts) = match pass.parts {
            0 => {
                let encoder = present(send!(Id; command_buffer.id(), "renderCommandEncoderWithDescriptor:", Id: descriptor)?, "renderCommandEncoderWithDescriptor: for a pass of no part")?;
                gpu::end_encoding(encoder, Ok(()))?;
                (None, Vec::new())
            }
            // One part is one plain encoder, left open for `record_part`: a
            // parallel encoder with a single encoder left the target's image
            // as it was on the Apple Paravirtual device.
            1 => {
                let encoder = Encoder::keep(send!(Id; command_buffer.id(), "renderCommandEncoderWithDescriptor:", Id: descriptor)?, "renderCommandEncoderWithDescriptor: for a pass of one part")?;
                (None, vec![Some(encoder)])
            }
            count => {
                let parallel = Encoder::keep(send!(Id; command_buffer.id(), "parallelRenderCommandEncoderWithDescriptor:", Id: descriptor)?, "parallelRenderCommandEncoderWithDescriptor: for a pass of the frames")?;
                // One encoder per part, made in the parts' order: the order
                // their commands run in.
                let mut parts = Vec::with_capacity(count);
                for _ in 0..count {
                    parts.push(Some(Encoder::keep(send!(Id; parallel.id(), "renderCommandEncoder")?, "renderCommandEncoder of the parallel encoder")?));
                }
                (Some(parallel), parts)
            }
        };
        Ok(self.register(&objects.passes, Pass { parts, parallel, command_buffer, bindings, closed: false }))
    }

    fn record_part(&self, pass: u64, part: usize, draws: &[FrameDraw]) -> Result<(), String> {
        let _pool = Pool::push();
        let buffers = self.vertex_buffers(draws)?;
        let (encoder, bindings) = {
            let mut passes = lock(&self.objects.passes);
            let found = passes.get_mut(&pass).ok_or_else(|| format!("record_part: there is no pass {pass}"))?;
            let parts = found.parts.len();
            let slot = found.parts.get_mut(part).ok_or_else(|| format!("record_part: the pass {pass} has {parts} parts, there is no part {part}"))?;
            let encoder = slot.take().ok_or_else(|| format!("record_part: the part {part} of the pass {pass} is already recorded"))?;
            (encoder, found.bindings)
        };
        // The lock is released: this part is recorded while others are.
        let recorded = bind_and_draw(encoder.id(), &bindings, draws, &buffers);
        let ended = encoder.end();
        recorded.and(ended)
    }

    fn close_pass(&self, pass: u64) -> Result<(), String> {
        let _pool = Pool::push();
        let parallel = {
            let mut passes = lock(&self.objects.passes);
            let found = passes.get_mut(&pass).ok_or_else(|| format!("close_pass: there is no pass {pass}"))?;
            if found.closed {
                return Err(format!("close_pass: the pass {pass} is already closed"));
            }
            if let Some(part) = found.parts.iter().position(Option::is_some) {
                return Err(format!("close_pass: the part {part} of the pass {pass} is not recorded"));
            }
            found.closed = true;
            found.parallel.take()
        };
        // The parallel encoder is ended with the lock released: parts of
        // other passes may be starting their recording.
        parallel.map_or(Ok(()), Encoder::end)
    }

    fn discard_pass(&self, pass: u64) {
        let _pool = Pool::push();
        // The encoders still open are ended in their order, then the command
        // buffer is released without having been committed.
        drop(remove(&self.objects.passes, pass));
    }

    fn submit(&self, passes: &[u64], read_back: &[u64]) -> Result<u64, String> {
        let _pool = Pool::push();
        let objects = &*self.objects;
        // Whatever can fail comes before the passes leave their registry: a
        // submission that fails leaves them as they were.
        let images: Vec<(Id, [u32; 2])> = {
            let targets = lock(&objects.targets);
            read_back
                .iter()
                .map(|identifier| targets.get(identifier).map(|target| (target.resolved.id(), target.size)).ok_or_else(|| format!("submit: there is no target {identifier} to read back")))
                .collect::<Result<_, _>>()?
        };
        let (copies, read_backs) = if images.is_empty() {
            (None, Vec::new())
        } else {
            let (command_buffer, read_backs) = self.encode_read_backs(&images)?;
            (Some(command_buffer), read_backs)
        };
        let mut command_buffers = self.take_closed(passes)?;
        command_buffers.extend(copies);
        // The queue runs the command buffers in the order they are committed
        // in, and `commit` returns without waiting. Only an object that does
        // not answer `commit` refuses it, and then the first one does: an
        // error leaves nothing in flight.
        for command_buffer in &command_buffers {
            send!((); command_buffer.id(), "commit")?;
        }
        Ok(self.register(&objects.submissions, Submission { command_buffers, read_backs }))
    }

    fn finished(&self, submission: u64) -> Result<bool, String> {
        let _pool = Pool::push();
        lock(&self.objects.submissions).get(&submission).ok_or_else(|| format!("finished: there is no submission {submission}"))?.finished()
    }

    fn take_images(&self, submission: u64) -> Result<Vec<Image>, String> {
        let _pool = Pool::push();
        let taken = {
            let mut submissions = lock(&self.objects.submissions);
            let taken = submissions.remove(&submission).ok_or_else(|| format!("take_images: there is no submission {submission}"))?;
            match taken.finished() {
                Ok(false) => {
                    submissions.insert(submission, taken);
                    return Err(format!("take_images: the submission {submission} is not finished"));
                }
                // A submission the device failed is freed with its error.
                outcome => outcome.map(|_| taken)?,
            }
        };
        // What the submission owned is freed when `taken` is dropped.
        taken.read_backs.iter().map(ReadBack::image).collect()
    }

    fn wait_idle(&self) -> Result<(), String> {
        let _pool = Pool::push();
        // The contract has one thread at a time submit, poll, take and wait:
        // nobody else wants this lock while the wait lasts.
        let submissions = lock(&self.objects.submissions);
        let mut outcome = Ok(());
        for command_buffer in submissions.values().flat_map(|submission| &submission.command_buffers) {
            // Every one is waited for, even after one that failed.
            outcome = outcome.and(gpu::wait_until_completed(command_buffer.id(), "a submission of the frames"));
        }
        outcome
    }

    fn validation_delivery(&self) -> String {
        validation::delivery(&class_name(self.gpu().device.id()), self.capture.is_some())
    }

    fn validation_messages(&self) -> Vec<String> {
        self.capture.as_ref().map(|capture| lock(capture).drain()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constant_block_starts_with_the_draw_constants_and_ends_with_zeros() {
        let constants = DrawConstants { transform: std::array::from_fn(|index| index as f32 + 1.0), tint: [0.25, 0.5, 0.75, 1.0], texture: 3, sampler: 2 };
        let block = constant_block(&constants);
        assert_eq!(block[..DRAW_CONSTANT_BYTES], constants.bytes());
        assert!(block[DRAW_CONSTANT_BYTES..].iter().all(|byte| *byte == 0));
        // The frame shader's structure is aligned on the 16 bytes of its matrix.
        assert_eq!(block.len() % 16, 0);
    }

    #[test]
    fn the_variant_is_defined_on_the_line_before_the_source() {
        assert_eq!(variant_text("source\n", 0), Ok(b"#define TINT_VARIANT 0\nsource\n".to_vec()));
        assert_eq!(variant_text("source\n", 1), Ok(b"#define TINT_VARIANT 1\nsource\n".to_vec()));
        assert!(variant_text("source\n", 2).is_err_and(|reason| reason.contains("not 2")));
    }
}
