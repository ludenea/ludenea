//! The Direct3D 11 path (Windows): the scene rendered to an offscreen texture
//! by the WARP software device, and by the hardware device when the machine has
//! one, read back through a staging texture. Throwaway, never product code.
//!
//! `d3d11.dll` and `d3dcompiler_47.dll` are loaded at run time and the COM
//! interfaces are called through their virtual tables by slot index, with the
//! structures declared here in the field order of `d3d11.h`. The HLSL is
//! compiled at run time by the system's `D3DCompile`.

use std::ffi::{c_char, c_void};
use std::fs;
use std::path::Path;
use std::ptr;
use std::time::Instant;

use crate::loader::{Library, c_string};
use crate::scene::{Frame, HEIGHT, Scene, VERTEX_STRIDE, WIDTH};
use crate::{FrameResult, Outcome};

type Object = *mut c_void;
type Hresult = i32;

const D3D_DRIVER_TYPE_HARDWARE: i32 = 1;
const D3D_DRIVER_TYPE_WARP: i32 = 5;
const D3D11_SDK_VERSION: u32 = 7;
const DXGI_FORMAT_R32G32B32_FLOAT: u32 = 6;
const DXGI_FORMAT_R32G32_FLOAT: u32 = 16;
const DXGI_FORMAT_R8G8B8A8_UNORM: u32 = 28;
const D3D11_USAGE_DEFAULT: u32 = 0;
const D3D11_USAGE_STAGING: u32 = 3;
const D3D11_BIND_VERTEX_BUFFER: u32 = 1;
const D3D11_BIND_RENDER_TARGET: u32 = 0x20;
const D3D11_CPU_ACCESS_READ: u32 = 0x20000;
const D3D11_MAP_READ: u32 = 1;
const D3D11_INPUT_PER_VERTEX_DATA: u32 = 0;
const D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST: u32 = 4;
const D3D11_FILL_SOLID: u32 = 3;
const D3D11_CULL_NONE: u32 = 1;

// Virtual table slots (after the three of IUnknown).
mod device {
    pub const CREATE_BUFFER: usize = 3;
    pub const CREATE_TEXTURE2D: usize = 5;
    pub const CREATE_RENDER_TARGET_VIEW: usize = 9;
    pub const CREATE_INPUT_LAYOUT: usize = 11;
    pub const CREATE_VERTEX_SHADER: usize = 12;
    pub const CREATE_PIXEL_SHADER: usize = 15;
    pub const CREATE_RASTERIZER_STATE: usize = 22;
    pub const GET_FEATURE_LEVEL: usize = 37;
}

mod context {
    pub const PS_SET_SHADER: usize = 9;
    pub const VS_SET_SHADER: usize = 11;
    pub const DRAW: usize = 13;
    pub const MAP: usize = 14;
    pub const UNMAP: usize = 15;
    pub const IA_SET_INPUT_LAYOUT: usize = 17;
    pub const IA_SET_VERTEX_BUFFERS: usize = 18;
    pub const IA_SET_PRIMITIVE_TOPOLOGY: usize = 24;
    pub const OM_SET_RENDER_TARGETS: usize = 33;
    pub const RS_SET_STATE: usize = 43;
    pub const RS_SET_VIEWPORTS: usize = 44;
    pub const COPY_RESOURCE: usize = 47;
    pub const CLEAR_RENDER_TARGET_VIEW: usize = 50;
    pub const FLUSH: usize = 111;
}

mod blob {
    pub const GET_BUFFER_POINTER: usize = 3;
    pub const GET_BUFFER_SIZE: usize = 4;
}

mod dxgi {
    /// IDXGIDevice::GetAdapter (IDXGIObject takes slots 3 to 6).
    pub const DEVICE_GET_ADAPTER: usize = 7;
    /// IDXGIAdapter::GetDesc (EnumOutputs is 7).
    pub const ADAPTER_GET_DESC: usize = 8;
}

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// IDXGIDevice: 54ec77fa-1377-44e6-8c32-88fd5f44c84c.
const IID_IDXGI_DEVICE: Guid =
    Guid { data1: 0x54ec_77fa, data2: 0x1377, data3: 0x44e6, data4: [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c] };

#[repr(C)]
struct Texture2dDesc {
    width: u32,
    height: u32,
    mip_levels: u32,
    array_size: u32,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access_flags: u32,
    misc_flags: u32,
}

#[repr(C)]
struct BufferDesc {
    byte_width: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access_flags: u32,
    misc_flags: u32,
    structure_byte_stride: u32,
}

