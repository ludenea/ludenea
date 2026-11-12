//! The Metal interface of the spike (macOS). The framework is opened at run
//! time and reached through the Objective-C runtime, every message sent
//! through `objc_msgSend` cast to the exact function type of its method; the
//! shaders are compiled from their sources by the system's Metal compiler.
//! No window, no layer, no drawable: the devices come from
//! `MTLCopyAllDevices` and everything is rendered to textures.
//!
//! `objc` holds the runtime, `values` the enumerations and structures of the
//! framework's headers, `gpu` the objects every exercise builds, and one
//! module stands for each exercise of the contract in `backend.rs`.
//! Throwaway, never product code.

#[macro_use]
mod objc;
mod capabilities;
mod compute;
mod gpu;
mod lit;
mod pass_through;
mod scene_pass;
mod timestamps;
mod validation;
mod values;

use std::env;
use std::fs;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use crate::backend::{Api, Capability, ComputeOutput, Device, DeviceInfo, DeviceKind, LitRender, LitVariant, Recording, SceneRender, SceneShaders, Setting, ShaderLoad};
use crate::loader::Library;
use crate::scene::{Image, Scene};

use capabilities::Families;
use gpu::Gpu;
use objc::{Id, Owned, Pool, class, class_name, flag, new_object, text, yes_no};
use timestamps::Clock;
use validation::StandardErrorCapture;
use values::SamplePosition;

const METAL_FRAMEWORK: &str = "/System/Library/Frameworks/Metal.framework/Metal";
/// Foundation holds `NSString` and `NSProcessInfo`.
const FOUNDATION_FRAMEWORK: &str = "/System/Library/Frameworks/Foundation.framework/Foundation";
/// The variable that makes the framework wrap its devices in the validation layer.
const VALIDATION_VARIABLE: &str = "MTL_DEBUG_LAYER";
/// The samples whose default positions are part of a device's settings.
const SAMPLES_OF_THE_SETTINGS: usize = 4;

/// `MTLCopyAllDevices` and `MTLCreateSystemDefaultDevice`: no argument, an
/// object the caller owns.
type DeviceFunction = unsafe extern "C" fn() -> Id;

/// The shader sources, read once when the interface is opened.
struct Sources {
    scene: String,
    compute: String,
    pass_through: String,
    lit_forward: String,
    lit_geometry: String,
    lit_resolve: String,
    atomics_64_bit: String,
}

impl Sources {
    fn read(shaders: &Path) -> Result<Sources, String> {
        let read = |file: &str| {
            let path = shaders.join(file);
            fs::read_to_string(&path).map_err(|error| format!("the shader source {} could not be read: {error}", path.display()))
        };
        Ok(Sources {
            scene: read("scene.metal")?,
            compute: read("compute.metal")?,
            pass_through: read("pass_through.metal")?,
            lit_forward: read("lit_forward.metal")?,
            lit_geometry: read("lit_geometry.metal")?,
            lit_resolve: read("lit_resolve.metal")?,
            atomics_64_bit: read("atomics_64_bit.metal")?,
        })
    }
}

struct MetalApi {
    framework: Library,
    /// Kept open for as long as its classes are used.
    _foundation: Library,
    copy_all_devices: Option<DeviceFunction>,
    create_system_default_device: DeviceFunction,
    sources: Sources,
    work: PathBuf,
    /// The shading language version the compiler takes by default, in words.
    language_version: String,
    compiler: String,
    timestamp_set: String,
}

/// A language version as the headers encode it: the major number above 16
/// bits of minor number.
fn language_version_text(version: usize) -> String {
    if version == 0 { "the compiler's default (the languageVersion property reads 0)".to_string() } else { format!("{}.{}", version >> 16, version & 0xffff) }
}

/// The shading language version the compiler takes when given no option, and
/// what identifies the compiler.
fn compiler_facts() -> Result<(String, String), String> {
    let options = new_object("MTLCompileOptions")?;
    let language_version = language_version_text(send!(usize; options.id(), "languageVersion")?);
    let fast_math = flag(options.id(), "fastMathEnabled").map_or("not answered", yes_no);
    let compiler = format!(
        "the system's Metal compiler behind newLibraryWithSource:options:error:, with its default options: Metal Shading Language {language_version}, fast math {fast_math}; {}",
        operating_system()
    );
    Ok((language_version, compiler))
}

/// The version of macOS, which is the version of the Metal implementation.
fn operating_system() -> String {
    let version = class("NSProcessInfo").and_then(|process_info| send!(Id; process_info, "processInfo")).and_then(|process| send!(Id; process, "operatingSystemVersionString"));
    match version {
        Ok(version) => format!("macOS {}", text(version)),
        Err(reason) => format!("macOS of an unknown version ({reason})"),
    }
}

