//! The wgpu renderer (feature `gpu`): draws a render frame's layers of tilemaps and sprites,
//! then the debug draw, through a fitted 2D camera into a target view, or offscreen into
//! RGBA pixels. Display only: nothing here feeds back into the simulation.

mod debug;
mod device;
mod sprites;
mod tilemaps;

use std::collections::{BTreeMap, BTreeSet};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ludenea_app::meta::Filter;
use ludenea_draw::Color;
use ludenea_draw::frame::{CameraView, RenderFrame, SpriteItem, TilemapItem};
use ludenea_ecs::{AssetId, Entity};
use ludenea_math::Vec2;
use serde::{Deserialize, Serialize};
use wgpu::util::DeviceExt;

use crate::atlas::{Atlas, Region};
use crate::camera::{DEFAULT_CAMERA, fit, world_to_clip};
use crate::interpolate::interpolate;
use sprites::{BLANK, INSTANCE_FLOATS, Sample};

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

/// The frames to draw: `current`, and the one before it with how far between them to draw.
/// With `previous`, what is drawn is `interpolate(previous, current, alpha)`.
#[derive(Clone, Copy, Debug)]
pub struct Frames<'a> {
    pub previous: Option<&'a RenderFrame>,
    pub current: &'a RenderFrame,
    pub alpha: f32,
}

/// What the viewer adds over the frame: the debug draw layers to show, the selected entities,
/// and a camera that overrides the frame's (the editor's free camera). Each field may be left
/// out of its JSON, which `web` reads.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Overlay {
    pub debug_layers: BTreeSet<String>,
    pub selected: BTreeSet<Entity>,
    pub camera: Option<CameraView>,
}

/// What a draw could not draw as asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DrawReport {
    /// What was drawn as a placeholder, in draw order: each sprite's entity, texture and
    /// frame, and each tile's tilemap entity, tileset and tile index.
    pub missing: Vec<(Entity, AssetId, u32)>,
}

/// What `render_rgba` read back, and what it could not draw as asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// `width * 4` bytes of RGBA per row, tightly packed, top row first.
    pub rgba: Vec<u8>,
    pub report: DrawReport,
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

