//! The Vulkan interface of the device baseline spike: every physical device
//! the loader lists, opened with no window and no surface, its capabilities
//! read from its features, limits, queue families and formats, and the
//! spike's scenes rendered with Vulkan 1.0 core calls so that they run on any
//! device. The loader is opened at run time and every entry point is fetched
//! through `vkGetInstanceProcAddr` and `vkGetDeviceProcAddr`: no Vulkan
//! header, no binding crate. The SPIR-V is compiled at run time from the GLSL
//! sources under `shaders/` by the glslang command-line tool found on the
//! path. Validation uses the Khronos layer and a debug utils messenger whose
//! callback stores the messages in the process. Throwaway, never product code.

mod capabilities;
mod compute;
mod ffi;
mod filtering;
mod gpu;
mod lit;
mod pass_through;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::ptr;

use crate::backend::{Api, Capability, ComputeOutput, Device, DeviceInfo, LitRender, LitVariant, Recording, SceneRender, Setting};
use crate::loader::Library;
use crate::scene::{Image, Scene};
use ffi::*;
use gpu::{Gpu, Instance, PhysicalDevice, Scope};

const LOADER_NAMES: &[&str] = if cfg!(target_os = "windows") {
    &["vulkan-1.dll"]
} else if cfg!(target_os = "macos") {
    &["libvulkan.1.dylib", "libvulkan.dylib"]
} else {
    &["libvulkan.so.1", "libvulkan.so"]
};
const SPIRV_MAGIC: u32 = 0x0723_0203;

/// The GLSL compiler on the path, with its version line.
fn find_glsl_compiler() -> Result<(String, String), String> {
    for name in ["glslangValidator", "glslang"] {
        if let Ok(output) = Command::new(name).arg("--version").output() {
            let text = String::from_utf8_lossy(&output.stdout);
            let version = text.lines().next().unwrap_or("").trim().to_string();
            return Ok((name.to_string(), version));
        }
    }
    Err("no GLSL compiler on the path (glslangValidator or glslang)".to_string())
}

