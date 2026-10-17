//! The viewport renderer: draws a frame's triangles with wgpu into a browser canvas (WebGPU,
//! or WebGL2 where WebGPU is missing). Built for the editor by `cargo xtask wasm-renderer`.

use std::collections::BTreeSet;

use ludenea_ecs::Entity;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;
use wgpu::util::DeviceExt;

use crate::DebugDrawFrame;
use crate::mesh::{self, Vertex};

/// Outline width in CSS pixels, whatever the zoom.
const LINE_PIXELS: f32 = 1.5;
const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.07,
    g: 0.07,
    b: 0.08,
    a: 1.0,
};
/// Clip-space position then RGBA color, all `f32`.
const VERTEX_BYTES: usize = 6 * 4;

const SHADER: &str = "
struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs(@location(0) position: vec2<f32>, @location(1) color: vec4<f32>) -> Varyings {
    return Varyings(vec4<f32>(position, 0.0, 1.0), color);
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}
";

#[wasm_bindgen]
pub struct ViewportRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    /// Canvas size in CSS pixels, which the camera's scale is expressed in.
    css_size: (f32, f32),
    center: (f32, f32),
    pixels_per_meter: f32,
}

fn failure(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[wasm_bindgen]
impl ViewportRenderer {
    pub async fn create(canvas: HtmlCanvasElement) -> Result<ViewportRenderer, JsValue> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(descriptor).await;
        // The surface comes first: on WebGL2 the adapter is made from the canvas's context.
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(failure)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(failure)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("viewport"),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(failure)?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or(capabilities.formats.first().copied())
            .ok_or_else(|| failure("the canvas offers no surface format"))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(&device, &config);
        let pipeline = pipeline(&device, format);
        Ok(ViewportRenderer {
            surface,
            device,
            queue,
            config,
            pipeline,
            css_size: (width as f32, height as f32),
            center: (0.0, 0.0),
            pixels_per_meter: 50.0,
        })
    }

    /// `width` and `height` in CSS pixels; the drawing buffer gets `pixel_ratio` times more.
    pub fn resize(&mut self, width: u32, height: u32, pixel_ratio: f32) {
        self.css_size = (width.max(1) as f32, height.max(1) as f32);
        self.config.width = ((width as f32 * pixel_ratio) as u32).max(1);
        self.config.height = ((height as f32 * pixel_ratio) as u32).max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_camera(&mut self, center_x: f32, center_y: f32, pixels_per_meter: f32) {
        self.center = (center_x, center_y);
        self.pixels_per_meter = pixels_per_meter;
    }

    /// Draws `frame_json` (a `DebugDrawFrame`), keeping the layers named in `layers_json` and
    /// highlighting the entities in `selected_json`.
    pub fn draw(
        &mut self,
        frame_json: &str,
        layers_json: &str,
        selected_json: &str,
    ) -> Result<(), JsValue> {
        let frame: DebugDrawFrame = serde_json::from_str(frame_json).map_err(failure)?;
        let layers: BTreeSet<String> = serde_json::from_str(layers_json).map_err(failure)?;
        let selected: BTreeSet<Entity> = serde_json::from_str(selected_json).map_err(failure)?;
        let vertices = mesh::triangulate(
            &frame,
            &layers,
            &selected,
            LINE_PIXELS / self.pixels_per_meter,
        );
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            // Skipped: the next draw tries again.
            _ => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
        };
        let view = texture.texture.create_view(&Default::default());
        let buffer = (!vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("debug draw"),
                    contents: &self.vertex_bytes(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let Some(buffer) = &buffer {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..vertices.len() as u32, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(texture);
        Ok(())
    }
}

impl ViewportRenderer {
    /// World positions to clip space through the camera, colors to floats (linear when the
    /// surface encodes to sRGB itself).
    fn vertex_bytes(&self, vertices: &[Vertex]) -> Vec<u8> {
        let scale_x = 2.0 * self.pixels_per_meter / self.css_size.0;
        let scale_y = 2.0 * self.pixels_per_meter / self.css_size.1;
        let linear = self.config.format.is_srgb();
        let mut bytes = Vec::with_capacity(vertices.len() * VERTEX_BYTES);
        for vertex in vertices {
            let x = (vertex.position[0] - self.center.0) * scale_x;
            let y = (vertex.position[1] - self.center.1) * scale_y;
            let channel = |shift: u32| {
                let value = ((vertex.color >> shift) & 0xFF) as f32 / 255.0;
                if linear && shift != 0 {
                    srgb_to_linear(value)
                } else {
                    value
                }
            };
            for value in [x, y, channel(24), channel(16), channel(8), channel(0)] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }
}

// Display only, never fed back into the simulation.
#[allow(clippy::disallowed_methods)]
fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("debug draw"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("debug draw"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: VERTEX_BYTES as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}
