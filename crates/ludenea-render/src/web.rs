//! The renderer in a browser canvas (feature `web`): `gpu::Renderer` on WebGPU, or on WebGL2
//! where the browser offers no WebGPU adapter, driven from JavaScript with JSON (and frames
//! in the control protocol's postcard encoding). Built by `cargo xtask wasm-render`. Display
//! only: nothing here feeds back into the simulation.
//!
//! Every error is a JavaScript `Error` whose `name` says what failed, so a page recovers by
//! name, and whose `message` is the cause: `NoAdapter`, `Gpu`, `Limit`, `DeviceLost` and
//! `InvalidAtlas` (the `RenderError` variants), `InvalidInput` (JSON, postcard, a PNG or a
//! meta that cannot be read, or arguments that do not match), `Surface` (the canvas cannot
//! be drawn into), and `NoSurfaceTexture` (the canvas has no texture this frame, as when it
//! is hidden or occluded: draw again on the next one).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Once;

use ludenea_app::meta::TextureMeta;
use ludenea_draw::DebugDrawFrame;
use ludenea_draw::frame::{CameraView, RenderFrame};
use ludenea_ecs::{AssetId, Entity};
use ludenea_math::Vec2;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;
use web_sys::js_sys::{Error, Function, Uint8Array};

use crate::atlas::{self, TextureError, TextureSource};
use crate::camera::{DEFAULT_CAMERA, screen_to_world};
use crate::gpu::{DrawReport, Overlay, RenderError, Renderer, Size};
use crate::interpolate::blended;
use crate::pick;

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
        Refused {
            id: error.id(),
            path: error.path().map(str::to_string),
            message: error.to_string(),
        }
    }
}

/// What `push_frame` and `push_frame_bytes` return: the frame's tick, what is drawn as a
/// placeholder, and the frame's debug draw, which the page's labels and outlines read.
#[derive(Serialize)]
struct Pushed<'a> {
    tick: u64,
    #[serde(flatten)]
    report: DrawReport,
    debug: &'a DebugDrawFrame,
}

/// The world box `bounds` returns.
#[derive(Serialize)]
struct Bounds {
    min: Vec2,
    max: Vec2,
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
    /// The size asked by the last `resize`, which the surface may be clamped below.
    size: Size,
    /// The canvas's size in CSS pixels, which `pick` and `screen_to_world` read: its layout
    /// size when the renderer is made, then the size of each `resize`.
    css_size: Size,
    /// The frame of the last push, which the next one blends from.
    pushed: Option<RenderFrame>,
    /// The frame pushed before it, which it is drawn blended from when it follows it.
    before: Option<RenderFrame>,
    /// How far from `before` to `pushed` the last successful `draw` showed; 1 until a draw
    /// after each push.
    drawn_alpha: f32,
    /// Whether new textures blanked the canvas since the last push: nothing is drawn to pick.
    blank: bool,
}

/// A JavaScript `Error` of `name` whose message is `error`.
fn named(name: &str, error: impl std::fmt::Display) -> JsValue {
    let error = Error::new(&error.to_string());
    error.set_name(name);
    error.into()
}

/// Input that cannot be read: bad JSON or postcard, or arguments that do not match.
fn invalid(error: impl std::fmt::Display) -> JsValue {
    named("InvalidInput", error)
}

/// A renderer error, named by its variant.
fn render_failure(error: RenderError) -> JsValue {
    let name = match &error {
        RenderError::NoAdapter(_) => "NoAdapter",
        RenderError::Gpu { .. } => "Gpu",
        RenderError::Limit { .. } => "Limit",
        RenderError::DeviceLost(_) => "DeviceLost",
        RenderError::InvalidAtlas { .. } => "InvalidAtlas",
    };
    named(name, error)
}

/// `json`, a `CameraView`, or `None` when it is not given.
fn camera_from(json: Option<String>) -> Result<Option<CameraView>, JsValue> {
    json.map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(invalid)
}

/// As JSON. The values serialized here are plain data, whose serialization does not fail; were
/// it to, the page would get the reason rather than a panic.
fn to_json(value: &impl Serialize) -> Result<String, JsValue> {
    serde_json::to_string(value).map_err(invalid)
}