/// The name of the timestamp counter set: the framework's constant
/// `MTLCommonCounterSetTimestamp`, or its documented text where the constant
/// is not exported.
fn timestamp_set_name(framework: &Library) -> String {
    let name = match framework.symbol("MTLCommonCounterSetTimestamp") {
        // SAFETY: the symbol is a constant of the framework that holds a pointer to an `NSString`.
        Ok(address) => text(unsafe { *(address as *const Id) }),
        Err(_) => String::new(),
    };
    if name.is_empty() { "timestamp".to_string() } else { name }
}

/// A device a virtual machine presents says so in its name; a low-power
/// device or one that shares the system's memory is integrated.
fn kind_of(name: &str, low_power: bool, unified_memory: bool) -> DeviceKind {
    if name.to_ascii_lowercase().contains("paravirtual") {
        DeviceKind::Virtual
    } else if low_power || unified_memory {
        DeviceKind::Integrated
    } else {
        DeviceKind::Discrete
    }
}

/// The default positions of the samples of a 4-sample pixel, where the device gives them.
fn sample_positions(device: Id) -> String {
    if !send!(u8; device, "supportsTextureSampleCount:", usize: SAMPLES_OF_THE_SETTINGS).is_ok_and(|answer| answer != 0) {
        return format!("no default sample positions: {SAMPLES_OF_THE_SETTINGS} samples are not supported");
    }
    let mut positions = [SamplePosition { x: 0.0, y: 0.0 }; SAMPLES_OF_THE_SETTINGS];
    match send!((); device, "getDefaultSamplePositions:count:", *mut SamplePosition: positions.as_mut_ptr(), usize: positions.len()) {
        Ok(()) => {
            let listed: Vec<String> = positions.iter().map(|position| format!("({:.4}, {:.4})", position.x, position.y)).collect();
            format!("default positions of {SAMPLES_OF_THE_SETTINGS} samples: {}", listed.join(" "))
        }
        Err(reason) => format!("no default sample positions: {reason}"),
    }
}

/// The variables that configure Metal among those of an environment, sorted.
fn metal_variables(environment: impl Iterator<Item = (String, String)>) -> String {
    let mut variables: Vec<String> = environment.filter(|(name, _)| name.starts_with("MTL_") || name.starts_with("METAL_")).map(|(name, value)| format!("{name}={value}")).collect();
    variables.sort();
    if variables.is_empty() { "no MTL_ or METAL_ variable in the environment".to_string() } else { variables.join(" ") }
}

fn metal_environment() -> String {
    metal_variables(env::vars_os().map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())))
}

impl MetalApi {
    /// Creates the device at `index` from scratch, with the scene's shaders
    /// in a supplied form when one is given.
    fn open_device(&self, index: usize, validation: bool, supplied: Option<SceneShaders>) -> Result<Box<dyn Device + '_>, String> {
        let _pool = Pool::push();
        let capture = if validation { Some(self.start_validation()?) } else { None };
        let devices = self.list_devices()?;
        let listed = devices.len();
        let device = devices.into_iter().nth(index).ok_or_else(|| format!("there is no Metal device at index {index}: {listed} listed"))?;
        let queue = Owned::adopt(send!(Id; device.id(), "newCommandQueue")?, "newCommandQueue")?;
        let settings = format!("{}; {}", sample_positions(device.id()), metal_environment());
        let clock = Clock::read(device.id());
        let gpu = Gpu { device, queue, timestamp_set: self.timestamp_set.clone(), clock };
        Ok(Box::new(MetalDevice { api: self, gpu: ManuallyDrop::new(gpu), settings, capture, supplied, shader_load: None }))
    }

    /// Every device the framework lists, each with a reference of its own;
    /// the system's default device where the list is empty.
    fn list_devices(&self) -> Result<Vec<Owned>, String> {
        let mut devices = Vec::new();
        if let Some(copy_all_devices) = self.copy_all_devices {
            // SAFETY: the function takes no argument and returns an array the caller owns, or nil.
            let listed = unsafe { copy_all_devices() };
            if !listed.is_null() {
                let listed = Owned::adopt(listed, "MTLCopyAllDevices")?;
                for index in 0..send!(usize; listed.id(), "count")? {
                    devices.push(Owned::retain(send!(Id; listed.id(), "objectAtIndex:", usize: index)?, "objectAtIndex: of MTLCopyAllDevices")?);
                }
            }
        }
        if devices.is_empty() {
            // SAFETY: the function takes no argument and returns a device the caller owns, or nil.
            let default_device = unsafe { (self.create_system_default_device)() };
            if !default_device.is_null() {
                devices.push(Owned::adopt(default_device, "MTLCreateSystemDefaultDevice")?);
            }
        }
        Ok(devices)
    }

    fn describe(&self, index: usize, device: Id) -> DeviceInfo {
        let name = send!(Id; device, "name").map(text).unwrap_or_else(|reason| reason);
        let low_power = flag(device, "isLowPower").unwrap_or(false);
        let unified_memory = flag(device, "hasUnifiedMemory").unwrap_or(false);
        let registry = send!(u64; device, "registryID").map_or("not answered".to_string(), |identifier| format!("{identifier:#x}"));
        DeviceInfo {
            index,
            kind: kind_of(&name, low_power, unified_memory),
            name,
            // Metal names neither a vendor nor a device by number.
            vendor_id: 0,
            device_id: 0,
            driver: format!("{}, device class {}, registry identifier {registry}", operating_system(), class_name(device)),
            interface_version: format!("Metal, {}; Metal Shading Language {}", Families::read(device).text(), self.language_version),
        }
    }

    /// With validation asked for: the reason the layer cannot be used, or the
    /// standard error captured from here on.
    fn start_validation(&self) -> Result<StandardErrorCapture, String> {
        if env::var_os(VALIDATION_VARIABLE).is_none() {
            return Err(format!("{VALIDATION_VARIABLE} is not set in the environment: the framework wraps its devices in the validation layer only in a process started with it"));
        }
        StandardErrorCapture::start(&self.work)
    }
}

