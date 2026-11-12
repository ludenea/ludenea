//! The lit scene on Vulkan: the height field drawn with depth into one 16-bit
//! float image lit in the fragment shader, or into a geometry buffer of four
//! or five attachments, optionally followed by a second render pass that
//! lights each pixel from the geometry buffer's texels. Each repetition is
//! recorded, submitted and waited for; the output is read back after the
//! last. Throwaway, never product code.

use std::time::Instant;

use super::Shaders;
use super::ffi::*;
use super::gpu::*;
use super::pass_through::nearest_sampler;
use crate::backend::{LitRender, LitVariant};
use crate::scene::{LIT_SIZE, Scene};

/// Clip position, world position, normal, albedo: 52 bytes.
const LIT_VERTEX_STRIDE: u32 = 52;

/// The formats of the fill pass's color attachments.
fn fill_formats(variant: LitVariant) -> Vec<u32> {
    let geometry = [FORMAT_R8G8B8A8_UNORM, FORMAT_R8G8B8A8_UNORM, FORMAT_R8G8B8A8_UNORM, FORMAT_R16G16B16A16_SFLOAT];
    match variant {
        LitVariant::Forward => vec![FORMAT_R16G16B16A16_SFLOAT],
        LitVariant::GeometryBuffer160 | LitVariant::GeometryBuffer160Lit => geometry.to_vec(),
        LitVariant::GeometryBuffer192 => geometry.iter().copied().chain([FORMAT_R8G8B8A8_UNORM]).collect(),
    }
}

/// A pass ready to record: what to begin and what to bind.
struct Pass {
    render_pass: Object,
    framebuffer: Object,
    pipeline: Object,
    layout: Object,
    set: Object,
    clears: Vec<ClearValue>,
    vertex_buffer: Option<Object>,
    vertex_count: u32,
}

/// The fill pass, the color images it writes, and the output image when the
/// fill is the last pass.
fn fill_pass(scope: &mut Scope, shaders: &Shaders, scene: &Scene, variant: LitVariant, constants: &HostBuffer) -> Result<(Pass, Vec<(Object, Object)>), String> {
    let size = LIT_SIZE;
    let formats = fill_formats(variant);
    let read_by_a_second_pass = variant == LitVariant::GeometryBuffer160Lit;
    let usage = IMAGE_USAGE_COLOR_ATTACHMENT | IMAGE_USAGE_TRANSFER_SRC | if read_by_a_second_pass { IMAGE_USAGE_SAMPLED } else { 0 };
    let mut colors = Vec::new();
    let mut attachments = Vec::new();
    for (position, &format) in formats.iter().enumerate() {
        let image = scope.image(&image_info(format, size, size, 1, SAMPLE_COUNT_1, usage, 0))?;
        colors.push((image, scope.image_view(image, format, ASPECT_COLOR, 1)?));
        let final_layout = match (read_by_a_second_pass, position) {
            (true, _) => LAYOUT_SHADER_READ_ONLY_OPTIMAL,
            (false, 0) => LAYOUT_TRANSFER_SRC_OPTIMAL,
            (false, _) => LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
        };
        attachments.push(attachment(format, SAMPLE_COUNT_1, LOAD_OP_CLEAR, STORE_OP_STORE, (LAYOUT_UNDEFINED, final_layout)));
    }
    let depth = scope.image(&image_info(FORMAT_D32_SFLOAT, size, size, 1, SAMPLE_COUNT_1, IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT, 0))?;
    let depth_view = scope.image_view(depth, FORMAT_D32_SFLOAT, ASPECT_DEPTH, 1)?;
    attachments.push(attachment(FORMAT_D32_SFLOAT, SAMPLE_COUNT_1, LOAD_OP_CLEAR, STORE_OP_DONT_CARE, (LAYOUT_UNDEFINED, LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL)));
    let color_indexes: Vec<u32> = (0..formats.len() as u32).collect();
    let dependency = after_color_writes(PIPELINE_STAGE_TRANSFER | PIPELINE_STAGE_FRAGMENT_SHADER, ACCESS_TRANSFER_READ | ACCESS_SHADER_READ);
    let render_pass = scope.render_pass(&attachments, &color_indexes, &[], Some(formats.len() as u32), &[dependency])?;
    let mut views: Vec<Object> = colors.iter().map(|&(_, view)| view).collect();
    views.push(depth_view);
    let framebuffer = scope.framebuffer(render_pass, &views, size, size)?;

    let bindings = [(DESCRIPTOR_UNIFORM_BUFFER, 1, STAGE_FRAGMENT)];
    let set_layout = scope.descriptor_set_layout(&bindings)?;
    let set = scope.descriptor_set(set_layout, &bindings)?;
    scope.write_buffer(set, 0, DESCRIPTOR_UNIFORM_BUFFER, constants);
    let layout = scope.pipeline_layout(&[set_layout], &[])?;
    let fragment_words = match variant {
        LitVariant::Forward => &shaders.lit_forward_fragment,
        LitVariant::GeometryBuffer192 => &shaders.lit_geometry_five_fragment,
        LitVariant::GeometryBuffer160 | LitVariant::GeometryBuffer160Lit => &shaders.lit_geometry_four_fragment,
    };
    let vertex = scope.shader_module(&shaders.lit_vertex)?;
    let fragment = scope.shader_module(fragment_words)?;
    let attributes = [
        VertexInputAttributeDescription { location: 0, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 0 },
        VertexInputAttributeDescription { location: 1, binding: 0, format: FORMAT_R32G32B32_SFLOAT, offset: 16 },
        VertexInputAttributeDescription { location: 2, binding: 0, format: FORMAT_R32G32B32_SFLOAT, offset: 28 },
        VertexInputAttributeDescription { location: 3, binding: 0, format: FORMAT_R32G32B32_SFLOAT, offset: 40 },
    ];
    let blends = vec![opaque_blend(); formats.len()];
    let pipeline = scope.graphics_pipeline(&GraphicsPipelineDescription {
        vertex,
        fragment,
        vertex_stride: LIT_VERTEX_STRIDE,
        attributes: &attributes,
        viewport: full_viewport(size, size),
        samples: SAMPLE_COUNT_1,
        depth_write: Some(true),
        blends: &blends,
        layout,
        render_pass,
    })?;
    let vertex_buffer = scope.buffer_with(scene.lit_vertex_bytes(), BUFFER_USAGE_VERTEX)?.buffer;
    let background = if variant == LitVariant::Forward { [0.0, 0.0, 0.0, 1.0] } else { [0.0; 4] };
    let mut clears = vec![background; formats.len()];
    clears.push([1.0, 0.0, 0.0, 0.0]);
    let pass = Pass { render_pass, framebuffer, pipeline, layout, set, clears, vertex_buffer: Some(vertex_buffer), vertex_count: scene.lit_vertices.len() as u32 };
    Ok((pass, colors))
}

