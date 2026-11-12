//! The surface with a computed coordinate, in a raster pass against a
//! compute pass. Throwaway, never product code.
//!
//! The lit shader whose surface function scales and rotates its coordinate
//! is drawn twice over the same geometry, a textured plane seen in
//! perspective that covers the target: once by the rasterizer, the surface
//! sampling with the hardware's differentials, and once by a compute pass
//! that finds, for each pixel, the triangle, the weights of its corners and
//! their derivatives, and hands the surface function differentials computed
//! from them, as the shading of a visibility buffer does. The two images are
//! compared within a bound stated before the run. Two more fragment stages
//! say where a difference comes from and what the engine's sampling costs:
//! the same surface sampling in the raster pass with the differentials its
//! coordinate carries, and the same surface written with plain sampling.
//!
//! Every stage is the SPIR-V the spike's front end makes from the spike's
//! own sources, the lighting module included. It reaches the device through
//! wgpu at its safe level, whose translator rewrites it for the interface:
//! one path over Vulkan, Direct3D 12 and Metal, where the native path of the
//! device baseline spike has a pass for its own scene only.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use reference_workload::{Json, fnv1a};
use render_device_baseline::scene::{Image, Scene, TEXTURE_TINTED_LEVELS};
use wgpu::util::DeviceExt;

use crate::chains::{self, Stage, Tools};

/// The side of the square target, in pixels.
pub const SIDE: u32 = 256;
/// The environment variable that names a directory holding the five stages
/// as SPIR-V, read on a machine that has no front end.
pub const STAGES_FROM: &str = "SHADER_FORMS_COMPUTED_COORDINATE_STAGES";
/// How many times a timed raster pass draws the plane, and how many passes
/// of each sampling are timed.
const DRAWS_PER_TIMED_PASS: u32 = 32;
const TIMED_REPETITIONS: usize = 21;
/// The fewest distinct colors an image of the plane is taken to have: under
/// that, a device did not draw the plane and its comparison says nothing.
const PLANE_COLORS_AT_LEAST: usize = 1_000;

/// The bounded difference the test states, in the three numbers the
/// rendering proposal gives such a comparison: a largest difference per
/// channel that every pixel must meet, a second, smaller difference, and the
/// count of pixels allowed to exceed the second.
///
/// The hardware's differentials are differences between neighbouring pixels.
/// On this plane the depth changes along the target's rows, so a difference
/// across one pixel departs from the derivative at the pixel's centre by up
/// to 1.05 percent at the far edge, 0.015 of a level; two adjacent levels of
/// the texture differ by up to 191 of 255, so the two passes are expected
/// to differ by up to 3 of 255 from that cause alone. The second difference
/// is that figure rounded up, the largest twice the second, and one pixel in
/// a hundred may exceed the second.
pub struct Bound {
    pub largest: u8,
    pub second: u8,
    pub pixels_beyond_the_second: u64,
}

pub const BOUND: Bound = Bound { largest: 8, second: 4, pixels_beyond_the_second: (SIDE as u64 * SIDE as u64) / 100 };

/// One stage of the measurement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The vertex stage of the lit shader, as the shader set has it.
    Vertex,
    /// The fragment stage of the lit shader, as the shader set has it: the
    /// engine's sampling compiled for a raster pass.
    RasterHardware,
    /// The same fragment stage with the lighting module compiled to sample
    /// with the differentials the coordinate carries.
    RasterSupplied,
    /// The same surface written with plain sampling.
    RasterPlain,
    /// The compute stage, the differentials computed per pixel.
    Compute,
}

impl Part {
    pub const ALL: [Part; 5] = [Part::Vertex, Part::RasterHardware, Part::RasterSupplied, Part::RasterPlain, Part::Compute];
    const RASTER: [Part; 3] = [Part::RasterHardware, Part::RasterSupplied, Part::RasterPlain];
    const DRAWN: [Part; 4] = [Part::RasterHardware, Part::RasterSupplied, Part::RasterPlain, Part::Compute];

    pub fn name(self) -> &'static str {
        match self {
            Part::Vertex => "vertex",
            Part::RasterHardware => "raster_hardware_differentials",
            Part::RasterSupplied => "raster_supplied_differentials",
            Part::RasterPlain => "raster_plain_sampling",
            Part::Compute => "compute_computed_differentials",
        }
    }

    fn what(self) -> &'static str {
        match self {
            Part::Vertex => "the vertex stage of the lit shader, as the shader set has it",
            Part::RasterHardware => "raster pass, the engine's sampling with the hardware's differentials (the fragment stage of the shader set)",
            Part::RasterSupplied => "raster pass, the engine's sampling with the differentials the coordinate carries",
            Part::RasterPlain => "raster pass, the same surface written with plain sampling",
            Part::Compute => "compute pass, the engine's sampling with computed differentials",
        }
    }

    fn stage(self) -> Stage {
        match self {
            Part::Vertex => Stage::Vertex,
            Part::Compute => Stage::Compute,
            _ => Stage::Fragment,
        }
    }
}

