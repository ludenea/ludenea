//! The wgpu renderer (feature `gpu`): draws a render frame's layers of tilemaps and sprites,
//! then the debug draw, through a fitted 2D camera into a target view, or offscreen into
//! RGBA pixels, anywhere between the previous tick and the current one. The geometry is
//! uploaded once per tick and stays on the GPU; a draw writes only the views, and the shader
//! interpolates. Display only: nothing here feeds back into the simulation.

mod debug;
mod device;
mod sprites;
mod tilemaps;

use std::collections::{BTreeMap, BTreeSet};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ludenea_app::meta::Filter;
use ludenea_draw::frame::{CameraView, RenderFrame, SpriteItem, TilemapItem};
use ludenea_draw::{Color, DebugDrawFrame};
use ludenea_ecs::{AssetId, Entity};
use ludenea_math::Vec2;
use serde::{Deserialize, Serialize};
use wgpu::util::DeviceExt;

use crate::atlas::{Atlas, Region};
use crate::camera::{DEFAULT_CAMERA, fit, world_to_clip};
use crate::interpolate::{Pairing, camera_at, clamp_alpha};
use sprites::{BLANK, INSTANCE_FLOATS, Sample};

#[cfg(not(target_arch = "wasm32"))]
pub use device::for_surface;
pub use device::{headless, headless_with_limits};

/// The color a target is cleared to before anything is drawn.
pub const BACKGROUND: Color = Color(0x121214FF);

/// The largest atlas page side the renderer accepts, before the device's own limit.
const MAX_PAGE_SIDE: u32 = 4096;

/// The operation of a GPU error pending before a call: raised by the host's use of the
/// device, or after an earlier call returned.
const EARLIER: &str = "earlier GPU work";

/// The format of pages and of `render_rgba`'s target: 8-bit RGBA, stored as given.
const RGBA: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A target's size in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// What the viewer adds over the frame: the debug draw layers to show and the selected
/// entities. Each field may be left out of its JSON, which `web` reads.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Overlay {
    pub debug_layers: BTreeSet<String>,
    pub selected: BTreeSet<Entity>,
}

/// What `set_frames` could not prepare as asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DrawReport {
    /// What is drawn as a placeholder, in draw order: each sprite's entity, texture and
    /// frame, and each tile's tilemap entity, tileset and tile index.
    pub missing: Vec<(Entity, AssetId, u32)>,
}

/// What the renderer did to the GPU since it was made, for diagnostics: the buffers it
/// created and the bytes it wrote into buffers (textures are not counted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub buffers_created: u64,
    pub bytes_written: u64,
}

/// Why the renderer could not do what was asked. Non-exhaustive: later versions may add
/// variants without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    #[error("no GPU adapter: tried {0}")]
    NoAdapter(String),
    #[error("{operation} failed on the GPU: {message}")]
    Gpu {
        operation: &'static str,
        message: String,
    },
    #[error("{operation} needs {requested} for `{limit}`, but at most {allowed} is allowed")]
    Limit {
        operation: &'static str,
        limit: &'static str,
        requested: u64,
        allowed: u64,
    },
    #[error("the GPU device was lost: {0}")]
    DeviceLost(String),
    /// An atlas page whose data cannot be uploaded: a side of 0, or pixels that do not match
    /// its size. `page` is its index in `Atlas::pages`.
    #[error("set_atlas refuses atlas page {page}: {reason}")]
    InvalidAtlas { page: usize, reason: String },
}

