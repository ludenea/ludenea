//! The compute exercise on Metal: one kernel dispatched as 8 by 8
//! threadgroups of 8 by 8 threads, reading a buffer, writing a second buffer
//! and a texture of one 32-bit unsigned channel opened for writing; both are
//! read back. Throwaway, never product code.

use crate::backend::ComputeOutput;
use crate::scene::{COMPUTE_ELEMENTS, COMPUTE_SIDE, Scene};

use super::gpu::{self, Gpu, TextureShape};
use super::objc::{Id, present};
use super::values;

const FILE: &str = "compute.metal";
const SIDE: usize = COMPUTE_SIDE as usize;
/// The threads of one threadgroup along each side.
const GROUP_SIDE: usize = 8;
const BYTES_PER_ELEMENT: usize = size_of::<u32>();

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes.as_chunks::<BYTES_PER_ELEMENT>().0.iter().map(|word| u32::from_ne_bytes(*word)).collect()
}

fn dispatch(encoder: Id, pipeline: Id, input: Id, output: Id, texture: Id) -> Result<(), String> {
    send!((); encoder, "setComputePipelineState:", Id: pipeline)?;
    send!((); encoder, "setBuffer:offset:atIndex:", Id: input, usize: 0, usize: 0)?;
    send!((); encoder, "setBuffer:offset:atIndex:", Id: output, usize: 0, usize: 1)?;
    send!((); encoder, "setTexture:atIndex:", Id: texture, usize: 0)?;
    let groups = values::Size { width: SIDE / GROUP_SIDE, height: SIDE / GROUP_SIDE, depth: 1 };
    let threads_per_group = values::Size { width: GROUP_SIDE, height: GROUP_SIDE, depth: 1 };
    send!((); encoder, "dispatchThreadgroups:threadsPerThreadgroup:", values::Size: groups, values::Size: threads_per_group)
}

pub fn run(gpu: &Gpu, source: &str, scene: &Scene) -> Result<ComputeOutput, String> {
    let library = gpu.library(FILE, source)?;
    let function = gpu::function(&library, FILE, "compute_kernel")?;
    let pipeline = gpu.compute_pipeline(&function, FILE)?;
    let largest_group = send!(usize; pipeline.id(), "maxTotalThreadsPerThreadgroup")?;
    if largest_group < GROUP_SIDE * GROUP_SIDE {
        return Err(format!("maxTotalThreadsPerThreadgroup = {largest_group}: a threadgroup of {GROUP_SIDE} by {GROUP_SIDE} threads does not fit"));
    }
    let input = gpu.buffer("the compute input", scene.compute_input_bytes())?;
    let output = gpu.empty_buffer("the compute output", COMPUTE_ELEMENTS * BYTES_PER_ELEMENT)?;
    let texture = gpu.texture("the compute storage texture", &TextureShape::target(values::PIXEL_FORMAT_R32_UINT, SIDE, SIDE, values::TEXTURE_USAGE_SHADER_WRITE))?;

    let command_buffer = gpu.command_buffer("the compute pass")?;
    let encoder = present(send!(Id; command_buffer, "computeCommandEncoder")?, "computeCommandEncoder")?;
    gpu::end_encoding(encoder, dispatch(encoder, pipeline.id(), input.id(), output.id(), texture.id()))?;
    gpu::complete(command_buffer, "the compute pass")?;

    let buffer = words(&gpu::buffer_bytes(&output, COMPUTE_ELEMENTS * BYTES_PER_ELEMENT, "the compute output")?);
    let texture = words(&gpu.read_texture("the compute storage texture", texture.id(), SIDE, SIDE, BYTES_PER_ELEMENT)?);
    Ok(ComputeOutput { buffer, texture })
}
