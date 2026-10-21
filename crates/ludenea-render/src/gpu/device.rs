//! A device without a window, for offscreen rendering, tests, and the command line, and a
//! device with a window's surface, for a game's native window.

use super::RenderError;

/// Opens a device on a hardware adapter, or on the fallback (software) adapter when there is
/// no hardware adapter or its device cannot be opened. `RenderError::NoAdapter` lists what
/// was tried and why each failed.
pub async fn headless() -> Result<(wgpu::Device, wgpu::Queue), RenderError> {
    headless_with_limits(|limits| limits).await
}

/// `headless` with the device's limits changed by `adjust`, which receives the limits
/// `headless` asks for: the broadly supported limits, raised to the adapter's texture size so
/// atlas pages can use it. For a test that needs a smaller limit, such as `max_buffer_size`.
pub async fn headless_with_limits(
    adjust: impl Fn(wgpu::Limits) -> wgpu::Limits,
) -> Result<(wgpu::Device, wgpu::Queue), RenderError> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_adapter, device, queue) = open(&instance, None, "ludenea headless", adjust).await?;
    Ok((device, queue))
}

/// Opens a surface on `window` and a device that can present to it, on a hardware adapter or
/// on the fallback adapter as `headless` does, failing with the same `RenderError::NoAdapter`
/// (or `RenderError::Gpu` when the window has no surface). The instance is made with the
/// window's display handle. The adapter is returned too: the surface's formats and present
/// modes depend on it.
#[cfg(not(target_arch = "wasm32"))]
pub async fn for_surface<W>(
    window: W,
) -> Result<
    (
        wgpu::Surface<'static>,
        wgpu::Adapter,
        wgpu::Device,
        wgpu::Queue,
    ),
    RenderError,
>
where
    W: wgpu::WindowHandle + wgpu::wgt::WgpuHasDisplayHandle + Clone,
{
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
        Box::new(window.clone()),
    ));
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Window(Box::new(window)))
        .map_err(|error| RenderError::Gpu {
            operation: "create_surface",
            message: error.to_string(),
        })?;
    let (adapter, device, queue) =
        open(&instance, Some(&surface), "ludenea window", |limits| limits).await?;
    Ok((surface, adapter, device, queue))
}

/// A device on a hardware adapter, else on the fallback adapter, compatible with `surface`
/// when one is given.
async fn open(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
    label: &str,
    adjust: impl Fn(wgpu::Limits) -> wgpu::Limits,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), RenderError> {
    let mut tried = Vec::new();
    for (name, force_fallback_adapter) in [
        ("a hardware adapter", false),
        ("the fallback adapter", true),
    ] {
        let options = wgpu::RequestAdapterOptions {
            force_fallback_adapter,
            compatible_surface: surface,
            ..Default::default()
        };
        let adapter = match instance.request_adapter(&options).await {
            Ok(adapter) => adapter,
            Err(error) => {
                tried.push(format!("{name} ({error})"));
                continue;
            }
        };
        let required_limits =
            adjust(wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()));
        let device = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some(label),
                required_limits,
                ..Default::default()
            })
            .await;
        match device {
            Ok((device, queue)) => return Ok((adapter, device, queue)),
            Err(error) => tried.push(format!(
                "{name}, {} (its device could not be opened: {error})",
                adapter.get_info().name
            )),
        }
    }
    Err(RenderError::NoAdapter(tried.join(", then ")))
}