/// The GLSL of a stage, composed by text as the shader set composes a lit
/// stage: the version line, the lighting module, then the parts. With
/// `supplied`, the module is compiled to sample with supplied differentials.
pub fn source(part: Part, lighting: &str, directory: &Path) -> Result<String, String> {
    let read = |file: &str| fs::read_to_string(directory.join(file)).map_err(|error| format!("{}: {error}", directory.join(file).display()));
    let (supplied, files): (bool, &[&str]) = match part {
        Part::Vertex => (false, &["surface_computed.glsl", "lit.vert.template"]),
        Part::RasterHardware => (false, &["surface_computed.glsl", "lit.frag.template"]),
        Part::RasterSupplied => (true, &["surface_computed.glsl", "lit.frag.template"]),
        Part::RasterPlain => (false, &["lit_computed_plain_sampling.frag.template"]),
        Part::Compute => (true, &["surface_computed.glsl", "lit.comp.template"]),
    };
    let parts: Vec<String> = files.iter().map(|file| read(file)).collect::<Result<_, _>>()?;
    let define = if supplied { "#define DIFFERENTIALS_ARE_SUPPLIED\n" } else { "" };
    Ok(format!("#version 450\n{define}{lighting}\n{}", parts.join("\n")))
}

/// The five stages as SPIR-V, each made or the reason it was not.
pub struct Stages {
    /// Where the stages come from: this run, or a directory named by the
    /// environment.
    pub origin: String,
    pub parts: Vec<(Part, Result<Vec<u8>, String>)>,
}

/// Makes the five stages through the front end; on a machine that has none,
/// reads them from the directory the environment names.
pub fn make(tools: &Tools, lighting: &str, directory: &Path, work: &Path) -> Stages {
    if tools.glslang.found.is_err() {
        if let Some(from) = std::env::var_os(STAGES_FROM).map(PathBuf::from) {
            let parts = Part::ALL.iter().map(|&part| (part, fs::read(from.join(format!("{}.spv", part.name()))).map_err(|error| format!("{}: {error}", from.display())))).collect();
            return Stages { origin: "made on another machine, read from a directory the environment names".to_string(), parts };
        }
    }
    let parts = Part::ALL
        .iter()
        .map(|&part| (part, source(part, lighting, directory).and_then(|text| chains::glslang(tools, &text, part.stage(), false, work, &format!("computed-{}", part.name())))))
        .collect();
    Stages { origin: "this run".to_string(), parts }
}

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_ne_bytes()).collect()
}

/// The plane: two triangles, three corners each, a corner being a position
/// in clip space (4 floats), a normal (3 floats and one of padding) and a
/// coordinate (2 floats and two of padding). The same bytes are the vertex
/// buffer of the raster pass and the corners the compute pass reads.
///
/// The four corners are a parallelogram in clip space: a rectangle that
/// recedes from a depth of 1 at the bottom of the target to a depth of 4 at
/// its top, wider than the view at every depth, so that every pixel is
/// covered, with coordinates that run from 0 to 1 across it and along it.
pub fn corners() -> [[f32; 12]; 6] {
    let normal = {
        let length = (1.5_f32 * 1.5 + 5.6 * 5.6).sqrt();
        [0.0, 1.5 / length, -5.6 / length]
    };
    let corner = |across: f32, along: f32| {
        let position = [-4.4 + 8.8 * across, -1.1 + 5.6 * along, 0.5 + 1.5 * along, 1.0 + 3.0 * along];
        [position[0], position[1], position[2], position[3], normal[0], normal[1], normal[2], 0.0, across, along, 0.0, 0.0]
    };
    [corner(0.0, 0.0), corner(1.0, 0.0), corner(1.0, 1.0), corner(0.0, 0.0), corner(1.0, 1.0), corner(0.0, 1.0)]
}

fn corner_bytes() -> Vec<u8> {
    floats(&corners().concat())
}

