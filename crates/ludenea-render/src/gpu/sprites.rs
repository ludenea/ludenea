//! The quad pipeline that sprites and tilemap tiles share, and the instances of sprites.

use std::collections::BTreeMap;

use ludenea_app::meta::Filter;
use ludenea_draw::frame::SpriteItem;
use ludenea_ecs::AssetId;

use crate::atlas::Region;

/// The floats of one instance, in the shader's `Instance` order: center, axis_x, axis_y,
/// offset, size, uv_min, uv_size (two each), tint (four).
pub(super) const INSTANCE_FLOATS: usize = 18;
pub(super) type Instance = [f32; INSTANCE_FLOATS];

const SHADER: &str = include_str!("shaders/sprite.wgsl");
const PLACEHOLDER: [f32; 4] = [1.0, 0.0, 1.0, 1.0];
const WHITE: [f32; 4] = [1.0; 4];

/// What a run of consecutive instances samples: a page (`None` for the blank texture the
/// placeholders use) through a filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Sample {
    pub page: Option<u32>,
    pub filter: Filter,
}

/// The placeholders' sample: the blank texture.
pub(super) const BLANK: Sample = Sample {
    page: None,
    filter: Filter::Nearest,
};

/// The region of `(texture, frame)` and its page's size, if both are uploaded. `pages`
/// holds each uploaded page's width and height.
pub(super) fn find<'a>(
    regions: &'a BTreeMap<(AssetId, u32), Region>,
    pages: &[(u32, u32)],
    texture: AssetId,
    frame: u32,
) -> Option<(&'a Region, (u32, u32))> {
    let region = regions.get(&(texture, frame))?;
    Some((region, *pages.get(region.page as usize)?))
}

/// A region's sample and its uv rectangle in its page (min then size, normalized).
pub(super) fn uv(region: &Region, page: (u32, u32)) -> (Sample, [f32; 4]) {
    let (page_width, page_height) = (page.0 as f32, page.1 as f32);
    let sample = Sample {
        page: Some(region.page),
        filter: region.filter,
    };
    let uv = [
        region.x as f32 / page_width,
        region.y as f32 / page_height,
        region.width as f32 / page_width,
        region.height as f32 / page_height,
    ];
    (sample, uv)
}

/// The sprite's instance in world space and what it samples, or `None` for the sample when
/// its region is missing and it is drawn as a placeholder.
pub(super) fn instance(
    item: &SpriteItem,
    regions: &BTreeMap<(AssetId, u32), Region>,
    pages: &[(u32, u32)],
) -> (Option<Sample>, Instance) {
    match find(regions, pages, item.texture, item.frame) {
        Some((region, page)) => {
            let (sample, uv) = uv(region, page);
            (Some(sample), place(item, textured(item, region, uv)))
        }
        None => (None, place(item, placeholder())),
    }
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
fn textured(item: &SpriteItem, region: &Region, uv: [f32; 4]) -> Quad {
    let unit = region.pixels_per_unit as f32;
    let (width, height) = (region.width as f32, region.height as f32);
    let (pivot_x, pivot_y) = (region.pivot.0 as f32, region.pivot.1 as f32);
    Quad {
        // The pivot counts from the top-left, the quad's local +y is up.
        offset: [-pivot_x / unit, -(height - pivot_y) / unit],
        size: [width / unit, height / unit],
        uv,
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

/// Places a quad on the sprite's pose in world space: flips mirror it around the pivot, and
/// the rotation turns it. The layer's view puts it in clip space on the GPU.
fn place(item: &SpriteItem, mut quad: Quad) -> Instance {
    if item.flip_x {
        quad.offset[0] = -quad.offset[0];
        quad.size[0] = -quad.size[0];
    }
    if item.flip_y {
        quad.offset[1] = -quad.offset[1];
        quad.size[1] = -quad.size[1];
    }
    let (cos, sin) = (item.rotation.cos, item.rotation.sin);
    let position = [item.position.x, item.position.y];
    instance_of(
        position,
        [cos, sin, -sin, cos],
        quad.offset,
        quad.size,
        quad.uv,
        quad.tint,
    )
}

/// An unrotated quad at `center`, in world space: a tile of a tilemap chunk.
pub(super) fn tile(
    center: [f32; 2],
    offset: [f32; 2],
    size: [f32; 2],
    uv: [f32; 4],
    placeholder: bool,
) -> Instance {
    let tint = if placeholder { PLACEHOLDER } else { WHITE };
    instance_of(center, [1.0, 0.0, 0.0, 1.0], offset, size, uv, tint)
}

fn instance_of(
    center: [f32; 2],
    axes: [f32; 4],
    offset: [f32; 2],
    size: [f32; 2],
    uv: [f32; 4],
    tint: [f32; 4],
) -> Instance {
    [
        center[0], center[1], axes[0], axes[1], axes[2], axes[3], offset[0], offset[1], size[0],
        size[1], uv[0], uv[1], uv[2], uv[3], tint[0], tint[1], tint[2], tint[3],
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

/// The quad pipeline for one target format, with premultiplied alpha blending: a page at
/// group 0, the layer's view at group 1.
pub(super) fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sprite"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sprite"),
        bind_group_layouts: &[Some(layout), Some(view_layout)],
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