/// Draws render frames with one device, in two steps: `set_frames`, once per tick, uploads
/// the geometry of the current frame with both ticks' transforms, and `draw` (or
/// `render_rgba`), once per displayed frame, draws it `alpha` of the way from the previous
/// tick to the current one, writing only the views. Pages come from `set_atlas`; until then,
/// and for any sprite whose region is not in the atlas, a magenta placeholder is drawn. Until
/// the first `set_frames`, a draw shows the background alone.
///
/// The renderer handles its device's errors: an error the device raises outside an error
/// scope is returned by the next `set_atlas`, `set_frames`, `draw` or `render_rgba` as
/// `RenderError::Gpu`. Its operation is the call's own when the error was raised while the
/// call ran, and `"earlier GPU work"` when it was pending before the call (raised by the
/// host's own use of the device, or after an earlier call returned); the call then does
/// nothing. A call that returns `Gpu` with its own operation ran: `set_atlas` has replaced the
/// atlas, `set_frames` the frames, and `draw` has submitted its work. Once the device is lost,
/// every call returns `RenderError::DeviceLost` without touching it, and the caller makes a
/// new renderer on a new device.
pub struct Renderer {
    device: wgpu::Device,
    faults: Arc<Mutex<Faults>>,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    /// The layout of a view: world to clip space and alpha, at a dynamic offset.
    view_layout: wgpu::BindGroupLayout,
    /// The quad and debug pipelines per target format, made on first use.
    pipelines: Vec<(wgpu::TextureFormat, Pipelines)>,
    /// One sampler per filter: nearest, then linear.
    samplers: [wgpu::Sampler; 2],
    /// Per page, its size and its bind group per filter.
    pages: Vec<Page>,
    /// A white texel, which the placeholders sample.
    blank: wgpu::BindGroup,
    regions: BTreeMap<(AssetId, u32), Region>,
    tilemaps: tilemaps::Cache,
    /// What the last `set_frames` prepared, which every draw until the next one reuses.
    scene: Scene,
    /// The sprite instances, one buffer per part of at most `per_buffer` instances, kept
    /// across ticks and replaced by a larger one only when a part outgrows it.
    instances: Vec<wgpu::Buffer>,
    /// The debug draw's vertices, kept and grown like `instances`, and how many vertices of
    /// each buffer the current debug draw uses (none past the last one it fills).
    debug: Vec<wgpu::Buffer>,
    debug_counts: Vec<u32>,
    /// The target size and visible area the debug vertices were built for (their outlines'
    /// width depends on both), or `None` when `set_frames` changed the debug draw since.
    debug_built_for: Option<(Size, Vec2)>,
    /// The views' uniform buffer and its bind group, made with the renderer for one view
    /// and replaced when the views outgrow it.
    views: Views,
    /// The target and read-back buffer of `render_rgba`, kept while the size stays.
    #[cfg(not(target_arch = "wasm32"))]
    offscreen: Option<Offscreen>,
    stats: RenderStats,
}

/// What the device reported through its callbacks, waiting for the next call to return it.
#[derive(Default)]
struct Faults {
    errors: Vec<String>,
    lost: Option<String>,
}

/// The faults, even if a callback panicked while holding them.
fn lock(faults: &Mutex<Faults>) -> MutexGuard<'_, Faults> {
    faults.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
struct Pipelines {
    quads: wgpu::RenderPipeline,
    debug: wgpu::RenderPipeline,
}

struct Page {
    size: (u32, u32),
    bind_groups: [wgpu::BindGroup; 2],
}

/// The current frame as `set_frames` prepared it: its draws in order, the views they go
/// through, its camera at both ticks, and its debug draw with the overlay to show it with.
#[derive(Default)]
struct Scene {
    draws: Vec<Draw>,
    /// The view of each slot before the debug draw's, in slot order.
    slots: Vec<Slot>,
    /// The frame's camera at the previous tick and at the current one.
    camera: Option<(CameraView, CameraView)>,
    debug: DebugDrawFrame,
    overlay: Overlay,
}

/// A view of a layer of `parallax`, for geometry placed relative to `origin`, given at the
/// previous tick and at the current one: zero for a layer's sprites, the tilemap's position
/// for a tilemap's chunks.
struct Slot {
    parallax: f32,
    origin: (Vec2, Vec2),
}

/// The views' uniform buffer and the bind group that reads one view of it at a dynamic
/// offset.
struct Views {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl Views {
    /// A buffer of `size` bytes (at least `VIEW_BYTES`) and its bind group.
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, size: u64) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("views"),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("views"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(VIEW_BYTES),
                }),
            }],
        });
        Views { buffer, group }
    }
}

/// The offscreen target of `render_rgba` and the buffer its pixels are read back through.
#[cfg(not(target_arch = "wasm32"))]
struct Offscreen {
    size: Size,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    /// The bytes of one read-back row, padded to the copy alignment.
    padded: u32,
}