#[repr(C)]
struct SubresourceData {
    sys_mem: *const c_void,
    sys_mem_pitch: u32,
    sys_mem_slice_pitch: u32,
}

#[repr(C)]
struct InputElementDesc {
    semantic_name: *const c_char,
    semantic_index: u32,
    format: u32,
    input_slot: u32,
    aligned_byte_offset: u32,
    input_slot_class: u32,
    instance_data_step_rate: u32,
}

#[repr(C)]
struct Viewport {
    top_left_x: f32,
    top_left_y: f32,
    width: f32,
    height: f32,
    min_depth: f32,
    max_depth: f32,
}

#[repr(C)]
struct MappedSubresource {
    data: *mut c_void,
    row_pitch: u32,
    depth_pitch: u32,
}

#[repr(C)]
struct RasterizerDesc {
    fill_mode: u32,
    cull_mode: u32,
    front_counter_clockwise: i32,
    depth_bias: i32,
    depth_bias_clamp: f32,
    slope_scaled_depth_bias: f32,
    depth_clip_enable: i32,
    scissor_enable: i32,
    multisample_enable: i32,
    antialiased_line_enable: i32,
}

#[repr(C)]
struct AdapterDesc {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: [u32; 2],
}

type CreateDevice = unsafe extern "system" fn(
    Object,
    i32,
    Object,
    u32,
    *const i32,
    u32,
    u32,
    *mut Object,
    *mut i32,
    *mut Object,
) -> Hresult;

type Compile = unsafe extern "system" fn(
    *const c_void,
    usize,
    *const c_char,
    *const c_void,
    *mut c_void,
    *const c_char,
    *const c_char,
    u32,
    u32,
    *mut Object,
    *mut Object,
) -> Hresult;

/// Calls slot `index` of `object`'s virtual table with the given signature.
macro_rules! com {
    ($object:expr, $index:expr, fn($($arg_type:ty),*) -> $ret:ty, $($arg:expr),*) => {{
        let object: Object = $object;
        let vtable = *(object as *const *const *const c_void);
        let function: unsafe extern "system" fn(Object $(, $arg_type)*) -> $ret = std::mem::transmute(*vtable.add($index));
        function(object $(, $arg)*)
    }};
}

unsafe fn release(object: Object) {
    if !object.is_null() {
        unsafe { com!(object, 2, fn() -> u32,) };
    }
}

fn check(result: Hresult, what: &str) -> Result<(), String> {
    if result >= 0 { Ok(()) } else { Err(format!("{what} failed with HRESULT {:#010x}", result as u32)) }
}

/// Compiles one HLSL entry point and returns its bytecode.
unsafe fn compile_shader(compile: Compile, source: &str, entry: &str, target: &str) -> Result<Vec<u8>, String> {
    let name = c_string("scene.hlsl");
    let entry_c = c_string(entry);
    let target_c = c_string(target);
    let mut code: Object = ptr::null_mut();
    let mut errors: Object = ptr::null_mut();
    let result = unsafe {
        compile(
            source.as_ptr() as *const c_void,
            source.len(),
            name.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            entry_c.as_ptr(),
            target_c.as_ptr(),
            0,
            0,
            &mut code,
            &mut errors,
        )
    };
    let messages = if errors.is_null() {
        String::new()
    } else {
        let pointer = unsafe { com!(errors, blob::GET_BUFFER_POINTER, fn() -> *const u8,) };
        let size = unsafe { com!(errors, blob::GET_BUFFER_SIZE, fn() -> usize,) };
        let text = unsafe { std::slice::from_raw_parts(pointer, size) };
        let text = String::from_utf8_lossy(text).trim_end_matches('\0').to_string();
        unsafe { release(errors) };
        text
    };
    if result < 0 || code.is_null() {
        return Err(format!("D3DCompile of {entry} failed with HRESULT {:#010x}: {messages}", result as u32));
    }
    let pointer = unsafe { com!(code, blob::GET_BUFFER_POINTER, fn() -> *const u8,) };
    let size = unsafe { com!(code, blob::GET_BUFFER_SIZE, fn() -> usize,) };
    let bytes = unsafe { std::slice::from_raw_parts(pointer, size) }.to_vec();
    unsafe { release(code) };
    Ok(bytes)
}

