//! One open Metal device with its command queue, and the objects every
//! exercise of the module builds the same way: shader libraries, pipelines,
//! textures, buffers, pass descriptors, the completion of a command buffer
//! and the copy of a texture to memory. Every call that returns an object is
//! checked for nil and every `NSError` is read, so that an `Err` names the
//! selector and what it was building. Throwaway, never product code.

use std::ffi::c_void;
use std::io::Write;
use std::ptr;
use std::time::{Duration, Instant};

use super::objc::{Id, Owned, answers, class, error_text, new_object, present, string};
use super::timestamps::Clock;
use super::values;

/// How long a polled command buffer may take before the wait is given up.
const POLL_LIMIT: Duration = Duration::from_secs(30);
/// The pause between two polls of a command buffer's status.
const POLL_INTERVAL: Duration = Duration::from_micros(50);

/// Says on the standard error what no return value can carry.
pub fn log(message: &str) {
    let _ = writeln!(std::io::stderr(), "metal: {message}");
}

pub struct Gpu {
    pub device: Owned,
    pub queue: Owned,
    /// The name the framework gives its timestamp counter set.
    pub timestamp_set: String,
    /// The device's clock read when the device was opened, where it gives one.
    pub clock: Option<Clock>,
}

/// A two-dimensional texture to create. `private` keeps it on the device
/// (targets, read back by a copy); otherwise the descriptor's default storage
/// applies, which `replaceRegion` can fill.
pub struct TextureShape {
    pub format: usize,
    pub width: usize,
    pub height: usize,
    pub levels: usize,
    pub samples: usize,
    pub usage: usize,
    pub private: bool,
}

impl TextureShape {
    /// A single-sample, single-level target kept on the device.
    pub fn target(format: usize, width: usize, height: usize, usage: usize) -> TextureShape {
        TextureShape { format, width, height, levels: 1, samples: 1, usage, private: true }
    }
}

/// One color attachment of a pipeline: its format, and whether it blends with
/// source alpha over what is there.
pub struct ColorTarget {
    pub format: usize,
    pub blended: bool,
}

pub struct PipelineShape<'a> {
    pub what: &'a str,
    pub vertex: &'a Owned,
    pub fragment: &'a Owned,
    pub colors: &'a [ColorTarget],
    /// Whether the pass has the 32-bit float depth attachment.
    pub depth: bool,
    pub samples: usize,
}

/// One color attachment of a pass. With a resolve texture the samples are
/// resolved into it and dropped; otherwise the attachment is stored.
#[derive(Clone, Copy)]
pub struct ColorAttachment {
    pub texture: Id,
    pub resolve: Option<Id>,
    pub load: usize,
    pub clear: values::ClearColor,
}

/// The depth attachment of a pass: cleared, tested, not kept.
#[derive(Clone, Copy)]
pub struct DepthAttachment {
    pub texture: Id,
    pub clear: f64,
}

impl Gpu {
    /// Compiles a shader source with the compiler's default options. The
    /// second value is what the compiler said beside a library: its warnings.
    pub fn compile(&self, file: &str, source: &str) -> Result<(Owned, Option<String>), String> {
        let source = string(source)?;
        let mut error: Id = ptr::null_mut();
        let library = send!(Id; self.device.id(), "newLibraryWithSource:options:error:", Id: source.id(), Id: ptr::null_mut(), *mut Id: &mut error)?;
        if library.is_null() {
            return Err(format!("newLibraryWithSource:options:error: for {file}: {}", error_text(error)));
        }
        let message = (!error.is_null()).then(|| error_text(error));
        Ok((Owned::adopt(library, "newLibraryWithSource:options:error:")?, message))
    }

    pub fn library(&self, file: &str, source: &str) -> Result<Owned, String> {
        self.compile(file, source).map(|(library, _)| library)
    }

    pub fn compute_pipeline(&self, function: &Owned, what: &str) -> Result<Owned, String> {
        let mut error: Id = ptr::null_mut();
        let pipeline = send!(Id; self.device.id(), "newComputePipelineStateWithFunction:error:", Id: function.id(), *mut Id: &mut error)?;
        if pipeline.is_null() {
            return Err(format!("newComputePipelineStateWithFunction:error: for {what}: {}", error_text(error)));
        }
        Owned::adopt(pipeline, "newComputePipelineStateWithFunction:error:")
    }

