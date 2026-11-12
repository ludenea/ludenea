//! The pass-through exercise on Vulkan: a source and a target created with
//! the mutable-format flag, each with a plain and an sRGB view; the left half
//! of the target drawn from the plain view of the source through the plain
//! view of the target, the right half through the two sRGB views. Throwaway,
//! never product code.

use super::Shaders;
use super::ffi::*;
use super::gpu::*;
use crate::scene::{Image, PASS_THROUGH_SIDE, Scene};

/// Nearest in both directions and between levels, clamped to the edge.
pub fn nearest_sampler() -> SamplerCreateInfo {
    SamplerCreateInfo {
        s_type: structure::SAMPLER_CREATE_INFO,
        p_next: std::ptr::null(),
        flags: 0,
        mag_filter: FILTER_NEAREST,
        min_filter: FILTER_NEAREST,
        mipmap_mode: MIPMAP_MODE_NEAREST,
        address_mode_u: ADDRESS_MODE_CLAMP_TO_EDGE,
        address_mode_v: ADDRESS_MODE_CLAMP_TO_EDGE,
        address_mode_w: ADDRESS_MODE_CLAMP_TO_EDGE,
        mip_lod_bias: 0.0,
        anisotropy_enable: 0,
        max_anisotropy: 1.0,
        compare_enable: 0,
        compare_op: 0,
        min_lod: 0.0,
        max_lod: LOD_CLAMP_NONE,
        border_color: BORDER_COLOR_FLOAT_TRANSPARENT_BLACK,
        unnormalized_coordinates: 0,
    }
}

/// The color writes of the pass before are complete before this pass loads.
fn after_the_previous_pass() -> SubpassDependency {
    SubpassDependency {
        src_subpass: SUBPASS_EXTERNAL,
        dst_subpass: 0,
        src_stage_mask: PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT,
        dst_stage_mask: PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT,
        src_access_mask: ACCESS_COLOR_ATTACHMENT_WRITE,
        dst_access_mask: ACCESS_COLOR_ATTACHMENT_READ | ACCESS_COLOR_ATTACHMENT_WRITE,
        dependency_flags: 0,
    }
}

/// One half: its render pass, framebuffer, pipeline and descriptor set.
struct Half {
    render_pass: Object,
    framebuffer: Object,
    pipeline: Object,
    set: Object,
}