/// The frame's constants: the camera, then the ambient color, four light
/// positions and four light colors with the intensity in alpha.
const CAMERA: [f32; 4] = [0.0, 0.0, -2.0, 1.0];
#[rustfmt::skip]
const LIGHTS: [f32; 36] = [
    0.2, 0.2, 0.2, 1.0,
    -2.0, 1.0, -1.5, 1.0,   3.0, 3.0, -1.0, 1.0,   0.0, 0.0, 0.0, 1.0,   0.0, 0.0, 0.0, 1.0,
    1.0, 0.9, 0.8, 4.0,   0.6, 0.7, 1.0, 3.0,   0.0, 0.0, 0.0, 0.0,   0.0, 0.0, 0.0, 0.0,
];
/// The material: a white tint, no sway.
const MATERIAL: [f32; 8] = [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0];
/// The centre of a pixel in normalized device coordinates, as wgpu has
/// them on every interface: y up, the first row at the top.
const PIXEL_TO_DEVICE: [f32; 4] = [2.0 / SIDE as f32, -2.0 / SIDE as f32, -1.0, 1.0];

/// What comparing two images gave.
pub struct Compared {
    pub differing_pixels: u64,
    /// The largest difference of each channel.
    pub largest: [u8; 4],
    pub pixels_beyond_the_second: u64,
    pub mean_absolute_difference: f64,
    pub within_the_bound: bool,
}

/// Compares two images of the same size under a bound; `None` where the
/// sizes differ.
pub fn compare(a: &Image, b: &Image, bound: &Bound) -> Option<Compared> {
    if a.width != b.width || a.height != b.height || a.rgba.len() != b.rgba.len() {
        return None;
    }
    let mut compared = Compared { differing_pixels: 0, largest: [0; 4], pixels_beyond_the_second: 0, mean_absolute_difference: 0.0, within_the_bound: false };
    let mut sum = 0_u64;
    for (one, other) in a.rgba.as_chunks::<4>().0.iter().zip(b.rgba.as_chunks::<4>().0) {
        let mut largest_of_pixel = 0;
        for channel in 0..4 {
            let difference = one[channel].abs_diff(other[channel]);
            compared.largest[channel] = compared.largest[channel].max(difference);
            largest_of_pixel = largest_of_pixel.max(difference);
            sum += u64::from(difference);
        }
        compared.differing_pixels += u64::from(largest_of_pixel > 0);
        compared.pixels_beyond_the_second += u64::from(largest_of_pixel > bound.second);
    }
    compared.mean_absolute_difference = sum as f64 / a.rgba.len().max(1) as f64;
    compared.within_the_bound = compared.largest.iter().all(|&difference| difference <= bound.largest) && compared.pixels_beyond_the_second <= bound.pixels_beyond_the_second;
    Some(compared)
}

/// How many levels of the texture the sampler may read: every level, or the
/// first only, under which the differentials decide nothing and what is
/// left of a difference comes from the rest of the pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Levels {
    Every,
    FirstOnly,
}

impl Levels {
    const ALL: [Levels; 2] = [Levels::Every, Levels::FirstOnly];

    fn name(self) -> &'static str {
        match self {
            Levels::Every => "every_level",
            Levels::FirstOnly => "first_level_only",
        }
    }
}

/// Polls a future to completion on the calling thread: wgpu's futures on
/// the native interfaces are ready at once or after the device was polled.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

/// The errors wgpu reported for one device since they were last taken.
#[derive(Clone, Default)]
struct Errors(Arc<Mutex<Vec<String>>>);

impl Errors {
    fn check(&self, step: &str) -> Result<(), String> {
        let errors = self.0.lock().map(|mut errors| std::mem::take(&mut *errors)).unwrap_or_default();
        if errors.is_empty() { Ok(()) } else { Err(format!("{step}: {}", errors.join(" / "))) }
    }
}

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Everything the two passes are run with on one device.
struct OnDevice {
    device: wgpu::Device,
    queue: wgpu::Queue,
    errors: Errors,
    raster: Vec<(Part, wgpu::RenderPipeline)>,
    compute: wgpu::ComputePipeline,
    frame: wgpu::BindGroup,
    materials: Vec<(Levels, wgpu::BindGroup)>,
    compute_pass: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    lit_image: wgpu::Texture,
    read_back: wgpu::Buffer,
}

fn spirv_words(bytes: &[u8]) -> Result<Vec<u32>, String> {
    let words: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect();
    if bytes.len() % 4 != 0 || words.first() != Some(&0x0723_0203) {
        return Err("not a SPIR-V module".to_string());
    }
    Ok(words)
}

fn wait(device: &wgpu::Device) -> Result<(), String> {
    device.poll(wgpu::PollType::wait_indefinitely()).map(|_| ()).map_err(|error| format!("the device could not be waited for: {error}"))
}

impl OnDevice {
    fn create(adapter: &wgpu::Adapter, stages: &[(Part, Vec<u8>)]) -> Result<OnDevice, String> {
        let descriptor = wgpu::DeviceDescriptor { label: Some("computed coordinate"), ..Default::default() };
        let (device, queue) = block_on(adapter.request_device(&descriptor)).map_err(|error| format!("the device was not created: {error}"))?;
        let errors = Errors::default();
        let sink = errors.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            if let Ok(mut errors) = sink.0.lock() {
                errors.push(error.to_string().split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }));

