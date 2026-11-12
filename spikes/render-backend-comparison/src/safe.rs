//! The device baseline's scene through wgpu at its safe level: the `wgpu`
//! crate, which validates every call before it reaches the interface.
//! Throwaway, never product code.
//!
//! The pass is the one `render_device_baseline::backend` specifies: the same
//! targets, formats, samplers, pipelines and order of draws, the five
//! textures bound as one array and the three samplers as another, the
//! per-draw indexes as immediate data. wgpu has no command buffer that
//! continues a pass, so the recording on four workers is four command
//! encoders, one per chunk of draws, each holding its own pass on the same
//! targets: the first clears them, the others load them, the last resolves.
//! A fifth thread submits the four command buffers in the chunks' order.

use std::borrow::Cow;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use wgpu::util::DeviceExt;

use render_device_baseline::scene::{self, Draw, Encoding, HEIGHT, Image, SAMPLES, Scene, TEXTURE_COUNT, WIDTH};

use crate::workload::block_on;

/// The interfaces the spike compares, as wgpu names them.
pub const BACKENDS: [(&str, wgpu::Backend); 3] = [("vulkan", wgpu::Backend::Vulkan), ("direct3d12", wgpu::Backend::Dx12), ("metal", wgpu::Backend::Metal)];

pub fn backend_named(name: &str) -> Option<wgpu::Backend> {
    BACKENDS.iter().find(|(known, _)| *known == name).map(|(_, backend)| *backend)
}

pub fn device_type_name(kind: wgpu::DeviceType) -> &'static str {
    match kind {
        wgpu::DeviceType::Other => "other",
        wgpu::DeviceType::IntegratedGpu => "integrated",
        wgpu::DeviceType::DiscreteGpu => "discrete",
        wgpu::DeviceType::VirtualGpu => "virtual",
        wgpu::DeviceType::Cpu => "software",
    }
}

/// An instance over the interfaces given, with the flags given.
pub fn instance(flags: wgpu::InstanceFlags, backends: wgpu::Backends) -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    descriptor.flags = flags;
    wgpu::Instance::new(descriptor)
}

/// The adapters of one interface, in the order wgpu lists them.
pub fn adapters(instance: &wgpu::Instance, backend: wgpu::Backend) -> Vec<wgpu::Adapter> {
    block_on(instance.enumerate_adapters(wgpu::Backends::from(backend)))
}

/// How the scene's two shader stages reach the device.
pub enum Shaders<'a> {
    /// WGSL, translated by wgpu for the interface when a pipeline is made.
    Wgsl(&'a str),
    /// SPIR-V compiled ahead, one module per stage, each with the entry
    /// point `main`.
    SpirV { vertex: &'a [u32], fragment: &'a [u32] },
    /// DXIL compiled ahead, one container per stage.
    Dxil { vertex: &'a [u8], fragment: &'a [u8] },
    /// A Metal library compiled ahead that holds both functions.
    MetalLibrary(&'a [u8]),
    /// Metal Shading Language text, compiled by the platform at load.
    MetalText(&'a str),
}

impl Shaders<'_> {
    pub fn is_compiled_ahead(&self) -> bool {
        !matches!(self, Shaders::Wgsl(_))
    }
}

/// The entry points of the sources this spike ships, per form.
pub const VERTEX_ENTRY: &str = "scene_vertex";
pub const FRAGMENT_ENTRY: &str = "scene_fragment";

/// The errors wgpu reported for one device since they were last taken.
#[derive(Clone, Default)]
pub struct Errors(Arc<Mutex<Vec<String>>>);

impl Errors {
    pub fn take(&self) -> Vec<String> {
        self.0.lock().map(|mut errors| std::mem::take(&mut *errors)).unwrap_or_default()
    }

    /// The step's errors as one failure, or nothing.
    pub fn check(&self, step: &str) -> Result<(), String> {
        let errors = self.take();
        if errors.is_empty() { Ok(()) } else { Err(format!("{step}: {}", errors.join(" / "))) }
    }
}

/// How the scene's five textures and three samplers are bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bindings {
    /// One array of five textures and one array of three samplers, as the
    /// device baseline's scene specifies.
    Arrays,
    /// Five texture bindings and three sampler bindings, chosen among in the
    /// fragment shader: for a device on which wgpu offers no array of
    /// textures, and for the Metal sources compiled ahead.
    Separate,
}