/// Draws render frames with one device. Pages come from `set_atlas`; until then, and for any
/// sprite whose region is not in the atlas, a magenta placeholder is drawn.
///
/// The renderer handles its device's errors: an error the device raises outside an error
/// scope is returned by the next `set_atlas`, `draw` or `render_rgba` as `RenderError::Gpu`.
/// Its operation is the call's own when the error was raised while the call ran, and
/// `"earlier GPU work"` when it was pending before the call (raised by the host's own use of
/// the device, or after an earlier call returned); the call then does nothing. A call that
/// returns `Gpu` with its own operation ran: `set_atlas` has replaced the atlas, and `draw`
/// has submitted its work. Once the device is lost, every call returns
/// `RenderError::DeviceLost` without touching it, and the caller makes a new renderer on a
/// new device.
pub struct Renderer {
    device: wgpu::Device,
    faults: Arc<Mutex<Faults>>,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    /// The layout of a layer's view: world to clip space, at a dynamic offset.
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
        }
    }

    /// The largest page side to pack atlases for: `min(4096, max_texture_dimension_2d)`.
    pub fn max_page_side(&self) -> u32 {
        MAX_PAGE_SIDE.min(self.device.limits().max_texture_dimension_2d)
    }

    /// Uploads the atlas's pages, replacing the previous atlas. A page wider or taller than
    /// `max_page_side` is refused with `RenderError::Limit`, naming `max_page_side` when the
    /// renderer's 4096 cap binds and `max_texture_dimension_2d` when the device's limit does,
    /// and a page with a side of 0 or whose pixels do not match its size with
    /// `RenderError::InvalidAtlas`; the previous atlas then stays.
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
        Ok(())
    }

    /// Draws `frames` into `target`, a view of `size` pixels in the renderer's format. A size
    /// past `max_texture_dimension_2d` is refused. Of tilemaps sharing an entity, which only
    /// a malformed frame holds, the first listed is drawn and the others are skipped.
    pub fn draw(
        &mut self,
        target: &wgpu::TextureView,
        size: Size,
        frames: Frames<'_>,
        overlay: &Overlay,
    ) -> Result<DrawReport, RenderError> {
        self.guarded("draw", |renderer| {
            renderer.fits("draw", size)?;
            renderer.encode(target, renderer.format, size, frames, overlay)
        })
    }

    /// Draws `frames` offscreen and reads it back, with the report `draw` gives. A target
    /// with no pixels gives no bytes and draws nothing; one past `max_texture_dimension_2d`,
    /// or whose read-back is past `max_buffer_size`, is refused. Not on WebAssembly: it waits
    /// for the GPU, which a browser's thread cannot do.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_rgba(
        &mut self,
        size: Size,
        frames: Frames<'_>,
        overlay: &Overlay,
    ) -> Result<Rendered, RenderError> {
        self.guarded("render_rgba", |renderer| {
            renderer.read_back(size, frames, overlay)
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
        frames: Frames<'_>,
        overlay: &Overlay,
    ) -> Result<Rendered, RenderError> {
        if size.width == 0 || size.height == 0 {
            return Ok(Rendered::default());
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
        let extent = wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: RGBA,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let report = self.encode(
            &texture.create_view(&Default::default()),
            RGBA,
            size,
            frames,
            overlay,
        )?;

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen read-back"),
            size: read_back,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(size.height),
                },
            },
            extent,
        );
        self.queue.submit([encoder.finish()]);

        let (sender, receiver) = mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
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
        let mapped = buffer
            .get_mapped_range(..)
            .map_err(|error| failed(error.to_string()))?;
        let rgba = mapped
            .chunks_exact(padded as usize)
            .flat_map(|line| &line[..row as usize])
            .copied()
            .collect();
        Ok(Rendered { rgba, report })
    }

    /// Clears `target` to the background, draws the layers in order (tilemaps and sprites
    /// by `(order, entity)` within each), then the debug draw.
    fn encode(
        &mut self,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: Size,
        frames: Frames<'_>,
        overlay: &Overlay,
    ) -> Result<DrawReport, RenderError> {
        let interpolated;
        let frame = match frames.previous {
            Some(previous) => {
                interpolated = interpolate(previous, frames.current, frames.alpha);
                &interpolated
            }
            None => frames.current,
        };
        let camera = overlay
            .camera
            .clone()
            .or_else(|| frame.camera.as_ref().map(CameraView::from))
            .unwrap_or(DEFAULT_CAMERA);
        let camera = &camera;
        let aspect = size.width.max(1) as f32 / size.height.max(1) as f32;
        let visible = fit(camera.view, aspect);
        let pages: Vec<(u32, u32)> = self.pages.iter().map(|page| page.size).collect();
        // A well-formed frame has one tilemap per entity; of a malformed frame's duplicates,
        // only the first listed is built and drawn.
        let mut seen = BTreeSet::new();
        let tilemaps: Vec<&TilemapItem> = frame
            .tilemaps
            .iter()
            .filter(|item| seen.insert(item.entity))
            .collect();
        self.tilemaps
            .update(&self.device, &self.queue, &tilemaps, &self.regions, &pages);

        // Tilemaps before sprites, so a stable sort draws a tilemap first on a tie.
        let mut pieces: Vec<(u16, i32, Entity, Piece<'_>)> = tilemaps
            .iter()
            .map(|&item| (item.layer, item.order, item.entity, Piece::Tilemap(item)))
            .chain(
                frame
                    .sprites
                    .iter()
                    .map(|item| (item.layer, item.order, item.entity, Piece::Sprite(item))),
            )
            .collect();
        pieces.sort_by_key(|(layer, order, entity, _)| (*layer, *order, *entity));

        // One view per layer of sprites drawn and one per tilemap (its layer's, moved to the
        // tilemap's position, so its chunks do not depend on it), then the debug draw's.
        let mut views: Vec<[f32; 4]> = Vec::new();
        let mut slots: BTreeMap<u16, u32> = BTreeMap::new();
        let mut draws: Vec<Draw> = Vec::new();
        let mut instances: Vec<sprites::Instance> = Vec::new();
        // Sprite instances go in buffers of at most `per_buffer`, below `max_buffer_size`.
        let per_buffer = self.per_buffer(INSTANCE_BYTES)?;
        let mut missing = Vec::new();
        let layer_view = |layer: u16| {
            let parallax = frame
                .layers
                .get(usize::from(layer))
                .map_or(1.0, |info| info.parallax);
            view(camera, visible, parallax)
        };
        for (layer, _, _, piece) in pieces {
            match piece {
                Piece::Sprite(item) => {
                    let slot = *slots.entry(layer).or_insert_with(|| {
                        views.push(layer_view(layer));
                        views.len() as u32 - 1
                    });
                    let (sample, instance) = sprites::instance(item, &self.regions, &pages);
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
                    views.push(moved(layer_view(layer), item.position));
                    let slot = views.len() as u32 - 1;
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
        views.push(view(camera, visible, 1.0));
        let debug_slot = views.len() as u32 - 1;

        let instances: Vec<wgpu::Buffer> = instances
            .chunks(per_buffer)
            .map(|part| {
                let bytes: Vec<u8> = part
                    .iter()
                    .flatten()
                    .flat_map(|value| value.to_le_bytes())
                    .collect();
                fill(
                    &self.device,
                    &self.queue,
                    "sprite instances",
                    &bytes,
                    wgpu::BufferUsages::VERTEX,
                )
            })
            .collect();
        let line_width = debug::LINE_PIXELS * visible.x / size.width.max(1) as f32;
        let debug_bytes = debug::vertices(&frame.debug, overlay, line_width);
        // Whole triangles per buffer, below `max_buffer_size`.
        let triangle = 3 * debug::VERTEX_BYTES;
        let debug_vertices: Vec<(wgpu::Buffer, u32)> = debug_bytes
            .chunks(self.per_buffer(triangle)?.saturating_mul(triangle))
            .map(|part| {
                let buffer = fill(
                    &self.device,
                    &self.queue,
                    "debug draw",
                    part,
                    wgpu::BufferUsages::VERTEX,
                );
                (buffer, debug::count(part))
            })
            .collect();

        // Each view on its own aligned slot, picked by a dynamic offset.
        let stride = self
            .device
            .limits()
            .min_uniform_buffer_offset_alignment
            .max(VIEW_BYTES as u32);
        let mut view_bytes = vec![0; views.len() * stride as usize];
        for (index, values) in views.iter().enumerate() {
            let start = index * stride as usize;
            for (at, value) in values.iter().enumerate() {
                view_bytes[start + at * 4..start + at * 4 + 4]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
        let view_buffer = self.buffer("views", &view_bytes, wgpu::BufferUsages::UNIFORM)?;
        let view_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("views"),
            layout: &self.view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &view_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(VIEW_BYTES),
                }),
            }],
        });
        let pipelines = self.pipelines(format).clone();

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
            if !draws.is_empty() {
                pass.set_pipeline(&pipelines.quads);
            }
            for draw in &draws {
                let buffer = match draw.source {
                    Source::Sprites(part) => instances.get(part),
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
                pass.set_bind_group(1, &view_group, &[draw.slot * stride]);
                pass.draw(0..4, draw.instances.clone());
            }
            if !debug_vertices.is_empty() {
                pass.set_pipeline(&pipelines.debug);
                pass.set_bind_group(0, &view_group, &[debug_slot * stride]);
            }
            for (vertices, count) in &debug_vertices {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        Ok(DrawReport { missing })
    }

    /// A buffer holding `bytes`, refused past `max_buffer_size`.
    fn buffer(
        &self,
        label: &'static str,
        bytes: &[u8],
        usage: wgpu::BufferUsages,
    ) -> Result<wgpu::Buffer, RenderError> {
        let allowed = self.device.limits().max_buffer_size;
        if bytes.len() as u64 > allowed {
            return Err(RenderError::Limit {
                operation: "draw",
                limit: "max_buffer_size",
                requested: bytes.len() as u64,
                allowed,
            });
        }
        Ok(fill(&self.device, &self.queue, label, bytes, usage))
    }

    /// How many items of `unit` bytes fit in one buffer; refused if not even one does.
    fn per_buffer(&self, unit: usize) -> Result<usize, RenderError> {
        let allowed = self.device.limits().max_buffer_size;
        match usize::try_from(allowed / unit as u64).unwrap_or(usize::MAX) {
            0 => Err(RenderError::Limit {
                operation: "draw",
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

/// The bytes of one view: world to clip scale, then translation, as `f32`.
const VIEW_BYTES: u64 = 16;

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
/// from a tilemap chunk, through a layer's view.
struct Draw {
    slot: u32,
    source: Source,
    sample: Sample,
    instances: std::ops::Range<u32>,
}

enum Source {
    /// One of the frame's sprite instance buffers.
    Sprites(usize),
    Chunk(Entity, usize),
}

/// A new buffer holding `bytes`, a multiple of 4 bytes long. It is written through the queue,
/// so a buffer the device refuses raises an error instead of panicking.
fn fill(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytes);
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
