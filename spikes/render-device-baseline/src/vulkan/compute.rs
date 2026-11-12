//! The compute exercise on Vulkan: one dispatch of 8 by 8 groups reading a
//! storage buffer and writing a second storage buffer and a 32-bit unsigned
//! storage image, both read back. Throwaway, never product code.

use super::Shaders;
use super::ffi::*;
use super::gpu::*;
use crate::backend::ComputeOutput;
use crate::scene::{COMPUTE_SIDE, Scene};

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect()
}

pub fn run(gpu: &Gpu, shaders: &Shaders, scene: &Scene) -> Result<ComputeOutput, String> {
    let mut scope = Scope::new(gpu);
    let side = COMPUTE_SIDE;
    let input = scope.buffer_with(scene.compute_input_bytes(), BUFFER_USAGE_STORAGE)?;
    let output = scope.host_buffer(scene.compute_input_bytes().len(), BUFFER_USAGE_STORAGE)?;
    let texture_readback = scope.host_buffer((side * side * 4) as usize, BUFFER_USAGE_TRANSFER_DST)?;
    let texture = scope.image(&image_info(FORMAT_R32_UINT, side, side, 1, SAMPLE_COUNT_1, IMAGE_USAGE_STORAGE | IMAGE_USAGE_TRANSFER_SRC, 0))?;
    let texture_view = scope.image_view(texture, FORMAT_R32_UINT, ASPECT_COLOR, 1)?;

    let bindings = [(DESCRIPTOR_STORAGE_BUFFER, 1, STAGE_COMPUTE), (DESCRIPTOR_STORAGE_BUFFER, 1, STAGE_COMPUTE), (DESCRIPTOR_STORAGE_IMAGE, 1, STAGE_COMPUTE)];
    let set_layout = scope.descriptor_set_layout(&bindings)?;
    let set = scope.descriptor_set(set_layout, &bindings)?;
    scope.write_buffer(set, 0, DESCRIPTOR_STORAGE_BUFFER, &input);
    scope.write_buffer(set, 1, DESCRIPTOR_STORAGE_BUFFER, &output);
    scope.write_images(set, 2, DESCRIPTOR_STORAGE_IMAGE, &[DescriptorImageInfo { sampler: 0, image_view: texture_view, image_layout: LAYOUT_GENERAL }]);
    let layout = scope.pipeline_layout(&[set_layout], &[])?;
    let module = scope.shader_module(&shaders.compute)?;
    let pipeline = scope.compute_pipeline(module, layout)?;

    let (output_buffer, readback_buffer) = (output.buffer, texture_readback.buffer);
    scope.run_once(|functions, command_buffer| {
        image_barrier(functions, command_buffer, texture, ASPECT_COLOR, 1, (LAYOUT_UNDEFINED, LAYOUT_GENERAL), (ACCESS_NONE, ACCESS_SHADER_WRITE), (PIPELINE_STAGE_TOP_OF_PIPE, PIPELINE_STAGE_COMPUTE_SHADER));
        // SAFETY: the command buffer is recording; the pipeline, its layout
        // and the set belong to the device.
        unsafe {
            (functions.cmd_bind_pipeline)(command_buffer, BIND_POINT_COMPUTE, pipeline);
            (functions.cmd_bind_descriptor_sets)(command_buffer, BIND_POINT_COMPUTE, layout, 0, 1, &set, 0, std::ptr::null());
            (functions.cmd_dispatch)(command_buffer, side / 8, side / 8, 1);
        }
        host_read_barrier(functions, command_buffer, output_buffer, PIPELINE_STAGE_COMPUTE_SHADER, ACCESS_SHADER_WRITE);
        image_barrier(
            functions,
            command_buffer,
            texture,
            ASPECT_COLOR,
            1,
            (LAYOUT_GENERAL, LAYOUT_TRANSFER_SRC_OPTIMAL),
            (ACCESS_SHADER_WRITE, ACCESS_TRANSFER_READ),
            (PIPELINE_STAGE_COMPUTE_SHADER, PIPELINE_STAGE_TRANSFER),
        );
        copy_to_host(functions, command_buffer, texture, side, side, readback_buffer);
    })?;
    Ok(ComputeOutput { buffer: words(&scope.read(&output)?), texture: words(&scope.read(&texture_readback)?) })
}