impl Api for MetalApi {
    fn name(&self) -> &'static str {
        "metal"
    }

    fn loader(&self) -> String {
        self.framework.name.clone()
    }

    fn shader_compiler(&self) -> String {
        self.compiler.clone()
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        let _pool = Pool::push();
        match self.list_devices() {
            Ok(devices) => devices.iter().enumerate().map(|(index, device)| self.describe(index, device.id())).collect(),
            Err(reason) => {
                gpu::log(&format!("the devices could not be listed: {reason}"));
                Vec::new()
            }
        }
    }

    fn open(&self, index: usize, validation: bool) -> Result<Box<dyn Device + '_>, String> {
        self.open_device(index, validation, None)
    }

    fn open_with_scene_shaders(&self, index: usize, shaders: &SceneShaders) -> Result<Box<dyn Device + '_>, String> {
        self.open_device(index, false, Some(shaders.clone()))
    }

    fn software_settings(&self, _device: &DeviceInfo) -> Vec<Setting> {
        // No Metal device here is a software rasterizer with a setting to vary.
        Vec::new()
    }

    fn validation_environment(&self) -> Vec<(String, String)> {
        [(VALIDATION_VARIABLE, "1"), ("MTL_DEBUG_LAYER_ERROR_MODE", "nslog"), ("MTL_DEBUG_LAYER_WARNING_MODE", "nslog")].into_iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
    }
}

struct MetalDevice<'a> {
    api: &'a MetalApi,
    /// Released by hand, inside an autorelease pool, before the capture ends.
    gpu: ManuallyDrop<Gpu>,
    settings: String,
    /// The standard error captured for the validation layer's messages.
    capture: Option<StandardErrorCapture>,
    /// The scene's shaders in a form a toolchain made, when one was given.
    supplied: Option<SceneShaders>,
    /// What loading the scene's shaders took on the last render.
    shader_load: Option<ShaderLoad>,
}

impl Drop for MetalDevice<'_> {
    fn drop(&mut self) {
        let _pool = Pool::push();
        // SAFETY: the device and its queue are dropped here, once, and the
        // field is not used again.
        unsafe { ManuallyDrop::drop(&mut self.gpu) };
    }
}

