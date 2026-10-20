//! The renderer in a browser canvas (feature `web`): `gpu::Renderer` on WebGPU, or on WebGL2
//! where the browser offers no WebGPU adapter, driven from JavaScript with JSON. Built by
//! `cargo xtask wasm-render`. Display only: nothing here feeds back into the simulation.

use ludenea_app::meta::TextureMeta;
use ludenea_draw::frame::RenderFrame;
use ludenea_ecs::AssetId;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;
use web_sys::js_sys::Uint8Array;

use crate::atlas::{self, Atlas, TextureError, TextureSource};
use crate::gpu::{Frames, Overlay, Renderer, Size};

/// One texture of `set_textures`, whose PNG comes in a separate array.
#[derive(Deserialize)]
struct Source {
    id: AssetId,
    path: Option<String>,
    meta: TextureMeta,
}

/// A texture `set_textures` could not load: which one, and why.
#[derive(Serialize)]
struct Refused {
    id: AssetId,
    path: Option<String>,
    message: String,
}

impl From<&TextureError> for Refused {
    fn from(error: &TextureError) -> Self {
        let (TextureError::Decode { id, path, .. }
        | TextureError::Grid { id, path, .. }
        | TextureError::TooLarge { id, path, .. }
        | TextureError::Meta { id, path, .. }) = error;
        Refused {
            id: *id,
            path: path.clone(),
            message: error.to_string(),
        }
    }
}

#[wasm_bindgen]
pub struct WebRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    canvas: HtmlCanvasElement,
    renderer: Renderer,
    backend: &'static str,
}

fn failure(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[wasm_bindgen]
impl WebRenderer {
    /// A renderer drawing into `canvas`, at the canvas's current size.
    pub async fn create(canvas: HtmlCanvasElement) -> Result<WebRenderer, JsValue> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(descriptor).await;
        // The surface comes first: on WebGL2 the adapter is made from the canvas's context.
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(failure)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(failure)?;
        let backend = match adapter.get_info().backend {
            wgpu::Backend::BrowserWebGpu => "webgpu",
            _ => "webgl2",
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ludenea web"),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(failure)?;
        let capabilities = surface.get_capabilities(&adapter);
        // Pages hold their texels as stored, so the canvas must not encode them to sRGB again.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .ok_or_else(|| {
                failure(format!(
                    "the canvas offers no surface format that stores colours as given: it offers {:?}",
                    capabilities.formats
                ))
            })?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            // Indexing could panic, which aborts the whole instance in the browser.
            alpha_mode: capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(&device, &config);
        let renderer = Renderer::new(device.clone(), queue.clone(), format);
        Ok(WebRenderer {
            surface,
            device,
            queue,
            config,
            canvas,
            renderer,
            backend,
        })
    }

    /// The browser backend drawing: `"webgpu"` or `"webgl2"`.
    #[wasm_bindgen(getter)]
    pub fn backend(&self) -> String {
        self.backend.to_string()
    }

    /// Packs `pngs` into an atlas and uploads it, replacing the previous textures.
    /// `sources_json` is `[{ id, path, meta }]`, one per PNG in the same order. Returns the
    /// textures that could not be loaded as JSON, `[{ id, path, message }]`; they draw as
    /// placeholders and the others draw.
    pub fn set_textures(
        &mut self,
        sources_json: &str,
        pngs: Vec<Uint8Array>,
    ) -> Result<String, JsValue> {
        let sources: Vec<Source> = serde_json::from_str(sources_json).map_err(failure)?;
        if sources.len() != pngs.len() {
            return Err(failure(format!(
                "{} texture sources need as many PNGs, but {} were given",
                sources.len(),
                pngs.len()
            )));
        }
        let pngs: Vec<Vec<u8>> = pngs.iter().map(Uint8Array::to_vec).collect();
        let sources: Vec<TextureSource<'_>> = sources
            .iter()
            .zip(&pngs)
            .map(|(source, png)| TextureSource {
                id: source.id,
                path: source.path.as_deref(),
                png,
                meta: &source.meta,
            })
            .collect();
        // When every texture is refused, none is left to draw.
        let atlas =
            atlas::pack(&sources, self.renderer.max_page_side()).unwrap_or_else(|errors| Atlas {
                pages: Vec::new(),
                regions: Default::default(),
                errors,
            });
        self.renderer.set_atlas(&atlas).map_err(failure)?;
        let refused: Vec<Refused> = atlas.errors.iter().map(Refused::from).collect();
        serde_json::to_string(&refused).map_err(failure)
    }

    /// `width` and `height` in CSS pixels; the canvas gets `pixel_ratio` times more.
    pub fn resize(&mut self, width: u32, height: u32, pixel_ratio: f32) {
        self.config.width = ((width as f32 * pixel_ratio) as u32).max(1);
        self.config.height = ((height as f32 * pixel_ratio) as u32).max(1);
        self.canvas.set_width(self.config.width);
        self.canvas.set_height(self.config.height);
        self.surface.configure(&self.device, &self.config);
    }

    /// Draws `current_json` (a `RenderFrame`), blended from `previous_json` by `alpha` when it
    /// is given, under `overlay_json` (`{ debug_layers, selected, camera }`, each optional).
    /// Returns the `DrawReport` as JSON. An error is the renderer's message, and an error the
    /// GPU raised since the last call is returned by this one.
    pub fn draw(
        &mut self,
        previous_json: Option<String>,
        current_json: &str,
        alpha: f32,
        overlay_json: &str,
    ) -> Result<String, JsValue> {
        let previous: Option<RenderFrame> = previous_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(failure)?;
        let current: RenderFrame = serde_json::from_str(current_json).map_err(failure)?;
        let overlay: Overlay = serde_json::from_str(overlay_json).map_err(failure)?;
        let texture = self.texture()?;
        let view = texture.texture.create_view(&Default::default());
        let frames = Frames {
            previous: previous.as_ref(),
            current: &current,
            alpha,
        };
        let size = Size {
            width: self.config.width,
            height: self.config.height,
        };
        let report = self
            .renderer
            .draw(&view, size, frames, &overlay)
            .map_err(failure)?;
        self.queue.present(texture);
        serde_json::to_string(&report).map_err(failure)
    }
}

impl WebRenderer {
    /// The canvas's next texture, configuring the canvas again once if it has none.
    fn texture(&mut self) -> Result<wgpu::SurfaceTexture, JsValue> {
        let mut state = "";
        for _ in 0..2 {
            state = match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(texture)
                | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => return Ok(texture),
                wgpu::CurrentSurfaceTexture::Timeout => "timed out",
                wgpu::CurrentSurfaceTexture::Occluded => "is occluded",
                wgpu::CurrentSurfaceTexture::Outdated => "is outdated",
                wgpu::CurrentSurfaceTexture::Lost => "was lost",
                wgpu::CurrentSurfaceTexture::Validation => "was refused by the GPU",
            };
            self.surface.configure(&self.device, &self.config);
        }
        Err(failure(format!(
            "the canvas has no texture to draw into: it {state}; draw again on the next frame"
        )))
    }
}