        let every_stage = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
        let constants = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("frame"), entries: &[constants(0, every_stage), constants(1, every_stage)] });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: every_stage,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: every_stage, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                constants(2, every_stage),
            ],
        });
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compute pass"),
            entries: &[
                constants(0, wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: TARGET_FORMAT, view_dimension: wgpu::TextureViewDimension::D2 },
                    count: None,
                },
            ],
        });
        let raster_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("raster pass"), bind_group_layouts: &[Some(&frame_layout), Some(&material_layout)], immediate_size: 0 });
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("compute pass"), bind_group_layouts: &[Some(&frame_layout), Some(&material_layout), Some(&compute_layout)], immediate_size: 0 });
        errors.check("the layouts")?;

        let module = |part: Part| -> Result<wgpu::ShaderModule, String> {
            let bytes = stages.iter().find(|(known, _)| *known == part).map(|(_, bytes)| bytes).ok_or_else(|| format!("no SPIR-V of {}", part.name()))?;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(part.name()), source: wgpu::ShaderSource::SpirV(Cow::Owned(spirv_words(bytes)?)) });
            errors.check(&format!("the shader module of {}", part.name()))?;
            Ok(module)
        };
        let vertex = module(Part::Vertex)?;
        let attributes = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 32, shader_location: 2 },
        ];
        let mut raster = Vec::new();
        for part in Part::RASTER {
            let fragment = module(part)?;
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(part.name()),
                layout: Some(&raster_layout),
                vertex: wgpu::VertexState {
                    module: &vertex,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
                },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &fragment,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: TARGET_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            });
            errors.check(&format!("the pipeline of {}", part.name()))?;
            raster.push((part, pipeline));
        }
        let compute_module = module(Part::Compute)?;
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(Part::Compute.name()),
            layout: Some(&compute_pipeline_layout),
            module: &compute_module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        errors.check(&format!("the pipeline of {}", Part::Compute.name()))?;

        let scene = Scene::build();
        let texture = &scene.textures[TEXTURE_TINTED_LEVELS as usize];
        let sampled = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(texture.name),
            size: wgpu::Extent3d { width: texture.width, height: texture.height, depth_or_array_layers: 1 },
            mip_level_count: texture.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, texels) in texture.levels.iter().enumerate() {
            let (width, height) = ((texture.width >> level).max(1), (texture.height >> level).max(1));
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &sampled, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                texels,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
        }
        let sampled_view = sampled.create_view(&wgpu::TextureViewDescriptor::default());
        // Linear in both directions and between levels, repeated, with no
        // anisotropy and no level bias; the second sampler stops at the
        // first level.
        let sampler = |label: &str, last_level: f32| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                lod_min_clamp: 0.0,
                lod_max_clamp: last_level,
                compare: None,
                anisotropy_clamp: 1,
                border_color: None,
            })
        };
        let buffer = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage });
        let view_constants = buffer("view", &floats(&CAMERA), wgpu::BufferUsages::UNIFORM);
        let light_constants = buffer("lights", &floats(&LIGHTS), wgpu::BufferUsages::UNIFORM);
        let material_constants = buffer("material", &floats(&MATERIAL), wgpu::BufferUsages::UNIFORM);
        let mut geometry = floats(&PIXEL_TO_DEVICE);
        geometry.extend(corner_bytes());
        let geometry_constants = buffer("geometry", &geometry, wgpu::BufferUsages::UNIFORM);
        let vertices = buffer("vertices", &corner_bytes(), wgpu::BufferUsages::VERTEX);
        let frame = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: view_constants.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: light_constants.as_entire_binding() }],
        });
        let materials = [(Levels::Every, 32.0), (Levels::FirstOnly, 0.0)]
            .into_iter()
            .map(|(levels, last_level)| {
                let filtering = sampler(levels.name(), last_level);
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(levels.name()),
                    layout: &material_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&sampled_view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&filtering) },
                        wgpu::BindGroupEntry { binding: 2, resource: material_constants.as_entire_binding() },
                    ],
                });
                (levels, group)
            })
            .collect();
        let image = |label: &str, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let target = image("raster target", wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let lit_image = image("compute image", wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC);
        let lit_view = lit_image.create_view(&wgpu::TextureViewDescriptor::default());
        let compute_pass = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compute pass"),
            layout: &compute_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: geometry_constants.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&lit_view) }],
        });
        let read_back = device.create_buffer(&wgpu::BufferDescriptor { label: Some("read-back"), size: u64::from(SIDE * SIDE * 4), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        errors.check("the textures, samplers, buffers and bind groups")?;
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(OnDevice { device, queue, errors, raster, compute, frame, materials, compute_pass, vertices, target, target_view, lit_image, read_back })
    }

    fn material(&self, levels: Levels) -> Result<&wgpu::BindGroup, String> {
        self.materials.iter().find(|(known, _)| *known == levels).map(|(_, group)| group).ok_or_else(|| format!("no material for {}", levels.name()))
    }

    /// One command buffer holding the raster pass of one fragment stage,
    /// which draws the plane `draws` times.
    fn record_raster(&self, part: Part, levels: Levels, draws: u32) -> Result<wgpu::CommandBuffer, String> {
        let pipeline = self.raster.iter().find(|(known, _)| *known == part).map(|(_, pipeline)| pipeline).ok_or_else(|| format!("no pipeline of {}", part.name()))?;
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(part.name()) });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(part.name()),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.frame, &[]);
            pass.set_bind_group(1, self.material(levels)?, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.draw(0..6, 0..draws);
        }
        Ok(encoder.finish())
    }

    fn read(&self, texture: &wgpu::Texture) -> Result<Image, String> {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("read-back") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &self.read_back, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIDE * 4), rows_per_image: Some(SIDE) } },
            wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let mapped = Arc::new(Mutex::new(None));
        let result = mapped.clone();
        self.read_back.slice(..).map_async(wgpu::MapMode::Read, move |outcome| {
            if let Ok(mut slot) = result.lock() {
                *slot = Some(outcome);
            }
        });
        wait(&self.device)?;
        match mapped.lock().ok().and_then(|mut slot| slot.take()) {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(format!("the read-back buffer was not mapped: {error}")),
            None => return Err("the read-back buffer was not mapped when the device had completed its work".to_string()),
        }
        let image = {
            let view = self.read_back.slice(..).get_mapped_range().map_err(|error| format!("the mapped read-back buffer could not be read: {error}"))?;
            Image::from_rows(SIDE, SIDE, SIDE as usize * 4, &view)
        };
        self.read_back.unmap();
        self.errors.check("the read-back")?;
        Ok(image)
    }

    /// The image one stage gives: a raster pass that draws the plane once,
    /// or the compute pass.
    fn image(&self, part: Part, levels: Levels) -> Result<Image, String> {
        if part == Part::Compute {
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(part.name()) });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(part.name()), timestamp_writes: None });
                pass.set_pipeline(&self.compute);
                pass.set_bind_group(0, &self.frame, &[]);
                pass.set_bind_group(1, self.material(levels)?, &[]);
                pass.set_bind_group(2, &self.compute_pass, &[]);
                pass.dispatch_workgroups(SIDE / 8, SIDE / 8, 1);
            }
            self.queue.submit([encoder.finish()]);
            wait(&self.device)?;
            self.errors.check("the compute pass")?;
            self.read(&self.lit_image)
        } else {
            self.queue.submit([self.record_raster(part, levels, 1)?]);
            wait(&self.device)?;
            self.errors.check(&format!("the raster pass of {}", part.name()))?;
            self.read(&self.target)
        }
    }

    /// The time, in microseconds, from the submission of a raster pass that
    /// draws the plane several times to the completion of the device's work.
    fn timed(&self, part: Part) -> Result<f64, String> {
        let buffer = self.record_raster(part, Levels::Every, DRAWS_PER_TIMED_PASS)?;
        let started = Instant::now();
        self.queue.submit([buffer]);
        wait(&self.device)?;
        let microseconds = started.elapsed().as_secs_f64() * 1_000_000.0;
        self.errors.check(&format!("the timed raster pass of {}", part.name()))?;
        Ok(microseconds)
    }
}