impl Device for MetalDevice<'_> {
    fn capabilities(&mut self) -> Vec<Capability> {
        let _pool = Pool::push();
        capabilities::read(&self.gpu, &self.api.sources.atomics_64_bit)
    }

    fn settings(&self) -> String {
        self.settings.clone()
    }

    fn render_scene(&mut self, scene: &Scene, recording: Recording) -> Result<SceneRender, String> {
        let _pool = Pool::push();
        let stages = match &self.supplied {
            Some(shaders) => scene_pass::Stages::Supplied(shaders),
            None => scene_pass::Stages::Source(&self.api.sources.scene),
        };
        let (render, load) = scene_pass::render(&self.gpu, &stages, scene, recording)?;
        self.shader_load = Some(load);
        Ok(render)
    }

    fn scene_shader_load(&self) -> Option<ShaderLoad> {
        self.shader_load
    }

    fn compute(&mut self, scene: &Scene) -> Result<ComputeOutput, String> {
        let _pool = Pool::push();
        compute::run(&self.gpu, &self.api.sources.compute, scene)
    }

    fn pass_through(&mut self, scene: &Scene) -> Result<Image, String> {
        let _pool = Pool::push();
        pass_through::run(&self.gpu, &self.api.sources.pass_through, scene)
    }

    fn lit(&mut self, scene: &Scene, variant: LitVariant, repetitions: usize) -> Result<LitRender, String> {
        let _pool = Pool::push();
        let sources = lit::Sources { forward: &self.api.sources.lit_forward, geometry: &self.api.sources.lit_geometry, resolve: &self.api.sources.lit_resolve };
        lit::render(&self.gpu, &sources, scene, variant, repetitions)
    }

    fn validation_delivery(&self) -> String {
        validation::delivery(&class_name(self.gpu.device.id()), self.capture.is_some())
    }

    fn validation_messages(&mut self) -> Vec<String> {
        self.capture.as_mut().map(StandardErrorCapture::drain).unwrap_or_default()
    }

    fn misuse(&mut self) -> String {
        if self.capture.is_none() {
            return "no misuse was committed: the device was opened without the validation layer, and nothing would report it".to_string();
        }
        let _pool = Pool::push();
        validation::commit_misuse(&self.gpu).unwrap_or_else(|reason| format!("the misuse could not be committed: {reason}"))
    }
}

/// Opens the Metal framework, reads the shader sources and checks that a
/// device is listed. The error is the reason Metal is unavailable here.
pub fn open(shaders: &Path, work: &Path) -> Result<Box<dyn Api>, String> {
    let framework = Library::open(&[METAL_FRAMEWORK]).map_err(|reason| format!("the Metal framework could not be loaded: {reason}"))?;
    let foundation = Library::open(&[FOUNDATION_FRAMEWORK]).map_err(|reason| format!("the Foundation framework could not be loaded: {reason}"))?;
    objc::print_exceptions();
    // SAFETY: the framework declares the function as taking no argument and returning an object.
    let create_system_default_device: DeviceFunction = unsafe { framework.function("MTLCreateSystemDefaultDevice") }?;
    // SAFETY: as above; a framework without the function lists its default device only.
    let copy_all_devices: Option<DeviceFunction> = unsafe { framework.function("MTLCopyAllDevices") }.ok();
    let sources = Sources::read(shaders)?;

    let _pool = Pool::push();
    let (language_version, compiler) = compiler_facts().map_err(|reason| format!("the options of the Metal compiler could not be read: {reason}"))?;
    let timestamp_set = timestamp_set_name(&framework);
    let api = MetalApi { framework, _foundation: foundation, copy_all_devices, create_system_default_device, sources, work: work.to_path_buf(), language_version, compiler, timestamp_set };
    if api.list_devices()?.is_empty() {
        return Err("Metal lists no device: MTLCopyAllDevices returned none and MTLCreateSystemDefaultDevice returned nil".to_string());
    }
    Ok(Box::new(api))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_is_virtual_by_its_name_then_integrated_by_its_memory_or_power() {
        assert_eq!(kind_of("Apple Paravirtual device", false, true), DeviceKind::Virtual);
        assert_eq!(kind_of("Apple M2", false, true), DeviceKind::Integrated);
        assert_eq!(kind_of("Intel(R) UHD Graphics 630", true, false), DeviceKind::Integrated);
        assert_eq!(kind_of("AMD Radeon Pro 5500M", false, false), DeviceKind::Discrete);
    }

    #[test]
    fn a_language_version_reads_as_major_and_minor() {
        assert_eq!(language_version_text((3 << 16) + 2), "3.2");
        assert_eq!(language_version_text(2 << 16), "2.0");
        assert!(language_version_text(0).contains("default"));
    }

    #[test]
    fn the_settings_keep_the_variables_that_configure_metal_sorted() {
        let variable = |name: &str, value: &str| (name.to_string(), value.to_string());
        let environment = vec![variable("PATH", "/usr/bin"), variable("MTL_HUD_ENABLED", "0"), variable("METAL_DEVICE_WRAPPER_TYPE", "1"), variable("MTL_DEBUG_LAYER", "1")];
        assert_eq!(metal_variables(environment.into_iter()), "METAL_DEVICE_WRAPPER_TYPE=1 MTL_DEBUG_LAYER=1 MTL_HUD_ENABLED=0");
        assert!(metal_variables(std::iter::empty()).starts_with("no MTL_"));
    }
}