#[wasm_bindgen]
impl WebRenderer {
    /// A renderer drawing into `canvas`, at the canvas's current size. A canvas wider or taller
    /// than the device's `max_texture_dimension_2d` is refused. Until the first `resize`,
    /// `pick` and `screen_to_world` read CSS pixels of the canvas's layout size (its pixel size
    /// when it has no layout, as when it is not in the document).
    pub async fn create(canvas: HtmlCanvasElement) -> Result<WebRenderer, JsValue> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let css_size = layout_size(&canvas).unwrap_or(Size { width, height });
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(descriptor).await;
        // The surface comes first: on WebGL2 the adapter is made from the canvas's context.
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|error| named("Surface", error))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|error| render_failure(RenderError::NoAdapter(error.to_string())))?;
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
            .map_err(|error| {
                render_failure(RenderError::Gpu {
                    operation: "create",
                    message: error.to_string(),
                })
            })?;
        let capabilities = surface.get_capabilities(&adapter);
        // Pages hold their texels as stored, so the canvas must not encode them to sRGB again.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .ok_or_else(|| {
                named(
                    "Surface",
                    format!(
                        "the canvas offers no surface format that stores colours as given: it offers {:?}",
                        capabilities.formats
                    ),
                )
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
        // The renderer first: it takes the device's errors, which would otherwise panic.
        let renderer = Renderer::new(device.clone(), queue.clone(), format);
        let allowed = device.limits().max_texture_dimension_2d;
        if width.max(height) > allowed {
            return Err(render_failure(RenderError::Limit {
                operation: "create",
                limit: "max_texture_dimension_2d",
                requested: u64::from(width.max(height)),
                allowed: u64::from(allowed),
            }));
        }
        surface.configure(&device, &config);
        Ok(WebRenderer {
            surface,
            device,
            queue,
            config,
            canvas,
            renderer,
            backend,
            size: Size { width, height },
            css_size,
            pushed: None,
            before: None,
            drawn_alpha: 1.0,
            blank: false,
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
    /// placeholders and the others draw. The canvas shows nothing until the next push, so
    /// until then `pick` hits nothing and `bounds` is `undefined`.
    pub fn set_textures(
        &mut self,
        sources_json: &str,
        pngs: Vec<Uint8Array>,
    ) -> Result<String, JsValue> {
        let sources: Vec<Source> = serde_json::from_str(sources_json).map_err(invalid)?;
        if sources.len() != pngs.len() {
            return Err(invalid(format!(
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
        let atlas = atlas::pack(&sources, self.renderer.max_page_side());
        self.renderer.set_atlas(&atlas).map_err(render_failure)?;
        self.blank = true;
        let refused: Vec<Refused> = atlas.errors.iter().map(Refused::from).collect();
        to_json(&refused)
    }

    /// Uploads a packed atlas from its files (`manifest` is `atlas.json`, `pages` the PNGs of
    /// `page-<n>.png` in order), replacing the previous textures. Like `set_textures`, it
    /// blanks the canvas until the next push, and `pick` and `bounds` with it. A refused file
    /// is `InvalidInput`.
    pub fn set_atlas(&mut self, manifest: &str, pages: Vec<Uint8Array>) -> Result<(), JsValue> {
        let pages: Vec<Vec<u8>> = pages.iter().map(Uint8Array::to_vec).collect();
        let pages: Vec<&[u8]> = pages.iter().map(Vec::as_slice).collect();
        let atlas = atlas::Atlas::from_files(manifest, &pages).map_err(invalid)?;
        self.renderer.set_atlas(&atlas).map_err(render_failure)?;
        self.blank = true;
        Ok(())
    }

    /// `width` and `height` in CSS pixels; the canvas gets `pixel_ratio` times more, at least
    /// 1 pixel a side. It cannot report an error, so an oversized size surfaces on the next
    /// `draw`: a side past the device's `max_texture_dimension_2d` keeps the canvas at that
    /// limit, and every `draw` until a resize within it returns the `max_texture_dimension_2d`
    /// limit error.
    pub fn resize(&mut self, width: u32, height: u32, pixel_ratio: f32) {
        self.css_size = Size {
            width: width.max(1),
            height: height.max(1),
        };
        self.size = Size {
            width: ((width as f32 * pixel_ratio) as u32).max(1),
            height: ((height as f32 * pixel_ratio) as u32).max(1),
        };
        let allowed = self.device.limits().max_texture_dimension_2d;
        self.config.width = self.size.width.min(allowed);
        self.config.height = self.size.height.min(allowed);
        self.canvas.set_width(self.config.width);
        self.canvas.set_height(self.config.height);
        self.surface.configure(&self.device, &self.config);
    }

    /// Sets the frame to draw, once per tick: `current_json` (a `RenderFrame`) under
    /// `overlay_json` (`{ debug_layers, selected }`, each optional), blended from the frame
    /// set before it when it is of the very next tick (`RenderFrame::follows`), else drawn as
    /// is. Each frame is read once: the renderer keeps the last one to blend the next from,
    /// and to pick in. Returns `{ tick, missing, debug }` as JSON: the frame's tick, what is
    /// drawn as a placeholder, and the frame's debug draw. An error the GPU raised since the
    /// last call is returned by this one.
    pub fn push_frame(
        &mut self,
        current_json: &str,
        overlay_json: &str,
    ) -> Result<String, JsValue> {
        let current: RenderFrame = serde_json::from_str(current_json).map_err(invalid)?;
        self.push(current, overlay_json)
    }

    /// `push_frame` for a frame in the control protocol's postcard encoding, as the session's
    /// worker sends it (`ludenea_control::protocol::encode_frame`), so the page never parses
    /// it.
    pub fn push_frame_bytes(
        &mut self,
        bytes: &[u8],
        overlay_json: &str,
    ) -> Result<String, JsValue> {
        let current: RenderFrame = postcard::from_bytes(bytes).map_err(|error| {
            invalid(format!(
                "the {} bytes are not a render frame in postcard: {error}",
                bytes.len()
            ))
        })?;
        self.push(current, overlay_json)
    }

    /// Draws the frames set last into the canvas, `alpha` of the way from the previous tick
    /// to the current one, through `camera_json` (a `CameraView`, `{ center, view }`, the
    /// editor's free camera) when it is given. An error the GPU raised since the last call is
    /// returned by this one; a lost device is `DeviceLost` before the canvas is asked for a
    /// texture.
    pub fn draw(&mut self, alpha: f32, camera_json: Option<String>) -> Result<(), JsValue> {
        let camera = camera_from(camera_json)?;
        self.renderer.check_device().map_err(render_failure)?;
        let texture = self.texture()?;
        let view = texture.texture.create_view(&Default::default());
        // The size asked, so the renderer refuses one past the limit.
        self.renderer
            .draw(&view, self.size, alpha, camera)
            .map_err(render_failure)?;
        self.queue.present(texture);
        self.drawn_alpha = alpha;
        Ok(())
    }

    /// The entities drawn under `(x, y)`, in CSS pixels from the canvas's top-left, topmost
    /// first, as a JSON `Entity[]` (`pick::pick`), in what the last `draw` showed: the frame
    /// pushed last, blended from the one before at that draw's `alpha` (as is before a draw).
    /// Seen through `camera_json` (a `CameraView`) when given, else through the frame's camera
    /// as shown. Empty before the first frame, and from new textures to the next push.
    pub fn pick(&self, x: f32, y: f32, camera_json: Option<String>) -> Result<String, JsValue> {
        let camera = camera_from(camera_json)?;
        let hits: Vec<Entity> = match self.shown().filter(|_| !self.blank) {
            Some(frame) => {
                let camera = camera.unwrap_or_else(|| frame_camera(&frame));
                pick::pick(
                    &frame,
                    self.renderer.regions(),
                    Vec2::new(x, y),
                    self.css_size,
                    camera,
                )
            }
            None => Vec::new(),
        };
        to_json(&hits)
    }

    /// The world box `{ min, max }` of the drawn sprites and tilemaps of `entities_json` (an
    /// `Entity[]`) on layers of parallax 1, as JSON (`pick::bounds`), in what the last `draw`
    /// showed, as `pick` reads it; `undefined` when none is drawn there, before the first
    /// frame, and from new textures to the next push.
    pub fn bounds(&self, entities_json: &str) -> Result<Option<String>, JsValue> {
        let entities: BTreeSet<Entity> = serde_json::from_str(entities_json).map_err(invalid)?;
        let Some(frame) = self.shown().filter(|_| !self.blank) else {
            return Ok(None);
        };
        pick::bounds(&frame, self.renderer.regions(), &entities)
            .map(|(min, max)| to_json(&Bounds { min, max }))
            .transpose()
    }

    /// The world point a layer of `parallax` shows at `(x, y)`, in CSS pixels from the
    /// canvas's top-left, as a JSON `Vec2`: through `camera_json` (a `CameraView`) when given,
    /// else through the camera the last `draw` showed (or the default camera before a frame).
    pub fn screen_to_world(
        &self,
        x: f32,
        y: f32,
        parallax: f32,
        camera_json: Option<String>,
    ) -> Result<String, JsValue> {
        let camera = camera_from(camera_json)?.unwrap_or_else(|| {
            self.shown()
                .map_or(DEFAULT_CAMERA, |frame| frame_camera(&frame))
        });
        to_json(&screen_to_world(
            Vec2::new(x, y),
            self.css_size,
            camera,
            parallax,
        ))
    }
}

/// The camera `draw` shows `frame` through without a free camera.
fn frame_camera(frame: &RenderFrame) -> CameraView {
    frame
        .camera
        .as_ref()
        .map_or(DEFAULT_CAMERA, CameraView::from)
}

/// The canvas's layout size in CSS pixels, or `None` when it has no layout.
fn layout_size(canvas: &HtmlCanvasElement) -> Option<Size> {
    let (width, height) = (canvas.client_width(), canvas.client_height());
    (width > 0 && height > 0).then_some(Size {
        width: width as u32,
        height: height as u32,
    })
}

impl WebRenderer {
    /// The frame the last `draw` showed: the frame pushed last, blended from the one before
    /// at that draw's alpha.
    fn shown(&self) -> Option<RenderFrame> {
        let pushed = self.pushed.as_ref()?;
        Some(blended(self.before.as_ref(), pushed, self.drawn_alpha))
    }

    /// Sets `current` as the frame to draw under `overlay_json`, and keeps it.
    fn push(&mut self, current: RenderFrame, overlay_json: &str) -> Result<String, JsValue> {
        let overlay: Overlay = serde_json::from_str(overlay_json).map_err(invalid)?;
        let report = self
            .renderer
            .set_frames(self.pushed.as_ref(), &current, &overlay)
            .map_err(render_failure)?;
        self.before = self.pushed.take();
        self.drawn_alpha = 1.0;
        self.blank = false;
        let pushed = self.pushed.insert(current);
        to_json(&Pushed {
            tick: pushed.tick,
            report,
            debug: &pushed.debug,
        })
    }

    /// The canvas's next texture, configuring the canvas again once if it has none. A canvas
    /// without a texture this frame is `NoSurfaceTexture`; one the GPU refuses is `Gpu`.
    fn texture(&mut self) -> Result<wgpu::SurfaceTexture, JsValue> {
        let mut state = ("", "NoSurfaceTexture");
        for _ in 0..2 {
            state = match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(texture)
                | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => return Ok(texture),
                wgpu::CurrentSurfaceTexture::Timeout => ("timed out", "NoSurfaceTexture"),
                wgpu::CurrentSurfaceTexture::Occluded => ("is occluded", "NoSurfaceTexture"),
                wgpu::CurrentSurfaceTexture::Outdated => ("is outdated", "NoSurfaceTexture"),
                wgpu::CurrentSurfaceTexture::Lost => ("was lost", "NoSurfaceTexture"),
                wgpu::CurrentSurfaceTexture::Validation => ("was refused by the GPU", "Gpu"),
            };
            self.surface.configure(&self.device, &self.config);
        }
        let (state, name) = state;
        Err(named(
            name,
            format!(
                "the canvas has no texture to draw into: it {state}; draw again on the next frame"
            ),
        ))
    }
}

/// The frames `meta_json` (a `TextureMeta`) cuts `png` into, as JSON
/// `{ width, height, frames: [{ x, y, width, height, pivot }] }` (`atlas::frames_of`): what a
/// texture inspector or a tile palette shows. A meta that cannot be read or is invalid, and
/// a PNG that cannot be decoded or cut, are `InvalidInput` naming the cause.
#[wasm_bindgen(js_name = textureFrames)]
pub fn texture_frames(png: &[u8], meta_json: &str) -> Result<String, JsValue> {
    let meta: TextureMeta = serde_json::from_str(meta_json).map_err(invalid)?;
    meta.validate()
        .map_err(|reason| invalid(format!("invalid texture meta: {reason}")))?;
    let frames = atlas::frames_of(png, &meta).map_err(invalid)?;
    to_json(&frames)
}

/// `frame_json` (a `DebugDrawFrame`) in the ghost style, as JSON: what the editor draws for
/// another branch.
#[wasm_bindgen(js_name = ghostFrame)]
pub fn ghost_frame(frame_json: &str) -> Result<String, JsValue> {
    let frame: DebugDrawFrame = serde_json::from_str(frame_json).map_err(invalid)?;
    to_json(&frame.ghost())
}

thread_local! {
    /// The function `setPanicHandler` set last.
    static PANIC_HANDLER: RefCell<Option<Function>> = const { RefCell::new(None) };
}

/// Calls `handler` with the message of any panic in this module (its text and where it
/// happened), before the module stops: a page shows it instead of a renderer that stopped
/// silently. A later call replaces the handler. A panic aborts the module, so the page drops
/// every renderer and loads the module again before drawing.
#[wasm_bindgen(js_name = setPanicHandler)]
pub fn set_panic_handler(handler: Function) {
    static HOOK: Once = Once::new();
    PANIC_HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info.to_string();
            PANIC_HANDLER.with(|slot| {
                // Borrowed while the handler is being replaced: there is no handler to call.
                if let Ok(slot) = slot.try_borrow()
                    && let Some(handler) = slot.as_ref()
                {
                    // A handler that throws cannot be reported anywhere else while panicking.
                    let _ = handler.call1(&JsValue::NULL, &JsValue::from_str(&message));
                }
            });
            previous(info);
        }));
    });
}

/// Panics with `message`. It exists so a page's test can check that `setPanicHandler`
/// receives a panic end to end, as no input makes the renderer panic; only the `test-hooks`
/// feature (`cargo xtask wasm-render --test-hooks`) exports it.
#[cfg(feature = "test-hooks")]
#[wasm_bindgen(js_name = panicForTesting)]
pub fn panic_for_testing(message: &str) {
    panic!("{message}");
}
