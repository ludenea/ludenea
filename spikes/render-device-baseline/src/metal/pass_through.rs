//! The pass-through exercise on Metal: a source and a target of 8-bit RGBA
//! created with the pixel format view usage, each with a second view in the
//! sRGB encoding. The left half of the target is drawn through the plain
//! views, the right half through the sRGB views, and the target's bytes are
//! read back as stored. Throwaway, never product code.

use crate::scene::{Image, PASS_THROUGH_SIDE, Scene};

use super::gpu::{self, ColorAttachment, ColorTarget, Gpu, PipelineShape, TextureShape};
use super::objc::{Id, Owned, present};
use super::values;

const FILE: &str = "pass_through.metal";
const SIDE: usize = PASS_THROUGH_SIDE as usize;
const BYTES_PER_PIXEL: usize = 4;
const TRANSPARENT_BLACK: values::ClearColor = values::ClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 };

/// One half of the target: the view written, what happens to what the target
/// holds, and the pipeline, the source view and the place of the half.
struct Half<'a> {
    target_view: &'a Owned,
    load: usize,
    pipeline: &'a Owned,
    source_view: &'a Owned,
    left: usize,
}

/// The source drawn texel for pixel into the half: one triangle that covers
/// the viewport, kept to the half by the scissor.
fn draw_source(encoder: Id, half: &Half, sampler: &Owned) -> Result<(), String> {
    send!((); encoder, "setRenderPipelineState:", Id: half.pipeline.id())?;
    send!((); encoder, "setViewport:", values::Viewport: values::Viewport::of(half.left, 0, SIDE, SIDE))?;
    send!((); encoder, "setScissorRect:", values::ScissorRect: values::ScissorRect { x: half.left, y: 0, width: SIDE, height: SIDE })?;
    send!((); encoder, "setFragmentTexture:atIndex:", Id: half.source_view.id(), usize: 0)?;
    send!((); encoder, "setFragmentSamplerState:atIndex:", Id: sampler.id(), usize: 0)?;
    send!((); encoder, "drawPrimitives:vertexStart:vertexCount:", usize: values::PRIMITIVE_TYPE_TRIANGLE, usize: 0, usize: 3)
}

/// One pass over the view of the target that the half is written through.
fn encode_half(command_buffer: Id, half: &Half, sampler: &Owned) -> Result<(), String> {
    let pass = gpu::render_pass(&[ColorAttachment { texture: half.target_view.id(), resolve: None, load: half.load, clear: TRANSPARENT_BLACK }], None)?;
    let encoder = present(send!(Id; command_buffer, "renderCommandEncoderWithDescriptor:", Id: pass)?, "renderCommandEncoderWithDescriptor: for the pass-through")?;
    gpu::end_encoding(encoder, draw_source(encoder, half, sampler))
}

pub fn run(gpu: &Gpu, source: &str, scene: &Scene) -> Result<Image, String> {
    let library = gpu.library(FILE, source)?;
    let vertex = gpu::function(&library, FILE, "pass_through_vertex")?;
    let fragment = gpu::function(&library, FILE, "pass_through_fragment")?;
    let pipeline = |what: &str, format: usize| gpu.render_pipeline(&PipelineShape { what, vertex: &vertex, fragment: &fragment, colors: &[ColorTarget { format, blended: false }], depth: false, samples: 1 });
    let plain_pipeline = pipeline("the pass-through pipeline of the plain view", values::PIXEL_FORMAT_RGBA8_UNORM)?;
    let srgb_pipeline = pipeline("the pass-through pipeline of the sRGB view", values::PIXEL_FORMAT_RGBA8_UNORM_SRGB)?;
    let nearest = gpu.sampler(values::SAMPLER_MIN_MAG_FILTER_NEAREST, values::SAMPLER_MIP_FILTER_NEAREST, values::SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE)?;

    let source_shape = TextureShape { format: values::PIXEL_FORMAT_RGBA8_UNORM, width: SIDE, height: SIDE, levels: 1, samples: 1, usage: values::TEXTURE_USAGE_SHADER_READ | values::TEXTURE_USAGE_PIXEL_FORMAT_VIEW, private: false };
    let source_plain = gpu.texture("the pass-through source", &source_shape)?;
    gpu::upload(&source_plain, "the pass-through source", 0, SIDE, SIDE, &scene.pass_through_source)?;
    let source_srgb = gpu::view(&source_plain, values::PIXEL_FORMAT_RGBA8_UNORM_SRGB, "the sRGB view of the pass-through source")?;
    let target_shape = TextureShape::target(values::PIXEL_FORMAT_RGBA8_UNORM, SIDE * 2, SIDE, values::TEXTURE_USAGE_RENDER_TARGET | values::TEXTURE_USAGE_PIXEL_FORMAT_VIEW);
    let target_plain = gpu.texture("the pass-through target", &target_shape)?;
    let target_srgb = gpu::view(&target_plain, values::PIXEL_FORMAT_RGBA8_UNORM_SRGB, "the sRGB view of the pass-through target")?;

    // The first pass clears the whole target to zero through the plain view;
    // the second keeps what the first wrote.
    let pass_through = Half { target_view: &target_plain, load: values::LOAD_ACTION_CLEAR, pipeline: &plain_pipeline, source_view: &source_plain, left: 0 };
    let converting = Half { target_view: &target_srgb, load: values::LOAD_ACTION_LOAD, pipeline: &srgb_pipeline, source_view: &source_srgb, left: SIDE };
    let command_buffer = gpu.command_buffer("the pass-through")?;
    encode_half(command_buffer, &pass_through, &nearest)?;
    encode_half(command_buffer, &converting, &nearest)?;
    gpu::complete(command_buffer, "the pass-through")?;

    let bytes = gpu.read_texture("the pass-through target", target_plain.id(), SIDE * 2, SIDE, BYTES_PER_PIXEL)?;
    Ok(Image::from_rows(PASS_THROUGH_SIDE * 2, PASS_THROUGH_SIDE, SIDE * 2 * BYTES_PER_PIXEL, &bytes))
}
