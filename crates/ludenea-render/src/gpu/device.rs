//! A device without a window, for offscreen rendering, tests, and the command line.

use super::RenderError;

/// Requests a hardware adapter, then the fallback (software) adapter, and opens a device on
/// the first one found. `RenderError::NoAdapter` lists what was tried and why each failed.
pub async fn headless() -> Result<(wgpu::Device, wgpu::Queue), RenderError> {
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
        match instance.request_adapter(&options).await {
            Ok(adapter) => return open(&adapter).await,
            Err(error) => tried.push(format!("{name} ({error})")),
        }
    }
    Err(RenderError::NoAdapter(tried.join(", then ")))
}

/// Opens a device with the broadly supported limits, raised to the adapter's texture size so
/// atlas pages can use it.
async fn open(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), RenderError> {
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("ludenea headless"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await
        .map_err(|error| RenderError::Gpu {
            operation: "request_device",
            message: error.to_string(),
        })
}