/// The lighting pass that reads the geometry buffer, and its output image.
fn lighting_pass(scope: &mut Scope, shaders: &Shaders, geometry: &[(Object, Object)], constants: &HostBuffer) -> Result<(Pass, Object), String> {
    let size = LIT_SIZE;
    let output = scope.image(&image_info(FORMAT_R16G16B16A16_SFLOAT, size, size, 1, SAMPLE_COUNT_1, IMAGE_USAGE_COLOR_ATTACHMENT | IMAGE_USAGE_TRANSFER_SRC, 0))?;
    let output_view = scope.image_view(output, FORMAT_R16G16B16A16_SFLOAT, ASPECT_COLOR, 1)?;
    let attachments = [attachment(FORMAT_R16G16B16A16_SFLOAT, SAMPLE_COUNT_1, LOAD_OP_DONT_CARE, STORE_OP_STORE, (LAYOUT_UNDEFINED, LAYOUT_TRANSFER_SRC_OPTIMAL))];
    let render_pass = scope.render_pass(&attachments, &[0], &[], None, &[after_color_writes(PIPELINE_STAGE_TRANSFER, ACCESS_TRANSFER_READ)])?;
    let framebuffer = scope.framebuffer(render_pass, &[output_view], size, size)?;
    let mut bindings = vec![(DESCRIPTOR_COMBINED_IMAGE_SAMPLER, 1, STAGE_FRAGMENT); geometry.len()];
    bindings.push((DESCRIPTOR_UNIFORM_BUFFER, 1, STAGE_FRAGMENT));
    let set_layout = scope.descriptor_set_layout(&bindings)?;
    let set = scope.descriptor_set(set_layout, &bindings)?;
    let sampler = scope.sampler(&nearest_sampler())?;
    for (binding, &(_, view)) in geometry.iter().enumerate() {
        scope.write_images(set, binding as u32, DESCRIPTOR_COMBINED_IMAGE_SAMPLER, &[DescriptorImageInfo { sampler, image_view: view, image_layout: LAYOUT_SHADER_READ_ONLY_OPTIMAL }]);
    }
    scope.write_buffer(set, geometry.len() as u32, DESCRIPTOR_UNIFORM_BUFFER, constants);
    let layout = scope.pipeline_layout(&[set_layout], &[])?;
    let vertex = scope.shader_module(&shaders.lit_resolve_vertex)?;
    let fragment = scope.shader_module(&shaders.lit_resolve_fragment)?;
    let pipeline = scope.graphics_pipeline(&GraphicsPipelineDescription {
        vertex,
        fragment,
        vertex_stride: 0,
        attributes: &[],
        viewport: full_viewport(size, size),
        samples: SAMPLE_COUNT_1,
        depth_write: None,
        blends: &[opaque_blend()],
        layout,
        render_pass,
    })?;
    Ok((Pass { render_pass, framebuffer, pipeline, layout, set, clears: Vec::new(), vertex_buffer: None, vertex_count: 3 }, output))
}

