//! What each graphics interface of the spike implements and reports.
//! Throwaway, never product code.
//!
//! Three modules implement these two traits, each against one interface
//! reached through the system's loader with no third-party graphics crate:
//! `vulkan` (every platform), `d3d12` (Windows) and `metal` (macOS). Each
//! exposes `pub fn open(shaders: &Path, work: &Path) -> Result<Box<dyn Api>,
//! String>`, the error being the reason the interface is unavailable on this
//! machine (no loader, no shader compiler). The driver in `main.rs` runs the
//! same measurements through each, so the rules below are what makes three
//! implementations render one scene.
//!
//! # Conventions
//!
//! - Clip space has y up and depth 0 to 1; Vulkan's vertex shaders negate y.
//!   No face culling. Images are returned as tight RGBA rows, top row first.
//! - A device is created with no window, no surface and no swapchain.
//! - Shaders are compiled at run time from the sources under `shaders/`.
//! - Nothing is cached between two calls of `Api::open`: the second device is
//!   created from scratch.
//!
//! # The filtering and blending scene (`Device::render_scene`)
//!
//! Target: 256 by 256, one color attachment of 8-bit RGBA in the sRGB
//! encoding (linear values converted on write) and one 32-bit float depth
//! attachment, both at 4 samples; the color is resolved to a single-sample
//! image of the same format, which is read back as stored. Cleared to
//! `scene::CLEAR_COLOR` and `scene::CLEAR_DEPTH`. Viewport and scissor cover
//! the target.
//!
//! Resources: the vertex buffer `Scene::vertex_bytes` (position 4 floats, uv
//! 2 floats, color 4 floats); the five textures of `Scene::textures`, bound
//! as one array of five textures in the fragment shader, each in the 8-bit
//! RGBA format of its encoding (sRGB or plain) with every level uploaded as
//! given; three samplers, no anisotropy, no level bias, the full level range:
//! nearest (nearest in both directions, nearest level, clamp to edge), linear
//! (linear in both directions, nearest level, clamp to edge) and trilinear
//! (linear in both directions, linear between levels, repeat); the indirect
//! arguments buffer `Scene::indirect_argument_bytes`.
//!
//! Shader: the vertex shader passes the position through (y negated on
//! Vulkan), with the uv and the color interpolated with perspective. The
//! fragment shader writes `sample(textures[draw.texture],
//! samplers[draw.sampler], uv) * color`; the texture and sampler indexes are
//! per-draw constants (push constants, root constants, fragment bytes).
//!
//! Pipelines: opaque draws have blending off, depth test less-or-equal and
//! depth write on. Blended draws have depth test less-or-equal, depth write
//! off, and blending with the factors source alpha and one minus source alpha
//! for color, one and one minus source alpha for alpha, operation add. All
//! four samples are written; no alpha to coverage.
//!
//! The draws are issued in the order of `Scene::draws`; the one marked
//! `indirect` is issued as an indirect draw from the arguments buffer. The
//! three recordings of [`Recording`] must produce the same pass: one command
//! buffer; or one command buffer per chunk of `Scene::chunks`, executed in
//! the chunks' order inside the one pass (secondary command buffers on
//! Vulkan, one command list per chunk on Direct3D 12, a parallel render
//! encoder on Metal), recorded by the calling thread; or the same with each
//! chunk recorded by its own thread, the four running at once.
//!
//! Two timestamps are written around the pass where the device offers
//! timestamp queries. The read-back is asynchronous: the copy to a buffer is
//! submitted, completion is polled without blocking, then the buffer is read.
//!
//! # The compute exercise (`Device::compute`)
//!
//! One compute pass of 8 by 8 groups of 8 by 8 invocations. For the
//! invocation at (x, y), with `i = y * 64 + x` and `v = input[i]` read from a
//! storage buffer holding `Scene::compute_input`: it writes `v *
//! 2654435761 + i` (32-bit wrapping) at index `i` of a second storage buffer,
//! and `v ^ 0x9e3779b9` to the texel (x, y) of a storage texture of one
//! 32-bit unsigned integer channel. Both are read back.
//!
//! # The pass-through exercise (`Device::pass_through`)
//!
//! A source texture of 32 by 32 texels holding `Scene::pass_through_source`
//! and a target of 64 by 32, both 8-bit RGBA and both created so that they
//! can be viewed in the sRGB encoding and in the plain one. The target is
//! cleared to zero. The left half receives the source texel for pixel,
//! sampled through the plain view with the nearest filter and written
//! through the plain view, blending off: the pass-through form. The right
//! half receives the same quad sampled through the sRGB view and written
//! through the sRGB view: the converting form. One sample per pixel, no
//! depth. The target's bytes are read back as stored.
//!
//! # The lit scene (`Device::lit`)
//!
//! Target: 512 by 512 with a 32-bit float depth attachment cleared to 1,
//! depth test less-or-equal, depth write on, one sample per pixel. The
//! vertex buffer is `Scene::lit_vertex_bytes` (clip position 4 floats, world
//! position 3, normal 3, albedo 3); the vertex shader passes the clip
//! position through (y negated on Vulkan). `Scene::lit_constant_bytes` is a
//! uniform block of ten vectors: camera position, ambient color, four light
//! positions, four light colors with the intensity in alpha.
//!
//! The lighting, with N the normalized normal, P the world position and
//! `albedo` the surface color:
//!
//! ```text
//! V = normalize(camera.xyz - P)
//! color = albedo * ambient.rgb
//! for each of the four lights:
//!     L = light_position.xyz - P;  d2 = dot(L, L);  L = L / sqrt(d2)
//!     attenuation = light_color.a / (1.0 + d2)
//!     diffuse = max(dot(N, L), 0.0)
//!     H = normalize(L + V)
//!     specular = pow(max(dot(N, H), 0.0), power) * strength
//!     color += (albedo * diffuse + specular) * light_color.rgb * attenuation
//! output = (color, 1.0)
//! ```
//!
//! - `Forward`: one 16-bit float RGBA attachment cleared to (0, 0, 0, 1);
//!   the fragment shader lights with `strength` 0.25 and `power` 32.
//! - `GeometryBuffer160`: four attachments, cleared to zero, no lighting:
//!   8-bit RGBA (albedo, 1), 8-bit RGBA (N * 0.5 + 0.5, 1), 8-bit RGBA
//!   (0.25, 32 / 255, 0, 1), 16-bit float RGBA (P, 1). 160 bits per pixel.
//! - `GeometryBuffer192`: the same with a fifth attachment, 8-bit RGBA
//!   (albedo * 0.5, 1). 192 bits per pixel.
//! - `GeometryBuffer160Lit`: the fill of `GeometryBuffer160`, then a second
//!   pass that draws one triangle covering a 16-bit float RGBA attachment
//!   and, for each pixel, loads the four texels by integer coordinates and
//!   lights with N = normalize(texel * 2 - 1), `strength` and `power * 255`
//!   read from the third attachment; a pixel whose first attachment has an
//!   alpha of zero is written (0, 0, 0, 1).
//!
//! Each repetition is timed from the start of the recording to the
//! completion of the device's work, resources and pipelines being created
//! before the first; the output is read back after the last.
//!
//! # Validation (`Api::open` with `validation` set)
//!
//! The device is created under the interface's own validation layer and its
//! messages are collected for [`Device::validation_messages`]. The driver
//! runs this in a child process started with `Api::validation_environment`,
//! renders the scene, drains the messages, calls [`Device::misuse`] and
//! drains again. An error from `open` is the reason no layer could be used.

