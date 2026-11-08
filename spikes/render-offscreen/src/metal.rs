//! The Metal path (macOS): the scene rendered to an offscreen texture by the
//! default Metal device, read back with `getBytes`. Throwaway, never product
//! code.
//!
//! Metal is reached through the Objective-C runtime, as the platform threading
//! constraints spike reached AppKit; the framework itself is loaded at run time
//! so that the binary imports no graphics library. The shader source is compiled
//! at run time by the system's Metal compiler through `newLibraryWithSource`.

use std::ffi::{CStr, c_char, c_void};
use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::loader::{Library, c_string};
use crate::scene::{Frame, HEIGHT, Scene, WIDTH};
use crate::{FrameResult, Outcome};

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C-unwind" {
    fn objc_msgSend();
}

/// `MTLClearColor`: four doubles, passed in floating-point registers on ARM64.
#[repr(C)]
#[derive(Clone, Copy)]
struct ClearColor {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

/// `MTLRegion`: an origin and a size of three `NSUInteger` each, passed by
/// reference to a copy on ARM64 since it exceeds sixteen bytes.
#[repr(C)]
#[derive(Clone, Copy)]
struct Region {
    x: usize,
    y: usize,
    z: usize,
    width: usize,
    height: usize,
    depth: usize,
}

const PIXEL_FORMAT_RGBA8_UNORM: u64 = 70;
const TEXTURE_USAGE_RENDER_TARGET: u64 = 4;
const STORAGE_MODE_SHARED: u64 = 0;
const RESOURCE_STORAGE_MODE_SHARED: u64 = 0;
const LOAD_ACTION_CLEAR: u64 = 2;
const STORE_ACTION_STORE: u64 = 1;
const PRIMITIVE_TYPE_TRIANGLE: u64 = 3;

fn selector(name: &str) -> Sel {
    let name = c_string(name);
    unsafe { sel_registerName(name.as_ptr()) }
}

fn class(name: &str) -> Result<Id, String> {
    let c_name = c_string(name);
    let class = unsafe { objc_getClass(c_name.as_ptr()) };
    if class.is_null() { Err(format!("Objective-C class {name} not found")) } else { Ok(class) }
}

/// `[receiver name:args...]` returning an object pointer. The receiver is
/// evaluated first so that a nested send stays outside the unsafe block.
macro_rules! msg {
    ($receiver:expr, $name:literal $(, $arg_type:ty : $arg:expr)*) => {{
        let receiver: Id = $receiver;
        let selector = selector($name);
        let call: unsafe extern "C-unwind" fn(Id, Sel $(, $arg_type)*) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
        #[allow(unused_unsafe)]
        unsafe { call(receiver, selector $(, $arg)*) }
    }};
}

/// `[receiver name:args...]` returning nothing.
macro_rules! msg_void {
    ($receiver:expr, $name:literal $(, $arg_type:ty : $arg:expr)*) => {{
        let receiver: Id = $receiver;
        let selector = selector($name);
        let call: unsafe extern "C-unwind" fn(Id, Sel $(, $arg_type)*) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
        #[allow(unused_unsafe)]
        unsafe { call(receiver, selector $(, $arg)*) }
    }};
}

/// The text of an `NSString` (or an `NSError`'s description).
fn string_of(object: Id) -> String {
    if object.is_null() {
        return String::new();
    }
    let call: unsafe extern "C-unwind" fn(Id, Sel) -> *const c_char =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
    let text = unsafe { call(object, selector("UTF8String")) };
    if text.is_null() { String::new() } else { unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned() }
}

fn error_text(error: Id, what: &str) -> String {
    if error.is_null() {
        return format!("{what} failed without an error object");
    }
    let description = msg!(error, "localizedDescription");
    format!("{what}: {}", string_of(description))
}

fn ns_string(text: &str) -> Result<Id, String> {
    let c_text = c_string(text);
    let string = msg!(class("NSString")?, "stringWithUTF8String:", *const c_char: c_text.as_ptr());
    if string.is_null() { Err("NSString creation failed".to_string()) } else { Ok(string) }
}

struct Pool(*mut c_void);

impl Pool {
    fn new() -> Pool {
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) };
    }
}

type CreateSystemDefaultDevice = unsafe extern "C" fn() -> Id;