pub fn run(gpu: &Gpu, shaders: &Shaders, scene: &Scene) -> Result<Image, String> {
    let side = PASS_THROUGH_SIDE;
    let mut scope = Scope::new(gpu);
    let source = scope.image(&image_info(FORMAT_R8G8B8A8_UNORM, side, side, 1, SAMPLE_COUNT_1, IMAGE_USAGE_SAMPLED | IMAGE_USAGE_TRANSFER_DST, IMAGE_CREATE_MUTABLE_FORMAT))?;
    scope.upload_images(&[ImageUpload { image: source, width: side, height: side, levels: vec![&scene.pass_through_source] }])?;
    let target_usage = IMAGE_USAGE_COLOR_ATTACHMENT | IMAGE_USAGE_TRANSFER_SRC | IMAGE_USAGE_TRANSFER_DST;
    let target = scope.image(&image_info(FORMAT_R8G8B8A8_UNORM, side * 2, side, 1, SAMPLE_COUNT_1, target_usage, IMAGE_CREATE_MUTABLE_FORMAT))?;
    let sampler = scope.sampler(&nearest_sampler())?;
    let vertex = scope.shader_module(&shaders.pass_through_vertex)?;
    let fragment = scope.shader_module(&shaders.pass_through_fragment)?;
    let bindings = [(DESCRIPTOR_COMBINED_IMAGE_SAMPLER, 1, STAGE_FRAGMENT)];
    let set_layout = scope.descriptor_set_layout(&bindings)?;
    let layout = scope.pipeline_layout(&[set_layout], &[])?;

    let mut halves = Vec::new();
    for (position, format) in [FORMAT_R8G8B8A8_UNORM, FORMAT_R8G8B8A8_SRGB].into_iter().enumerate() {
        let last = position == 1;
        let source_view = scope.image_view(source, format, ASPECT_COLOR, 1)?;
        let target_view = scope.image_view(target, format, ASPECT_COLOR, 1)?;
        let set = scope.descriptor_set(set_layout, &bindings)?;
        scope.write_images(set, 0, DESCRIPTOR_COMBINED_IMAGE_SAMPLER, &[DescriptorImageInfo { sampler, image_view: source_view, image_layout: LAYOUT_SHADER_READ_ONLY_OPTIMAL }]);
        let final_layout = if last { LAYOUT_TRANSFER_SRC_OPTIMAL } else { LAYOUT_COLOR_ATTACHMENT_OPTIMAL };
        let attachments = [attachment(format, SAMPLE_COUNT_1, LOAD_OP_LOAD, STORE_OP_STORE, (LAYOUT_COLOR_ATTACHMENT_OPTIMAL, final_layout))];
        let mut dependencies = vec![after_the_previous_pass()];
        if last {
            dependencies.push(after_color_writes(PIPELINE_STAGE_TRANSFER, ACCESS_TRANSFER_READ));
        }
        let render_pass = scope.render_pass(&attachments, &[0], &[], None, &dependencies)?;
        let framebuffer = scope.framebuffer(render_pass, &[target_view], side * 2, side)?;
        let viewport = Viewport { x: (position as u32 * side) as f32, y: 0.0, width: side as f32, height: side as f32, min_depth: 0.0, max_depth: 1.0 };
        let pipeline = scope.graphics_pipeline(&GraphicsPipelineDescription {
            vertex,
            fragment,
            vertex_stride: 0,
            attributes: &[],
            viewport,
            samples: SAMPLE_COUNT_1,
            depth_write: None,
            blends: &[opaque_blend()],
            layout,
            render_pass,
        })?;
        halves.push(Half { render_pass, framebuffer, pipeline, set });
    }

    let readback = scope.host_buffer((side * side * 8) as usize, BUFFER_USAGE_TRANSFER_DST)?;
    let readback_buffer = readback.buffer;
    scope.run_once(|functions, command_buffer| {
        let zero: ClearValue = [0.0; 4];
        let whole = range(ASPECT_COLOR, 1);
        image_barrier(functions, command_buffer, target, ASPECT_COLOR, 1, (LAYOUT_UNDEFINED, LAYOUT_TRANSFER_DST_OPTIMAL), (ACCESS_NONE, ACCESS_TRANSFER_WRITE), (PIPELINE_STAGE_TOP_OF_PIPE, PIPELINE_STAGE_TRANSFER));
        // SAFETY: the command buffer is recording and the target is in the
        // transfer destination layout.
        unsafe { (functions.cmd_clear_color_image)(command_buffer, target, LAYOUT_TRANSFER_DST_OPTIMAL, &zero, 1, &whole) };
        image_barrier(
            functions,
            command_buffer,
            target,
            ASPECT_COLOR,
            1,
            (LAYOUT_TRANSFER_DST_OPTIMAL, LAYOUT_COLOR_ATTACHMENT_OPTIMAL),
            (ACCESS_TRANSFER_WRITE, ACCESS_COLOR_ATTACHMENT_READ | ACCESS_COLOR_ATTACHMENT_WRITE),
            (PIPELINE_STAGE_TRANSFER, PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT),
        );
        for half in &halves {
            begin_render_pass(functions, command_buffer, half.render_pass, half.framebuffer, [side * 2, side], &[], SUBPASS_CONTENTS_INLINE);
            // SAFETY: the command buffer is recording inside the half's render
            // pass; the pipeline, its layout and the set belong to the device.
            unsafe {
                (functions.cmd_bind_pipeline)(command_buffer, BIND_POINT_GRAPHICS, half.pipeline);
                (functions.cmd_bind_descriptor_sets)(command_buffer, BIND_POINT_GRAPHICS, layout, 0, 1, &half.set, 0, std::ptr::null());
                (functions.cmd_draw)(command_buffer, 3, 1, 0, 0);
                (functions.cmd_end_render_pass)(command_buffer);
            }
        }
        copy_to_host(functions, command_buffer, target, side * 2, side, readback_buffer);
    })?;
    let bytes = scope.read(&readback)?;
    Ok(Image::from_rows(side * 2, side, (side * 8) as usize, &bytes))
}