/// Compiles one GLSL stage to SPIR-V words; `defines` are passed to the
/// preprocessor and `label` names the output file.
fn compile_glsl(compiler: &str, source: &Path, work: &Path, label: &str, defines: &[&str]) -> Result<Vec<u32>, String> {
    let output_path = work.join(format!("vulkan-{}-{label}.spv", std::process::id()));
    let mut command = Command::new(compiler);
    command.arg("-V");
    for define in defines {
        command.arg(format!("-D{define}"));
    }
    let output = command.arg(source).arg("-o").arg(&output_path).output().map_err(|error| format!("{compiler}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{compiler} failed on {}: {} {}",
            source.display(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let bytes = fs::read(&output_path).map_err(|error| format!("{}: {error}", output_path.display()))?;
    let _ = fs::remove_file(&output_path);
    if bytes.len() % 4 != 0 || bytes.len() < 20 {
        return Err(format!("{} is not a SPIR-V module ({} bytes)", output_path.display(), bytes.len()));
    }
    let words: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect();
    if words[0] != SPIRV_MAGIC {
        return Err(format!("{} has no SPIR-V magic number", output_path.display()));
    }
    Ok(words)
}

/// The SPIR-V of every stage the module uses.
pub struct Shaders {
    pub scene_vertex: Vec<u32>,
    pub scene_fragment: Vec<u32>,
    pub compute: Vec<u32>,
    pub pass_through_vertex: Vec<u32>,
    pub pass_through_fragment: Vec<u32>,
    pub lit_vertex: Vec<u32>,
    pub lit_forward_fragment: Vec<u32>,
    pub lit_geometry_four_fragment: Vec<u32>,
    pub lit_geometry_five_fragment: Vec<u32>,
    pub lit_resolve_vertex: Vec<u32>,
    pub lit_resolve_fragment: Vec<u32>,
}

impl Shaders {
    fn compile(compiler: &str, shaders: &Path, work: &Path) -> Result<Shaders, String> {
        let stage = |file: &str, label: &str, defines: &[&str]| compile_glsl(compiler, &shaders.join(file), work, label, defines);
        Ok(Shaders {
            scene_vertex: stage("scene.vert", "scene-vertex", &[])?,
            scene_fragment: stage("scene.frag", "scene-fragment", &[])?,
            compute: stage("compute.comp", "compute", &[])?,
            pass_through_vertex: stage("pass_through.vert", "pass-through-vertex", &[])?,
            pass_through_fragment: stage("pass_through.frag", "pass-through-fragment", &[])?,
            lit_vertex: stage("lit_forward.vert", "lit-vertex", &[])?,
            lit_forward_fragment: stage("lit_forward.frag", "lit-forward-fragment", &[])?,
            lit_geometry_four_fragment: stage("lit_geometry.frag", "lit-geometry-four", &[])?,
            lit_geometry_five_fragment: stage("lit_geometry.frag", "lit-geometry-five", &["FIFTH_ATTACHMENT=1"])?,
            lit_resolve_vertex: stage("lit_resolve.vert", "lit-resolve-vertex", &[])?,
            lit_resolve_fragment: stage("lit_resolve.frag", "lit-resolve-fragment", &[])?,
        })
    }
}

struct VulkanApi {
    library: Library,
    compiler: String,
    shaders: Shaders,
    devices: Vec<DeviceInfo>,
}

/// Every physical device, described through an instance created for the purpose.
fn list_devices(library: &Library) -> Result<Vec<DeviceInfo>, String> {
    let instance = Instance::create(library, None)?;
    let mut devices = Vec::new();
    for (index, handle) in instance.physical_devices()?.into_iter().enumerate() {
        let physical = PhysicalDevice::describe(&instance, handle)?;
        devices.push(DeviceInfo {
            index,
            name: physical.name,
            kind: physical.kind,
            vendor_id: physical.vendor_id,
            device_id: physical.device_id,
            driver: physical.driver,
            interface_version: version_text(physical.reported_version),
        });
    }
    Ok(devices)
}

pub fn open(shaders: &Path, work: &Path) -> Result<Box<dyn Api>, String> {
    let library = Library::open(LOADER_NAMES).map_err(|error| format!("no Vulkan loader: {error}"))?;
    let (compiler, version) = find_glsl_compiler()?;
    let shaders = Shaders::compile(&compiler, shaders, work)?;
    let devices = list_devices(&library)?;
    Ok(Box::new(VulkanApi { library, compiler: format!("{compiler} ({version})"), shaders, devices }))
}

impl Api for VulkanApi {
    fn name(&self) -> &'static str {
        "vulkan"
    }

    fn loader(&self) -> String {
        self.library.name.clone()
    }

    fn shader_compiler(&self) -> String {
        self.compiler.clone()
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.clone()
    }

    fn open(&self, index: usize, validation: bool) -> Result<Box<dyn Device + '_>, String> {
        let gpu = Gpu::open(&self.library, index, validation)?;
        Ok(Box::new(VulkanDevice { gpu, shaders: &self.shaders }))
    }

    fn software_settings(&self, device: &DeviceInfo) -> Vec<Setting> {
        if !device.name.starts_with("llvmpipe") {
            return Vec::new();
        }
        let setting = |label: &str, what: &str, environment: &[(&str, &str)]| Setting {
            label: label.to_string(),
            what: what.to_string(),
            environment: environment.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect(),
            affinity_cpus: None,
        };
        // Mesa's on-disk shader cache does not key its entries on the vector
        // width: a process at 128 bits would load the code another process
        // compiled for 256-bit vectors (measured: the heap is corrupted and the
        // process aborts in most runs), so the width settings disable the cache.
        let width = |label: &str, what: &str, bits: &str| setting(label, what, &[("LP_NATIVE_VECTOR_WIDTH", bits), ("MESA_SHADER_CACHE_DISABLE", "true")]);
        vec![
            setting("one_thread", "the software rasterizer runs its work on one thread", &[("LP_NUM_THREADS", "1")]),
            setting("four_threads", "the software rasterizer runs its work on four threads", &[("LP_NUM_THREADS", "4")]),
            width("vector_width_128", "the generated code uses 128-bit vectors, compiled afresh with the shader cache disabled", "128"),
            width("vector_width_256", "the generated code uses 256-bit vectors, compiled afresh with the shader cache disabled", "256"),
        ]
    }

    fn validation_environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

struct VulkanDevice<'a> {
    gpu: Gpu,
    shaders: &'a Shaders,
}

impl VulkanDevice<'_> {
    /// Creates a sampler that enables anisotropy on a device created without
    /// the anisotropy feature, then destroys it.
    fn sampler_without_the_anisotropy_feature(&self) -> Result<(), String> {
        let mut scope = Scope::new(&self.gpu);
        let info = SamplerCreateInfo {
            s_type: structure::SAMPLER_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            mag_filter: FILTER_LINEAR,
            min_filter: FILTER_LINEAR,
            mipmap_mode: MIPMAP_MODE_LINEAR,
            address_mode_u: ADDRESS_MODE_REPEAT,
            address_mode_v: ADDRESS_MODE_REPEAT,
            address_mode_w: ADDRESS_MODE_REPEAT,
            mip_lod_bias: 0.0,
            anisotropy_enable: 1,
            max_anisotropy: 4.0,
            compare_enable: 0,
            compare_op: 0,
            min_lod: 0.0,
            max_lod: LOD_CLAMP_NONE,
            border_color: BORDER_COLOR_FLOAT_TRANSPARENT_BLACK,
            unnormalized_coordinates: 0,
        };
        scope.sampler(&info).map(|_| ())
    }
}

