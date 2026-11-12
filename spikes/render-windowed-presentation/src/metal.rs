//! Presentation to a window through Metal, natively (macOS): the classes
//! reached through the Objective-C runtime by name, with no binding crate. A
//! layer is given to the window's view on the platform thread, as AppKit
//! asks; the presentation thread then takes its drawables, clears each with
//! a render pass that draws nothing, and presents it. Throwaway, never
//! product code.

use std::ffi::{CStr, c_char, c_void};

use crate::clock::now_ns;
use crate::host::NativeWindow;
use crate::json::Json;
use crate::presenter::{DeviceInfo, FrameOutcome, Mode, Presenter, Request, frame_color};

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCopyAllDevices() -> Id;
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClearColor {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

/// Sends a message with the given argument and return types: the runtime's
/// one entry point, called through the signature of the method.
macro_rules! send {
    ($receiver:expr, $selector:literal, fn($($arg_type:ty),*) -> $ret:ty $(, $arg:expr)*) => {{
        let function: unsafe extern "C" fn(Id, Sel $(, $arg_type)*) -> $ret = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        function($receiver, sel_registerName(concat!($selector, "\0").as_ptr().cast()) $(, $arg)*)
    }};
}

fn class(name: &CStr) -> Result<Id, String> {
    // SAFETY: the name is NUL-terminated.
    let class = unsafe { objc_getClass(name.as_ptr()) };
    if class.is_null() { Err(format!("the Objective-C class {} is not loaded", name.to_string_lossy())) } else { Ok(class) }
}

const PIXEL_FORMAT_RGBA8_UNORM: u64 = 70;
const PIXEL_FORMAT_BGRA8_UNORM: u64 = 80;
const LOAD_ACTION_CLEAR: u64 = 2;
const STORE_ACTION_STORE: u64 = 1;
const TEXTURE_USAGE_RENDER_TARGET: u64 = 4;
const STORAGE_MODE_PRIVATE: u64 = 2;
const OFFSCREEN_SIZE: u64 = 256;

/// The devices Metal lists, as retained objects with their names.
fn all_devices() -> Vec<(Id, String)> {
    let mut devices = Vec::new();
    // SAFETY: the function returns a retained array of devices; each device
    // is retained here before the array is released.
    unsafe {
        let array = MTLCopyAllDevices();
        if array.is_null() {
            return devices;
        }
        let count = send!(array, "count", fn() -> u64);
        for index in 0..count {
            let device = send!(array, "objectAtIndex:", fn(u64) -> Id, index);
            send!(device, "retain", fn() -> Id);
            let name = send!(device, "name", fn() -> Id);
            let text = send!(name, "UTF8String", fn() -> *const c_char);
            devices.push((device, if text.is_null() { "unnamed".to_string() } else { CStr::from_ptr(text).to_string_lossy().into_owned() }));
        }
        send!(array, "release", fn() -> ());
    }
    devices
}

fn release_all(devices: Vec<(Id, String)>, keep: Option<usize>) {
    for (index, (device, _)) in devices.into_iter().enumerate() {
        if Some(index) != keep {
            // SAFETY: the device was retained by `all_devices`.
            unsafe { send!(device, "release", fn() -> ()) };
        }
    }
}

/// The devices Metal lists on this machine, in its order.
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    // SAFETY: the pool is pushed and popped on this thread around the calls.
    let pool = unsafe { objc_autoreleasePoolPush() };
    let listed = all_devices();
    let infos = listed.iter().enumerate().map(|(index, (_, name))| DeviceInfo { index, name: name.clone(), kind: "as Metal lists it".to_string() }).collect();
    release_all(listed, None);
    // SAFETY: the pool was pushed above.
    unsafe { objc_autoreleasePoolPop(pool) };
    Ok(infos)
}

/// The content view of a window, for a library that gives the window only.
pub fn content_view(window: usize) -> usize {
    if window == 0 {
        return 0;
    }
    // SAFETY: the pointer is a live NSWindow the library gave.
    unsafe { send!(window as Id, "contentView", fn() -> Id) as usize }
}