fn record_pass(functions: &DeviceFunctions, command_buffer: Handle, pass: &Pass) {
    begin_render_pass(functions, command_buffer, pass.render_pass, pass.framebuffer, [LIT_SIZE, LIT_SIZE], &pass.clears, SUBPASS_CONTENTS_INLINE);
    let offset = 0_u64;
    // SAFETY: the command buffer is recording inside the pass's render pass;
    // every object belongs to the device and the pointers live for each call.
    unsafe {
        (functions.cmd_bind_pipeline)(command_buffer, BIND_POINT_GRAPHICS, pass.pipeline);
        (functions.cmd_bind_descriptor_sets)(command_buffer, BIND_POINT_GRAPHICS, pass.layout, 0, 1, &pass.set, 0, std::ptr::null());
        if let Some(buffer) = &pass.vertex_buffer {
            (functions.cmd_bind_vertex_buffers)(command_buffer, 0, 1, buffer, &offset);
        }
        (functions.cmd_draw)(command_buffer, pass.vertex_count, 1, 0, 0);
        (functions.cmd_end_render_pass)(command_buffer);
    }
}

/// Records, submits and waits for one repetition; its time in microseconds.
fn repetition(gpu: &Gpu, pool: Object, command_buffer: Handle, fence: Object, passes: &[Pass]) -> Result<f64, String> {
    let functions = &gpu.functions;
    // SAFETY: the pool's command buffer is not pending: the previous
    // repetition was waited for.
    check(unsafe { (functions.reset_command_pool)(gpu.device, pool, 0) }, "vkResetCommandPool")?;
    let started = Instant::now();
    begin(functions, command_buffer, COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT, None)?;
    for pass in passes {
        record_pass(functions, command_buffer, pass);
    }
    end(functions, command_buffer)?;
    gpu.submit(command_buffer, fence)?;
    gpu.wait_fence(fence)?;
    let microseconds = started.elapsed().as_secs_f64() * 1_000_000.0;
    // SAFETY: the fence is signaled and no submission uses it.
    check(unsafe { (functions.reset_fences)(gpu.device, 1, &fence) }, "vkResetFences")?;
    Ok(microseconds)
}

pub fn render(gpu: &Gpu, shaders: &Shaders, scene: &Scene, variant: LitVariant, repetitions: usize) -> Result<LitRender, String> {
    if repetitions == 0 {
        return Err("the lit scene needs at least one repetition to have an output".to_string());
    }
    let size = LIT_SIZE;
    let mut scope = Scope::new(gpu);
    let constants = scope.buffer_with(scene.lit_constant_bytes(), BUFFER_USAGE_UNIFORM)?;
    let (fill, colors) = fill_pass(&mut scope, shaders, scene, variant, &constants)?;
    let mut passes = vec![fill];
    let output = if variant == LitVariant::GeometryBuffer160Lit {
        let (lighting, output) = lighting_pass(&mut scope, shaders, &colors, &constants)?;
        passes.push(lighting);
        output
    } else {
        colors[0].0
    };
    let pool = scope.command_pool()?;
    let command_buffer = scope.command_buffer(pool, COMMAND_BUFFER_LEVEL_PRIMARY)?.0;
    let fence = scope.fence()?;
    let microseconds = (0..repetitions).map(|_| repetition(gpu, pool, command_buffer, fence, &passes)).collect::<Result<Vec<f64>, String>>()?;

    let bytes_per_pixel = if variant.output_is_lit() { 8 } else { 4 };
    let readback = scope.host_buffer((size * size * bytes_per_pixel) as usize, BUFFER_USAGE_TRANSFER_DST)?;
    let readback_buffer = readback.buffer;
    scope.run_once(|functions, command_buffer| copy_to_host(functions, command_buffer, output, size, size, readback_buffer))?;
    Ok(LitRender { microseconds, output: scope.read(&readback)? })
}
