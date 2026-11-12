//! The device baseline's scene through wgpu at its lower level: the
//! `wgpu-hal` crate, one unsafe interface over Vulkan, Direct3D 12 and
//! Metal that validates nothing, tracks no resource state and leaves
//! transitions, lifetimes and destruction to its caller. Throwaway, never
//! product code.
//!
//! The pass and its recordings are those of `safe.rs`: one command encoder,
//! or four command encoders on four worker threads, one pass each on the
//! same targets, submitted in order by a fifth thread. Shaders enter
//! compiled ahead only: this level takes no WGSL without a translator the
//! caller runs itself.

use std::iter;
use std::ops::Range;
use std::time::Instant;

use wgpu_hal as hal;
use wgpu_types as wgt;

use hal::{Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _};

use render_device_baseline::scene::{self, Draw, Encoding, HEIGHT, Image, SAMPLES, Scene, TEXTURE_COUNT, WIDTH};

use crate::safe::{Bindings, FRAGMENT_ENTRY, Recorded, Rendered, Shaders, VERTEX_ENTRY};

/// Direct3D 12 places the rows of a copy at multiples of this many bytes and
/// its sub-resources at multiples of twice as many; the two other interfaces
/// accept the same layout.
const ROW_ALIGNMENT: u64 = 256;
const PLACEMENT_ALIGNMENT: u64 = 512;

const COLOR_FORMAT: wgt::TextureFormat = wgt::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgt::TextureFormat = wgt::TextureFormat::Depth32Float;

fn failed(what: &str, error: impl std::fmt::Display) -> String {
    format!("{what}: {error}")
}

/// An instance of one interface and the adapters it exposes.
pub fn instance<A: hal::Api>(flags: wgt::InstanceFlags) -> Result<(A::Instance, Vec<hal::ExposedAdapter<A>>), String> {
    let descriptor = hal::InstanceDescriptor {
        name: "backend comparison",
        flags,
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions::default(),
        telemetry: None,
        display: None,
    };
    // SAFETY: the descriptor names no display; the instance is used by this
    // process alone and outlives the adapters it lists.
    let instance = unsafe { A::Instance::init(&descriptor) }.map_err(|error| failed("the instance was not created", error))?;
    // SAFETY: the instance is live and no surface is asked for.
    let adapters = unsafe { instance.enumerate_adapters(None) };
    Ok((instance, adapters))
}

/// Opens a device with the features and limits of the scene's bindings.
pub fn open<A: hal::Api>(exposed: &hal::ExposedAdapter<A>, bindings: Bindings) -> Result<hal::OpenDevice<A>, String> {
    let wanted = bindings.features();
    let missing = wanted - exposed.features;
    if !missing.is_empty() {
        return Err(format!("the adapter does not offer {missing:?}"));
    }
    // SAFETY: the features are offered by the adapter and the limits are
    // within those it exposes.
    unsafe { exposed.adapter.open(wanted, &bindings.limits(), &wgt::MemoryHints::default()) }.map_err(|error| failed("the device was not created", error))
}

