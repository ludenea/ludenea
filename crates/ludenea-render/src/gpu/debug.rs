//! The debug draw over the frame: the requested layers' triangles, selected entities
//! highlighted, as the editor draws them.

use ludenea_draw::DebugDrawFrame;
use ludenea_draw::mesh;

use super::Overlay;

/// Outline width in target pixels, whatever the zoom.
pub(super) const LINE_PIXELS: f32 = 1.5;

/// World position then RGBA color, all `f32`.
const VERTEX_FLOATS: usize = 6;

const SHADER: &str = include_str!("shaders/debug.wgsl");

/// The vertex bytes of the overlay's debug layers, outlines `line_width` world units wide.
pub(super) fn vertices(frame: &DebugDrawFrame, overlay: &Overlay, line_width: f32) -> Vec<u8> {
    let vertices = mesh::triangulate(frame, &overlay.debug_layers, &overlay.selected, line_width);
    let mut bytes = Vec::with_capacity(vertices.len() * VERTEX_FLOATS * 4);
    for vertex in vertices {
        let [r, g, b, a] = vertex
            .color
            .to_be_bytes()
            .map(|channel| f32::from(channel) / 255.0);
        for value in [vertex.position[0], vertex.position[1], r, g, b, a] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

/// How many vertices `bytes` holds.
pub(super) fn count(bytes: &[u8]) -> u32 {
    (bytes.len() / (VERTEX_FLOATS * 4)) as u32
}

/// The debug draw pipeline for one target format, alpha blended as the editor's: the view
/// at group 0.
pub(super) fn pipeline(
    device: &wgpu::Device,
    view_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("debug draw"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("debug draw"),
        bind_group_layouts: &[Some(view_layout)],
        immediate_size: 0,
    });
    let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("debug draw"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: (VERTEX_FLOATS * 4) as wgpu::BufferAddress,
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
