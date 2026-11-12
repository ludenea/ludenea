//! The Direct3D 12 interface of the device baseline spike (Windows): every
//! DXGI adapter, the software one included, opened as a Direct3D 12 device
//! with no window, and the spike's scenes rendered on it.
//!
//! `d3d12.dll`, `dxgi.dll` and `d3dcompiler_47.dll` are opened at run time
//! through the loader, never linked. The COM interfaces are called through
//! their virtual tables by slot index, with the structures declared in
//! `ffi.rs` in the field order of the headers. The HLSL sources under
//! `shaders/` are compiled at run time by `D3DCompile` to the 5.1 targets.
//!
//! Throwaway, never product code.

mod capabilities;
mod device;
mod exercises;
mod ffi;
mod filtering;
mod gpu;
mod lit;
mod shaders;
mod validation;

use std::cell::OnceCell;
use std::path::Path;
use std::ptr;

use crate::backend::{Api, Device, DeviceInfo, DeviceKind, SceneShaders, Setting};
use crate::loader::Library;
use ffi::*;
use gpu::Gpu;
use shaders::Shaders;

/// The functions the module calls from the three libraries.
#[derive(Clone, Copy)]
struct Functions {
    create_device: CreateDevice,
    serialize_root_signature: SerializeRootSignature,
    get_debug_interface: GetDebugInterface,
}

/// One DXGI adapter as enumerated, with what DXGI says of it.
struct Adapter {
    name: String,
    vendor_id: u32,
    device_id: u32,
    software: bool,
    driver: String,
    adapter: Com,
}

struct Direct3d12 {
    adapters: Vec<Adapter>,
    /// Filled on the first call of `devices`: each device is created and
    /// released to read its kind and levels, which is kept out of the child
    /// processes that never list the devices (the validation child must
    /// enable the debug layer before any device exists).
    devices: OnceCell<Vec<DeviceInfo>>,
    functions: Functions,
    shaders: Shaders,
    compiler_name: String,
    // The libraries are released last, after every object they created.
    _compiler: Library,
    _dxgi: Library,
    d3d12: Library,
}

pub fn open(shaders: &Path, _work: &Path) -> Result<Box<dyn Api>, String> {
    let d3d12 = Library::open(&["d3d12.dll"])?;
    let dxgi = Library::open(&["dxgi.dll"])?;
    let compiler = Library::open(&["d3dcompiler_47.dll"])?;
    // SAFETY: the libraries export these functions with these signatures.
    let (functions, create_factory, compile) = unsafe {
        let functions = Functions {
            create_device: d3d12.function("D3D12CreateDevice")?,
            serialize_root_signature: d3d12.function("D3D12SerializeRootSignature")?,
            get_debug_interface: d3d12.function("D3D12GetDebugInterface")?,
        };
        (functions, dxgi.function::<CreateDxgiFactory1>("CreateDXGIFactory1")?, compiler.function::<Compile>("D3DCompile")?)
    };
    let shaders = Shaders::compile(compile, shaders)?;
    let adapters = enumerate_adapters(create_factory)?;
    let compiler_name = format!("{} (D3DCompile, shader model 5.1 targets)", compiler.name);
    Ok(Box::new(Direct3d12 { adapters, devices: OnceCell::new(), functions, shaders, compiler_name, _compiler: compiler, _dxgi: dxgi, d3d12 }))
}

fn enumerate_adapters(create_factory: CreateDxgiFactory1) -> Result<Vec<Adapter>, String> {
    let mut factory: Object = ptr::null_mut();
    // SAFETY: the function is CreateDXGIFactory1; it writes an IDXGIFactory1.
    let result = unsafe { create_factory(&IID_IDXGI_FACTORY1, &mut factory) };
    let factory = created(result, factory, "CreateDXGIFactory1")?;
    let mut adapters = Vec::new();
    for index in 0_u32.. {
        let mut adapter: Object = ptr::null_mut();
        // SAFETY: the factory is live; the call writes an IDXGIAdapter1.
        let result = unsafe { com!(factory.raw(), slot::factory::ENUM_ADAPTERS1, fn(u32, *mut Object) -> Hresult, index, &mut adapter) };
        if result == DXGI_ERROR_NOT_FOUND {
            break;
        }
        let adapter = created(result, adapter, &format!("IDXGIFactory1::EnumAdapters1({index})"))?;
        adapters.push(describe_adapter(adapter)?);
    }
    Ok(adapters)
}