use crate::scene::{Image, Scene};

/// What stands behind a device, as the interface reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    /// A rasterizer that runs on the processor.
    Software,
    /// A device a virtual machine presents.
    Virtual,
    Integrated,
    Discrete,
    Other,
}

impl DeviceKind {
    pub fn name(self) -> &'static str {
        match self {
            DeviceKind::Software => "software",
            DeviceKind::Virtual => "virtual",
            DeviceKind::Integrated => "integrated",
            DeviceKind::Discrete => "discrete",
            DeviceKind::Other => "other",
        }
    }
}

/// One device an interface lists on this machine.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    /// Position in the interface's own enumeration, stable across processes.
    pub index: usize,
    pub name: String,
    pub kind: DeviceKind,
    pub vendor_id: u32,
    pub device_id: u32,
    /// The driver or implementation with its version, as reported.
    pub driver: String,
    /// The version or level of the interface the device offers.
    pub interface_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reported {
    Yes,
    No,
    /// The interface has no way to ask, or the device does not answer.
    NotReported,
}

impl Reported {
    pub fn name(self) -> &'static str {
        match self {
            Reported::Yes => "yes",
            Reported::No => "no",
            Reported::NotReported => "not reported",
        }
    }
}

/// One capability, by a name of [`BASELINE`] or [`BEYOND`], with the feature
/// bits, limits or formats it was read from.
#[derive(Clone, Debug)]
pub struct Capability {
    pub name: &'static str,
    pub reported: Reported,
    pub evidence: String,
}

/// The baseline every device of the boundary is assumed to meet. A device
/// reports each item once.
pub const BASELINE: [&str; 9] = [
    "several_color_attachments_and_depth",
    "multisampling_4_samples",
    "compute_passes",
    "indirect_draws",
    "storage_buffers",
    "storage_textures",
    "arrays_of_textures",
    "timestamp_queries",
    "asynchronous_read_back",
];

/// What a device may offer beyond the baseline. A device reports each item
/// once; the three compression items list the formats it decodes.
pub const BEYOND: [&str; 9] = [
    "texture_compression_bc",
    "texture_compression_etc2",
    "texture_compression_astc",
    "atomics_64_bit",
    "indirect_draw_count_from_a_buffer",
    "resource_arrays_without_a_fixed_bound",
    "mesh_shaders",
    "ray_queries",
    "second_queue",
];

/// How the commands of the filtering and blending scene are recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recording {
    SerialOneBuffer,
    ChunkedOneThread,
    ChunkedFourThreads,
}

