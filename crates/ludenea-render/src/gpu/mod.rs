//! The wgpu renderer (feature `gpu`): draws a render frame's sprites through a fitted 2D camera
//! into a target view, or offscreen into RGBA pixels. Display only: nothing here feeds back
//! into the simulation.

mod device;
mod sprites;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;

use ludenea_app::meta::Filter;
use ludenea_draw::Color;
use ludenea_draw::frame::{CameraView, RenderFrame};
use ludenea_ecs::{AssetId, Entity};
use wgpu::util::DeviceExt;

use crate::atlas::{Atlas, Region};
use crate::camera::{DEFAULT_CAMERA, fit};

pub use device::headless;

/// The color a target is cleared to before anything is drawn.
pub const BACKGROUND: Color = Color(0x121214FF);

/// The largest atlas page side the renderer accepts, before the device's own limit.
const MAX_PAGE_SIDE: u32 = 4096;

/// The format of pages and of `render_rgba`'s target: 8-bit RGBA, stored as given.
const RGBA: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A target's size in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// The frames to draw: `current`, and the one before it with how far between them to draw.
#[derive(Clone, Copy, Debug)]
pub struct Frames<'a> {
    pub previous: Option<&'a RenderFrame>,
    pub current: &'a RenderFrame,
    pub alpha: f32,
}

/// What the viewer adds over the frame: the debug draw layers to show, the selected entities,
/// and a camera that overrides the frame's (the editor's free camera).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    pub debug_layers: BTreeSet<String>,
    pub selected: BTreeSet<Entity>,
    pub camera: Option<CameraView>,
}

/// What a draw could not draw as asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrawReport {
    /// The sprites drawn as a placeholder: entity, texture, and frame.
    pub missing: Vec<(Entity, AssetId, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
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
}

/// Draws render frames with one device. Pages come from `set_atlas`; until then, and for any
/// sprite whose region is not in the atlas, a magenta placeholder is drawn.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    /// The sprite pipeline per target format, made on first use.
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    /// One sampler per filter: nearest, then linear.
    samplers: [wgpu::Sampler; 2],
    /// Per page, its size and its bind group per filter.
    pages: Vec<Page>,
    /// A white texel, which the placeholders sample.
    blank: wgpu::BindGroup,
    regions: BTreeMap<(AssetId, u32), Region>,
}

struct Page {
    size: (u32, u32),
    bind_groups: [wgpu::BindGroup; 2],
}

