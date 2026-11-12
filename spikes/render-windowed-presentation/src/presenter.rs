//! What the spike asks of a presentation path, said once for the three
//! native interfaces and for wgpu's surface: frames presented to a window's
//! swapchain, frames of an offscreen stream served between them, and the
//! time each call takes. Throwaway, never product code.

use crate::host::NativeWindow;
use crate::json::Json;

/// How presentation relates to the display's refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// In step with the display, frames queued.
    Synchronized,
    /// In step with the display, the latest frame replacing a waiting one.
    Latest,
    /// Not in step with the display.
    Unsynchronized,
}

impl Mode {
    pub fn parse(text: &str) -> Result<Mode, String> {
        match text {
            "synchronized" => Ok(Mode::Synchronized),
            "latest" => Ok(Mode::Latest),
            "unsynchronized" => Ok(Mode::Unsynchronized),
            other => Err(format!("unknown presentation mode: {other}")),
        }
    }

}

/// What one turn of the presenter did for the window.
pub struct FrameOutcome {
    /// Time spent obtaining the swapchain's image.
    pub acquire_ns: u64,
    /// Time spent recording and submitting the frame's commands.
    pub submit_ns: u64,
    /// Time spent waiting for the device to finish commands: the work last
    /// submitted for the image, or the frame's own commands.
    pub wait_ns: u64,
    /// Time spent in the presentation call.
    pub present_ns: u64,
    /// Whether an image was presented, and otherwise what the interface said.
    pub presented: bool,
    pub status: String,
}

impl FrameOutcome {
    pub fn nothing(acquire_ns: u64, status: impl Into<String>) -> FrameOutcome {
        FrameOutcome { acquire_ns, submit_ns: 0, wait_ns: 0, present_ns: 0, presented: false, status: status.into() }
    }
}

/// One device a path can open, as the path lists it.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub index: usize,
    pub name: String,
    pub kind: String,
}

/// What a presenter is opened with.
#[derive(Clone, Debug)]
pub struct Request {
    pub path: String,
    pub device: usize,
    pub mode: Mode,
    pub window: NativeWindow,
    pub width: u32,
    pub height: u32,
    /// How long an acquisition may wait, in milliseconds, where the
    /// interface takes a limit; zero asks it not to wait.
    pub acquire_wait_ms: u32,
}

pub trait Presenter {
    /// The device, the swapchain and what the surface offers.
    fn describe(&self) -> Json;
    /// Follows a new size of the window's drawable area.
    fn resize(&mut self, width: u32, height: u32) -> Result<(), String>;
    /// Acquires the window's image, clears it to the color of frame `index`
    /// and presents it. An acquisition that gives nothing is an outcome, not
    /// an error.
    fn frame(&mut self, index: u64) -> Result<FrameOutcome, String>;
    /// Draws one frame of an offscreen stream of the same device and waits
    /// for the device to finish it.
    fn offscreen(&mut self, index: u64) -> Result<(), String>;
    /// The presentation times the platform gave, where it gives them.
    fn presentation_times(&mut self) -> Json;
}

/// The color of frame `index`: every frame differs from its neighbours.
pub fn frame_color(index: u64) -> [f32; 4] {
    [(index % 256) as f32 / 255.0, ((index / 256) % 256) as f32 / 255.0, 0.25, 1.0]
}



/// The devices a path lists on this machine, or why it lists none.
pub fn devices(path: &str) -> Result<Vec<DeviceInfo>, String> {
    match path {
        #[cfg(windows)]
        "direct3d12" => crate::d3d12::devices(),
        #[cfg(any(windows, target_os = "linux"))]
        "vulkan" => crate::vulkan::devices(),
        #[cfg(target_os = "macos")]
        "metal" => crate::metal::devices(),
        "wgpu-direct3d12" | "wgpu-vulkan" | "wgpu-metal" => crate::wgpu_surface::devices(path),
        other => Err(format!("{other} is not an interface of this platform")),
    }
}

/// Opens the presenter of a path on a device, for a window. Called on the
/// thread that presents.
/// What a path needs made on the platform thread before its presenter is
/// opened on the thread that presents: on macOS a view takes its layer on the
/// main thread, and wgpu makes its surface there. Empty elsewhere, where the
/// presenter makes everything on its own thread.
#[derive(Default)]
pub struct Prepared {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub metal_layer: usize,
    pub wgpu: Option<(wgpu::Instance, wgpu::Surface<'static>)>,
}

/// Called on the platform thread, before `open`.
pub fn prepare(request: &Request) -> Result<Prepared, String> {
    match request.path.as_str() {
        #[cfg(target_os = "macos")]
        "metal" => Ok(Prepared { metal_layer: crate::metal::attach_layer(request.window)?, wgpu: None }),
        #[cfg(target_os = "macos")]
        "wgpu-metal" => Ok(Prepared { metal_layer: 0, wgpu: Some(crate::wgpu_surface::surface_on_this_thread(request)?) }),
        _ => Ok(Prepared::default()),
    }
}

pub fn open(request: &Request, prepared: Prepared) -> Result<Box<dyn Presenter>, String> {
    let _ = &prepared;
    match request.path.as_str() {
        #[cfg(windows)]
        "direct3d12" => Ok(Box::new(crate::d3d12::Direct3d12Presenter::open(request)?)),
        #[cfg(any(windows, target_os = "linux"))]
        "vulkan" => Ok(Box::new(crate::vulkan::VulkanPresenter::open(request)?)),
        #[cfg(target_os = "macos")]
        "metal" => Ok(Box::new(crate::metal::MetalPresenter::open(request, prepared.metal_layer)?)),
        "wgpu-direct3d12" | "wgpu-vulkan" | "wgpu-metal" => Ok(Box::new(crate::wgpu_surface::WgpuPresenter::open(request, prepared.wgpu)?)),
        other => Err(format!("{other} is not an interface of this platform")),
    }
}