/// What one device gave: the image of each stage under each sampler, and
/// the fastest and median time of each raster stage.
struct Measured {
    images: Vec<(Levels, Part, Image)>,
    times: Vec<(Part, f64, f64)>,
}

fn measure(adapter: &wgpu::Adapter, stages: &[(Part, Vec<u8>)]) -> Result<Measured, String> {
    let on_device = OnDevice::create(adapter, stages)?;
    let mut images = Vec::new();
    for levels in Levels::ALL {
        for part in Part::DRAWN {
            images.push((levels, part, on_device.image(part, levels)?));
        }
    }
    // The three samplings in turn, so that a drift of the machine falls on
    // each alike; the first round is not kept.
    let mut times: Vec<(Part, Vec<f64>)> = Part::RASTER.iter().map(|&part| (part, Vec::new())).collect();
    for round in 0..=TIMED_REPETITIONS {
        for (part, kept) in &mut times {
            let microseconds = on_device.timed(*part)?;
            if round > 0 {
                kept.push(microseconds);
            }
        }
    }
    let times = times
        .into_iter()
        .map(|(part, mut kept)| {
            kept.sort_by(f64::total_cmp);
            (part, kept[0], kept[kept.len() / 2])
        })
        .collect();
    Ok(Measured { images, times })
}