/// One complete render: device, library, pipeline, texture, one frame, read back.
fn render_once(create_device: CreateSystemDefaultDevice, source: &str, scene: &Scene, outcome: &mut Outcome) -> Result<Frame, String> {
    let _pool = Pool::new();
    let started = Instant::now();
    let device = unsafe { create_device() };
    if device.is_null() {
        return Err("MTLCreateSystemDefaultDevice returned nil".to_string());
    }
    outcome.device_name = string_of(msg!(device, "name"));
    let low_power: bool = {
        let call: unsafe extern "C-unwind" fn(Id, Sel) -> bool = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
        unsafe { call(device, selector("isLowPower")) }
    };
    let registry_id: u64 = {
        let call: unsafe extern "C-unwind" fn(Id, Sel) -> u64 = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
        unsafe { call(device, selector("registryID")) }
    };
    outcome.device_type = if low_power { "low power gpu".to_string() } else { "gpu".to_string() };
    outcome.driver = format!("registry id {registry_id:#x}");
    outcome.device_us = started.elapsed().as_secs_f64() * 1_000_000.0;

    let pipeline_started = Instant::now();
    let mut error: Id = std::ptr::null_mut();
    let library = msg!(device, "newLibraryWithSource:options:error:", Id: ns_string(source)?, Id: std::ptr::null_mut(), *mut Id: &mut error);
    if library.is_null() {
        return Err(error_text(error, "newLibraryWithSource"));
    }
    let vertex_function = msg!(library, "newFunctionWithName:", Id: ns_string("vertex_main")?);
    let fragment_function = msg!(library, "newFunctionWithName:", Id: ns_string("fragment_main")?);
    if vertex_function.is_null() || fragment_function.is_null() {
        return Err("the shader functions were not found in the compiled library".to_string());
    }
    let descriptor = msg!(msg!(class("MTLRenderPipelineDescriptor")?, "alloc"), "init");
    msg_void!(descriptor, "setVertexFunction:", Id: vertex_function);
    msg_void!(descriptor, "setFragmentFunction:", Id: fragment_function);
    let color_attachment = msg!(msg!(descriptor, "colorAttachments"), "objectAtIndexedSubscript:", usize: 0);
    msg_void!(color_attachment, "setPixelFormat:", u64: PIXEL_FORMAT_RGBA8_UNORM);
    let mut error: Id = std::ptr::null_mut();
    let pipeline = msg!(device, "newRenderPipelineStateWithDescriptor:error:", Id: descriptor, *mut Id: &mut error);
    if pipeline.is_null() {
        return Err(error_text(error, "newRenderPipelineStateWithDescriptor"));
    }
    let texture_descriptor = msg!(
        class("MTLTextureDescriptor")?,
        "texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
        u64: PIXEL_FORMAT_RGBA8_UNORM,
        usize: WIDTH as usize,
        usize: HEIGHT as usize,
        bool: false
    );
    msg_void!(texture_descriptor, "setUsage:", u64: TEXTURE_USAGE_RENDER_TARGET);
    msg_void!(texture_descriptor, "setStorageMode:", u64: STORAGE_MODE_SHARED);
    let texture = msg!(device, "newTextureWithDescriptor:", Id: texture_descriptor);
    if texture.is_null() {
        return Err("newTextureWithDescriptor returned nil".to_string());
    }
    let vertex_bytes = scene.vertex_bytes();
    let vertex_buffer = msg!(
        device,
        "newBufferWithBytes:length:options:",
        *const c_void: vertex_bytes.as_ptr() as *const c_void,
        usize: vertex_bytes.len(),
        u64: RESOURCE_STORAGE_MODE_SHARED
    );
    if vertex_buffer.is_null() {
        return Err("newBufferWithBytes returned nil".to_string());
    }
    let queue = msg!(device, "newCommandQueue");
    outcome.pipeline_us = pipeline_started.elapsed().as_secs_f64() * 1_000_000.0;

    let render_started = Instant::now();
    let command_buffer = msg!(queue, "commandBuffer");
    let pass = msg!(class("MTLRenderPassDescriptor")?, "renderPassDescriptor");
    let attachment = msg!(msg!(pass, "colorAttachments"), "objectAtIndexedSubscript:", usize: 0);
    msg_void!(attachment, "setTexture:", Id: texture);
    msg_void!(attachment, "setLoadAction:", u64: LOAD_ACTION_CLEAR);
    msg_void!(attachment, "setStoreAction:", u64: STORE_ACTION_STORE);
    msg_void!(attachment, "setClearColor:", ClearColor: ClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 1.0 });
    let encoder = msg!(command_buffer, "renderCommandEncoderWithDescriptor:", Id: pass);
    if encoder.is_null() {
        return Err("renderCommandEncoderWithDescriptor returned nil".to_string());
    }
    msg_void!(encoder, "setRenderPipelineState:", Id: pipeline);
    msg_void!(encoder, "setVertexBuffer:offset:atIndex:", Id: vertex_buffer, usize: 0, usize: 0);
    msg_void!(encoder, "drawPrimitives:vertexStart:vertexCount:", u64: PRIMITIVE_TYPE_TRIANGLE, usize: 0, usize: scene.vertices.len());
    msg_void!(encoder, "endEncoding");
    msg_void!(command_buffer, "commit");
    msg_void!(command_buffer, "waitUntilCompleted");
    let status: u64 = {
        let call: unsafe extern "C-unwind" fn(Id, Sel) -> u64 = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
        unsafe { call(command_buffer, selector("status")) }
    };
    // MTLCommandBufferStatusCompleted is 4; 5 is an error.
    if status != 4 {
        let error = msg!(command_buffer, "error");
        return Err(error_text(error, &format!("command buffer ended with status {status}")));
    }
    let render_us = render_started.elapsed().as_secs_f64() * 1_000_000.0;

    let readback_started = Instant::now();
    let row_bytes = WIDTH as usize * 4;
    let mut pixels = vec![0_u8; row_bytes * HEIGHT as usize];
    let region = Region { x: 0, y: 0, z: 0, width: WIDTH as usize, height: HEIGHT as usize, depth: 1 };
    msg_void!(
        texture,
        "getBytes:bytesPerRow:fromRegion:mipmapLevel:",
        *mut c_void: pixels.as_mut_ptr() as *mut c_void,
        usize: row_bytes,
        Region: region,
        usize: 0
    );
    let frame = Frame { width: WIDTH, height: HEIGHT, rgba: pixels };
    let readback_us = readback_started.elapsed().as_secs_f64() * 1_000_000.0;
    outcome.frames.push(FrameResult { hash: frame.hash(), lit_pixels: frame.lit_pixels(), render_us, readback_us });

    for object in [queue, vertex_buffer, texture, pipeline, descriptor, fragment_function, vertex_function, library, device] {
        msg_void!(object, "release");
    }
    Ok(frame)
}