impl Renderer {
    /// A renderer drawing into targets of `format`.
    /// It replaces the device's uncaptured error handler and its device lost callback.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let faults = Arc::new(Mutex::new(Faults::default()));
        let sink = Arc::clone(&faults);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            lock(&sink).errors.push(error.to_string());
        }));
        let sink = Arc::clone(&faults);
        device.set_device_lost_callback(move |reason, message| {
            lock(&sink).lost = Some(if message.is_empty() {
                format!("{reason:?}")
            } else {
                message
            });
        });
        let layout = sprites::page_layout(&device);
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(VIEW_BYTES),
                },
                count: None,
            }],
        });
        let sampler = |filter: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("sprite page"),
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let samplers = [
            sampler(wgpu::FilterMode::Nearest),
            sampler(wgpu::FilterMode::Linear),
        ];
        let white = upload(&device, &queue, 1, 1, &[255; 4]);
        let blank = bind_group(&device, &layout, &white, &samplers[0]);
        let views = Views::new(&device, &view_layout, VIEW_BYTES);
        Renderer {
            device,
            faults,
            queue,
            format,
            layout,
            view_layout,
            pipelines: Vec::new(),
            samplers,
            pages: Vec::new(),
            blank,
            regions: BTreeMap::new(),
            tilemaps: tilemaps::Cache::default(),
            scene: Scene::default(),
            instances: Vec::new(),
            debug: Vec::new(),
            debug_counts: Vec::new(),
            debug_built_for: None,
            views,
            #[cfg(not(target_arch = "wasm32"))]
            offscreen: None,
            stats: RenderStats {
                buffers_created: 1,
                bytes_written: 0,
            },
        }
    }

    /// The largest page side to pack atlases for: `min(4096, max_texture_dimension_2d)`.
    pub fn max_page_side(&self) -> u32 {
        MAX_PAGE_SIDE.min(self.device.limits().max_texture_dimension_2d)
    }

    /// The buffers created and the bytes written into buffers since the renderer was made.
    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    /// Uploads the atlas's pages, replacing the previous atlas. A page wider or taller than
    /// `max_page_side` is refused with `RenderError::Limit`, naming `max_page_side` when the
    /// renderer's 4096 cap binds and `max_texture_dimension_2d` when the device's limit does,
    /// and a page with a side of 0 or whose pixels do not match its size with
    /// `RenderError::InvalidAtlas`; the previous atlas then stays. A new atlas drops the frames
    /// set before, which sample the previous one: until the next `set_frames`, a draw shows
    /// the background and the debug draw.
    pub fn set_atlas(&mut self, atlas: &Atlas) -> Result<(), RenderError> {
        self.guarded("set_atlas", |renderer| renderer.replace_atlas(atlas))
    }

    fn replace_atlas(&mut self, atlas: &Atlas) -> Result<(), RenderError> {
        let allowed = self.max_page_side();
        let limit = if MAX_PAGE_SIDE < self.device.limits().max_texture_dimension_2d {
            "max_page_side"
        } else {
            "max_texture_dimension_2d"
        };
        for (index, page) in atlas.pages.iter().enumerate() {
            let side = page.width.max(page.height);
            if side > allowed {
                return Err(RenderError::Limit {
                    operation: "set_atlas",
                    limit,
                    requested: u64::from(side),
                    allowed: u64::from(allowed),
                });
            }
            let invalid = |reason: String| RenderError::InvalidAtlas {
                page: index,
                reason,
            };
            if page.width == 0 || page.height == 0 {
                return Err(invalid(format!(
                    "a {}x{} page has no pixels",
                    page.width, page.height
                )));
            }
            let expected = u64::from(page.width) * u64::from(page.height) * 4;
            if page.rgba.len() as u64 != expected {
                return Err(invalid(format!(
                    "a {}x{} page needs {expected} bytes of RGBA pixels, but it has {}",
                    page.width,
                    page.height,
                    page.rgba.len()
                )));
            }
        }
        self.pages = atlas
            .pages
            .iter()
            .map(|page| {
                let texture = upload(
                    &self.device,
                    &self.queue,
                    page.width,
                    page.height,
                    &page.rgba,
                );
                let [nearest, linear] = &self.samplers;
                Page {
                    size: (page.width, page.height),
                    bind_groups: [
                        bind_group(&self.device, &self.layout, &texture, nearest),
                        bind_group(&self.device, &self.layout, &texture, linear),
                    ],
                }
            })
            .collect();
        self.regions = atlas.regions.clone();
        self.tilemaps.clear();
        // The draws set before sample the previous atlas's pages and regions.
        self.scene.draws.clear();
        Ok(())
    }

    /// Prepares `current` for drawing, blended from `previous` when it is of the tick right
    /// before (`RenderFrame::follows`; after a gap, such as a displayed frame that ran several
    /// ticks or a seek, `current` is drawn as is): builds and uploads every sprite instance
    /// with both ticks' transforms (equal for an entity new in `current` or teleported on its
    /// tick), the tilemap chunks (a tilemap whose tiles did not change keeps its chunks), and
    /// the views' layout, reusing the buffers that fit. The
    /// debug draw of `overlay`'s layers, its selected entities highlighted, is built at the
    /// next draw. Of tilemaps sharing an entity, which only a malformed frame holds, the first
    /// well-formed one listed is drawn and the others are skipped. Returns what is drawn as a
    /// placeholder.
    pub fn set_frames(
        &mut self,
        previous: Option<&RenderFrame>,
        current: &RenderFrame,
        overlay: &Overlay,
    ) -> Result<DrawReport, RenderError> {
        let previous = previous.filter(|previous| current.follows(previous));
        self.guarded("set_frames", |renderer| {
            renderer.prepare(previous, current, overlay)
        })
    }

    /// Draws the frames set last into `target`, a view of `size` pixels in the renderer's
    /// format, `alpha` of the way from the previous tick to the current one (clamped to 0 to
    /// 1, NaN as 1), through `camera` when it is given (the editor's free camera) and else
    /// the frame's camera, blended. It writes the views and nothing else, unless the size or
    /// the current frame's visible area changed, which rebuilds the debug draw's outlines. A
    /// size past `max_texture_dimension_2d` is refused.
    pub fn draw(
        &mut self,
        target: &wgpu::TextureView,
        size: Size,
        alpha: f32,
        camera: Option<CameraView>,
    ) -> Result<(), RenderError> {
        self.guarded("draw", |renderer| {
            renderer.fits("draw", size)?;
            renderer.encode(target, renderer.format, size, alpha, camera)
        })
    }

    /// Draws as `draw` does, offscreen, and reads the pixels back: `width * 4` bytes of RGBA
    /// per row, tightly packed, top row first. A target with no pixels gives no bytes and
    /// draws nothing; one past `max_texture_dimension_2d`, or whose read-back is past
    /// `max_buffer_size`, is refused. Not on WebAssembly: it waits for the GPU, which a
    /// browser's thread cannot do.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_rgba(
        &mut self,
        size: Size,
        alpha: f32,
        camera: Option<CameraView>,
    ) -> Result<Vec<u8>, RenderError> {
        self.guarded("render_rgba", |renderer| {
            renderer.read_back(size, alpha, camera)
        })
    }

    /// Runs `run` between two checks of the device's faults, which take precedence over its
    /// result: the loss of the device, or the errors it raised before `run`, reported as
    /// `EARLIER` without running it, or during `run`, reported as `operation`.
    fn guarded<T>(
        &mut self,
        operation: &'static str,
        run: impl FnOnce(&mut Self) -> Result<T, RenderError>,
    ) -> Result<T, RenderError> {
        self.faults(EARLIER)?;
        let result = run(self);
        self.faults(operation)?;
        result
    }

    /// The device's loss, or the errors it raised since the last call, taken.
    fn faults(&self, operation: &'static str) -> Result<(), RenderError> {
        // A lost device is never touched again, not even polled.
        if let Some(message) = &lock(&self.faults).lost {
            return Err(RenderError::DeviceLost(message.clone()));
        }
        // A non-blocking poll runs the callbacks the device has pending, its loss among them.
        // Without a wait or a submission index, it has no error of its own to return.
        let _ = self.device.poll(wgpu::PollType::Poll);
        let mut faults = lock(&self.faults);
        if let Some(message) = &faults.lost {
            return Err(RenderError::DeviceLost(message.clone()));
        }
        if faults.errors.is_empty() {
            return Ok(());
        }
        Err(RenderError::Gpu {
            operation,
            message: std::mem::take(&mut faults.errors).join("; "),
        })
    }

    /// Checks the device without drawing: `RenderError::DeviceLost` once it is lost, and the
    /// errors it raised since the last call as `RenderError::Gpu` of `"earlier GPU work"`, as
    /// the next call would return them. For a caller that touches the device before drawing,
    /// such as a window asking its surface for a texture.
    pub fn check_device(&self) -> Result<(), RenderError> {
        self.faults(EARLIER)
    }

    /// Refuses a target of `size` wider or taller than `max_texture_dimension_2d`, as
    /// `render_rgba` would, so a caller can fail before it does any work.
    pub fn check_size(&self, size: Size) -> Result<(), RenderError> {
        self.fits("render_rgba", size)
    }

    /// Refuses a target wider or taller than `max_texture_dimension_2d`.
    fn fits(&self, operation: &'static str, size: Size) -> Result<(), RenderError> {
        let allowed = self.device.limits().max_texture_dimension_2d;
        let side = size.width.max(size.height);
        if side > allowed {
            return Err(RenderError::Limit {
                operation,
                limit: "max_texture_dimension_2d",
                requested: u64::from(side),
                allowed: u64::from(allowed),
            });
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_back(
        &mut self,
        size: Size,
        alpha: f32,
        camera: Option<CameraView>,
    ) -> Result<Vec<u8>, RenderError> {
        if size.width == 0 || size.height == 0 {
            return Ok(Vec::new());
        }
        self.fits("render_rgba", size)?;
        // Rows are copied out padded to the copy alignment, then packed. The side fits the
        // texture limit, so a padded row fits a u32.
        let row = size.width * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let read_back = u64::from(padded) * u64::from(size.height);
        let allowed = self.device.limits().max_buffer_size;
        if read_back > allowed {
            return Err(RenderError::Limit {
                operation: "render_rgba",
                limit: "max_buffer_size",
                requested: read_back,
                allowed,
            });
        }
        // Taken while in use: a read-back that fails leaves it out, and the next call makes
        // a new one rather than reuse a buffer in an unknown state.
        let offscreen = match self.offscreen.take() {
            Some(offscreen) if offscreen.size == size => offscreen,
            _ => self.offscreen(size, padded, read_back),
        };
        self.encode(&offscreen.view, RGBA, size, alpha, camera)?;

        let extent = wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            offscreen.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &offscreen.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(offscreen.padded),
                    rows_per_image: Some(size.height),
                },
            },
            extent,
        );
        self.queue.submit([encoder.finish()]);

        let (sender, receiver) = mpsc::channel();
        offscreen
            .buffer
            .map_async(wgpu::MapMode::Read, .., move |result| {
                // The receiver waits below, so it is still there.
                let _ = sender.send(result);
            });
        let failed = |message: String| RenderError::Gpu {
            operation: "render_rgba",
            message,
        };
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| failed(error.to_string()))?;
        receiver
            .recv()
            .map_err(|_| failed("the read-back was dropped before it completed".to_string()))?
            .map_err(|error| failed(error.to_string()))?;
        let rgba = {
            let mapped = offscreen
                .buffer
                .get_mapped_range(..)
                .map_err(|error| failed(error.to_string()))?;
            mapped
                .chunks_exact(offscreen.padded as usize)
                .flat_map(|line| &line[..row as usize])
                .copied()
                .collect()
        };
        offscreen.buffer.unmap();
        self.offscreen = Some(offscreen);
        Ok(rgba)
    }

    /// A new offscreen target of `size` and its read-back buffer of `read_back` bytes, rows
    /// of `padded` bytes.
    #[cfg(not(target_arch = "wasm32"))]
    fn offscreen(&mut self, size: Size, padded: u32, read_back: u64) -> Offscreen {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: RGBA,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.stats.buffers_created += 1;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen read-back"),
            size: read_back,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Offscreen {
            size,
            view: texture.create_view(&Default::default()),
            texture,
            buffer,
            padded,
        }
    }

    /// Builds the scene of `current`: the draws of its layers in order (tilemaps and sprites
    /// by `(order, entity)` within each), and uploads its sprite instances.
    fn prepare(
        &mut self,
        previous: Option<&RenderFrame>,
        current: &RenderFrame,
        overlay: &Overlay,
    ) -> Result<DrawReport, RenderError> {
        // Sprite instances go in buffers of at most `per_buffer`, below `max_buffer_size`.
        let per_buffer = self.per_buffer("set_frames", INSTANCE_BYTES)?;
        let pages: Vec<(u32, u32)> = self.pages.iter().map(|page| page.size).collect();
        // A well-formed frame has one tilemap per entity; of a malformed frame's duplicates,
        // only the first well-formed one listed is built and drawn.
        let mut seen = BTreeSet::new();
        let tilemaps: Vec<&TilemapItem> = current
            .tilemaps
            .iter()
            .filter(|item| tilemaps::well_formed(item) && seen.insert(item.entity))
            .collect();
        self.tilemaps.update(
            &self.device,
            &self.queue,
            &mut self.stats,
            &tilemaps,
            &self.regions,
            &pages,
        );

        // Tilemaps before sprites, so a stable sort draws a tilemap first on a tie.
        let mut pieces: Vec<(u16, i32, Entity, Piece<'_>)> = tilemaps
            .iter()
            .map(|&item| (item.layer, item.order, item.entity, Piece::Tilemap(item)))
            .chain(
                current
                    .sprites
                    .iter()
                    .map(|item| (item.layer, item.order, item.entity, Piece::Sprite(item))),
            )
            .collect();
        pieces.sort_by_key(|(layer, order, entity, _)| (*layer, *order, *entity));

        // One view per layer of sprites drawn and one per tilemap (its layer's, moved to the
        // tilemap's position, so its chunks do not depend on it).
        let pairing = Pairing::new(previous, current);
        let parallax = |layer: u16| {
            current
                .layers
                .get(usize::from(layer))
                .map_or(1.0, |info| info.parallax)
        };
        let mut slots: Vec<Slot> = Vec::new();
        let mut layer_slots: BTreeMap<u16, u32> = BTreeMap::new();
        let mut draws: Vec<Draw> = Vec::new();
        let mut instances: Vec<sprites::Instance> = Vec::new();
        let mut missing = Vec::new();
        for (layer, _, _, piece) in pieces {
            match piece {
                Piece::Sprite(item) => {
                    let slot = *layer_slots.entry(layer).or_insert_with(|| {
                        slots.push(Slot {
                            parallax: parallax(layer),
                            origin: (Vec2::ZERO, Vec2::ZERO),
                        });
                        slots.len() as u32 - 1
                    });
                    let motion = pairing.sprite(item);
                    let (sample, instance) =
                        sprites::instance(item, &motion, &self.regions, &pages);
                    let sample = sample.unwrap_or_else(|| {
                        missing.push((item.entity, item.texture, item.frame));
                        BLANK
                    });
                    let part = instances.len() / per_buffer;
                    let index = (instances.len() % per_buffer) as u32;
                    instances.push(instance);
                    match draws.last_mut() {
                        Some(Draw {
                            slot: last_slot,
                            source: Source::Sprites(last_part),
                            sample: last,
                            instances: run,
                        }) if *last_slot == slot && *last_part == part && *last == sample => {
                            run.end = index + 1
                        }
                        _ => draws.push(Draw {
                            slot,
                            source: Source::Sprites(part),
                            sample,
                            instances: index..index + 1,
                        }),
                    }
                }
                Piece::Tilemap(item) => {
                    let Some(built) = self.tilemaps.get(item.entity) else {
                        continue;
                    };
                    slots.push(Slot {
                        parallax: parallax(layer),
                        origin: pairing.tilemap(item),
                    });
                    let slot = slots.len() as u32 - 1;
                    missing.extend_from_slice(&built.missing);
                    for (chunk_index, chunk) in built.chunks.iter().enumerate() {
                        for (sample, run) in &chunk.batches {
                            draws.push(Draw {
                                slot,
                                source: Source::Chunk(item.entity, chunk_index),
                                sample: *sample,
                                instances: run.clone(),
                            });
                        }
                    }
                }
            }
        }

        let cap = (per_buffer * INSTANCE_BYTES) as u64;
        for (part, chunk) in instances.chunks(per_buffer).enumerate() {
            let bytes: Vec<u8> = chunk
                .iter()
                .flatten()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            let buffer = reserve(
                &self.device,
                &mut self.stats,
                &mut self.instances,
                part,
                bytes.len() as u64,
                cap,
                "sprite instances",
            );
            write(&self.queue, &mut self.stats, buffer, &bytes);
        }
        self.scene = Scene {
            draws,
            slots,
            camera: pairing.camera(current),
            debug: current.debug.clone(),
            overlay: overlay.clone(),
        };
        self.debug_built_for = None;
        Ok(DrawReport { missing })
    }

    /// Clears `target` to the background, draws the scene `alpha` of the way between its
    /// ticks, then the debug draw.
    fn encode(
        &mut self,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: Size,
        alpha: f32,
        camera: Option<CameraView>,
    ) -> Result<(), RenderError> {
        let alpha = clamp_alpha(alpha);
        let aspect = size.width.max(1) as f32 / size.height.max(1) as f32;
        // The camera shown, and the current tick's, whose visible area the debug draw's
        // outlines follow, as the debug draw is the current tick's.
        let (shown, current) = match (camera, &self.scene.camera) {
            (Some(free), _) => (free.clone(), free),
            (None, Some((from, to))) => (camera_at(from, to, alpha), to.clone()),
            (None, None) => (DEFAULT_CAMERA, DEFAULT_CAMERA),
        };
        let visible = fit(shown.view, aspect);
        let built_for = (size, fit(current.view, aspect));
        if self.debug_built_for != Some(built_for) {
            self.build_debug(size, built_for.1)?;
            self.debug_built_for = Some(built_for);
        }

        // One view per slot, then the debug draw's, each on its own aligned slot of the
        // uniform buffer, picked by a dynamic offset.
        let views: Vec<[f32; 4]> = self
            .scene
            .slots
            .iter()
            .map(|slot| {
                let origin = Vec2::lerp(slot.origin.0, slot.origin.1, alpha);
                moved(view(&shown, visible, slot.parallax), origin)
            })
            .chain([view(&shown, visible, 1.0)])
            .collect();
        let debug_slot = views.len() as u32 - 1;
        let stride = self
            .device
            .limits()
            .min_uniform_buffer_offset_alignment
            .max(VIEW_BYTES as u32);
        // Up to the end of the last view, so the slots' padding is not written.
        let mut view_bytes = vec![0; (views.len() - 1) * stride as usize + VIEW_BYTES as usize];
        for (index, values) in views.iter().enumerate() {
            let start = index * stride as usize;
            for (at, value) in values.iter().chain([&alpha]).enumerate() {
                view_bytes[start + at * 4..start + at * 4 + 4]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
        self.write_views(&view_bytes)?;
        let pipelines = self.pipelines(format).clone();
        let view_group = &self.views.group;

        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color(BACKGROUND)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if !self.scene.draws.is_empty() {
                pass.set_pipeline(&pipelines.quads);
            }
            for draw in &self.scene.draws {
                let buffer = match draw.source {
                    Source::Sprites(part) => self.instances.get(part),
                    Source::Chunk(entity, chunk) => self
                        .tilemaps
                        .get(entity)
                        .and_then(|built| built.chunks.get(chunk))
                        .map(|chunk| &chunk.buffer),
                };
                let Some(buffer) = buffer else {
                    continue;
                };
                let bind_group = match draw.sample.page {
                    Some(page) => {
                        let filter = match draw.sample.filter {
                            Filter::Nearest => 0,
                            Filter::Linear => 1,
                        };
                        &self.pages[page as usize].bind_groups[filter]
                    }
                    None => &self.blank,
                };
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_bind_group(1, view_group, &[draw.slot * stride]);
                pass.draw(0..4, draw.instances.clone());
            }
            if !self.debug_counts.is_empty() {
                pass.set_pipeline(&pipelines.debug);
                pass.set_bind_group(0, view_group, &[debug_slot * stride]);
            }
            for (vertices, &count) in self.debug.iter().zip(&self.debug_counts) {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.draw(0..count, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    /// Builds and uploads the scene's debug draw for a target of `size` showing `visible`:
    /// outlines `LINE_PIXELS` wide, in whole triangles per buffer below `max_buffer_size`.
    fn build_debug(&mut self, size: Size, visible: Vec2) -> Result<(), RenderError> {
        let line_width = debug::LINE_PIXELS * visible.x / size.width.max(1) as f32;
        let bytes = debug::vertices(&self.scene.debug, &self.scene.overlay, line_width);
        let triangle = 3 * debug::VERTEX_BYTES;
        let per_buffer = self.per_buffer("draw", triangle)?.saturating_mul(triangle);
        self.debug_counts.clear();
        for (part, chunk) in bytes.chunks(per_buffer).enumerate() {
            let buffer = reserve(
                &self.device,
                &mut self.stats,
                &mut self.debug,
                part,
                chunk.len() as u64,
                per_buffer as u64,
                "debug draw",
            );
            write(&self.queue, &mut self.stats, buffer, chunk);
            self.debug_counts.push(debug::count(chunk));
        }
        Ok(())
    }

    /// Writes `bytes` at the start of the views' buffer, replaced by a larger one (and its
    /// bind group with it) when it is too small; refused past `max_buffer_size`.
    fn write_views(&mut self, bytes: &[u8]) -> Result<(), RenderError> {
        let needed = bytes.len() as u64;
        let allowed = self.device.limits().max_buffer_size;
        if needed > allowed {
            return Err(RenderError::Limit {
                operation: "draw",
                limit: "max_buffer_size",
                requested: needed,
                allowed,
            });
        }
        if self.views.buffer.size() < needed {
            self.stats.buffers_created += 1;
            self.views = Views::new(&self.device, &self.view_layout, grown(needed, allowed));
        }
        write(&self.queue, &mut self.stats, &self.views.buffer, bytes);
        Ok(())
    }

    /// How many items of `unit` bytes fit in one buffer; refused for `operation` if not even
    /// one does.
    fn per_buffer(&self, operation: &'static str, unit: usize) -> Result<usize, RenderError> {
        let allowed = self.device.limits().max_buffer_size;
        match usize::try_from(allowed / unit as u64).unwrap_or(usize::MAX) {
            0 => Err(RenderError::Limit {
                operation,
                limit: "max_buffer_size",
                requested: unit as u64,
                allowed,
            }),
            count => Ok(count),
        }
    }

    fn pipelines(&mut self, format: wgpu::TextureFormat) -> &Pipelines {
        let index = match self.pipelines.iter().position(|(made, _)| *made == format) {
            Some(index) => index,
            None => {
                let pipelines = Pipelines {
                    quads: sprites::pipeline(&self.device, &self.layout, &self.view_layout, format),
                    debug: debug::pipeline(&self.device, &self.view_layout, format),
                };
                self.pipelines.push((format, pipelines));
                self.pipelines.len() - 1
            }
        };
        &self.pipelines[index].1
    }
}

/// The bytes of one view: world to clip scale, then translation, then alpha and padding, as
/// `f32`.
const VIEW_BYTES: u64 = 32;

/// The bytes of one sprite or tile instance.
const INSTANCE_BYTES: usize = INSTANCE_FLOATS * 4;

/// The view of a layer of `parallax`: `world * scale + translate` is `world_to_clip`.
fn view(camera: &CameraView, visible: Vec2, parallax: f32) -> [f32; 4] {
    let translate = world_to_clip(Vec2::ZERO, camera.clone(), visible, parallax);
    [2.0 / visible.x, 2.0 / visible.y, translate[0], translate[1]]
}

/// `view` for geometry placed relative to `origin`: `(local + origin) * scale + translate`.
fn moved(view: [f32; 4], origin: Vec2) -> [f32; 4] {
    let [scale_x, scale_y, x, y] = view;
    [
        scale_x,
        scale_y,
        x + origin.x * scale_x,
        y + origin.y * scale_y,
    ]
}

/// A tilemap or a sprite, in draw order.
enum Piece<'a> {
    Tilemap(&'a TilemapItem),
    Sprite(&'a SpriteItem),
}

/// One instanced draw of the quad pipeline: a run of instances from the frame's sprites or
/// from a tilemap chunk, through a view.
struct Draw {
    slot: u32,
    source: Source,
    sample: Sample,
    instances: std::ops::Range<u32>,
}

enum Source {
    /// One of the sprite instance buffers.
    Sprites(usize),
    Chunk(Entity, usize),
}

/// The size of a buffer made to hold `needed` bytes: the next power of two, so a growing
/// scene makes a buffer only now and then, but at most `cap` (and never below `needed`).
fn grown(needed: u64, cap: u64) -> u64 {
    needed
        .checked_next_power_of_two()
        .unwrap_or(needed)
        .min(cap)
        .max(needed)
}

/// `buffers[index]` holding at least `needed` bytes (a multiple of 4), made or replaced by a
/// `grown` one when it is missing or too small. `index` is at most `buffers.len()`.
fn reserve<'a>(
    device: &wgpu::Device,
    stats: &mut RenderStats,
    buffers: &'a mut Vec<wgpu::Buffer>,
    index: usize,
    needed: u64,
    cap: u64,
    label: &'static str,
) -> &'a wgpu::Buffer {
    if buffers
        .get(index)
        .is_none_or(|buffer| buffer.size() < needed)
    {
        stats.buffers_created += 1;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: grown(needed, cap),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if index < buffers.len() {
            buffers[index] = buffer;
        } else {
            buffers.push(buffer);
        }
    }
    &buffers[index]
}

/// Writes `bytes` (a multiple of 4 bytes long) at the start of `buffer`, through the queue,
/// so a write the device refuses raises an error instead of panicking.
fn write(queue: &wgpu::Queue, stats: &mut RenderStats, buffer: &wgpu::Buffer, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    stats.bytes_written += bytes.len() as u64;
    queue.write_buffer(buffer, 0, bytes);
}

/// A new buffer holding `bytes`, a multiple of 4 bytes long. It is written through the queue,
/// so a buffer the device refuses raises an error instead of panicking.
fn fill(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    stats: &mut RenderStats,
    label: &'static str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    stats.buffers_created += 1;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    write(queue, stats, &buffer, bytes);
    buffer
}

/// A texture of `width` by `height` RGBA pixels, row by row from the top-left.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("sprite page"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: RGBA,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        rgba,
    )
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sprite page"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn clear_color(color: Color) -> wgpu::Color {
    let [r, g, b, a] = color
        .0
        .to_be_bytes()
        .map(|channel| f64::from(channel) / 255.0);
    wgpu::Color { r, g, b, a }
}
