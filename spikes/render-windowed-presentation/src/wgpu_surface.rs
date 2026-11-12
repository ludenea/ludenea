//! Presentation to a window through wgpu's surface, at its safe level, over
//! one interface at a time. Throwaway, never product code.

use std::ffi::c_void;
use std::num::{NonZeroIsize, NonZeroU32};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, Win32WindowHandle,
    WindowsDisplayHandle, XcbDisplayHandle, XcbWindowHandle, XlibDisplayHandle, XlibWindowHandle,
};

use crate::clock::now_ns;
use crate::host::NativeWindow;
use crate::json::Json;
use crate::presenter::{DeviceInfo, FrameOutcome, Mode, Presenter, Request, frame_color};

pub const VERSION: &str = "30.0.1";
pub const LICENSE: &str = "MIT OR Apache-2.0";
const OFFSCREEN_SIZE: u32 = 256;

/// Runs a future to completion on the calling thread. The futures of wgpu's
/// native backends are ready at once or after the device was polled, so the
/// loop yields between polls and needs no reactor.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

fn backends(path: &str) -> Result<wgpu::Backends, String> {
    match path {
        "wgpu-direct3d12" => Ok(wgpu::Backends::DX12),
        "wgpu-vulkan" => Ok(wgpu::Backends::VULKAN),
        "wgpu-metal" => Ok(wgpu::Backends::METAL),
        other => Err(format!("{other} is not a path through wgpu")),
    }
}

fn instance(backends: wgpu::Backends) -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    wgpu::Instance::new(descriptor)
}

fn kind(device_type: wgpu::DeviceType) -> &'static str {
    match device_type {
        wgpu::DeviceType::Other => "other",
        wgpu::DeviceType::IntegratedGpu => "integrated",
        wgpu::DeviceType::DiscreteGpu => "discrete",
        wgpu::DeviceType::VirtualGpu => "virtual",
        wgpu::DeviceType::Cpu => "software",
    }
}

/// The adapters wgpu lists for the interface of `path`, in its order.
pub fn devices(path: &str) -> Result<Vec<DeviceInfo>, String> {
    let backends = backends(path)?;
    let instance = instance(backends);
    Ok(block_on(instance.enumerate_adapters(backends))
        .iter()
        .enumerate()
        .map(|(index, adapter)| {
            let info = adapter.get_info();
            DeviceInfo { index, name: info.name, kind: kind(info.device_type).to_string() }
        })
        .collect())
}

fn handles(window: NativeWindow) -> Result<(RawDisplayHandle, RawWindowHandle), String> {
    let pointer = |address: usize| NonNull::new(address as *mut c_void).ok_or("a null native handle".to_string());
    Ok(match window {
        NativeWindow::Win32 { hwnd, hinstance } => {
            let mut handle = Win32WindowHandle::new(NonZeroIsize::new(hwnd as isize).ok_or("a null window handle")?);
            handle.hinstance = NonZeroIsize::new(hinstance as isize);
            (RawDisplayHandle::Windows(WindowsDisplayHandle::new()), RawWindowHandle::Win32(handle))
        }
        NativeWindow::Xlib { display, window } => (RawDisplayHandle::Xlib(XlibDisplayHandle::new(Some(pointer(display)?), 0)), RawWindowHandle::Xlib(XlibWindowHandle::new(window as _))),
        NativeWindow::Xcb { connection, window } => {
            (RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(pointer(connection)?), 0)), RawWindowHandle::Xcb(XcbWindowHandle::new(NonZeroU32::new(window).ok_or("a null window")?)))
        }
        NativeWindow::Wayland { display, surface } => (RawDisplayHandle::Wayland(WaylandDisplayHandle::new(pointer(display)?)), RawWindowHandle::Wayland(WaylandWindowHandle::new(pointer(surface)?))),
        NativeWindow::AppKit { view } => (RawDisplayHandle::AppKit(AppKitDisplayHandle::new()), RawWindowHandle::AppKit(AppKitWindowHandle::new(pointer(view)?))),
    })
}

/// An instance over the interface of the path and its surface for the window,
/// made on the calling thread.
pub fn surface_on_this_thread(request: &Request) -> Result<(wgpu::Instance, wgpu::Surface<'static>), String> {
    let instance = instance(backends(&request.path)?);
    let (display, window) = handles(request.window)?;
    // SAFETY: the window outlives the presenter: the platform thread closes
    // it only after the presentation thread has ended.
    let surface = unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(display), raw_window_handle: window }) }.map_err(|error| format!("wgpu created no surface: {error}"))?;
    Ok((instance, surface))
}

pub struct WgpuPresenter {
    surface_made_on: &'static str,
    info: wgpu::AdapterInfo,
    offered_modes: Vec<wgpu::PresentMode>,
    created_on_thread: String,
    errors: Arc<Mutex<Vec<String>>>,
    configuration: wgpu::SurfaceConfiguration,
    offscreen_view: wgpu::TextureView,
    _offscreen: wgpu::Texture,
    surface: wgpu::Surface<'static>,
    queue: wgpu::Queue,
    device: wgpu::Device,
    _instance: wgpu::Instance,
}