impl Bindings {
    pub fn name(self) -> &'static str {
        match self {
            Bindings::Arrays => "one array of five textures and one array of three samplers",
            Bindings::Separate => "five texture bindings and three sampler bindings",
        }
    }

    /// Arrays where the adapter's features offer them, separate bindings
    /// otherwise.
    pub fn offered_by(features: wgpu::Features) -> Bindings {
        if features.contains(wgpu::Features::TEXTURE_BINDING_ARRAY) { Bindings::Arrays } else { Bindings::Separate }
    }

    /// The features the scene asks of a device, beyond those of the shader form.
    pub fn features(self) -> wgpu::Features {
        match self {
            Bindings::Arrays => wgpu::Features::IMMEDIATES | wgpu::Features::TEXTURE_BINDING_ARRAY,
            Bindings::Separate => wgpu::Features::IMMEDIATES,
        }
    }

    pub fn limits(self) -> wgpu::Limits {
        let arrays = match self {
            Bindings::Arrays => (TEXTURE_COUNT as u32 + 3, 3),
            Bindings::Separate => (0, 0),
        };
        wgpu::Limits { max_immediate_size: 8, max_binding_array_elements_per_shader_stage: arrays.0, max_binding_array_sampler_elements_per_shader_stage: arrays.1, ..wgpu::Limits::default() }
    }

    /// The entries of the scene's one bind group layout: the textures from
    /// binding 0, then the samplers.
    pub fn layout_entries(self) -> Vec<wgpu::BindGroupLayoutEntry> {
        let texture = |binding: u32, count: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: NonZeroU32::new(count),
        };
        let sampler = |binding: u32, count: u32| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: NonZeroU32::new(count) };
        match self {
            Bindings::Arrays => vec![texture(0, TEXTURE_COUNT as u32), sampler(1, 3)],
            Bindings::Separate => (0..TEXTURE_COUNT as u32).map(|binding| texture(binding, 0)).chain((0..3).map(|index| sampler(TEXTURE_COUNT as u32 + index, 0))).collect(),
        }
    }
}

/// Opens a device with the features of the scene's bindings and the extra
/// ones given; every error wgpu reports for it afterwards is collected.
pub fn open(adapter: &wgpu::Adapter, bindings: Bindings, extra: wgpu::Features) -> Result<(wgpu::Device, wgpu::Queue, Errors), String> {
    let wanted = bindings.features() | extra;
    let missing = wanted - adapter.features();
    if !missing.is_empty() {
        return Err(format!("the adapter does not offer {missing:?}"));
    }
    let descriptor = wgpu::DeviceDescriptor { label: Some("backend comparison"), required_features: wanted, required_limits: bindings.limits(), ..Default::default() };
    let (device, queue) = block_on(adapter.request_device(&descriptor)).map_err(|error| format!("the device was not created: {error}"))?;
    let errors = Errors::default();
    let sink = errors.clone();
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        if let Ok(mut errors) = sink.0.lock() {
            errors.push(error.to_string().split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }));
    Ok((device, queue, errors))
}

/// Everything the scene is drawn with on one device.
pub struct SceneOnDevice {
    opaque: wgpu::RenderPipeline,
    blended: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    indirect_buffer: wgpu::Buffer,
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolved_view: wgpu::TextureView,
    resolved: wgpu::Texture,
    read_back: wgpu::Buffer,
}

/// How the pass is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recorded {
    /// One command encoder, on the calling thread, which also submits.
    Serial,
    /// Four command encoders on four worker threads at once, submitted in
    /// the chunks' order by a fifth thread.
    FourWorkersOneSubmitter,
}

impl Recorded {
    pub const ALL: [Recorded; 2] = [Recorded::Serial, Recorded::FourWorkersOneSubmitter];

    pub fn name(self) -> &'static str {
        match self {
            Recorded::Serial => "serial",
            Recorded::FourWorkersOneSubmitter => "four_workers_one_submitter",
        }
    }
}

pub struct Rendered {
    pub image: Image,
    /// From the start of the recording to the last command buffer finished.
    pub recorded_us: f64,
    /// From the start of the recording to the return of the submission.
    pub submitted_us: f64,
    /// From the start of the recording to the completion of the device's work.
    pub completed_us: f64,
}