    pub fn render_pipeline(&self, shape: &PipelineShape) -> Result<Owned, String> {
        let descriptor = new_object("MTLRenderPipelineDescriptor")?;
        send!((); descriptor.id(), "setVertexFunction:", Id: shape.vertex.id())?;
        send!((); descriptor.id(), "setFragmentFunction:", Id: shape.fragment.id())?;
        // The property was named `sampleCount` before `rasterSampleCount` replaced it.
        if answers(descriptor.id(), "setRasterSampleCount:") {
            send!((); descriptor.id(), "setRasterSampleCount:", usize: shape.samples)?;
        } else {
            send!((); descriptor.id(), "setSampleCount:", usize: shape.samples)?;
        }
        if shape.depth {
            send!((); descriptor.id(), "setDepthAttachmentPixelFormat:", usize: values::PIXEL_FORMAT_DEPTH32_FLOAT)?;
        }
        let attachments = present(send!(Id; descriptor.id(), "colorAttachments")?, "colorAttachments of the pipeline descriptor")?;
        for (index, color) in shape.colors.iter().enumerate() {
            let attachment = present(send!(Id; attachments, "objectAtIndexedSubscript:", usize: index)?, "objectAtIndexedSubscript: of the pipeline's color attachments")?;
            send!((); attachment, "setPixelFormat:", usize: color.format)?;
            if color.blended {
                blend_with_source_alpha(attachment)?;
            }
        }
        let mut error: Id = ptr::null_mut();
        let pipeline = send!(Id; self.device.id(), "newRenderPipelineStateWithDescriptor:error:", Id: descriptor.id(), *mut Id: &mut error)?;
        if pipeline.is_null() {
            return Err(format!("newRenderPipelineStateWithDescriptor:error: for {}: {}", shape.what, error_text(error)));
        }
        Owned::adopt(pipeline, "newRenderPipelineStateWithDescriptor:error:")
    }

    /// A depth state that tests less-or-equal and writes or not.
    pub fn depth_state(&self, write: bool) -> Result<Owned, String> {
        let descriptor = new_object("MTLDepthStencilDescriptor")?;
        send!((); descriptor.id(), "setDepthCompareFunction:", usize: values::COMPARE_FUNCTION_LESS_EQUAL)?;
        send!((); descriptor.id(), "setDepthWriteEnabled:", bool: write)?;
        Owned::adopt(send!(Id; self.device.id(), "newDepthStencilStateWithDescriptor:", Id: descriptor.id())?, "newDepthStencilStateWithDescriptor:")
    }

    /// A sampler with one filter in both directions, one filter between
    /// levels and one address mode. The descriptor's defaults give the rest:
    /// no anisotropy, the full level range.
    pub fn sampler(&self, min_mag_filter: usize, mip_filter: usize, address_mode: usize) -> Result<Owned, String> {
        let descriptor = new_object("MTLSamplerDescriptor")?;
        send!((); descriptor.id(), "setMinFilter:", usize: min_mag_filter)?;
        send!((); descriptor.id(), "setMagFilter:", usize: min_mag_filter)?;
        send!((); descriptor.id(), "setMipFilter:", usize: mip_filter)?;
        send!((); descriptor.id(), "setSAddressMode:", usize: address_mode)?;
        send!((); descriptor.id(), "setTAddressMode:", usize: address_mode)?;
        Owned::adopt(send!(Id; self.device.id(), "newSamplerStateWithDescriptor:", Id: descriptor.id())?, "newSamplerStateWithDescriptor:")
    }

    pub fn texture(&self, what: &str, shape: &TextureShape) -> Result<Owned, String> {
        let descriptor = new_object("MTLTextureDescriptor")?;
        let texture_type = if shape.samples > 1 { values::TEXTURE_TYPE_2D_MULTISAMPLE } else { values::TEXTURE_TYPE_2D };
        send!((); descriptor.id(), "setTextureType:", usize: texture_type)?;
        send!((); descriptor.id(), "setPixelFormat:", usize: shape.format)?;
        send!((); descriptor.id(), "setWidth:", usize: shape.width)?;
        send!((); descriptor.id(), "setHeight:", usize: shape.height)?;
        send!((); descriptor.id(), "setMipmapLevelCount:", usize: shape.levels)?;
        send!((); descriptor.id(), "setSampleCount:", usize: shape.samples)?;
        send!((); descriptor.id(), "setUsage:", usize: shape.usage)?;
        if shape.private {
            send!((); descriptor.id(), "setStorageMode:", usize: values::STORAGE_MODE_PRIVATE)?;
        }
        Owned::adopt(send!(Id; self.device.id(), "newTextureWithDescriptor:", Id: descriptor.id())?, &format!("newTextureWithDescriptor: for {what}"))
    }

    /// A buffer in shared storage holding a copy of `bytes`.
    pub fn buffer(&self, what: &str, bytes: &[u8]) -> Result<Owned, String> {
        if bytes.is_empty() {
            return Err(format!("{what}: there is no byte to put in a buffer"));
        }
        let buffer = send!(Id; self.device.id(), "newBufferWithBytes:length:options:", *const c_void: bytes.as_ptr().cast(), usize: bytes.len(), usize: values::RESOURCE_STORAGE_MODE_SHARED)?;
        Owned::adopt(buffer, &format!("newBufferWithBytes:length:options: for {what}"))
    }