impl Renderer {
    /// A renderer drawing into targets of `format`.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layout = sprites::page_layout(&device);
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
            queue,
            format,
            layout,
            pipelines: Vec::new(),
            samplers,
            pages: Vec::new(),
            blank,
            regions: BTreeMap::new(),
        }
    }

    /// The largest page side to pack atlases for: `min(4096, max_texture_dimension_2d)`.
    pub fn max_page_side(&self) -> u32 {
        MAX_PAGE_SIDE.min(self.device.limits().max_texture_dimension_2d)
    }

    /// Uploads the atlas's pages, replacing the previous atlas. A page wider or taller than
    /// `max_page_side` is refused, and so is a page whose pixels do not match its size; the
    /// previous atlas then stays.
    pub fn set_atlas(&mut self, atlas: &Atlas) -> Result<(), RenderError> {
        let allowed = self.max_page_side();
        for page in &atlas.pages {
            let side = page.width.max(page.height);
            if side > allowed {
                return Err(RenderError::Limit {
                    operation: "set_atlas",
                    limit: "max_texture_dimension_2d",
                    requested: u64::from(side),
                    allowed: u64::from(allowed),
                });
            }
            let expected = u64::from(page.width) * u64::from(page.height) * 4;
            if page.width == 0 || page.height == 0 || page.rgba.len() as u64 != expected {
                return Err(RenderError::Gpu {
                    operation: "set_atlas",
                    message: format!(
                        "a {}x{} page needs {expected} bytes of RGBA pixels, but it has {}",
                        page.width,
                        page.height,
                        page.rgba.len()
                    ),
                });
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
        Ok(())
    }

    /// Draws `frames.current` into `target`, a view of `size` pixels in the renderer's format.
    pub fn draw(
        &mut self,
        target: &wgpu::TextureView,
        size: Size,
        frames: Frames<'_>,
        overlay: &Overlay,
    ) -> Result<DrawReport, RenderError> {
        self.encode(target, self.format, size, frames, overlay)
    }

    /// Draws `frames.current` offscreen and reads it back: `size.width * 4` bytes of RGBA per
    /// row, tightly packed, top row first. A target with no pixels gives no bytes.
    pub fn render_rgba(
        &mut self,
        size: Size,
        frames: Frames<'_>,
        overlay: &Overlay,
    ) -> Result<Vec<u8>, RenderError> {
        if size.width == 0 || size.height == 0 {
            return Ok(Vec::new());
        }
        let allowed = self.device.limits().max_texture_dimension_2d;
        let side = size.width.max(size.height);
        if side > allowed {
            return Err(RenderError::Limit {
                operation: "render_rgba",
                limit: "max_texture_dimension_2d",
                requested: u64::from(side),
                allowed: u64::from(allowed),
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
        self.encode(
            &texture.create_view(&Default::default()),
            RGBA,
            size,
            frames,
            overlay,
        )?;

        // Rows are copied out padded to the copy alignment, then packed.
        let row = size.width * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen read-back"),
            size: u64::from(padded) * u64::from(size.height),
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
        let pixels = mapped
            .chunks_exact(padded as usize)
            .flat_map(|line| &line[..row as usize])
            .copied()
            .collect();
        Ok(pixels)
    }

    /// Clears `target` to the background and draws the sprites in item order, a batch per
    /// `(layer, page, filter)` run.
    fn encode(
        &mut self,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: Size,
        frames: Frames<'_>,
        overlay: &Overlay,
    ) -> Result<DrawReport, RenderError> {
        let frame = frames.current;
        let camera = overlay
            .camera
            .as_ref()
            .or(frame.camera.as_ref())
            .unwrap_or(&DEFAULT_CAMERA);
        let aspect = size.width.max(1) as f32 / size.height.max(1) as f32;
        let visible = fit(camera.view, aspect);
        let pages: Vec<(u32, u32)> = self.pages.iter().map(|page| page.size).collect();
        let sprites = sprites::build(frame, camera, visible, &self.regions, &pages);

        let bytes: Vec<u8> = sprites
            .instances
            .iter()
            .flatten()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let allowed = self.device.limits().max_buffer_size;
        if bytes.len() as u64 > allowed {
            return Err(RenderError::Limit {
                operation: "draw",
                limit: "max_buffer_size",
                requested: bytes.len() as u64,
                allowed,
            });
        }
        let instances = (!bytes.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("sprite instances"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        let pipeline = self.pipeline(format).clone();

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
            if let Some(instances) = &instances {
                pass.set_pipeline(&pipeline);
                pass.set_vertex_buffer(0, instances.slice(..));
                for batch in &sprites.batches {
                    let bind_group = match batch.key.page {
                        Some(page) => {
                            let filter = match batch.key.filter {
                                Filter::Nearest => 0,
                                Filter::Linear => 1,
                            };
                            &self.pages[page as usize].bind_groups[filter]
                        }
                        None => &self.blank,
                    };
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.draw(0..4, batch.instances.clone());
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        Ok(DrawReport {
            missing: sprites.missing,
        })
    }

    fn pipeline(&mut self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let index = match self.pipelines.iter().position(|(made, _)| *made == format) {
            Some(index) => index,
            None => {
                let pipeline = sprites::pipeline(&self.device, &self.layout, format);
                self.pipelines.push((format, pipeline));
                self.pipelines.len() - 1
            }
        };
        &self.pipelines[index].1
    }
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
