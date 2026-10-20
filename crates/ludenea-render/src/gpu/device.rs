//! A device without a window, for offscreen rendering, tests, and the command line.

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
    let mut tried = Vec::new();
    for (name, force_fallback_adapter) in [
        ("a hardware adapter", false),
        ("the fallback adapter", true),
    ] {
        let options = wgpu::RequestAdapterOptions {
            force_fallback_adapter,
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
                label: Some("ludenea headless"),
                required_limits,
                ..Default::default()
            })
            .await;
        match device {
            Ok(device) => return Ok(device),
            Err(error) => tried.push(format!(
                "{name}, {} (its device could not be opened: {error})",
                adapter.get_info().name
            )),
        }
    }
    Err(RenderError::NoAdapter(tried.join(", then ")))
}