/// The pairs of stages compared, the first being the hypothesis.
const PAIRS: [(Part, Part); 4] = [(Part::RasterHardware, Part::Compute), (Part::RasterHardware, Part::RasterSupplied), (Part::RasterSupplied, Part::Compute), (Part::RasterHardware, Part::RasterPlain)];

fn device_type_name(kind: wgpu::DeviceType) -> &'static str {
    match kind {
        wgpu::DeviceType::Other => "other",
        wgpu::DeviceType::IntegratedGpu => "integrated",
        wgpu::DeviceType::DiscreteGpu => "discrete",
        wgpu::DeviceType::VirtualGpu => "virtual",
        wgpu::DeviceType::Cpu => "software",
    }
}

fn instance(backend: wgpu::Backend, dxc: Option<&Path>) -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::from(backend);
    descriptor.flags = wgpu::InstanceFlags::empty();
    descriptor.backend_options.dx12.shader_compiler = match dxc {
        Some(library) => wgpu::Dx12Compiler::DynamicDxc { dxc_path: library.to_string_lossy().into_owned() },
        None => wgpu::Dx12Compiler::Fxc,
    };
    wgpu::Instance::new(descriptor)
}

/// The adapters of one interface with how the shaders reach its devices.
/// On Direct3D 12 wgpu compiles the HLSL its translator writes with the
/// DirectX shader compiler where the setup fetched its library, with the
/// system's older compiler otherwise.
fn adapters(name: &str, backend: wgpu::Backend, tools: &Tools) -> (Vec<wgpu::Adapter>, String) {
    let list = |instance: &wgpu::Instance| block_on(instance.enumerate_adapters(wgpu::Backends::from(backend)));
    match name {
        "direct3d12" => {
            let library = tools.dxc.found.as_ref().ok().and_then(|tool| tool.parent()).map(|directory| directory.join("dxcompiler.dll")).filter(|library| library.is_file());
            if let Some(library) = library {
                let found = list(&instance(backend, Some(&library)));
                if !found.is_empty() {
                    return (found, "HLSL that wgpu's translator writes from the front end's SPIR-V, compiled at load by the DirectX shader compiler".to_string());
                }
            }
            (list(&instance(backend, None)), "HLSL that wgpu's translator writes from the front end's SPIR-V, compiled at load by the system's older compiler (d3dcompiler_47.dll)".to_string())
        }
        "metal" => (list(&instance(backend, None)), "Metal text that wgpu's translator writes from the front end's SPIR-V, compiled at load by the platform".to_string()),
        _ => (list(&instance(backend, None)), "SPIR-V that wgpu's translator writes again from the front end's SPIR-V".to_string()),
    }
}