/// The adapter the device was created on, from DXGI.
unsafe fn adapter_description(device: Object) -> Result<(String, u32, u32), String> {
    let mut dxgi_device: Object = ptr::null_mut();
    check(unsafe { com!(device, 0, fn(*const Guid, *mut Object) -> Hresult, &IID_IDXGI_DEVICE, &mut dxgi_device) }, "QueryInterface(IDXGIDevice)")?;
    let mut adapter: Object = ptr::null_mut();
    let result = unsafe { com!(dxgi_device, dxgi::DEVICE_GET_ADAPTER, fn(*mut Object) -> Hresult, &mut adapter) };
    unsafe { release(dxgi_device) };
    check(result, "IDXGIDevice::GetAdapter")?;
    let mut desc = AdapterDesc {
        description: [0; 128],
        vendor_id: 0,
        device_id: 0,
        sub_sys_id: 0,
        revision: 0,
        dedicated_video_memory: 0,
        dedicated_system_memory: 0,
        shared_system_memory: 0,
        adapter_luid: [0; 2],
    };
    let result = unsafe { com!(adapter, dxgi::ADAPTER_GET_DESC, fn(*mut AdapterDesc) -> Hresult, &mut desc) };
    unsafe { release(adapter) };
    check(result, "IDXGIAdapter::GetDesc")?;
    let end = desc.description.iter().position(|&c| c == 0).unwrap_or(128);
    Ok((String::from_utf16_lossy(&desc.description[..end]), desc.vendor_id, desc.device_id))
}

struct Shaders {
    vertex: Vec<u8>,
    pixel: Vec<u8>,
}