/// Everything the scene is drawn with on one device, owned and destroyed by
/// hand as this level requires.
pub struct LowerScene<A: hal::Api> {
    opaque: A::RenderPipeline,
    blended: A::RenderPipeline,
    pipeline_layout: A::PipelineLayout,
    bind_group_layout: A::BindGroupLayout,
    bind_group: A::BindGroup,
    modules: Vec<A::ShaderModule>,
    samplers: Vec<A::Sampler>,
    textures: Vec<A::Texture>,
    views: Vec<A::TextureView>,
    vertex_buffer: A::Buffer,
    indirect_buffer: A::Buffer,
    color: (A::Texture, A::TextureView),
    depth: (A::Texture, A::TextureView),
    resolved: (A::Texture, A::TextureView),
    read_back: A::Buffer,
    /// One encoder per worker; the first also records the serial pass.
    encoders: Vec<A::CommandEncoder>,
    fence: A::Fence,
    fence_value: hal::FenceValue,
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

/// A buffer the processor can write, holding the bytes given.
unsafe fn buffer_with<A: hal::Api>(device: &A::Device, label: &str, bytes: &[u8], usage: wgt::BufferUses) -> Result<A::Buffer, String> {
    let size = bytes.len() as u64;
    let descriptor = hal::BufferDescriptor { label: Some(label), size, usage: usage | wgt::BufferUses::MAP_WRITE, memory_flags: hal::MemoryFlags::PREFER_COHERENT };
    // SAFETY (whole function): the device is live; the mapping covers the
    // buffer just created, written within its size and flushed where the
    // memory is not coherent.
    unsafe {
        let buffer = device.create_buffer(&descriptor).map_err(|error| failed(label, error))?;
        let mapping = device.map_buffer(&buffer, 0..size).map_err(|error| failed(label, error))?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapping.ptr.as_ptr(), bytes.len());
        if !mapping.is_coherent {
            device.flush_mapped_ranges(&buffer, iter::once(0..size));
        }
        device.unmap_buffer(&buffer);
        Ok(buffer)
    }
}

fn whole() -> wgt::ImageSubresourceRange {
    wgt::ImageSubresourceRange::default()
}

fn barrier<'a, A: hal::Api>(texture: &'a A::Texture, from: wgt::TextureUses, to: wgt::TextureUses) -> hal::TextureBarrier<'a, A::Texture> {
    hal::TextureBarrier { texture, range: whole(), usage: hal::StateTransition { from, to } }
}

unsafe fn target<A: hal::Api>(device: &A::Device, label: &str, format: wgt::TextureFormat, samples: u32, usage: wgt::TextureUses) -> Result<(A::Texture, A::TextureView), String> {
    let descriptor = hal::TextureDescriptor {
        label: Some(label),
        size: wgt::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgt::TextureDimension::D2,
        format,
        usage,
        memory_flags: hal::MemoryFlags::empty(),
        view_formats: vec![],
    };
    let view_usage = usage - wgt::TextureUses::COPY_SRC;
    // SAFETY: the device is live and the view names the texture just created.
    unsafe {
        let texture = device.create_texture(&descriptor).map_err(|error| failed(label, error))?;
        let view = device
            .create_texture_view(&texture, &hal::TextureViewDescriptor { label: Some(label), format, dimension: wgt::TextureViewDimension::D2, usage: view_usage, range: whole() })
            .map_err(|error| failed(label, error))?;
        Ok((texture, view))
    }
}

