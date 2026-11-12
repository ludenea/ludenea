//! The lit scene on Metal: the height field drawn to one 16-bit float image
//! with the lighting in its fragment function, or to a geometry buffer of
//! four or five attachments, optionally followed by a pass that reads the
//! geometry buffer and lights one triangle covering the target. Each
//! repetition is timed from the command buffer's creation to its completion.
//! Throwaway, never product code.

use std::time::Instant;

use crate::backend::{LitRender, LitVariant};
use crate::scene::{LIT_SIZE, Scene};

use super::gpu::{self, ColorAttachment, ColorTarget, DepthAttachment, Gpu, PipelineShape, TextureShape};
use super::objc::{Id, Owned, Pool, present};
use super::values;

const SIZE: usize = LIT_SIZE as usize;
const OPAQUE_BLACK: values::ClearColor = values::ClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 1.0 };
const ZERO: values::ClearColor = values::ClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 };
/// The geometry buffer: albedo, normal, material, world position, and for the
/// wider form the albedo at half intensity.
const GEOMETRY_FORMATS: [usize; 5] =
    [values::PIXEL_FORMAT_RGBA8_UNORM, values::PIXEL_FORMAT_RGBA8_UNORM, values::PIXEL_FORMAT_RGBA8_UNORM, values::PIXEL_FORMAT_RGBA16_FLOAT, values::PIXEL_FORMAT_RGBA8_UNORM];
/// The attachments the lighting pass reads.
const GEOMETRY_ATTACHMENTS_READ: usize = 4;

/// The sources of the three shader files of the lit scene.
pub struct Sources<'a> {
    pub forward: &'a str,
    pub geometry: &'a str,
    pub resolve: &'a str,
}

/// What every variant draws with.
struct Frame {
    vertices: Owned,
    vertex_count: usize,
    constants: Owned,
    depth: Owned,
    depth_state: Owned,
}

/// The pass that draws the height field into its attachments.
struct Fill {
    pipeline: Owned,
    targets: Vec<Owned>,
    clear: values::ClearColor,
}

/// The pass that lights one triangle from the geometry buffer.
struct Lighting {
    pipeline: Owned,
    output: Owned,
}

fn target(gpu: &Gpu, what: &str, format: usize) -> Result<Owned, String> {
    gpu.texture(what, &TextureShape::target(format, SIZE, SIZE, values::TEXTURE_USAGE_RENDER_TARGET | values::TEXTURE_USAGE_SHADER_READ))
}

fn create_frame(gpu: &Gpu, scene: &Scene) -> Result<Frame, String> {
    Ok(Frame {
        vertices: gpu.buffer("the lit scene's vertices", scene.lit_vertex_bytes())?,
        vertex_count: scene.lit_vertices.len(),
        constants: gpu.buffer("the lit scene's constants", scene.lit_constant_bytes())?,
        depth: gpu.texture("the lit scene's depth target", &TextureShape::target(values::PIXEL_FORMAT_DEPTH32_FLOAT, SIZE, SIZE, values::TEXTURE_USAGE_RENDER_TARGET))?,
        depth_state: gpu.depth_state(true)?,
    })
}

/// A fill pass from one shader file: its pipeline and one target per format.
fn create_fill(gpu: &Gpu, file: &str, source: &str, functions: [&str; 2], formats: &[usize], clear: values::ClearColor) -> Result<Fill, String> {
    let library = gpu.library(file, source)?;
    let vertex = gpu::function(&library, file, functions[0])?;
    let fragment = gpu::function(&library, file, functions[1])?;
    let colors: Vec<ColorTarget> = formats.iter().map(|format| ColorTarget { format: *format, blended: false }).collect();
    let pipeline = gpu.render_pipeline(&PipelineShape { what: file, vertex: &vertex, fragment: &fragment, colors: &colors, depth: true, samples: 1 })?;
    let targets = formats.iter().enumerate().map(|(index, format)| target(gpu, &format!("attachment {index} of {file}"), *format)).collect::<Result<_, _>>()?;
    Ok(Fill { pipeline, targets, clear })
}

fn forward_fill(gpu: &Gpu, sources: &Sources) -> Result<Fill, String> {
    create_fill(gpu, "lit_forward.metal", sources.forward, ["lit_forward_vertex", "lit_forward_fragment"], &[values::PIXEL_FORMAT_RGBA16_FLOAT], OPAQUE_BLACK)
}

fn geometry_fill(gpu: &Gpu, sources: &Sources, attachments: usize) -> Result<Fill, String> {
    let fragment = if attachments == GEOMETRY_FORMATS.len() { "lit_geometry_fragment_five" } else { "lit_geometry_fragment_four" };
    create_fill(gpu, "lit_geometry.metal", sources.geometry, ["lit_geometry_vertex", fragment], &GEOMETRY_FORMATS[..attachments], ZERO)
}

fn create_lighting(gpu: &Gpu, sources: &Sources) -> Result<Lighting, String> {
    const FILE: &str = "lit_resolve.metal";
    let library = gpu.library(FILE, sources.resolve)?;
    let vertex = gpu::function(&library, FILE, "lit_resolve_vertex")?;
    let fragment = gpu::function(&library, FILE, "lit_resolve_fragment")?;
    let colors = [ColorTarget { format: values::PIXEL_FORMAT_RGBA16_FLOAT, blended: false }];
    let pipeline = gpu.render_pipeline(&PipelineShape { what: FILE, vertex: &vertex, fragment: &fragment, colors: &colors, depth: false, samples: 1 })?;
    Ok(Lighting { pipeline, output: target(gpu, "the lit output of the lighting pass", values::PIXEL_FORMAT_RGBA16_FLOAT)? })
}