fn write_device(out: &mut Json, measured: &Measured, images_directory: &Path, label: &str) {
    out.key("images");
    out.array_start();
    for (levels, part, image) in &measured.images {
        let texels = image.rgba.as_chunks::<4>().0;
        out.object_start();
        out.string("levels", levels.name());
        out.string("pass", part.name());
        out.hex("hash", image.hash());
        out.u64("distinct_colors", texels.iter().collect::<HashSet<_>>().len() as u64);
        out.u64("pixels_not_covered", texels.iter().filter(|texel| texel[3] == 0).count() as u64);
        out.object_end();
        // The two images of the hypothesis are published for the eye.
        if *levels == Levels::Every && (*part == Part::RasterHardware || *part == Part::Compute) && fs::create_dir_all(images_directory).is_ok() {
            let _ = fs::write(images_directory.join(format!("{label}-{}.png", part.name())), image.png());
        }
    }
    out.array_end();

    let image = |levels: Levels, part: Part| measured.images.iter().find(|(known_levels, known_part, _)| *known_levels == levels && *known_part == part).map(|(_, _, image)| image);
    // An agreement between two images that do not show the plane would say
    // nothing: the plane lit and textured has thousands of colors.
    let shows_the_plane = |part: Part| image(Levels::Every, part).is_some_and(|image| image.rgba.as_chunks::<4>().0.iter().collect::<HashSet<_>>().len() >= PLANE_COLORS_AT_LEAST);
    out.bool("both_images_show_the_plane", shows_the_plane(Part::RasterHardware) && shows_the_plane(Part::Compute));
    let mut holds = None;
    out.key("comparisons");
    out.array_start();
    for levels in Levels::ALL {
        for (one, other) in PAIRS {
            let (Some(a), Some(b)) = (image(levels, one), image(levels, other)) else { continue };
            let Some(compared) = compare(a, b, &BOUND) else { continue };
            out.object_start();
            out.string("levels", levels.name());
            out.string("of", one.name());
            out.string("against", other.name());
            out.bool("bytes_equal", compared.differing_pixels == 0);
            out.u64("differing_pixels", compared.differing_pixels);
            out.u64("largest_difference", u64::from(compared.largest.iter().copied().max().unwrap_or(0)));
            out.string("largest_difference_per_channel", &format!("{:?}", compared.largest));
            out.u64("pixels_beyond_the_second_difference", compared.pixels_beyond_the_second);
            out.decimal("mean_absolute_difference", compared.mean_absolute_difference);
            out.bool("within_the_bound", compared.within_the_bound);
            out.object_end();
            if levels == Levels::Every && (one, other) == PAIRS[0] {
                holds = Some(compared.within_the_bound);
            }
        }
    }
    out.array_end();
    match holds {
        Some(holds) => out.bool("the_two_passes_agree_within_the_bound", holds),
        None => out.string("the_two_passes_agree_within_the_bound", "not compared"),
    }

    out.key("raster_pass_times");
    out.array_start();
    for (part, fastest, median) in &measured.times {
        out.object_start();
        out.string("sampling", part.name());
        out.decimal("fastest_us", *fastest);
        out.decimal("median_us", *median);
        out.object_end();
    }
    out.array_end();
}

