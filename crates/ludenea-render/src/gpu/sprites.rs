//! The sprite pipeline and the instances it draws: one quad per sprite, batched by
//! `(layer, page, filter)` in item order.

use std::collections::BTreeMap;
use std::ops::Range;

use ludenea_app::meta::Filter;
use ludenea_draw::frame::{CameraView, RenderFrame, SpriteItem};
use ludenea_ecs::{AssetId, Entity};
use ludenea_math::Vec2;

use crate::atlas::Region;
use crate::camera::world_to_clip;

/// The floats of one instance, in the shader's `Instance` order: center, axis_x, axis_y,
/// offset, size, uv_min, uv_size (two each), tint (four).
pub(super) const INSTANCE_FLOATS: usize = 18;
pub(super) type Instance = [f32; INSTANCE_FLOATS];

const SHADER: &str = include_str!("shaders/sprite.wgsl");
const PLACEHOLDER: [f32; 4] = [1.0, 0.0, 1.0, 1.0];

/// What a run of consecutive instances samples: a page (`None` for the blank texture the
/// placeholders use) through a filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BatchKey {
    pub layer: u16,
    pub page: Option<u32>,
    pub filter: Filter,
}

pub(super) struct Batch {
    pub key: BatchKey,
    pub instances: Range<u32>,
}

/// The instances of a frame's sprites, their batches, and the sprites whose region is missing.
pub(super) struct Sprites {
    pub instances: Vec<Instance>,
    pub batches: Vec<Batch>,
    pub missing: Vec<(Entity, AssetId, u32)>,
}

/// Builds the instances of `frame`'s sprites, in item order, seen by `camera` over `visible`.
/// `pages` holds each uploaded page's width and height; a region on a page that is not
/// uploaded counts as missing.
pub(super) fn build(
    frame: &RenderFrame,
    camera: &CameraView,
    visible: Vec2,
    regions: &BTreeMap<(AssetId, u32), Region>,
    pages: &[(u32, u32)],
) -> Sprites {
    let mut sprites = Sprites {
        instances: Vec::with_capacity(frame.sprites.len()),
        batches: Vec::new(),
        missing: Vec::new(),
    };
    for item in &frame.sprites {
        let parallax = frame
            .layers
            .get(usize::from(item.layer))
            .map_or(1.0, |layer| layer.parallax);
        let found = regions
            .get(&(item.texture, item.frame))
            .and_then(|region| Some((region, *pages.get(region.page as usize)?)));
        let (page, filter, quad) = match found {
            Some((region, page)) => (
                Some(region.page),
                region.filter,
                textured(item, region, page),
            ),
            None => {
                sprites
                    .missing
                    .push((item.entity, item.texture, item.frame));
                (None, Filter::Nearest, placeholder())
            }
        };
        let key = BatchKey {
            layer: item.layer,
            page,
            filter,
        };
        let index = sprites.instances.len() as u32;
        sprites
            .instances
            .push(place(item, camera, visible, parallax, quad));
        match sprites.batches.last_mut() {
            Some(batch) if batch.key == key => batch.instances.end = index + 1,
            _ => sprites.batches.push(Batch {
                key,
                instances: index..index + 1,
            }),
        }
    }
    sprites
}

/// A quad before it is placed: its bottom-left corner and size in world units from the
/// pivot, its uv rectangle (min then size), and its tint.
struct Quad {
    offset: [f32; 2],
    size: [f32; 2],
    uv: [f32; 4],
    tint: [f32; 4],
}

/// The region's quad: `region size / pixels_per_unit` world units, its pivot on the position.
fn textured(item: &SpriteItem, region: &Region, page: (u32, u32)) -> Quad {
    let unit = region.pixels_per_unit as f32;
    let (width, height) = (region.width as f32, region.height as f32);
    let (pivot_x, pivot_y) = (region.pivot.0 as f32, region.pivot.1 as f32);
    let (page_width, page_height) = (page.0 as f32, page.1 as f32);
    Quad {
        // The pivot counts from the top-left, the quad's local +y is up.
        offset: [-pivot_x / unit, -(height - pivot_y) / unit],
        size: [width / unit, height / unit],
        uv: [
            region.x as f32 / page_width,
            region.y as f32 / page_height,
            width / page_width,
            height / page_height,
        ],
        tint: tint(item),
    }
}

/// One magenta world unit centred on the position, sampling the blank texture.
fn placeholder() -> Quad {
    Quad {
        offset: [-0.5, -0.5],
        size: [1.0, 1.0],
        uv: [0.0, 0.0, 1.0, 1.0],
        tint: PLACEHOLDER,
    }
}

/// Places a quad on the sprite's pose: flips mirror it around the pivot, the camera and the
/// layer's parallax put the pivot in clip space, and the rotation turns it.
fn place(
    item: &SpriteItem,
    camera: &CameraView,
    visible: Vec2,
    parallax: f32,
    mut quad: Quad,
) -> Instance {
    if item.flip_x {
        quad.offset[0] = -quad.offset[0];
        quad.size[0] = -quad.size[0];
    }
    if item.flip_y {
        quad.offset[1] = -quad.offset[1];
        quad.size[1] = -quad.size[1];
    }
    let center = world_to_clip(item.position, camera.clone(), visible, parallax);
    let (scale_x, scale_y) = (2.0 / visible.x, 2.0 / visible.y);
    let (cos, sin) = (item.rotation.cos, item.rotation.sin);
    let Quad {
        offset,
        size,
        uv,
        tint,
    } = quad;
    [
        center[0],
        center[1],
        scale_x * cos,
        scale_y * sin,
        -scale_x * sin,
        scale_y * cos,
        offset[0],
        offset[1],
        size[0],
        size[1],
        uv[0],
        uv[1],
        uv[2],
        uv[3],
        tint[0],
        tint[1],
        tint[2],
        tint[3],
    ]
}

/// `0xRRGGBBAA` to floats.
fn tint(item: &SpriteItem) -> [f32; 4] {
    item.tint
        .0
        .to_be_bytes()
        .map(|channel| f32::from(channel) / 255.0)
}

/// The bind group layout every page shares: the page texture and its filter's sampler.
pub(super) fn page_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sprite page"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

/// The sprite pipeline for one target format, with premultiplied alpha blending.
pub(super) fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sprite"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sprite"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2,
        4 => Float32x2, 5 => Float32x2, 6 => Float32x2, 7 => Float32x4
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sprite"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: (INSTANCE_FLOATS * 4) as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &attributes,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}