fn draw_fill(encoder: Id, frame: &Frame, fill: &Fill) -> Result<(), String> {
    send!((); encoder, "setRenderPipelineState:", Id: fill.pipeline.id())?;
    send!((); encoder, "setDepthStencilState:", Id: frame.depth_state.id())?;
    send!((); encoder, "setVertexBuffer:offset:atIndex:", Id: frame.vertices.id(), usize: 0, usize: 0)?;
    send!((); encoder, "setFragmentBuffer:offset:atIndex:", Id: frame.constants.id(), usize: 0, usize: 0)?;
    send!((); encoder, "drawPrimitives:vertexStart:vertexCount:", usize: values::PRIMITIVE_TYPE_TRIANGLE, usize: 0, usize: frame.vertex_count)
}

fn encode_fill(command_buffer: Id, frame: &Frame, fill: &Fill) -> Result<(), String> {
    let colors: Vec<ColorAttachment> = fill.targets.iter().map(|target| ColorAttachment { texture: target.id(), resolve: None, load: values::LOAD_ACTION_CLEAR, clear: fill.clear }).collect();
    let pass = gpu::render_pass(&colors, Some(&DepthAttachment { texture: frame.depth.id(), clear: 1.0 }))?;
    let encoder = present(send!(Id; command_buffer, "renderCommandEncoderWithDescriptor:", Id: pass)?, "renderCommandEncoderWithDescriptor: for the lit scene's fill")?;
    gpu::end_encoding(encoder, draw_fill(encoder, frame, fill))
}

fn draw_lighting(encoder: Id, frame: &Frame, geometry: &Fill, lighting: &Lighting) -> Result<(), String> {
    send!((); encoder, "setRenderPipelineState:", Id: lighting.pipeline.id())?;
    send!((); encoder, "setFragmentBuffer:offset:atIndex:", Id: frame.constants.id(), usize: 0, usize: 0)?;
    for (index, attachment) in geometry.targets.iter().take(GEOMETRY_ATTACHMENTS_READ).enumerate() {
        send!((); encoder, "setFragmentTexture:atIndex:", Id: attachment.id(), usize: index)?;
    }
    send!((); encoder, "drawPrimitives:vertexStart:vertexCount:", usize: values::PRIMITIVE_TYPE_TRIANGLE, usize: 0, usize: 3)
}

fn encode_lighting(command_buffer: Id, frame: &Frame, geometry: &Fill, lighting: &Lighting) -> Result<(), String> {
    let pass = gpu::render_pass(&[ColorAttachment { texture: lighting.output.id(), resolve: None, load: values::LOAD_ACTION_CLEAR, clear: OPAQUE_BLACK }], None)?;
    let encoder = present(send!(Id; command_buffer, "renderCommandEncoderWithDescriptor:", Id: pass)?, "renderCommandEncoderWithDescriptor: for the lit scene's lighting pass")?;
    gpu::end_encoding(encoder, draw_lighting(encoder, frame, geometry, lighting))
}

pub fn render(gpu: &Gpu, sources: &Sources, scene: &Scene, variant: LitVariant, repetitions: usize) -> Result<LitRender, String> {
    if repetitions == 0 {
        return Err("the lit scene was asked for no repetition: there is no output to read".to_string());
    }
    let frame = create_frame(gpu, scene)?;
    let (fill, lighting) = match variant {
        LitVariant::Forward => (forward_fill(gpu, sources)?, None),
        LitVariant::GeometryBuffer160 => (geometry_fill(gpu, sources, 4)?, None),
        LitVariant::GeometryBuffer192 => (geometry_fill(gpu, sources, 5)?, None),
        LitVariant::GeometryBuffer160Lit => (geometry_fill(gpu, sources, 4)?, Some(create_lighting(gpu, sources)?)),
    };

    let mut microseconds = Vec::with_capacity(repetitions);
    for _ in 0..repetitions {
        // The command buffer and its pass descriptors go with each repetition.
        let _pool = Pool::push();
        let started = Instant::now();
        let command_buffer = gpu.command_buffer("the lit scene")?;
        encode_fill(command_buffer, &frame, &fill)?;
        if let Some(lighting) = &lighting {
            encode_lighting(command_buffer, &frame, &fill, lighting)?;
        }
        gpu::complete(command_buffer, "the lit scene")?;
        microseconds.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }

    // The lit image is 16-bit float RGBA; the geometry buffer's first attachment is 8-bit RGBA.
    let first_target = fill.targets.first().ok_or("the lit scene's fill has no attachment")?;
    let (texture, bytes_per_pixel) = match (&lighting, variant.output_is_lit()) {
        (Some(lighting), _) => (lighting.output.id(), 8),
        (None, true) => (first_target.id(), 8),
        (None, false) => (first_target.id(), 4),
    };
    let output = gpu.read_texture("the lit scene's output", texture, SIZE, SIZE, bytes_per_pixel)?;
    Ok(LitRender { microseconds, output })
}