fn modules(device: &wgpu::Device, shaders: &Shaders) -> (wgpu::ShaderModule, wgpu::ShaderModule, &'static str, &'static str) {
    let entry = |name: &'static str| wgpu::PassthroughShaderEntryPoint { name: Cow::Borrowed(name), workgroup_size: (0, 0, 0) };
    let passthrough = |label: &'static str, entries: Vec<wgpu::PassthroughShaderEntryPoint<'static>>, fill: &dyn Fn(&mut wgpu::ShaderModuleDescriptorPassthrough)| {
        let mut descriptor = wgpu::ShaderModuleDescriptorPassthrough { label: Some(label), entry_points: Cow::Owned(entries), ..Default::default() };
        fill(&mut descriptor);
        // SAFETY: the shader was written against the pipeline layout of
        // `SceneOnDevice::create` and against wgpu's binding scheme for the
        // interface, which is what the passthrough path leaves to its caller.
        unsafe { device.create_shader_module_passthrough(descriptor) }
    };
    match shaders {
        Shaders::Wgsl(source) => {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("scene, WGSL"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(source)) });
            (module.clone(), module, VERTEX_ENTRY, FRAGMENT_ENTRY)
        }
        Shaders::SpirV { vertex, fragment } => (
            passthrough("scene vertex, SPIR-V", vec![entry("main")], &|descriptor| descriptor.spirv = Some(Cow::Owned(vertex.to_vec()))),
            passthrough("scene fragment, SPIR-V", vec![entry("main")], &|descriptor| descriptor.spirv = Some(Cow::Owned(fragment.to_vec()))),
            "main",
            "main",
        ),
        Shaders::Dxil { vertex, fragment } => (
            passthrough("scene vertex, DXIL", vec![entry(VERTEX_ENTRY)], &|descriptor| descriptor.dxil = Some(Cow::Owned(vertex.to_vec()))),
            passthrough("scene fragment, DXIL", vec![entry(FRAGMENT_ENTRY)], &|descriptor| descriptor.dxil = Some(Cow::Owned(fragment.to_vec()))),
            VERTEX_ENTRY,
            FRAGMENT_ENTRY,
        ),
        Shaders::MetalLibrary(library) => {
            let module = passthrough("scene, Metal library", vec![entry(VERTEX_ENTRY), entry(FRAGMENT_ENTRY)], &|descriptor| descriptor.metallib = Some(Cow::Owned(library.to_vec())));
            (module.clone(), module, VERTEX_ENTRY, FRAGMENT_ENTRY)
        }
        Shaders::MetalText(source) => {
            let module = passthrough("scene, Metal text", vec![entry(VERTEX_ENTRY), entry(FRAGMENT_ENTRY)], &|descriptor| descriptor.msl = Some(Cow::Owned(source.to_string())));
            (module.clone(), module, VERTEX_ENTRY, FRAGMENT_ENTRY)
        }
    }
}

fn sampler(device: &wgpu::Device, label: &str, filter: wgpu::FilterMode, mipmap_filter: wgpu::MipmapFilterMode, address_mode: wgpu::AddressMode) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: address_mode,
        address_mode_v: address_mode,
        address_mode_w: address_mode,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter,
        lod_min_clamp: 0.0,
        lod_max_clamp: 32.0,
        compare: None,
        anisotropy_clamp: 1,
        border_color: None,
    })
}

fn target(device: &wgpu::Device, label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// One texture of the scene in the format of its encoding, every level
/// uploaded as given.
fn sampled_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &scene::Texture) -> wgpu::TextureView {
    let format = match texture.encoding {
        Encoding::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        Encoding::Unorm => wgpu::TextureFormat::Rgba8Unorm,
    };
    let created = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(texture.name),
        size: wgpu::Extent3d { width: texture.width, height: texture.height, depth_or_array_layers: 1 },
        mip_level_count: texture.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, texels) in texture.levels.iter().enumerate() {
        let (width, height) = ((texture.width >> level).max(1), (texture.height >> level).max(1));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &created, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            texels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }
    created.create_view(&wgpu::TextureViewDescriptor::default())
}

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