/// Creates a Metal layer and gives it to the window's view. Called on the
/// platform thread: AppKit's views are the main thread's. The layer returned
/// carries one reference, which the presenter releases.
pub fn attach_layer(window: NativeWindow) -> Result<usize, String> {
    let NativeWindow::AppKit { view } = window else {
        return Err("Metal presents to an AppKit view only".to_string());
    };
    if view == 0 {
        return Err("the library gave no view".to_string());
    }
    let layer_class = class(c"CAMetalLayer")?;
    // SAFETY: the class is loaded; the view is live and this is its thread.
    unsafe {
        let layer = send!(send!(layer_class, "alloc", fn() -> Id), "init", fn() -> Id);
        if layer.is_null() {
            return Err("CAMetalLayer could not be created".to_string());
        }
        send!(view as Id, "setLayer:", fn(Id) -> (), layer);
        send!(view as Id, "setWantsLayer:", fn(bool) -> (), true);
        Ok(layer as usize)
    }
}

pub struct MetalPresenter {
    mode: Mode,
    device_name: String,
    created_on_thread: String,
    offscreen: Id,
    queue: Id,
    layer: Id,
    device: Id,
}

impl MetalPresenter {
    pub fn open(request: &Request, layer: usize) -> Result<MetalPresenter, String> {
        if layer == 0 {
            return Err("no layer was given to the window's view".to_string());
        }
        if request.mode == Mode::Latest {
            return Err("not offered: a Metal layer presents in step with the display or not, with no mode that replaces a waiting frame".to_string());
        }
        let layer = layer as Id;
        let descriptor_class = class(c"MTLTextureDescriptor")?;
        // SAFETY: every receiver is a live object of the class the selector
        // belongs to; the argument and return types are those of the methods.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let mut listed = all_devices();
            if request.device >= listed.len() {
                release_all(listed, None);
                objc_autoreleasePoolPop(pool);
                return Err(format!("no device at index {}", request.device));
            }
            let (device, device_name) = listed[request.device].clone();
            release_all(std::mem::take(&mut listed), Some(request.device));
            send!(layer, "setDevice:", fn(Id) -> (), device);
            send!(layer, "setPixelFormat:", fn(u64) -> (), PIXEL_FORMAT_BGRA8_UNORM);
            send!(layer, "setFramebufferOnly:", fn(bool) -> (), true);
            send!(layer, "setDrawableSize:", fn(Size) -> (), Size { width: f64::from(request.width.max(1)), height: f64::from(request.height.max(1)) });
            send!(layer, "setDisplaySyncEnabled:", fn(bool) -> (), request.mode == Mode::Synchronized);
            let queue = send!(device, "newCommandQueue", fn() -> Id);
            let descriptor = send!(descriptor_class, "texture2DDescriptorWithPixelFormat:width:height:mipmapped:", fn(u64, u64, u64, bool) -> Id, PIXEL_FORMAT_RGBA8_UNORM, OFFSCREEN_SIZE, OFFSCREEN_SIZE, false);
            send!(descriptor, "setUsage:", fn(u64) -> (), TEXTURE_USAGE_RENDER_TARGET);
            send!(descriptor, "setStorageMode:", fn(u64) -> (), STORAGE_MODE_PRIVATE);
            let offscreen = send!(device, "newTextureWithDescriptor:", fn(Id) -> Id, descriptor);
            objc_autoreleasePoolPop(pool);
            if queue.is_null() || offscreen.is_null() {
                return Err("Metal created no command queue or no offscreen texture".to_string());
            }
            Ok(MetalPresenter { mode: request.mode, device_name, created_on_thread: std::thread::current().name().unwrap_or("unnamed").to_string(), offscreen, queue, layer, device })
        }
    }

    /// A command buffer whose one pass clears `texture` to `color` and draws
    /// nothing, not yet committed.
    ///
    /// # Safety
    ///
    /// `texture` must be a live Metal texture, inside an autorelease pool.
    unsafe fn clear_pass(&self, texture: Id, color: [f32; 4]) -> Result<Id, String> {
        let pass_class = class(c"MTLRenderPassDescriptor")?;
        // SAFETY: as the function's contract; the selectors belong to the
        // classes of their receivers.
        unsafe {
            let pass = send!(pass_class, "renderPassDescriptor", fn() -> Id);
            let attachment = send!(send!(pass, "colorAttachments", fn() -> Id), "objectAtIndexedSubscript:", fn(u64) -> Id, 0);
            send!(attachment, "setTexture:", fn(Id) -> (), texture);
            send!(attachment, "setLoadAction:", fn(u64) -> (), LOAD_ACTION_CLEAR);
            send!(attachment, "setStoreAction:", fn(u64) -> (), STORE_ACTION_STORE);
            send!(attachment, "setClearColor:", fn(ClearColor) -> (), ClearColor { red: f64::from(color[0]), green: f64::from(color[1]), blue: f64::from(color[2]), alpha: 1.0 });
            let buffer = send!(self.queue, "commandBuffer", fn() -> Id);
            let encoder = send!(buffer, "renderCommandEncoderWithDescriptor:", fn(Id) -> Id, pass);
            if buffer.is_null() || encoder.is_null() {
                return Err("Metal gave no command buffer or no encoder".to_string());
            }
            send!(encoder, "endEncoding", fn() -> ());
            Ok(buffer)
        }
    }
}