    /// A buffer of zeros in shared storage.
    pub fn empty_buffer(&self, what: &str, length: usize) -> Result<Owned, String> {
        let buffer = send!(Id; self.device.id(), "newBufferWithLength:options:", usize: length, usize: values::RESOURCE_STORAGE_MODE_SHARED)?;
        Owned::adopt(buffer, &format!("newBufferWithLength:options: for {what}"))
    }

    /// A command buffer of the queue, autoreleased: it lives as long as the caller's pool.
    pub fn command_buffer(&self, what: &str) -> Result<Id, String> {
        present(send!(Id; self.queue.id(), "commandBuffer")?, &format!("commandBuffer for {what}"))
    }

    /// The bytes of a single-sample texture, tight rows, top row first: a
    /// copy to a shared buffer, waited for.
    pub fn read_texture(&self, what: &str, texture: Id, width: usize, height: usize, bytes_per_pixel: usize) -> Result<Vec<u8>, String> {
        let length = width * height * bytes_per_pixel;
        let buffer = self.empty_buffer(what, length)?;
        let command_buffer = self.command_buffer(what)?;
        encode_copy_to_buffer(command_buffer, texture, buffer.id(), width, height, bytes_per_pixel)?;
        complete(command_buffer, what)?;
        buffer_bytes(&buffer, length, what)
    }
}

/// Source alpha over the destination for color; one and one minus source
/// alpha for alpha; both added.
fn blend_with_source_alpha(attachment: Id) -> Result<(), String> {
    send!((); attachment, "setBlendingEnabled:", bool: true)?;
    send!((); attachment, "setSourceRGBBlendFactor:", usize: values::BLEND_FACTOR_SOURCE_ALPHA)?;
    send!((); attachment, "setDestinationRGBBlendFactor:", usize: values::BLEND_FACTOR_ONE_MINUS_SOURCE_ALPHA)?;
    send!((); attachment, "setRgbBlendOperation:", usize: values::BLEND_OPERATION_ADD)?;
    send!((); attachment, "setSourceAlphaBlendFactor:", usize: values::BLEND_FACTOR_ONE)?;
    send!((); attachment, "setDestinationAlphaBlendFactor:", usize: values::BLEND_FACTOR_ONE_MINUS_SOURCE_ALPHA)?;
    send!((); attachment, "setAlphaBlendOperation:", usize: values::BLEND_OPERATION_ADD)
}

/// A function of a compiled library, by the name the source gives it.
pub fn function(library: &Owned, file: &str, name: &str) -> Result<Owned, String> {
    let function_name = string(name)?;
    Owned::adopt(send!(Id; library.id(), "newFunctionWithName:", Id: function_name.id())?, &format!("newFunctionWithName: for {name} of {file}"))
}

/// Fills one level of a texture of 8-bit RGBA texels.
pub fn upload(texture: &Owned, what: &str, level: usize, width: usize, height: usize, texels: &[u8]) -> Result<(), String> {
    if texels.len() != width * height * 4 {
        return Err(format!("{what}, level {level}: {} bytes for {width} by {height} texels", texels.len()));
    }
    let region = values::Region::of_level(width, height);
    send!((); texture.id(), "replaceRegion:mipmapLevel:withBytes:bytesPerRow:", values::Region: region, usize: level, *const c_void: texels.as_ptr().cast(), usize: width * 4)
}

/// A second view of a texture in another pixel format of the same size.
pub fn view(texture: &Owned, format: usize, what: &str) -> Result<Owned, String> {
    Owned::adopt(send!(Id; texture.id(), "newTextureViewWithPixelFormat:", usize: format)?, &format!("newTextureViewWithPixelFormat: for {what}"))
}

/// A pass descriptor, autoreleased: it lives as long as the caller's pool.
pub fn render_pass(colors: &[ColorAttachment], depth: Option<&DepthAttachment>) -> Result<Id, String> {
    let pass = present(send!(Id; class("MTLRenderPassDescriptor")?, "renderPassDescriptor")?, "[MTLRenderPassDescriptor renderPassDescriptor]")?;
    let attachments = present(send!(Id; pass, "colorAttachments")?, "colorAttachments of the pass descriptor")?;
    for (index, color) in colors.iter().enumerate() {
        let attachment = present(send!(Id; attachments, "objectAtIndexedSubscript:", usize: index)?, "objectAtIndexedSubscript: of the pass's color attachments")?;
        send!((); attachment, "setTexture:", Id: color.texture)?;
        send!((); attachment, "setLoadAction:", usize: color.load)?;
        send!((); attachment, "setClearColor:", values::ClearColor: color.clear)?;
        match color.resolve {
            Some(resolve) => {
                send!((); attachment, "setResolveTexture:", Id: resolve)?;
                send!((); attachment, "setStoreAction:", usize: values::STORE_ACTION_MULTISAMPLE_RESOLVE)?;
            }
            None => send!((); attachment, "setStoreAction:", usize: values::STORE_ACTION_STORE)?,
        }
    }
    if let Some(depth) = depth {
        let attachment = present(send!(Id; pass, "depthAttachment")?, "depthAttachment of the pass descriptor")?;
        send!((); attachment, "setTexture:", Id: depth.texture)?;
        send!((); attachment, "setLoadAction:", usize: values::LOAD_ACTION_CLEAR)?;
        send!((); attachment, "setStoreAction:", usize: values::STORE_ACTION_DONT_CARE)?;
        send!((); attachment, "setClearDepth:", f64: depth.clear)?;
    }
    Ok(pass)
}

