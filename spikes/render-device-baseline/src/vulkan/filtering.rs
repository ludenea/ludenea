//! The filtering and blending scene on Vulkan: one render pass with a
//! 4-sample color attachment, a 4-sample depth attachment and a resolve
//! attachment; the draws recorded inline in the primary command buffer, or
//! as one secondary command buffer per chunk (each from its own command
//! pool), recorded by the calling thread or by four threads at once; per-draw
//! texture and sampler indexes as push constants; the indirect draw through
//! `vkCmdDrawIndirect`; two timestamps around the pass; the resolved image
//! copied to a host-visible buffer whose fence is polled. Throwaway, never
//! product code.

use std::ops::Range;
use std::time::Instant;

use super::Shaders;
use super::ffi::*;
use super::gpu::*;
use crate::backend::{Recording, SceneRender};
use crate::scene::{self, CLEAR_COLOR, CLEAR_DEPTH, Draw, Encoding, HEIGHT, Image, SAMPLES, Scene, TEXTURE_COUNT, WIDTH};

/// Position, uv and color: 40 bytes.
const VERTEX_STRIDE: u32 = 40;

/// What every recording of the draws binds.
struct DrawState {
    opaque: Object,
    blended: Object,
    layout: Object,
    set: Object,
    vertex_buffer: Object,
    indirect_buffer: Object,
    render_pass: Object,
    framebuffer: Object,
}