impl Recording {
    pub const ALL: [Recording; 3] = [Recording::SerialOneBuffer, Recording::ChunkedOneThread, Recording::ChunkedFourThreads];

    pub fn name(self) -> &'static str {
        match self {
            Recording::SerialOneBuffer => "serial_one_buffer",
            Recording::ChunkedOneThread => "chunked_one_thread",
            Recording::ChunkedFourThreads => "chunked_four_threads",
        }
    }
}

pub struct SceneRender {
    pub image: Image,
    /// From the start of the recording to the last command recorded.
    pub recorded_us: f64,
    /// From the start of the recording to the return of the call that hands
    /// the commands to the device.
    pub submitted_us: f64,
    /// From the start of the recording to the completion of the device's work.
    pub render_us: f64,
    pub readback_us: f64,
    /// How many times completion was polled before the buffer could be read.
    pub readback_polls: u64,
    /// The time between the two timestamps around the pass, where offered.
    pub timestamp_ns: Option<u64>,
}

pub struct ComputeOutput {
    pub buffer: Vec<u32>,
    /// Row by row, top row first.
    pub texture: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LitVariant {
    Forward,
    GeometryBuffer160,
    GeometryBuffer192,
    GeometryBuffer160Lit,
}

impl LitVariant {
    pub const ALL: [LitVariant; 4] = [LitVariant::Forward, LitVariant::GeometryBuffer160, LitVariant::GeometryBuffer192, LitVariant::GeometryBuffer160Lit];

    pub fn name(self) -> &'static str {
        match self {
            LitVariant::Forward => "forward_one_image",
            LitVariant::GeometryBuffer160 => "geometry_buffer_160_bits_fill",
            LitVariant::GeometryBuffer192 => "geometry_buffer_192_bits_fill",
            LitVariant::GeometryBuffer160Lit => "geometry_buffer_160_bits_fill_then_light",
        }
    }

    /// The color bits written per pixel by the scene's pass.
    pub fn bits_per_pixel(self) -> u64 {
        match self {
            LitVariant::Forward => 64,
            LitVariant::GeometryBuffer160 | LitVariant::GeometryBuffer160Lit => 160,
            LitVariant::GeometryBuffer192 => 192,
        }
    }

    /// Whether the output is the lit image (16-bit float RGBA) or the first
    /// attachment of the geometry buffer (8-bit RGBA).
    pub fn output_is_lit(self) -> bool {
        matches!(self, LitVariant::Forward | LitVariant::GeometryBuffer160Lit)
    }
}

pub struct LitRender {
    /// One time per repetition.
    pub microseconds: Vec<f64>,
    /// The bytes of the output after the last repetition, tight rows, top row
    /// first: 16-bit float RGBA for a lit output, 8-bit RGBA otherwise.
    pub output: Vec<u8>,
}

/// A setting of a software device under which the scene is rendered again in
/// a child process: a thread count or a processor capability.
#[derive(Clone, Debug)]
pub struct Setting {
    /// A short name made of lowercase letters, digits and underscores.
    pub label: String,
    /// What the setting changes, in words.
    pub what: String,
    pub environment: Vec<(String, String)>,
    /// Restricts the child process to this many processors before the device
    /// is created (Windows only), for a device that sizes its threads on them.
    pub affinity_cpus: Option<u32>,
}

/// One graphics interface, loaded.
pub trait Api {
    /// `vulkan`, `direct3d12` or `metal`.
    fn name(&self) -> &'static str;
    /// The library the system's loader opened.
    fn loader(&self) -> String;
    /// The shader compiler, with its version where it gives one.
    fn shader_compiler(&self) -> String;
    /// Every device the interface lists, software devices included.
    fn devices(&self) -> Vec<DeviceInfo>;
    /// Creates the device at `index` from scratch.
    fn open(&self, index: usize, validation: bool) -> Result<Box<dyn Device + '_>, String>;
    /// The settings under which a software device is measured again; empty
    /// for a device that has none.
    fn software_settings(&self, device: &DeviceInfo) -> Vec<Setting>;
    /// The environment a process needs for the validation layer.
    fn validation_environment(&self) -> Vec<(String, String)>;
}

/// One device, open.
pub trait Device {
    /// Every item of [`BASELINE`] and of [`BEYOND`], once each.
    fn capabilities(&mut self) -> Vec<Capability>;
    /// What configures the device beyond its name and version (the settings
    /// read from the environment, the sample positions), for the reference key.
    fn settings(&self) -> String;
    fn render_scene(&mut self, scene: &Scene, recording: Recording) -> Result<SceneRender, String>;
    fn compute(&mut self, scene: &Scene) -> Result<ComputeOutput, String>;
    fn pass_through(&mut self, scene: &Scene) -> Result<Image, String>;
    fn lit(&mut self, scene: &Scene, variant: LitVariant, repetitions: usize) -> Result<LitRender, String>;
    /// How the validation layer's messages reach the program, in words.
    fn validation_delivery(&self) -> String;
    /// The messages delivered since the last call.
    fn validation_messages(&mut self) -> Vec<String>;
    /// Commits one deliberate misuse the device survives and says which.
    fn misuse(&mut self) -> String;
}