/// Runs the measurement on every device of every interface wgpu reaches on
/// this machine and reports it under one key.
pub fn write(out: &mut Json, stages: &Stages, tools: &Tools, output: &Path) {
    let directory = output.join("computed-coordinate");
    out.key("the_computed_coordinate");
    out.object_start();
    out.u64("target_side", u64::from(SIDE));
    out.key("bound");
    out.object_start();
    out.u64("largest_difference_per_channel", u64::from(BOUND.largest));
    out.u64("second_difference", u64::from(BOUND.second));
    out.u64("pixels_allowed_beyond_the_second", BOUND.pixels_beyond_the_second);
    out.object_end();
    out.u64("draws_per_timed_pass", u64::from(DRAWS_PER_TIMED_PASS));
    out.u64("timed_repetitions", TIMED_REPETITIONS as u64);
    out.string("stages_from", &stages.origin);

    let mut made = Vec::new();
    out.key("stages");
    out.array_start();
    for (part, outcome) in &stages.parts {
        out.object_start();
        out.string("stage", part.name());
        out.string("what", part.what());
        match outcome {
            Ok(bytes) => {
                out.bool("made", true);
                out.u64("bytes", bytes.len() as u64);
                out.hex("hash", fnv1a(bytes));
                made.push((*part, bytes.clone()));
                if fs::create_dir_all(&directory).is_ok() {
                    let _ = fs::write(directory.join(format!("{}.spv", part.name())), bytes);
                }
            }
            Err(reason) => {
                out.bool("made", false);
                out.string("reason", &reason.chars().take(700).collect::<String>());
            }
        }
        out.object_end();
    }
    out.array_end();

    out.key("interfaces");
    out.array_start();
    if made.len() == stages.parts.len() {
        for (name, backend) in [("vulkan", wgpu::Backend::Vulkan), ("direct3d12", wgpu::Backend::Dx12), ("metal", wgpu::Backend::Metal)] {
            let (found, reaches) = adapters(name, backend, tools);
            out.object_start();
            out.string("interface", name);
            out.string("the_shaders_reach_the_device_as", &reaches);
            out.key("devices");
            out.array_start();
            let mut seen = Vec::new();
            for (index, adapter) in found.iter().enumerate() {
                let info = adapter.get_info();
                // A device an interface lists twice is measured once.
                if seen.contains(&(info.vendor, info.device, info.name.clone())) {
                    continue;
                }
                seen.push((info.vendor, info.device, info.name.clone()));
                out.object_start();
                out.string("name", &info.name);
                out.string("kind", device_type_name(info.device_type));
                out.string("driver", &format!("{} {}", info.driver, info.driver_info).trim().to_string());
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| measure(adapter, &made))).unwrap_or_else(|panic| {
                    let message = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string())).unwrap_or_default();
                    Err(format!("the measurement panicked: {message}"))
                });
                match outcome {
                    Ok(measured) => {
                        out.bool("measured", true);
                        write_device(out, &measured, &directory, &format!("{name}-{index}"));
                    }
                    Err(reason) => {
                        out.bool("measured", false);
                        out.string("reason", &reason.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(900).collect::<String>());
                    }
                }
                out.object_end();
            }
            out.array_end();
            out.object_end();
        }
    }
    out.array_end();
    out.object_end();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders").join("glsl")
    }

    #[test]
    fn a_stage_is_composed_for_the_kind_of_pass_it_serves() {
        let lighting = fs::read_to_string(directory().join("lighting.glsl")).unwrap();
        for part in Part::ALL {
            let text = source(part, &lighting, &directory()).unwrap();
            assert!(text.starts_with("#version 450\n"), "{}", part.name());
            let supplied = matches!(part, Part::RasterSupplied | Part::Compute);
            assert_eq!(text.contains("#define DIFFERENTIALS_ARE_SUPPLIED"), supplied, "{}", part.name());
            assert!(text.contains("vec3 light_surface("), "{}", part.name());
        }
        // The module samples with the hardware's differentials unless it is
        // compiled for supplied ones, and the plain surface has no
        // coordinate of the engine's type.
        assert!(lighting.contains("#ifdef DIFFERENTIALS_ARE_SUPPLIED\n    return textureGrad(sampler2D(image, filtering), c.value, c.ddx, c.ddy);\n#else\n    return texture(sampler2D(image, filtering), c.value);\n#endif"));
        let plain = fs::read_to_string(directory().join("lit_computed_plain_sampling.frag.template")).unwrap();
        assert!(!plain.contains("Coordinate") && !plain.contains("engine_sample"));
        assert!(source(Part::Compute, &lighting, &directory()).unwrap().contains("imageStore(lit_image"));
    }

    #[test]
    fn the_plane_is_a_parallelogram_that_covers_the_target() {
        let corners = corners();
        // Two triangles over four corners: near left, near right, far right, far left.
        assert_eq!(corners[0], corners[3]);
        assert_eq!(corners[2], corners[4]);
        let (near_left, near_right, far_right, far_left) = (corners[0], corners[1], corners[2], corners[5]);
        for index in 0..12 {
            // Opposite sides are equal, positions and coordinates alike, so
            // both triangles interpolate one function.
            assert!((near_left[index] + far_right[index] - near_right[index] - far_left[index]).abs() < 1e-5, "component {index}");
        }
        for corner in [near_left, near_right, far_right, far_left] {
            assert!(corner[3] > 0.0, "a corner behind the eye would need clipping in the compute pass");
            assert!((corner[4] * corner[4] + corner[5] * corner[5] + corner[6] * corner[6] - 1.0).abs() < 1e-5);
        }
        // After the division the near edge is below the target and the far
        // edge above it, and both are wider than it.
        let device = |corner: [f32; 12]| (corner[0] / corner[3], corner[1] / corner[3]);
        assert!(device(near_left).0 <= -1.0 && device(near_right).0 >= 1.0 && device(near_left).1 <= -1.0);
        assert!(device(far_left).0 <= -1.0 && device(far_right).0 >= 1.0 && device(far_left).1 >= 1.0);
        assert_eq!(corner_bytes().len(), 6 * 48);
    }

    #[test]
    fn two_images_are_compared_under_the_three_numbers_of_a_bound() {
        let image = |rgba: Vec<u8>| Image { width: 2, height: 2, rgba };
        let base = image(vec![10; 16]);
        let bound = Bound { largest: 8, second: 4, pixels_beyond_the_second: 1 };
        let same = compare(&base, &base, &bound).unwrap();
        assert!(same.within_the_bound && same.differing_pixels == 0 && same.mean_absolute_difference == 0.0);
        // One pixel beyond the second difference and within the largest.
        let mut bytes = vec![10; 16];
        bytes[1] = 17;
        bytes[4] = 12;
        let near = compare(&base, &image(bytes.clone()), &bound).unwrap();
        assert_eq!((near.differing_pixels, near.pixels_beyond_the_second, near.largest), (2, 1, [2, 7, 0, 0]));
        assert!(near.within_the_bound);
        // A second pixel beyond the second difference, then one beyond the largest.
        bytes[8] = 15;
        assert!(!compare(&base, &image(bytes.clone()), &bound).unwrap().within_the_bound);
        bytes[8] = 10;
        bytes[1] = 19;
        assert!(!compare(&base, &image(bytes), &bound).unwrap().within_the_bound);
        assert!(compare(&base, &Image { width: 1, height: 1, rgba: vec![0; 4] }, &bound).is_none());
        assert!(spirv_words(&[3, 2, 35, 7, 0, 0, 1, 0]).is_ok() && spirv_words(&[0; 8]).is_err() && spirv_words(&[3, 2, 35]).is_err());
    }
}