impl Presenter for MetalPresenter {
    fn describe(&self) -> Json {
        Json::object()
            .with("interface", "Metal with a layer of the window's view, natively")
            .with("device", self.device_name.clone())
            .with("device_kind", "as Metal lists it")
            .with("swapchain", format!("the layer's drawables, {}", if self.mode == Mode::Synchronized { "in step with the display" } else { "not in step with the display" }))
            .with("created_on_thread", format!("the layer on the platform thread, the device and the queue on the thread {}", self.created_on_thread))
            .with("acquisition", "the layer's call for its next drawable, which waits up to one second and may give nothing")
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width != 0 && height != 0 {
            // SAFETY: the layer is live.
            unsafe { send!(self.layer, "setDrawableSize:", fn(Size) -> (), Size { width: f64::from(width), height: f64::from(height) }) };
        }
        Ok(())
    }

    fn frame(&mut self, index: u64) -> Result<FrameOutcome, String> {
        // SAFETY: the layer and the queue are live; the pool pushed here is
        // popped on every path; the drawable and its texture live in it.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let started = now_ns();
            crate::presenter::in_call(crate::presenter::ACQUISITION);
            let drawable = send!(self.layer, "nextDrawable", fn() -> Id);
            let acquired = now_ns();
            if drawable.is_null() {
                objc_autoreleasePoolPop(pool);
                return Ok(FrameOutcome::nothing(acquired - started, "no image: the layer gave no drawable"));
            }
            crate::presenter::in_call(crate::presenter::SUBMISSION);
            let texture = send!(drawable, "texture", fn() -> Id);
            let buffer = match self.clear_pass(texture, frame_color(index)) {
                Ok(buffer) => buffer,
                Err(error) => {
                    objc_autoreleasePoolPop(pool);
                    return Err(error);
                }
            };
            send!(buffer, "presentDrawable:", fn(Id) -> (), drawable);
            let submitted = now_ns();
            crate::presenter::in_call(crate::presenter::PRESENTATION);
            send!(buffer, "commit", fn() -> ());
            let presented = now_ns();
            crate::presenter::in_call(crate::presenter::BETWEEN_CALLS);
            objc_autoreleasePoolPop(pool);
            Ok(FrameOutcome { acquire_ns: acquired - started, submit_ns: submitted - acquired, wait_ns: 0, present_ns: presented - submitted, presented: true, status: "presented".to_string() })
        }
    }

    fn offscreen(&mut self, index: u64) -> Result<(), String> {
        // SAFETY: the texture and the queue are live; the pool pushed here
        // is popped on every path.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let result = self.clear_pass(self.offscreen, frame_color(index)).map(|buffer| {
                send!(buffer, "commit", fn() -> ());
                send!(buffer, "waitUntilCompleted", fn() -> ());
            });
            objc_autoreleasePoolPop(pool);
            result
        }
    }

    fn presentation_times(&mut self) -> Json {
        Json::object().with("source", "none read: Metal gives a drawable's presented time to a handler block, which the spike does not register")
    }
}

impl Drop for MetalPresenter {
    fn drop(&mut self) {
        // SAFETY: each object carries one reference this value owns.
        unsafe {
            send!(self.offscreen, "release", fn() -> ());
            send!(self.queue, "release", fn() -> ());
            send!(self.layer, "release", fn() -> ());
            send!(self.device, "release", fn() -> ());
        }
    }
}