impl<A: hal::Api> LowerScene<A> {
    /// Creates every resource and pipeline of the scene, before any timing.
    ///
    /// # Safety
    ///
    /// The device and the queue are those of one open device, used by the
    /// calling thread alone during the call.
    pub unsafe fn create(device: &A::Device, queue: &A::Queue, scene: &Scene, bindings: Bindings, shaders: &Shaders) -> Result<LowerScene<A>, String> {
        if scene.textures.len() != TEXTURE_COUNT {
            return Err(format!("the scene has {} textures where the shader's array holds {TEXTURE_COUNT}", scene.textures.len()));
        }
        // SAFETY (whole function): every object is created on the live
        // device and used with the usages it was created with; the staging
        // buffer outlives the submission that reads it.
        unsafe {
            // The textures, every level laid out in one staging buffer.
            let mut staging = Vec::new();
            let mut copies = Vec::new();
            let mut textures = Vec::new();
            let mut views = Vec::new();
            for texture in &scene.textures {
                let format = match texture.encoding {
                    Encoding::Srgb => wgt::TextureFormat::Rgba8UnormSrgb,
                    Encoding::Unorm => wgt::TextureFormat::Rgba8Unorm,
                };
                let created = device
                    .create_texture(&hal::TextureDescriptor {
                        label: Some(texture.name),
                        size: wgt::Extent3d { width: texture.width, height: texture.height, depth_or_array_layers: 1 },
                        mip_level_count: texture.levels.len() as u32,
                        sample_count: 1,
                        dimension: wgt::TextureDimension::D2,
                        format,
                        usage: wgt::TextureUses::COPY_DST | wgt::TextureUses::RESOURCE,
                        memory_flags: hal::MemoryFlags::empty(),
                        view_formats: vec![],
                    })
                    .map_err(|error| failed(texture.name, error))?;
                let view = device
                    .create_texture_view(&created, &hal::TextureViewDescriptor { label: Some(texture.name), format, dimension: wgt::TextureViewDimension::D2, usage: wgt::TextureUses::RESOURCE, range: whole() })
                    .map_err(|error| failed(texture.name, error))?;
                let mut level_copies = Vec::new();
                for (level, texels) in texture.levels.iter().enumerate() {
                    let (width, height) = ((texture.width >> level).max(1), (texture.height >> level).max(1));
                    let pitch = align_up(u64::from(width) * 4, ROW_ALIGNMENT);
                    let offset = align_up(staging.len() as u64, PLACEMENT_ALIGNMENT);
                    staging.resize((offset + pitch * u64::from(height)) as usize, 0);
                    for row in 0..height as usize {
                        let source = &texels[row * width as usize * 4..(row + 1) * width as usize * 4];
                        let at = offset as usize + row * pitch as usize;
                        staging[at..at + source.len()].copy_from_slice(source);
                    }
                    level_copies.push(hal::BufferTextureCopy {
                        buffer_layout: wgt::TexelCopyBufferLayout { offset, bytes_per_row: Some(pitch as u32), rows_per_image: None },
                        texture_base: hal::TextureCopyBase { mip_level: level as u32, array_layer: 0, origin: wgt::Origin3d::ZERO, aspect: hal::FormatAspects::COLOR },
                        size: hal::CopyExtent { width, height, depth: 1 },
                    });
                }
                copies.push(level_copies);
                textures.push(created);
                views.push(view);
            }
            let staging_buffer = buffer_with::<A>(device, "texture staging", &staging, wgt::BufferUses::COPY_SRC)?;

            let color = target::<A>(device, "4-sample color target", COLOR_FORMAT, SAMPLES, wgt::TextureUses::COLOR_TARGET)?;
            let depth = target::<A>(device, "4-sample depth target", DEPTH_FORMAT, SAMPLES, wgt::TextureUses::DEPTH_STENCIL_WRITE)?;
            let resolved = target::<A>(device, "resolved color", COLOR_FORMAT, 1, wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::COPY_SRC)?;

            // One submission uploads the textures and puts every texture in
            // the state the first frame expects.
            let mut encoders = Vec::new();
            for _ in 0..scene.chunks.len() {
                encoders.push(device.create_command_encoder(&hal::CommandEncoderDescriptor { label: Some("scene"), queue }).map_err(|error| failed("a command encoder", error))?);
            }
            let fence = device.create_fence().map_err(|error| failed("the fence", error))?;
            let setup = &mut encoders[0];
            setup.begin_encoding(Some("scene setup")).map_err(|error| failed("the setup commands", error))?;
            setup.transition_buffers(iter::once(hal::BufferBarrier { buffer: &staging_buffer, usage: hal::StateTransition { from: wgt::BufferUses::empty(), to: wgt::BufferUses::COPY_SRC } }));
            for (texture, level_copies) in textures.iter().zip(copies) {
                setup.transition_textures(iter::once(barrier::<A>(texture, wgt::TextureUses::UNINITIALIZED, wgt::TextureUses::COPY_DST)));
                setup.copy_buffer_to_texture(&staging_buffer, texture, level_copies.into_iter());
                setup.transition_textures(iter::once(barrier::<A>(texture, wgt::TextureUses::COPY_DST, wgt::TextureUses::RESOURCE)));
            }
            setup.transition_textures(
                [
                    barrier::<A>(&color.0, wgt::TextureUses::UNINITIALIZED, wgt::TextureUses::COLOR_TARGET),
                    barrier::<A>(&depth.0, wgt::TextureUses::UNINITIALIZED, wgt::TextureUses::DEPTH_STENCIL_WRITE),
                    barrier::<A>(&resolved.0, wgt::TextureUses::UNINITIALIZED, wgt::TextureUses::COPY_SRC),
                ]
                .into_iter(),
            );
            let commands = setup.end_encoding().map_err(|error| failed("the setup commands", error))?;
            queue.submit(&[&commands], &[], (&fence, 1)).map_err(|error| failed("the setup submission", error))?;
            device.wait(&fence, 1, None).map_err(|error| failed("the setup submission", error))?;
            encoders[0].reset_all(iter::once(commands));
            device.destroy_buffer(staging_buffer);

            let sampler = |label: &str, filter: wgt::FilterMode, mipmap_filter: wgt::MipmapFilterMode, address_mode: wgt::AddressMode| {
                device
                    .create_sampler(&hal::SamplerDescriptor {
                        label: Some(label),
                        address_modes: [address_mode; 3],
                        mag_filter: filter,
                        min_filter: filter,
                        mipmap_filter,
                        lod_clamp: 0.0..32.0,
                        compare: None,
                        anisotropy_clamp: 1,
                        border_color: None,
                    })
                    .map_err(|error| failed(label, error))
            };
            let samplers = vec![
                sampler("nearest", wgt::FilterMode::Nearest, wgt::MipmapFilterMode::Nearest, wgt::AddressMode::ClampToEdge)?,
                sampler("linear", wgt::FilterMode::Linear, wgt::MipmapFilterMode::Nearest, wgt::AddressMode::ClampToEdge)?,
                sampler("trilinear", wgt::FilterMode::Linear, wgt::MipmapFilterMode::Linear, wgt::AddressMode::Repeat)?,
            ];

            let entries = bindings.layout_entries();
            let group_entries: Vec<hal::BindGroupEntry> = match bindings {
                Bindings::Arrays => vec![hal::BindGroupEntry { binding: 0, resource_index: 0, count: TEXTURE_COUNT as u32 }, hal::BindGroupEntry { binding: 1, resource_index: 0, count: 3 }],
                Bindings::Separate => (0..TEXTURE_COUNT as u32)
                    .map(|index| hal::BindGroupEntry { binding: index, resource_index: index, count: 1 })
                    .chain((0..3).map(|index| hal::BindGroupEntry { binding: TEXTURE_COUNT as u32 + index, resource_index: index, count: 1 }))
                    .collect(),
            };
            let bind_group_layout = device
                .create_bind_group_layout(&hal::BindGroupLayoutDescriptor { label: Some("scene"), flags: hal::BindGroupLayoutFlags::empty(), entries: &entries })
                .map_err(|error| failed("the bind group layout", error))?;
            let pipeline_layout = device
                .create_pipeline_layout(&hal::PipelineLayoutDescriptor { label: Some("scene"), flags: hal::PipelineLayoutFlags::empty(), bind_group_layouts: &[Some(&bind_group_layout)], immediate_size: 8 })
                .map_err(|error| failed("the pipeline layout", error))?;
            let texture_bindings: Vec<hal::TextureBinding<A::TextureView>> = views.iter().map(|view| hal::TextureBinding { view, usage: wgt::TextureUses::RESOURCE }).collect();
            let sampler_bindings: Vec<&A::Sampler> = samplers.iter().collect();
            let bind_group = device
                .create_bind_group(&hal::BindGroupDescriptor {
                    label: Some("scene"),
                    layout: &bind_group_layout,
                    buffers: &[],
                    samplers: &sampler_bindings,
                    textures: &texture_bindings,
                    entries: &group_entries,
                    acceleration_structures: &[],
                    external_textures: &[],
                })
                .map_err(|error| failed("the bind group", error))?;

            let module = |label: &str, input: hal::ShaderInput| {
                device.create_shader_module(&hal::ShaderModuleDescriptor { label: Some(label), runtime_checks: wgt::ShaderRuntimeChecks::unchecked() }, input).map_err(|error| failed(label, error))
            };
            // The Metal backend looks every entry point up in this map, whatever
            // its stage: a function that is no compute kernel is given no size.
            let no_workgroups = || [VERTEX_ENTRY, FRAGMENT_ENTRY].into_iter().map(|entry| (entry.to_string(), (0, 0, 0))).collect();
            let (modules, vertex_entry, fragment_entry) = match shaders {
                Shaders::Wgsl(_) => return Err("the lower level takes no WGSL: the caller would run a translator itself".to_string()),
                Shaders::SpirV { vertex, fragment } => (vec![module("scene vertex, SPIR-V", hal::ShaderInput::SpirV(vertex))?, module("scene fragment, SPIR-V", hal::ShaderInput::SpirV(fragment))?], "main", "main"),
                Shaders::Dxil { vertex, fragment } => (
                    vec![module("scene vertex, DXIL", hal::ShaderInput::Dxil { shader: vertex })?, module("scene fragment, DXIL", hal::ShaderInput::Dxil { shader: fragment })?],
                    VERTEX_ENTRY,
                    FRAGMENT_ENTRY,
                ),
                Shaders::MetalLibrary(file) => (
                    vec![
                        module("scene vertex, Metal library", hal::ShaderInput::MetalLib { file, num_workgroups: no_workgroups() })?,
                        module("scene fragment, Metal library", hal::ShaderInput::MetalLib { file, num_workgroups: no_workgroups() })?,
                    ],
                    VERTEX_ENTRY,
                    FRAGMENT_ENTRY,
                ),
                Shaders::MetalText(shader) => (
                    vec![
                        module("scene vertex, Metal text", hal::ShaderInput::Msl { shader, num_workgroups: no_workgroups() })?,
                        module("scene fragment, Metal text", hal::ShaderInput::Msl { shader, num_workgroups: no_workgroups() })?,
                    ],
                    VERTEX_ENTRY,
                    FRAGMENT_ENTRY,
                ),
            };
            let attributes = [
                wgt::VertexAttribute { format: wgt::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
                wgt::VertexAttribute { format: wgt::VertexFormat::Float32x2, offset: 16, shader_location: 1 },
                wgt::VertexAttribute { format: wgt::VertexFormat::Float32x4, offset: 24, shader_location: 2 },
            ];
            let alpha_blend = wgt::BlendState {
                color: wgt::BlendComponent { src_factor: wgt::BlendFactor::SrcAlpha, dst_factor: wgt::BlendFactor::OneMinusSrcAlpha, operation: wgt::BlendOperation::Add },
                alpha: wgt::BlendComponent { src_factor: wgt::BlendFactor::One, dst_factor: wgt::BlendFactor::OneMinusSrcAlpha, operation: wgt::BlendOperation::Add },
            };
            let constants = Default::default();
            let pipeline = |label: &str, blended: bool| {
                device
                    .create_render_pipeline(&hal::RenderPipelineDescriptor::<A::PipelineLayout, A::ShaderModule, A::PipelineCache> {
                        label: Some(label),
                        layout: &pipeline_layout,
                        vertex_processor: hal::VertexProcessor::Standard {
                            vertex_buffers: &[Some(hal::VertexBufferLayout { array_stride: 40, step_mode: wgt::VertexStepMode::Vertex, attributes: &attributes })],
                            vertex_stage: hal::ProgrammableStage { module: &modules[0], entry_point: vertex_entry, constants: &constants, zero_initialize_workgroup_memory: false },
                        },
                        primitive: wgt::PrimitiveState { topology: wgt::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                        depth_stencil: Some(wgt::DepthStencilState {
                            format: DEPTH_FORMAT,
                            depth_write_enabled: Some(!blended),
                            depth_compare: Some(wgt::CompareFunction::LessEqual),
                            stencil: wgt::StencilState::default(),
                            bias: wgt::DepthBiasState::default(),
                        }),
                        multisample: wgt::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                        fragment_stage: Some(hal::ProgrammableStage { module: &modules[1], entry_point: fragment_entry, constants: &constants, zero_initialize_workgroup_memory: false }),
                        color_targets: &[Some(wgt::ColorTargetState { format: COLOR_FORMAT, blend: blended.then_some(alpha_blend), write_mask: wgt::ColorWrites::ALL })],
                        multiview_mask: None,
                        cache: None,
                    })
                    .map_err(|error| failed(label, error))
            };
            let opaque = pipeline("the opaque pipeline", false)?;
            let blended = pipeline("the blended pipeline", true)?;

            let vertex_buffer = buffer_with::<A>(device, "scene vertices", scene.vertex_bytes(), wgt::BufferUses::VERTEX)?;
            let indirect_buffer = buffer_with::<A>(device, "indirect arguments", scene.indirect_argument_bytes(), wgt::BufferUses::INDIRECT)?;
            let read_back = device
                .create_buffer(&hal::BufferDescriptor {
                    label: Some("scene read-back"),
                    size: u64::from(WIDTH * HEIGHT * 4),
                    usage: wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
                    memory_flags: hal::MemoryFlags::PREFER_COHERENT,
                })
                .map_err(|error| failed("the read-back buffer", error))?;
            Ok(LowerScene {
                opaque,
                blended,
                pipeline_layout,
                bind_group_layout,
                bind_group,
                modules,
                samplers,
                textures,
                views,
                vertex_buffer,
                indirect_buffer,
                color,
                depth,
                resolved,
                read_back,
                encoders,
                fence,
                fence_value: 1,
            })
        }
    }

    /// One command buffer holding one pass over the draws given; the first
    /// clears the targets, the last resolves the color and copies it to the
    /// read-back buffer.
    unsafe fn record(&self, encoder: &mut A::CommandEncoder, draws: &[Draw], first: bool, last: bool) -> Result<A::CommandBuffer, String> {
        let load = if first { hal::AttachmentOps::LOAD_CLEAR } else { hal::AttachmentOps::LOAD };
        let store = if last { hal::AttachmentOps::STORE_DISCARD } else { hal::AttachmentOps::STORE };
        let [red, green, blue, alpha] = scene::CLEAR_COLOR.map(f64::from);
        // SAFETY (whole function): the encoder is closed and recorded by
        // this thread alone; every object belongs to its device and is in
        // the state the transitions of `create` and of this function leave.
        unsafe {
            encoder.begin_encoding(Some("scene")).map_err(|error| failed("the scene's commands", error))?;
            if last {
                encoder.transition_textures(iter::once(barrier::<A>(&self.resolved.0, wgt::TextureUses::COPY_SRC, wgt::TextureUses::COLOR_TARGET)));
            }
            encoder
                .begin_render_pass(&hal::RenderPassDescriptor::<A::QuerySet, A::TextureView> {
                    label: Some("scene"),
                    extent: wgt::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
                    sample_count: SAMPLES,
                    color_attachments: &[Some(hal::ColorAttachment {
                        target: hal::Attachment { view: &self.color.1, usage: wgt::TextureUses::COLOR_TARGET },
                        depth_slice: None,
                        resolve_target: last.then_some(hal::Attachment { view: &self.resolved.1, usage: wgt::TextureUses::COLOR_TARGET }),
                        ops: load | store,
                        clear_value: wgt::Color { r: red, g: green, b: blue, a: alpha },
                    })],
                    depth_stencil_attachment: Some(hal::DepthStencilAttachment {
                        target: hal::Attachment { view: &self.depth.1, usage: wgt::TextureUses::DEPTH_STENCIL_WRITE },
                        depth_ops: load | store,
                        stencil_ops: hal::AttachmentOps::LOAD_DONT_CARE | hal::AttachmentOps::STORE_DISCARD,
                        clear_value: (scene::CLEAR_DEPTH, 0),
                    }),
                    multiview_mask: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .map_err(|error| failed("the scene's pass", error))?;
            encoder.set_viewport(&hal::Rect { x: 0.0, y: 0.0, w: WIDTH as f32, h: HEIGHT as f32 }, 0.0..1.0);
            encoder.set_scissor_rect(&hal::Rect { x: 0, y: 0, w: WIDTH, h: HEIGHT });
            let mut bound = None;
            let mut shared_bound = false;
            for draw in draws {
                if bound != Some(draw.blended) {
                    encoder.set_render_pipeline(if draw.blended { &self.blended } else { &self.opaque });
                    bound = Some(draw.blended);
                }
                if !shared_bound {
                    encoder.set_bind_group(&self.pipeline_layout, 0, &self.bind_group, &[]);
                    encoder.set_vertex_buffer(0, hal::BufferBinding::new_unchecked(&self.vertex_buffer, 0, None));
                    shared_bound = true;
                }
                encoder.set_immediates(&self.pipeline_layout, 0, &[draw.texture, draw.sampler]);
                if draw.indirect {
                    encoder.draw_indirect(&self.indirect_buffer, 0, 1);
                } else {
                    encoder.draw(draw.first_vertex, draw.vertex_count, 0, 1);
                }
            }
            encoder.end_render_pass();
            if last {
                encoder.transition_textures(iter::once(barrier::<A>(&self.resolved.0, wgt::TextureUses::COLOR_TARGET, wgt::TextureUses::COPY_SRC)));
                encoder.transition_buffers(iter::once(hal::BufferBarrier { buffer: &self.read_back, usage: hal::StateTransition { from: wgt::BufferUses::empty(), to: wgt::BufferUses::COPY_DST } }));
                encoder.copy_texture_to_buffer(
                    &self.resolved.0,
                    wgt::TextureUses::COPY_SRC,
                    &self.read_back,
                    iter::once(hal::BufferTextureCopy {
                        buffer_layout: wgt::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(WIDTH * 4), rows_per_image: None },
                        texture_base: hal::TextureCopyBase { mip_level: 0, array_layer: 0, origin: wgt::Origin3d::ZERO, aspect: hal::FormatAspects::COLOR },
                        size: hal::CopyExtent { width: WIDTH, height: HEIGHT, depth: 1 },
                    }),
                );
            }
            encoder.end_encoding().map_err(|error| failed("the scene's commands", error))
        }
    }

    /// # Safety
    ///
    /// The device and the queue are those the scene was created on, and no
    /// other thread uses them or the scene during the call.
    pub unsafe fn render(&mut self, device: &A::Device, queue: &A::Queue, scene: &Scene, recorded: Recorded) -> Result<Rendered, String> {
        let microseconds = |since: Instant| since.elapsed().as_secs_f64() * 1_000_000.0;
        let mut encoders = std::mem::take(&mut self.encoders);
        let value = self.fence_value + 1;
        self.fence_value = value;
        let this = &*self;
        let started = Instant::now();
        // SAFETY (whole function): each encoder is recorded by one thread;
        // the command buffers are submitted once, in the chunks' order, and
        // kept until the fence says the device has completed them.
        let outcome = (|| unsafe {
            let buffers: Vec<A::CommandBuffer> = match recorded {
                Recorded::Serial => vec![this.record(&mut encoders[0], &scene.draws, true, true)?],
                Recorded::FourWorkersOneSubmitter => {
                    let count = scene.chunks.len();
                    let chunks: Vec<Range<usize>> = scene.chunks.to_vec();
                    std::thread::scope(|threads| {
                        let workers: Vec<_> = encoders
                            .iter_mut()
                            .zip(chunks)
                            .enumerate()
                            .map(|(position, (encoder, chunk))| threads.spawn(move || this.record(encoder, &scene.draws[chunk], position == 0, position + 1 == count)))
                            .collect();
                        workers.into_iter().map(|worker| worker.join().unwrap_or_else(|_| Err("a recording worker panicked".to_string()))).collect::<Result<_, _>>()
                    })?
                }
            };
            let recorded_us = microseconds(started);
            let references: Vec<&A::CommandBuffer> = buffers.iter().collect();
            let submit = || queue.submit(&references, &[], (&this.fence, value)).map_err(|error| failed("the submission", error));
            match recorded {
                Recorded::Serial => submit()?,
                Recorded::FourWorkersOneSubmitter => std::thread::scope(|threads| threads.spawn(submit).join().unwrap_or_else(|_| Err("the submitting thread panicked".to_string())))?,
            }
            let submitted_us = microseconds(started);
            device.wait(&this.fence, value, None).map_err(|error| failed("the wait for the device", error))?;
            let completed_us = microseconds(started);
            drop(references);
            Ok::<_, String>((buffers, recorded_us, submitted_us, completed_us))
        })();
        let result = match outcome {
            Ok((buffers, recorded_us, submitted_us, completed_us)) => {
                // SAFETY: the device has completed the command buffers, each
                // returned to the encoder that built it.
                unsafe {
                    for (encoder, buffer) in encoders.iter_mut().zip(buffers) {
                        encoder.reset_all(iter::once(buffer));
                    }
                }
                // SAFETY: the copy to the read-back buffer has completed.
                unsafe { self.read_back(device) }.map(|image| Rendered { image, recorded_us, submitted_us, completed_us })
            }
            Err(reason) => Err(reason),
        };
        self.encoders = encoders;
        result
    }

    unsafe fn read_back(&self, device: &A::Device) -> Result<Image, String> {
        let size = (WIDTH * HEIGHT * 4) as usize;
        // SAFETY: the buffer holds `size` bytes, is not mapped, and no
        // command that writes it is pending.
        unsafe {
            let mapping = device.map_buffer(&self.read_back, 0..size as u64).map_err(|error| failed("the read-back buffer", error))?;
            if !mapping.is_coherent {
                device.invalidate_mapped_ranges(&self.read_back, iter::once(0..size as u64));
            }
            let bytes = std::slice::from_raw_parts(mapping.ptr.as_ptr(), size).to_vec();
            device.unmap_buffer(&self.read_back);
            Ok(Image::from_rows(WIDTH, HEIGHT, WIDTH as usize * 4, &bytes))
        }
    }

    /// Destroys every object of the scene, as this level leaves to its caller.
    ///
    /// # Safety
    ///
    /// The device is the one the scene was created on and has completed
    /// every submission that names the scene's objects.
    pub unsafe fn destroy(self, device: &A::Device) {
        // SAFETY: nothing names these objects any more.
        unsafe {
            device.destroy_render_pipeline(self.opaque);
            device.destroy_render_pipeline(self.blended);
            device.destroy_bind_group(self.bind_group);
            device.destroy_pipeline_layout(self.pipeline_layout);
            device.destroy_bind_group_layout(self.bind_group_layout);
            for module in self.modules {
                device.destroy_shader_module(module);
            }
            for sampler in self.samplers {
                device.destroy_sampler(sampler);
            }
            for view in self.views {
                device.destroy_texture_view(view);
            }
            for texture in self.textures {
                device.destroy_texture(texture);
            }
            for (texture, view) in [self.color, self.depth, self.resolved] {
                device.destroy_texture_view(view);
                device.destroy_texture(texture);
            }
            device.destroy_buffer(self.vertex_buffer);
            device.destroy_buffer(self.indirect_buffer);
            device.destroy_buffer(self.read_back);
            drop(self.encoders);
            device.destroy_fence(self.fence);
        }
    }

    /// A view of the first texture in a format its texture was not created
    /// for: a deliberate misuse a device survives, which only the
    /// interface's own validation layer can report at this level.
    ///
    /// # Safety
    ///
    /// The device is the one the scene was created on.
    pub unsafe fn misuse(&self, device: &A::Device) -> String {
        let what = "created a view in the format R32Float of a texture created as Rgba8UnormSrgb with no other view format, then destroyed it";
        let descriptor = hal::TextureViewDescriptor { label: Some("misuse"), format: wgt::TextureFormat::R32Float, dimension: wgt::TextureViewDimension::D2, usage: wgt::TextureUses::RESOURCE, range: whole() };
        // SAFETY: the call is the misuse under test; the texture is live and
        // the view, when one is returned, is destroyed at once.
        match unsafe { device.create_texture_view(&self.textures[0], &descriptor) } {
            Ok(view) => {
                // SAFETY: nothing names the view.
                unsafe { device.destroy_texture_view(view) };
                what.to_string()
            }
            Err(error) => format!("{what}; the creation returned an error: {error}"),
        }
    }
}