/// Ends an encoder whatever its recording returned: a command buffer
/// released with an encoder still open ends the process.
pub fn end_encoding(encoder: Id, recorded: Result<(), String>) -> Result<(), String> {
    let ended = send!((); encoder, "endEncoding");
    recorded.and(ended)
}

/// The status and the error of a command buffer that did not complete.
fn failure(command_buffer: Id, status: usize) -> String {
    let name = values::COMMAND_BUFFER_STATUS_NAMES.get(status).copied().unwrap_or("unknown");
    let error = send!(Id; command_buffer, "error").map(error_text).unwrap_or_else(|reason| reason);
    format!("status {status} ({name}): {error}")
}

/// Commits a command buffer and waits for the device to complete it.
pub fn complete(command_buffer: Id, what: &str) -> Result<(), String> {
    send!((); command_buffer, "commit")?;
    wait_until_completed(command_buffer, what)
}

/// Waits for a committed command buffer and checks that the device completed it.
pub fn wait_until_completed(command_buffer: Id, what: &str) -> Result<(), String> {
    send!((); command_buffer, "waitUntilCompleted")?;
    let status = send!(usize; command_buffer, "status")?;
    if status == values::COMMAND_BUFFER_STATUS_COMPLETED { Ok(()) } else { Err(format!("the command buffer of {what} ended with {}", failure(command_buffer, status))) }
}

/// Polls the status of a committed command buffer until the device has
/// completed it, without blocking in the framework; returns the polls made.
pub fn poll_until_completed(command_buffer: Id, what: &str) -> Result<u64, String> {
    let started = Instant::now();
    let mut polls = 0;
    loop {
        polls += 1;
        match send!(usize; command_buffer, "status")? {
            values::COMMAND_BUFFER_STATUS_COMPLETED => return Ok(polls),
            values::COMMAND_BUFFER_STATUS_ERROR => return Err(format!("the command buffer of {what} ended with {}", failure(command_buffer, values::COMMAND_BUFFER_STATUS_ERROR))),
            status if started.elapsed() > POLL_LIMIT => {
                return Err(format!("the command buffer of {what} is not completed after {} seconds and {polls} polls: {}", POLL_LIMIT.as_secs(), failure(command_buffer, status)));
            }
            _ => std::thread::sleep(POLL_INTERVAL),
        }
    }
}

/// Records the copy of the first level of a texture to a buffer, tight rows.
pub fn encode_copy_to_buffer(command_buffer: Id, texture: Id, buffer: Id, width: usize, height: usize, bytes_per_pixel: usize) -> Result<(), String> {
    let encoder = present(send!(Id; command_buffer, "blitCommandEncoder")?, "blitCommandEncoder")?;
    let bytes_per_row = width * bytes_per_pixel;
    let copied = send!(
        ();
        encoder,
        "copyFromTexture:sourceSlice:sourceLevel:sourceOrigin:sourceSize:toBuffer:destinationOffset:destinationBytesPerRow:destinationBytesPerImage:",
        Id: texture,
        usize: 0,
        usize: 0,
        values::Origin: values::Origin { x: 0, y: 0, z: 0 },
        values::Size: values::Size { width, height, depth: 1 },
        Id: buffer,
        usize: 0,
        usize: bytes_per_row,
        usize: bytes_per_row * height
    );
    end_encoding(encoder, copied)
}

/// A copy of the first `length` bytes of a shared buffer.
pub fn buffer_bytes(buffer: &Owned, length: usize, what: &str) -> Result<Vec<u8>, String> {
    let contents = present(send!(*mut c_void; buffer.id(), "contents")?, &format!("contents of the buffer of {what}"))?;
    let available = send!(usize; buffer.id(), "length")?;
    if available < length {
        return Err(format!("the buffer of {what} holds {available} bytes where {length} are expected"));
    }
    // SAFETY: `contents` points to the `available` bytes of a shared buffer
    // that is alive for the whole call, and `length` does not exceed them.
    Ok(unsafe { std::slice::from_raw_parts(contents as *const u8, length) }.to_vec())
}