impl WgpuPresenter {
    pub fn open(request: &Request, prepared: Option<(wgpu::Instance, wgpu::Surface<'static>)>) -> Result<WgpuPresenter, String> {
        let backends = backends(&request.path)?;
        let surface_made_on = if prepared.is_some() { "the platform thread" } else { "the thread that presents" };
        let (instance, surface) = match prepared {
            Some(made) => made,
            None => surface_on_this_thread(request)?,
        };
        let mut adapters = block_on(instance.enumerate_adapters(backends));
        if request.device >= adapters.len() {
            return Err(format!("no adapter at index {}", request.device));
        }
        let adapter = adapters.swap_remove(request.device);
        drop(adapters);
        let info = adapter.get_info();
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("windowed presentation"), ..Default::default() })).map_err(|error| format!("wgpu created no device: {error}"))?;
        let errors: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = errors.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            if let Ok(mut errors) = sink.lock() {
                errors.push(error.to_string().split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }));
        let capabilities = surface.get_capabilities(&adapter);
        if capabilities.formats.is_empty() {
            return Err("the adapter does not present to this surface: wgpu lists no format for it".to_string());
        }
        let present_mode = match request.mode {
            Mode::Synchronized => wgpu::PresentMode::Fifo,
            Mode::Latest => wgpu::PresentMode::Mailbox,
            Mode::Unsynchronized => wgpu::PresentMode::Immediate,
        };
        if !capabilities.present_modes.contains(&present_mode) {
            return Err(format!("not offered: wgpu lists the presentation modes {:?}", capabilities.present_modes));
        }
        let mut configuration = surface.get_default_config(&adapter, request.width.max(1), request.height.max(1)).ok_or("wgpu gave no default configuration for the surface")?;
        configuration.present_mode = present_mode;
        surface.configure(&device, &configuration);
        let offscreen = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen stream"),
            size: wgpu::Extent3d { width: OFFSCREEN_SIZE, height: OFFSCREEN_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let offscreen_view = offscreen.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(WgpuPresenter {
            info,
            offered_modes: capabilities.present_modes,
            created_on_thread: std::thread::current().name().unwrap_or("unnamed").to_string(),
            errors,
            surface_made_on,
            configuration,
            offscreen_view,
            _offscreen: offscreen,
            surface,
            queue,
            device,
            _instance: instance,
        })
    }

    /// Records a pass that clears `view` to the color of frame `index`, and
    /// submits it.
    fn clear_and_submit(&self, view: &wgpu::TextureView, index: u64) {
        let color = frame_color(index);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: f64::from(color[0]), g: f64::from(color[1]), b: f64::from(color[2]), a: 1.0 }), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
        self.queue.submit(Some(encoder.finish()));
    }

    fn reported_error(&self) -> Result<(), String> {
        match self.errors.lock() {
            Ok(mut errors) if !errors.is_empty() => Err(format!("wgpu reported: {}", errors.remove(0))),
            _ => Ok(()),
        }
    }
}

impl Presenter for WgpuPresenter {
    fn describe(&self) -> Json {
        Json::object()
            .with("interface", format!("wgpu {VERSION} at its safe level over {:?}, through its surface", self.info.backend))
            .with("device", self.info.name.clone())
            .with("device_kind", kind(self.info.device_type))
            .with("swapchain", format!("presentation mode {:?}, format {:?}, {} frames of latency asked", self.configuration.present_mode, self.configuration.format, self.configuration.desired_maximum_frame_latency))
            .with("presentation_modes_offered", format!("{:?}", self.offered_modes))
            .with("created_on_thread", format!("the surface on {}, the device on the thread {}", self.surface_made_on, self.created_on_thread))
            .with("acquisition", "the surface's call for its current texture, which waits as wgpu decides")
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 || (width, height) == (self.configuration.width, self.configuration.height) {
            return Ok(());
        }
        self.configuration.width = width;
        self.configuration.height = height;
        self.surface.configure(&self.device, &self.configuration);
        self.reported_error()
    }

    fn frame(&mut self, index: u64) -> Result<FrameOutcome, String> {
        let started = now_ns();
        let current = self.surface.get_current_texture();
        let acquired = now_ns();
        let (texture, suboptimal) = match current {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Timeout => return Ok(FrameOutcome::nothing(acquired - started, "no image: wgpu answered timeout")),
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(FrameOutcome::nothing(acquired - started, "no image: wgpu answered occluded")),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.configuration);
                return Ok(FrameOutcome::nothing(now_ns() - started, "no image: wgpu answered outdated, the surface was configured again"));
            }
            wgpu::CurrentSurfaceTexture::Lost => return Err("wgpu answered that the surface is lost".to_string()),
            wgpu::CurrentSurfaceTexture::Validation => {
                self.reported_error()?;
                return Err("wgpu answered a validation error on the acquisition".to_string());
            }
        };
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.clear_and_submit(&view, index);
        drop(view);
        let submitted = now_ns();
        self.queue.present(texture);
        let presented_at = now_ns();
        if suboptimal {
            self.surface.configure(&self.device, &self.configuration);
        }
        self.reported_error()?;
        Ok(FrameOutcome {
            acquire_ns: acquired - started,
            submit_ns: submitted - acquired,
            wait_ns: 0,
            present_ns: presented_at - submitted,
            presented: true,
            status: if suboptimal { "presented, the acquisition answered suboptimal".to_string() } else { "presented".to_string() },
        })
    }

    fn offscreen(&mut self, index: u64) -> Result<(), String> {
        self.clear_and_submit(&self.offscreen_view, index);
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|error| format!("the device could not be waited for: {error}"))?;
        self.reported_error()
    }

    fn presentation_times(&mut self) -> Json {
        Json::object().with("source", "none: wgpu's surface gives no time at which a frame was shown; its adapter gives the current time of the presentation clock only")
    }
}