/// One complete render on `driver_type`: device, shaders, pipeline state, one
/// frame, read back through the staging texture.
unsafe fn render_once(create_device: CreateDevice, shaders: &Shaders, driver_type: i32, scene: &Scene, outcome: &mut Outcome) -> Result<Frame, String> {
    let started = Instant::now();
    let mut device: Object = ptr::null_mut();
    let mut context: Object = ptr::null_mut();
    let mut feature_level = 0_i32;
    let result = unsafe {
        create_device(ptr::null_mut(), driver_type, ptr::null_mut(), 0, ptr::null(), 0, D3D11_SDK_VERSION, &mut device, &mut feature_level, &mut context)
    };
    check(result, "D3D11CreateDevice")?;
    outcome.device_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    let (name, vendor, device_id) = unsafe { adapter_description(device) }.unwrap_or_else(|error| (error, 0, 0));
    outcome.device_name = name;
    outcome.device_type = if driver_type == D3D_DRIVER_TYPE_WARP { "software (WARP)".to_string() } else { "hardware".to_string() };
    let level = unsafe { com!(device, device::GET_FEATURE_LEVEL, fn() -> i32,) };
    outcome.driver = format!("feature level {level:#06x}, vendor {vendor:#06x}, device {device_id:#06x}");

    let pipeline_started = Instant::now();
    let mut vertex_shader: Object = ptr::null_mut();
    check(
        unsafe { com!(device, device::CREATE_VERTEX_SHADER, fn(*const c_void, usize, Object, *mut Object) -> Hresult, shaders.vertex.as_ptr() as *const c_void, shaders.vertex.len(), ptr::null_mut(), &mut vertex_shader) },
        "CreateVertexShader",
    )?;
    let mut pixel_shader: Object = ptr::null_mut();
    check(
        unsafe { com!(device, device::CREATE_PIXEL_SHADER, fn(*const c_void, usize, Object, *mut Object) -> Hresult, shaders.pixel.as_ptr() as *const c_void, shaders.pixel.len(), ptr::null_mut(), &mut pixel_shader) },
        "CreatePixelShader",
    )?;
    let position = c_string("POSITION");
    let color = c_string("COLOR");
    let elements = [
        InputElementDesc { semantic_name: position.as_ptr(), semantic_index: 0, format: DXGI_FORMAT_R32G32_FLOAT, input_slot: 0, aligned_byte_offset: 0, input_slot_class: D3D11_INPUT_PER_VERTEX_DATA, instance_data_step_rate: 0 },
        InputElementDesc { semantic_name: color.as_ptr(), semantic_index: 0, format: DXGI_FORMAT_R32G32B32_FLOAT, input_slot: 0, aligned_byte_offset: 8, input_slot_class: D3D11_INPUT_PER_VERTEX_DATA, instance_data_step_rate: 0 },
    ];
    let mut layout: Object = ptr::null_mut();
    check(
        unsafe { com!(device, device::CREATE_INPUT_LAYOUT, fn(*const InputElementDesc, u32, *const c_void, usize, *mut Object) -> Hresult, elements.as_ptr(), 2, shaders.vertex.as_ptr() as *const c_void, shaders.vertex.len(), &mut layout) },
        "CreateInputLayout",
    )?;
    let vertex_bytes = scene.vertex_bytes();
    let buffer_desc = BufferDesc { byte_width: vertex_bytes.len() as u32, usage: D3D11_USAGE_DEFAULT, bind_flags: D3D11_BIND_VERTEX_BUFFER, cpu_access_flags: 0, misc_flags: 0, structure_byte_stride: 0 };
    let initial = SubresourceData { sys_mem: vertex_bytes.as_ptr() as *const c_void, sys_mem_pitch: 0, sys_mem_slice_pitch: 0 };
    let mut vertex_buffer: Object = ptr::null_mut();
    check(unsafe { com!(device, device::CREATE_BUFFER, fn(*const BufferDesc, *const SubresourceData, *mut Object) -> Hresult, &buffer_desc, &initial, &mut vertex_buffer) }, "CreateBuffer")?;
    let target_desc = Texture2dDesc { width: WIDTH, height: HEIGHT, mip_levels: 1, array_size: 1, format: DXGI_FORMAT_R8G8B8A8_UNORM, sample_count: 1, sample_quality: 0, usage: D3D11_USAGE_DEFAULT, bind_flags: D3D11_BIND_RENDER_TARGET, cpu_access_flags: 0, misc_flags: 0 };
    let mut target: Object = ptr::null_mut();
    check(unsafe { com!(device, device::CREATE_TEXTURE2D, fn(*const Texture2dDesc, *const c_void, *mut Object) -> Hresult, &target_desc, ptr::null(), &mut target) }, "CreateTexture2D (target)")?;
    let staging_desc = Texture2dDesc { usage: D3D11_USAGE_STAGING, bind_flags: 0, cpu_access_flags: D3D11_CPU_ACCESS_READ, ..target_desc };
    let mut staging: Object = ptr::null_mut();
    check(unsafe { com!(device, device::CREATE_TEXTURE2D, fn(*const Texture2dDesc, *const c_void, *mut Object) -> Hresult, &staging_desc, ptr::null(), &mut staging) }, "CreateTexture2D (staging)")?;
    let mut view: Object = ptr::null_mut();
    check(unsafe { com!(device, device::CREATE_RENDER_TARGET_VIEW, fn(Object, *const c_void, *mut Object) -> Hresult, target, ptr::null(), &mut view) }, "CreateRenderTargetView")?;
    let rasterizer_desc = RasterizerDesc { fill_mode: D3D11_FILL_SOLID, cull_mode: D3D11_CULL_NONE, front_counter_clockwise: 0, depth_bias: 0, depth_bias_clamp: 0.0, slope_scaled_depth_bias: 0.0, depth_clip_enable: 1, scissor_enable: 0, multisample_enable: 0, antialiased_line_enable: 0 };
    let mut rasterizer: Object = ptr::null_mut();
    check(unsafe { com!(device, device::CREATE_RASTERIZER_STATE, fn(*const RasterizerDesc, *mut Object) -> Hresult, &rasterizer_desc, &mut rasterizer) }, "CreateRasterizerState")?;
    outcome.pipeline_us = pipeline_started.elapsed().as_secs_f64() * 1_000_000.0;

    let render_started = Instant::now();
    let viewport = Viewport { top_left_x: 0.0, top_left_y: 0.0, width: WIDTH as f32, height: HEIGHT as f32, min_depth: 0.0, max_depth: 1.0 };
    let clear = [0.0_f32, 0.0, 0.0, 1.0];
    let stride = VERTEX_STRIDE;
    let offset = 0_u32;
    unsafe {
        com!(context, context::OM_SET_RENDER_TARGETS, fn(u32, *const Object, Object) -> (), 1, &view, ptr::null_mut());
        com!(context, context::RS_SET_VIEWPORTS, fn(u32, *const Viewport) -> (), 1, &viewport);
        com!(context, context::RS_SET_STATE, fn(Object) -> (), rasterizer);
        com!(context, context::IA_SET_INPUT_LAYOUT, fn(Object) -> (), layout);
        com!(context, context::IA_SET_VERTEX_BUFFERS, fn(u32, u32, *const Object, *const u32, *const u32) -> (), 0, 1, &vertex_buffer, &stride, &offset);
        com!(context, context::IA_SET_PRIMITIVE_TOPOLOGY, fn(u32) -> (), D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        com!(context, context::VS_SET_SHADER, fn(Object, *const Object, u32) -> (), vertex_shader, ptr::null(), 0);
        com!(context, context::PS_SET_SHADER, fn(Object, *const Object, u32) -> (), pixel_shader, ptr::null(), 0);
        com!(context, context::CLEAR_RENDER_TARGET_VIEW, fn(Object, *const f32) -> (), view, clear.as_ptr());
        com!(context, context::DRAW, fn(u32, u32) -> (), scene.vertices.len() as u32, 0);
        com!(context, context::COPY_RESOURCE, fn(Object, Object) -> (), staging, target);
        com!(context, context::FLUSH, fn() -> (),);
    }
    let render_us = render_started.elapsed().as_secs_f64() * 1_000_000.0;

    let readback_started = Instant::now();
    let mut mapped = MappedSubresource { data: ptr::null_mut(), row_pitch: 0, depth_pitch: 0 };
    check(unsafe { com!(context, context::MAP, fn(Object, u32, u32, u32, *mut MappedSubresource) -> Hresult, staging, 0, D3D11_MAP_READ, 0, &mut mapped) }, "Map")?;
    let pitch = mapped.row_pitch as usize;
    let rows = unsafe { std::slice::from_raw_parts(mapped.data as *const u8, pitch * HEIGHT as usize) };
    let frame = Frame::from_rows(WIDTH, HEIGHT, pitch, rows);
    unsafe { com!(context, context::UNMAP, fn(Object, u32) -> (), staging, 0) };
    let readback_us = readback_started.elapsed().as_secs_f64() * 1_000_000.0;
    outcome.frames.push(FrameResult { hash: frame.hash(), lit_pixels: frame.lit_pixels(), render_us, readback_us });
    outcome.notes.push(format!("staging row pitch {pitch} bytes"));

    unsafe {
        for object in [rasterizer, view, staging, target, vertex_buffer, layout, pixel_shader, vertex_shader, context, device] {
            release(object);
        }
    }
    Ok(frame)
}

/// Renders the scene `runs` times from scratch on the WARP device (`warp`
/// true) or on the hardware adapter, and reports every frame.
pub fn render(scene: &Scene, shaders: &Path, warp: bool, runs: usize) -> Outcome {
    let mut outcome = Outcome::new(if warp { "d3d11-warp" } else { "d3d11-hardware" });
    let d3d11 = match Library::open(&["d3d11.dll"]) {
        Ok(library) => library,
        Err(error) => {
            outcome.reason = error;
            return outcome;
        }
    };
    let compiler = match Library::open(&["d3dcompiler_47.dll"]) {
        Ok(library) => library,
        Err(error) => {
            outcome.reason = error;
            return outcome;
        }
    };
    outcome.loader = d3d11.name.clone();
    outcome.shader_compiler = format!("{} (D3DCompile)", compiler.name);
    // SAFETY: the libraries export these functions with these signatures.
    let (create_device, compile): (CreateDevice, Compile) = match (unsafe { d3d11.function("D3D11CreateDevice") }, unsafe { compiler.function("D3DCompile") }) {
        (Ok(create), Ok(compile)) => (create, compile),
        (Err(error), _) | (_, Err(error)) => {
            outcome.reason = error;
            return outcome;
        }
    };
    let source = match fs::read_to_string(shaders.join("scene.hlsl")) {
        Ok(source) => source,
        Err(error) => {
            outcome.reason = format!("scene.hlsl: {error}");
            return outcome;
        }
    };
    let compile_started = Instant::now();
    let compiled = unsafe {
        match (compile_shader(compile, &source, "vs_main", "vs_5_0"), compile_shader(compile, &source, "ps_main", "ps_5_0")) {
            (Ok(vertex), Ok(pixel)) => Shaders { vertex, pixel },
            (Err(error), _) | (_, Err(error)) => {
                outcome.reason = error;
                return outcome;
            }
        }
    };
    outcome.shader_compile_us = compile_started.elapsed().as_secs_f64() * 1_000_000.0;
    outcome.notes.push(format!("bytecode: vertex {} bytes, pixel {} bytes", compiled.vertex.len(), compiled.pixel.len()));
    let driver_type = if warp { D3D_DRIVER_TYPE_WARP } else { D3D_DRIVER_TYPE_HARDWARE };
    for _ in 0..runs {
        match unsafe { render_once(create_device, &compiled, driver_type, scene, &mut outcome) } {
            Ok(frame) => {
                if outcome.frame.is_none() {
                    outcome.frame = Some(frame);
                }
            }
            Err(error) => {
                outcome.reason = error;
                return outcome;
            }
        }
    }
    outcome.available = true;
    outcome
}