impl SceneOnDevice {
    /// Creates every resource and pipeline of the scene, before any timing.
    /// The pipelines are made through the cache when one is given.
    pub fn create(device: &wgpu::Device, queue: &wgpu::Queue, errors: &Errors, scene: &Scene, bindings: Bindings, shaders: &Shaders, cache: Option<&wgpu::PipelineCache>) -> Result<SceneOnDevice, String> {
        if scene.textures.len() != TEXTURE_COUNT {
            return Err(format!("the scene has {} textures where the shader's array holds {TEXTURE_COUNT}", scene.textures.len()));
        }
        let views: Vec<wgpu::TextureView> = scene.textures.iter().map(|texture| sampled_texture(device, queue, texture)).collect();
        let samplers = [
            sampler(device, "nearest", wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest, wgpu::AddressMode::ClampToEdge),
            sampler(device, "linear", wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Nearest, wgpu::AddressMode::ClampToEdge),
            sampler(device, "trilinear", wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Linear, wgpu::AddressMode::Repeat),
        ];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("scene"), entries: &bindings.layout_entries() });
        let view_refs: Vec<&wgpu::TextureView> = views.iter().collect();
        let sampler_refs: Vec<&wgpu::Sampler> = samplers.iter().collect();
        let entries: Vec<wgpu::BindGroupEntry> = match bindings {
            Bindings::Arrays => vec![
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureViewArray(&view_refs) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::SamplerArray(&sampler_refs) },
            ],
            Bindings::Separate => views
                .iter()
                .enumerate()
                .map(|(index, view)| wgpu::BindGroupEntry { binding: index as u32, resource: wgpu::BindingResource::TextureView(view) })
                .chain(samplers.iter().enumerate().map(|(index, sampler)| wgpu::BindGroupEntry { binding: (TEXTURE_COUNT + index) as u32, resource: wgpu::BindingResource::Sampler(sampler) }))
                .collect(),
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("scene"), layout: &layout, entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("scene"), bind_group_layouts: &[Some(&layout)], immediate_size: 8 });
        errors.check("the scene's textures, samplers and layouts")?;

        let (vertex, fragment, vertex_entry, fragment_entry) = modules(device, shaders);
        errors.check("the scene's shader modules")?;
        let attributes = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 24, shader_location: 2 },
        ];
        let alpha_blend = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        };
        let pipeline = |label: &str, blended: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &vertex,
                    entry_point: Some(vertex_entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 40, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
                },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!blended),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &fragment,
                    entry_point: Some(fragment_entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend: blended.then_some(alpha_blend), write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache,
            })
        };
        let opaque = pipeline("scene, opaque", false);
        let blended = pipeline("scene, blended", true);
        errors.check("the scene's pipelines")?;

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("scene vertices"), contents: scene.vertex_bytes(), usage: wgpu::BufferUsages::VERTEX });
        let indirect_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("indirect arguments"), contents: scene.indirect_argument_bytes(), usage: wgpu::BufferUsages::INDIRECT });
        let color = target(device, "4-sample color target", COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = target(device, "4-sample depth target", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolved = target(device, "resolved color", COLOR_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let read_back = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene read-back"),
            size: u64::from(WIDTH * HEIGHT * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        errors.check("the scene's buffers and targets")?;
        Ok(SceneOnDevice {
            opaque,
            blended,
            bind_group,
            vertex_buffer,
            indirect_buffer,
            color: color.create_view(&wgpu::TextureViewDescriptor::default()),
            depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            resolved_view: resolved.create_view(&wgpu::TextureViewDescriptor::default()),
            resolved,
            read_back,
        })
    }

    /// One command buffer holding one pass over the draws given. The pass
    /// clears the targets when it is the first of the frame and loads them
    /// otherwise; it resolves the color when it is the last and keeps the
    /// samples otherwise.
    fn record(&self, device: &wgpu::Device, draws: &[Draw], first: bool, last: bool) -> wgpu::CommandBuffer {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene") });
        let [red, green, blue, alpha] = scene::CLEAR_COLOR.map(f64::from);
        let store = if last { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color,
                    depth_slice: None,
                    resolve_target: last.then_some(&self.resolved_view),
                    ops: wgpu::Operations { load: if first { wgpu::LoadOp::Clear(wgpu::Color { r: red, g: green, b: blue, a: alpha }) } else { wgpu::LoadOp::Load }, store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: if first { wgpu::LoadOp::Clear(scene::CLEAR_DEPTH) } else { wgpu::LoadOp::Load }, store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            let mut bound = None;
            for draw in draws {
                if bound != Some(draw.blended) {
                    pass.set_pipeline(if draw.blended { &self.blended } else { &self.opaque });
                    bound = Some(draw.blended);
                }
                let mut indexes = [0_u8; 8];
                indexes[..4].copy_from_slice(&draw.texture.to_ne_bytes());
                indexes[4..].copy_from_slice(&draw.sampler.to_ne_bytes());
                pass.set_immediates(0, &indexes);
                if draw.indirect {
                    pass.draw_indirect(&self.indirect_buffer, 0);
                } else {
                    pass.draw(draw.first_vertex..draw.first_vertex + draw.vertex_count, 0..1);
                }
            }
        }
        encoder.finish()
    }

    /// The resolved target copied to a buffer, mapped and read once the
    /// device has completed the copy.
    fn read_back(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Image, String> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene read-back") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.resolved, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &self.read_back, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(WIDTH * 4), rows_per_image: Some(HEIGHT) } },
            wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        let mapped = Arc::new(Mutex::new(None));
        let result = mapped.clone();
        self.read_back.slice(..).map_async(wgpu::MapMode::Read, move |outcome| {
            if let Ok(mut slot) = result.lock() {
                *slot = Some(outcome);
            }
        });
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|error| format!("the device could not be waited for: {error}"))?;
        match mapped.lock().ok().and_then(|mut slot| slot.take()) {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(format!("the read-back buffer was not mapped: {error}")),
            None => return Err("the read-back buffer was not mapped when the device had completed its work".to_string()),
        }
        let image = {
            let view = self.read_back.slice(..).get_mapped_range().map_err(|error| format!("the mapped read-back buffer could not be read: {error}"))?;
            Image::from_rows(WIDTH, HEIGHT, WIDTH as usize * 4, &view)
        };
        self.read_back.unmap();
        Ok(image)
    }

    pub fn render(&self, device: &wgpu::Device, queue: &wgpu::Queue, errors: &Errors, scene: &Scene, recorded: Recorded) -> Result<Rendered, String> {
        let microseconds = |since: Instant| since.elapsed().as_secs_f64() * 1_000_000.0;
        let started = Instant::now();
        let (recorded_us, submitted_us) = match recorded {
            Recorded::Serial => {
                let buffer = self.record(device, &scene.draws, true, true);
                let recorded_us = microseconds(started);
                queue.submit([buffer]);
                (recorded_us, microseconds(started))
            }
            Recorded::FourWorkersOneSubmitter => {
                let count = scene.chunks.len();
                let buffers: Vec<wgpu::CommandBuffer> = std::thread::scope(|threads| {
                    let workers: Vec<_> = scene.chunks.iter().enumerate().map(|(position, chunk)| threads.spawn(move || self.record(device, &scene.draws[chunk.clone()], position == 0, position + 1 == count))).collect();
                    workers.into_iter().map(|worker| worker.join().map_err(|_| "a recording worker panicked".to_string())).collect::<Result<_, _>>()
                })?;
                let recorded_us = microseconds(started);
                std::thread::scope(|threads| threads.spawn(move || queue.submit(buffers)).join().map(|_| ()).map_err(|_| "the submitting thread panicked".to_string()))?;
                (recorded_us, microseconds(started))
            }
        };
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|error| format!("the device could not be waited for: {error}"))?;
        let completed_us = microseconds(started);
        errors.check("the scene's pass")?;
        let image = self.read_back(device, queue)?;
        errors.check("the scene's read-back")?;
        Ok(Rendered { image, recorded_us, submitted_us, completed_us })
    }
}

/// What saving and loading a pipeline cache gave on one device.
pub struct CacheRoundTrip {
    /// The bytes the cache gave back after the scene's two pipelines were
    /// made through it.
    pub saved_bytes: usize,
    /// The time to make the scene's resources and pipelines through an empty
    /// cache, then through a cache created from the saved bytes.
    pub cold_us: f64,
    pub loaded_us: f64,
    /// The bytes a cache created from the saved bytes gives back.
    pub bytes_after_load: usize,
}

/// Makes the scene's pipelines through an empty cache, saves it, creates a
/// second cache from the saved bytes and makes the pipelines again.
pub fn pipeline_cache_round_trip(device: &wgpu::Device, queue: &wgpu::Queue, errors: &Errors, scene: &Scene, bindings: Bindings, shaders: &Shaders) -> Result<CacheRoundTrip, String> {
    let create = |data: Option<&[u8]>| {
        // SAFETY: the data, when given, is what `get_data` returned on this
        // same adapter and driver in this same process; `fallback` lets wgpu
        // start from an empty cache if it refuses them.
        unsafe { device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor { label: Some("scene"), data, fallback: true }) }
    };
    let cold = create(None);
    errors.check("the empty pipeline cache")?;
    let started = Instant::now();
    SceneOnDevice::create(device, queue, errors, scene, bindings, shaders, Some(&cold))?;
    let cold_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    let saved = cold.get_data().ok_or("the pipeline cache gave no data back")?;
    let loaded = create(Some(&saved));
    errors.check("the pipeline cache created from the saved bytes")?;
    let started = Instant::now();
    SceneOnDevice::create(device, queue, errors, scene, bindings, shaders, Some(&loaded))?;
    let loaded_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    Ok(CacheRoundTrip { saved_bytes: saved.len(), cold_us, loaded_us, bytes_after_load: loaded.get_data().map_or(0, |data| data.len()) })
}