/// Renders the scene `runs` times from scratch and reports every frame.
pub fn render(scene: &Scene, shaders: &Path, runs: usize) -> Outcome {
    let mut outcome = Outcome::new("metal");
    let metal = match Library::open(&["/System/Library/Frameworks/Metal.framework/Metal"]) {
        Ok(library) => library,
        Err(error) => {
            outcome.reason = format!("Metal framework not loadable: {error}");
            return outcome;
        }
    };
    // Foundation provides NSString; loading it by path keeps it a run-time choice too.
    let _foundation = match Library::open(&["/System/Library/Frameworks/Foundation.framework/Foundation"]) {
        Ok(library) => library,
        Err(error) => {
            outcome.reason = format!("Foundation framework not loadable: {error}");
            return outcome;
        }
    };
    outcome.loader = metal.name.clone();
    outcome.shader_compiler = "the system's Metal compiler (newLibraryWithSource)".to_string();
    // SAFETY: the framework exports this function with this signature.
    let create_device: CreateSystemDefaultDevice = match unsafe { metal.function("MTLCreateSystemDefaultDevice") } {
        Ok(function) => function,
        Err(error) => {
            outcome.reason = error;
            return outcome;
        }
    };
    let source = match fs::read_to_string(shaders.join("scene.metal")) {
        Ok(source) => source,
        Err(error) => {
            outcome.reason = format!("scene.metal: {error}");
            return outcome;
        }
    };
    for _ in 0..runs {
        match render_once(create_device, &source, scene, &mut outcome) {
            Ok(frame) => {
                if outcome.frame.is_none() {
                    outcome.frame = Some(frame);
                }
            }
            Err(error) => {
                outcome.reason = error;
                return outcome;
            }
        }
    }
    outcome.available = true;
    outcome
}