fn describe_adapter(adapter: Com) -> Result<Adapter, String> {
    // SAFETY: the structure is plain integers, for which zero is valid.
    let mut desc: AdapterDesc1 = unsafe { std::mem::zeroed() };
    // SAFETY: the adapter is a live IDXGIAdapter1; the call fills the structure.
    check(unsafe { com!(adapter.raw(), slot::adapter::GET_DESC1, fn(*mut AdapterDesc1) -> Hresult, &mut desc) }, "IDXGIAdapter1::GetDesc1")?;
    let end = desc.description.iter().position(|&unit| unit == 0).unwrap_or(desc.description.len());
    let mut version = 0_i64;
    // SAFETY: the adapter is live; the call writes the user-mode driver
    // version as one 64-bit value.
    let result = unsafe { com!(adapter.raw(), slot::adapter::CHECK_INTERFACE_SUPPORT, fn(*const Guid, *mut i64) -> Hresult, &IID_IDXGI_DEVICE, &mut version) };
    let driver = if result >= 0 {
        let part = |shift: u32| (version >> shift) & 0xffff;
        format!("user-mode driver {}.{}.{}.{}", part(48), part(32), part(16), part(0))
    } else {
        format!("no user-mode driver version: IDXGIAdapter::CheckInterfaceSupport failed with HRESULT {:#010x}", result as u32)
    };
    Ok(Adapter {
        name: String::from_utf16_lossy(&desc.description[..end]),
        vendor_id: desc.vendor_id,
        device_id: desc.device_id,
        // DXGI may list the Basic Render Driver a second time without the
        // software flag (observed on a Windows 11 machine with two hardware
        // adapters); its identifiers still name the processor's rasterizer.
        software: desc.flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0 || (desc.vendor_id, desc.device_id) == BASIC_RENDER_DRIVER_IDENTIFIERS,
        driver,
        adapter,
    })
}

impl Direct3d12 {
    fn create_gpu(&self, adapter: &Adapter) -> Result<Gpu, String> {
        Gpu::create(self.functions.create_device, self.functions.serialize_root_signature, adapter.adapter.raw())
    }

    /// The kind and the interface version of an adapter, read from a device
    /// created for the purpose and released at once.
    fn device_info(&self, index: usize, adapter: &Adapter) -> DeviceInfo {
        let (kind, interface_version) = match self.create_gpu(adapter) {
            Ok(gpu) => {
                let kind = match (adapter.software, capabilities::unified_memory(&gpu)) {
                    (true, _) => DeviceKind::Software,
                    (false, Ok(true)) => DeviceKind::Integrated,
                    (false, Ok(false)) => DeviceKind::Discrete,
                    (false, Err(_)) => DeviceKind::Other,
                };
                let level = capabilities::feature_level(&gpu).map_or_else(|error| error, |level| format!("feature level {}", capabilities::level_name(level)));
                let model = capabilities::shader_model(&gpu).map_or_else(|error| error, |model| format!("shader model {}", capabilities::model_name(model)));
                (kind, format!("{level}, {model}"))
            }
            Err(error) => (if adapter.software { DeviceKind::Software } else { DeviceKind::Other }, format!("no device: {error}")),
        };
        DeviceInfo { index, name: adapter.name.clone(), kind, vendor_id: adapter.vendor_id, device_id: adapter.device_id, driver: adapter.driver.clone(), interface_version }
    }
}

impl Api for Direct3d12 {
    fn name(&self) -> &'static str {
        "direct3d12"
    }

    fn loader(&self) -> String {
        format!("{} with dxgi.dll", self.d3d12.name)
    }

    fn shader_compiler(&self) -> String {
        self.compiler_name.clone()
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.get_or_init(|| self.adapters.iter().enumerate().map(|(index, adapter)| self.device_info(index, adapter)).collect()).clone()
    }

    fn open(&self, index: usize, validation: bool) -> Result<Box<dyn Device + '_>, String> {
        let adapter = self.adapters.get(index).ok_or_else(|| format!("no DXGI adapter at index {index} ({} enumerated)", self.adapters.len()))?;
        if validation {
            validation::enable_layer(self.functions.get_debug_interface)?;
        }
        let gpu = self.create_gpu(adapter)?;
        let collector = if validation { Some(validation::Collector::attach(gpu.raw())?) } else { None };
        Ok(Box::new(device::Direct3d12Device::new(gpu, std::borrow::Cow::Borrowed(&self.shaders), collector)?))
    }

    fn open_with_scene_shaders(&self, index: usize, scene: &SceneShaders) -> Result<Box<dyn Device + '_>, String> {
        let adapter = self.adapters.get(index).ok_or_else(|| format!("no DXGI adapter at index {index} ({} enumerated)", self.adapters.len()))?;
        let mut shaders = self.shaders.clone();
        shaders.scene_vertex = scene.vertex.clone();
        shaders.scene_pixel = scene.fragment.clone();
        Ok(Box::new(device::Direct3d12Device::new(self.create_gpu(adapter)?, std::borrow::Cow::Owned(shaders), None)?))
    }

    fn software_settings(&self, device: &DeviceInfo) -> Vec<Setting> {
        if device.kind != DeviceKind::Software {
            return Vec::new();
        }
        let what = |count: &str| {
            format!("the child process restricted to {count} before the device is created; the Microsoft Basic Render Driver documents no setting of its own (no environment variable, no thread count), so the processors it may run on are the one lever")
        };
        vec![
            Setting { label: "one_processor".to_string(), what: what("one processor"), environment: Vec::new(), affinity_cpus: Some(1) },
            Setting { label: "four_processors".to_string(), what: what("four processors"), environment: Vec::new(), affinity_cpus: Some(4) },
        ]
    }

    fn validation_environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}