fn sampler_info(filter: u32, mipmap_mode: u32, address_mode: u32) -> SamplerCreateInfo {
    SamplerCreateInfo {
        s_type: structure::SAMPLER_CREATE_INFO,
        p_next: std::ptr::null(),
        flags: 0,
        mag_filter: filter,
        min_filter: filter,
        mipmap_mode,
        address_mode_u: address_mode,
        address_mode_v: address_mode,
        address_mode_w: address_mode,
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

/// The nearest, linear and trilinear samplers, in the scene's order.
fn samplers(scope: &mut Scope) -> Result<[Object; 3], String> {
    Ok([
        scope.sampler(&sampler_info(FILTER_NEAREST, MIPMAP_MODE_NEAREST, ADDRESS_MODE_CLAMP_TO_EDGE))?,
        scope.sampler(&sampler_info(FILTER_LINEAR, MIPMAP_MODE_NEAREST, ADDRESS_MODE_CLAMP_TO_EDGE))?,
        scope.sampler(&sampler_info(FILTER_LINEAR, MIPMAP_MODE_LINEAR, ADDRESS_MODE_REPEAT))?,
    ])
}

/// The five textures with every level uploaded, as views ready for sampling.
fn textures(scope: &mut Scope, scene: &Scene) -> Result<Vec<Object>, String> {
    let mut images = Vec::new();
    for texture in &scene.textures {
        let format = match texture.encoding {
            Encoding::Srgb => FORMAT_R8G8B8A8_SRGB,
            Encoding::Unorm => FORMAT_R8G8B8A8_UNORM,
        };
        let levels = texture.levels.len() as u32;
        let image = scope.image(&image_info(format, texture.width, texture.height, levels, SAMPLE_COUNT_1, IMAGE_USAGE_SAMPLED | IMAGE_USAGE_TRANSFER_DST, 0))?;
        images.push((image, format, levels));
    }
    let uploads: Vec<ImageUpload> = scene
        .textures
        .iter()
        .zip(&images)
        .map(|(texture, &(image, _, _))| ImageUpload { image, width: texture.width, height: texture.height, levels: texture.levels.iter().map(Vec::as_slice).collect() })
        .collect();
    scope.upload_images(&uploads)?;
    images.iter().map(|&(image, format, levels)| scope.image_view(image, format, ASPECT_COLOR, levels)).collect()
}

/// The render pass, its framebuffer and the resolved image the copy reads.
fn targets(scope: &mut Scope) -> Result<(Object, Object, Object), String> {
    // `VkSampleCountFlagBits` has the value of the count it names.
    let samples = SAMPLES;
    let color = scope.image(&image_info(FORMAT_R8G8B8A8_SRGB, WIDTH, HEIGHT, 1, samples, IMAGE_USAGE_COLOR_ATTACHMENT, 0))?;
    let depth = scope.image(&image_info(FORMAT_D32_SFLOAT, WIDTH, HEIGHT, 1, samples, IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT, 0))?;
    let resolved = scope.image(&image_info(FORMAT_R8G8B8A8_SRGB, WIDTH, HEIGHT, 1, SAMPLE_COUNT_1, IMAGE_USAGE_COLOR_ATTACHMENT | IMAGE_USAGE_TRANSFER_SRC, 0))?;
    let views = [
        scope.image_view(color, FORMAT_R8G8B8A8_SRGB, ASPECT_COLOR, 1)?,
        scope.image_view(depth, FORMAT_D32_SFLOAT, ASPECT_DEPTH, 1)?,
        scope.image_view(resolved, FORMAT_R8G8B8A8_SRGB, ASPECT_COLOR, 1)?,
    ];
    let attachments = [
        attachment(FORMAT_R8G8B8A8_SRGB, samples, LOAD_OP_CLEAR, STORE_OP_DONT_CARE, (LAYOUT_UNDEFINED, LAYOUT_COLOR_ATTACHMENT_OPTIMAL)),
        attachment(FORMAT_D32_SFLOAT, samples, LOAD_OP_CLEAR, STORE_OP_DONT_CARE, (LAYOUT_UNDEFINED, LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL)),
        attachment(FORMAT_R8G8B8A8_SRGB, SAMPLE_COUNT_1, LOAD_OP_DONT_CARE, STORE_OP_STORE, (LAYOUT_UNDEFINED, LAYOUT_TRANSFER_SRC_OPTIMAL)),
    ];
    let render_pass = scope.render_pass(&attachments, &[0], &[2], Some(1), &[after_color_writes(PIPELINE_STAGE_TRANSFER, ACCESS_TRANSFER_READ)])?;
    let framebuffer = scope.framebuffer(render_pass, &views, WIDTH, HEIGHT)?;
    Ok((render_pass, framebuffer, resolved))
}

fn alpha_blend() -> PipelineColorBlendAttachmentState {
    PipelineColorBlendAttachmentState {
        blend_enable: 1,
        src_color_blend_factor: BLEND_FACTOR_SRC_ALPHA,
        dst_color_blend_factor: BLEND_FACTOR_ONE_MINUS_SRC_ALPHA,
        color_blend_op: BLEND_OP_ADD,
        src_alpha_blend_factor: BLEND_FACTOR_ONE,
        dst_alpha_blend_factor: BLEND_FACTOR_ONE_MINUS_SRC_ALPHA,
        alpha_blend_op: BLEND_OP_ADD,
        color_write_mask: COLOR_COMPONENTS_RGBA,
    }
}

/// Creates every resource and pipeline of the scene, before any timing.
fn prepare(scope: &mut Scope, shaders: &Shaders, scene: &Scene) -> Result<(DrawState, Object), String> {
    if scope.gpu.core_features[feature::SHADER_SAMPLED_IMAGE_ARRAY_DYNAMIC_INDEXING] == 0 {
        return Err("the device does not offer shaderSampledImageArrayDynamicIndexing, which indexing the array of five textures by a push constant needs".to_string());
    }
    let views = textures(scope, scene)?;
    if views.len() != TEXTURE_COUNT {
        return Err(format!("the scene has {} textures where the shader's array holds {TEXTURE_COUNT}", views.len()));
    }
    let samplers = samplers(scope)?;
    let bindings = [(DESCRIPTOR_SAMPLED_IMAGE, TEXTURE_COUNT as u32, STAGE_FRAGMENT), (DESCRIPTOR_SAMPLER, samplers.len() as u32, STAGE_FRAGMENT)];
    let set_layout = scope.descriptor_set_layout(&bindings)?;
    let set = scope.descriptor_set(set_layout, &bindings)?;
    let image_infos: Vec<DescriptorImageInfo> = views.iter().map(|&image_view| DescriptorImageInfo { sampler: 0, image_view, image_layout: LAYOUT_SHADER_READ_ONLY_OPTIMAL }).collect();
    scope.write_images(set, 0, DESCRIPTOR_SAMPLED_IMAGE, &image_infos);
    let sampler_infos: Vec<DescriptorImageInfo> = samplers.iter().map(|&sampler| DescriptorImageInfo { sampler, image_view: 0, image_layout: LAYOUT_UNDEFINED }).collect();
    scope.write_images(set, 1, DESCRIPTOR_SAMPLER, &sampler_infos);
    let push_constants = [PushConstantRange { stage_flags: STAGE_FRAGMENT, offset: 0, size: 8 }];
    let layout = scope.pipeline_layout(&[set_layout], &push_constants)?;
    let (render_pass, framebuffer, resolved) = targets(scope)?;
    let vertex = scope.shader_module(&shaders.scene_vertex)?;
    let fragment = scope.shader_module(&shaders.scene_fragment)?;
    let attributes = [
        VertexInputAttributeDescription { location: 0, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 0 },
        VertexInputAttributeDescription { location: 1, binding: 0, format: FORMAT_R32G32_SFLOAT, offset: 16 },
        VertexInputAttributeDescription { location: 2, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 24 },
    ];
    let mut pipeline = |depth_write: bool, blend: PipelineColorBlendAttachmentState| {
        scope.graphics_pipeline(&GraphicsPipelineDescription {
            vertex,
            fragment,
            vertex_stride: VERTEX_STRIDE,
            attributes: &attributes,
            viewport: full_viewport(WIDTH, HEIGHT),
            samples: SAMPLES,
            depth_write: Some(depth_write),
            blends: &[blend],
            layout,
            render_pass,
        })
    };
    let opaque = pipeline(true, opaque_blend())?;
    let blended = pipeline(false, alpha_blend())?;
    let vertex_buffer = scope.buffer_with(scene.vertex_bytes(), BUFFER_USAGE_VERTEX)?.buffer;
    let indirect_buffer = scope.buffer_with(scene.indirect_argument_bytes(), BUFFER_USAGE_INDIRECT)?.buffer;
    Ok((DrawState { opaque, blended, layout, set, vertex_buffer, indirect_buffer, render_pass, framebuffer }, resolved))
}

/// Records draws in the scene's order: binds, then for each draw its
/// pipeline (when it changes), its two indexes and the draw itself.
fn record_draws(functions: &DeviceFunctions, command_buffer: Handle, state: &DrawState, draws: &[Draw]) {
    let offset = 0_u64;
    // SAFETY: the command buffer is recording inside the scene's render pass
    // (inline or continuing it); every object belongs to the device and the
    // pointers live for each call.
    unsafe {
        (functions.cmd_bind_descriptor_sets)(command_buffer, BIND_POINT_GRAPHICS, state.layout, 0, 1, &state.set, 0, std::ptr::null());
        (functions.cmd_bind_vertex_buffers)(command_buffer, 0, 1, &state.vertex_buffer, &offset);
        let mut bound_blended = None;
        for draw in draws {
            if bound_blended != Some(draw.blended) {
                (functions.cmd_bind_pipeline)(command_buffer, BIND_POINT_GRAPHICS, if draw.blended { state.blended } else { state.opaque });
                bound_blended = Some(draw.blended);
            }
            let indexes = [draw.texture, draw.sampler];
            (functions.cmd_push_constants)(command_buffer, state.layout, STAGE_FRAGMENT, 0, 8, indexes.as_ptr() as *const std::ffi::c_void);
            if draw.indirect {
                (functions.cmd_draw_indirect)(command_buffer, state.indirect_buffer, 0, 1, 16);
            } else {
                (functions.cmd_draw)(command_buffer, draw.vertex_count, 1, draw.first_vertex, 0);
            }
        }
    }
}

/// Records one chunk into its secondary command buffer, continuing the pass.
fn record_chunk(functions: &DeviceFunctions, command_buffer: CommandBuffer, state: &DrawState, draws: &[Draw]) -> Result<(), String> {
    let inheritance = CommandBufferInheritanceInfo {
        s_type: structure::COMMAND_BUFFER_INHERITANCE_INFO,
        p_next: std::ptr::null(),
        render_pass: state.render_pass,
        subpass: 0,
        framebuffer: state.framebuffer,
        occlusion_query_enable: 0,
        query_flags: 0,
        pipeline_statistics: 0,
    };
    begin(functions, command_buffer.0, COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT | COMMAND_BUFFER_USAGE_RENDER_PASS_CONTINUE, Some(&inheritance))?;
    record_draws(functions, command_buffer.0, state, draws);
    end(functions, command_buffer.0)
}

/// Records the four chunks, on the calling thread or on four threads at once.
fn record_chunks(functions: &DeviceFunctions, buffers: &[CommandBuffer], state: &DrawState, scene: &Scene, on_four_threads: bool) -> Result<(), String> {
    let chunks: Vec<Range<usize>> = scene.chunks.to_vec();
    if !on_four_threads {
        for (&buffer, chunk) in buffers.iter().zip(chunks) {
            record_chunk(functions, buffer, state, &scene.draws[chunk])?;
        }
        return Ok(());
    }
    std::thread::scope(|threads| {
        let running: Vec<_> = buffers.iter().zip(chunks).map(|(&buffer, chunk)| threads.spawn(move || record_chunk(functions, buffer, state, &scene.draws[chunk]))).collect();
        running.into_iter().map(|thread| thread.join().unwrap_or_else(|_| Err("a recording thread panicked".to_string()))).collect()
    })
}

/// The time between the two timestamps, in nanoseconds.
fn elapsed_between_timestamps(gpu: &Gpu, pool: Object) -> Result<u64, String> {
    let mut values = [0_u64; 2];
    // SAFETY: the two queries were written by the completed submission and
    // the array holds two 64-bit results.
    check(
        unsafe { (gpu.functions.get_query_pool_results)(gpu.device, pool, 0, 2, 16, values.as_mut_ptr() as *mut std::ffi::c_void, 8, QUERY_RESULT_64 | QUERY_RESULT_WAIT) },
        "vkGetQueryPoolResults",
    )?;
    let bits = gpu.timestamp_bits();
    let mask = if bits >= 64 { u64::MAX } else { (1_u64 << bits) - 1 };
    let ticks = values[1].wrapping_sub(values[0]) & mask;
    let period = f64::from(gpu.physical.properties.f32_at(property::TIMESTAMP_PERIOD));
    Ok((ticks as f64 * period).round() as u64)
}

/// The primary command buffer: timestamps around the pass, the draws inline
/// or the chunks executed in order, then the copy of the resolved image.
fn record_primary(functions: &DeviceFunctions, primary: Handle, state: &DrawState, scene: &Scene, chunks: &[CommandBuffer], timestamps: Option<Object>, resolved: Object, readback: Object) -> Result<(), String> {
    begin(functions, primary, COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT, None)?;
    let clears = [CLEAR_COLOR, [CLEAR_DEPTH, 0.0, 0.0, 0.0], [0.0; 4]];
    let contents = if chunks.is_empty() { SUBPASS_CONTENTS_INLINE } else { SUBPASS_CONTENTS_SECONDARY_COMMAND_BUFFERS };
    // SAFETY: the primary command buffer is recording; the query pool, the
    // secondary command buffers and every object belong to the device.
    unsafe {
        if let Some(pool) = timestamps {
            (functions.cmd_reset_query_pool)(primary, pool, 0, 2);
            (functions.cmd_write_timestamp)(primary, PIPELINE_STAGE_TOP_OF_PIPE, pool, 0);
        }
        begin_render_pass(functions, primary, state.render_pass, state.framebuffer, [WIDTH, HEIGHT], &clears, contents);
        if chunks.is_empty() {
            record_draws(functions, primary, state, &scene.draws);
        } else {
            let handles: Vec<Handle> = chunks.iter().map(|buffer| buffer.0).collect();
            (functions.cmd_execute_commands)(primary, handles.len() as u32, handles.as_ptr());
        }
        (functions.cmd_end_render_pass)(primary);
        if let Some(pool) = timestamps {
            (functions.cmd_write_timestamp)(primary, PIPELINE_STAGE_BOTTOM_OF_PIPE, pool, 1);
        }
    }
    copy_to_host(functions, primary, resolved, WIDTH, HEIGHT, readback);
    end(functions, primary)
}

pub fn render(gpu: &Gpu, shaders: &Shaders, scene: &Scene, recording: Recording) -> Result<SceneRender, String> {
    let mut scope = Scope::new(gpu);
    let (state, resolved) = prepare(&mut scope, shaders, scene)?;
    let readback = scope.host_buffer((WIDTH * HEIGHT * 4) as usize, BUFFER_USAGE_TRANSFER_DST)?;
    let timestamps = if gpu.timestamp_bits() > 0 { Some(scope.timestamp_pool(2)?) } else { None };
    let primary_pool = scope.command_pool()?;
    let primary = scope.command_buffer(primary_pool, COMMAND_BUFFER_LEVEL_PRIMARY)?.0;
    let mut chunks = Vec::new();
    if recording != Recording::SerialOneBuffer {
        for _ in &scene.chunks {
            let pool = scope.command_pool()?;
            chunks.push(scope.command_buffer(pool, COMMAND_BUFFER_LEVEL_SECONDARY)?);
        }
    }
    let fence = scope.fence()?;

    let started = Instant::now();
    if !chunks.is_empty() {
        record_chunks(&gpu.functions, &chunks, &state, scene, recording == Recording::ChunkedFourThreads)?;
    }
    record_primary(&gpu.functions, primary, &state, scene, &chunks, timestamps, resolved, readback.buffer)?;
    gpu.submit(primary, fence)?;
    let readback_polls = gpu.poll_fence(fence)?;
    let render_us = started.elapsed().as_secs_f64() * 1_000_000.0;

    let read_started = Instant::now();
    let bytes = scope.read(&readback)?;
    let image = Image::from_rows(scene::WIDTH, scene::HEIGHT, WIDTH as usize * 4, &bytes);
    let readback_us = read_started.elapsed().as_secs_f64() * 1_000_000.0;
    let timestamp_ns = timestamps.map(|pool| elapsed_between_timestamps(gpu, pool)).transpose()?;
    Ok(SceneRender { image, render_us, readback_us, readback_polls, timestamp_ns })
}