impl Device for VulkanDevice<'_> {
    fn capabilities(&mut self) -> Vec<Capability> {
        capabilities::report(&self.gpu)
    }

    fn settings(&self) -> String {
        let mut variables: Vec<String> = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.to_string_lossy().into_owned())))
            .filter(|(key, _)| key.starts_with("LP_") || key.starts_with("GALLIVM_") || key == "MESA_SHADER_CACHE_DISABLE")
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        variables.sort();
        let environment = if variables.is_empty() { "no LP_, GALLIVM_ or MESA_SHADER_CACHE_DISABLE variable set".to_string() } else { variables.join(" ") };
        format!("{environment}; standardSampleLocations = {}", self.gpu.physical.properties.u32_at(property::STANDARD_SAMPLE_LOCATIONS))
    }

    fn render_scene(&mut self, scene: &Scene, recording: Recording) -> Result<SceneRender, String> {
        filtering::render(&self.gpu, self.shaders, scene, recording)
    }

    fn compute(&mut self, scene: &Scene) -> Result<ComputeOutput, String> {
        compute::run(&self.gpu, self.shaders, scene)
    }

    fn pass_through(&mut self, scene: &Scene) -> Result<Image, String> {
        pass_through::run(&self.gpu, self.shaders, scene)
    }

    fn lit(&mut self, scene: &Scene, variant: LitVariant, repetitions: usize) -> Result<LitRender, String> {
        lit::render(&self.gpu, self.shaders, scene, variant, repetitions)
    }

    fn validation_delivery(&self) -> String {
        match self.gpu.sink {
            Some(_) => format!(
                "a messenger of the VK_EXT_debug_utils extension, registered by the program on the instance under {}, whose callback stores each warning and error in the process",
                gpu::VALIDATION_LAYER
            ),
            None => "validation is off on this device".to_string(),
        }
    }

    fn validation_messages(&mut self) -> Vec<String> {
        self.gpu.sink.as_ref().map(|sink| sink.drain()).unwrap_or_default()
    }

    fn misuse(&mut self) -> String {
        let what = "created a sampler with anisotropy enabled (maxAnisotropy 4) on a device created without the samplerAnisotropy feature, then destroyed it";
        match self.sampler_without_the_anisotropy_feature() {
            Ok(()) => what.to_string(),
            Err(reason) => format!("{what}; the creation returned an error: {reason}"),
        }
    }
}
