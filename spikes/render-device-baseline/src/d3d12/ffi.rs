//! The Direct3D 12, DXGI, shader compiler and debug layer declarations the
//! module calls, written by hand in the field order of `d3d12.h`, `dxgi.h`,
//! `d3dcompiler.h` and `d3d12sdklayers.h`, with the virtual table slot of each
//! method. Throwaway, never product code.

use std::ffi::{c_char, c_void};

pub type Object = *mut c_void;
pub type Hresult = i32;

/// Calls slot `index` of `object`'s virtual table with the given signature.
/// Methods that return a structure take a hidden pointer to it as their first
/// argument after the object, as the Windows C++ convention passes it.
macro_rules! com {
    ($object:expr, $index:expr, fn($($arg_type:ty),*) -> $ret:ty $(, $arg:expr)*) => {{
        let object: $crate::d3d12::ffi::Object = $object;
        let vtable = *(object as *const *const *const std::ffi::c_void);
        let function: unsafe extern "system" fn($crate::d3d12::ffi::Object $(, $arg_type)*) -> $ret = std::mem::transmute(*vtable.add($index));
        function(object $(, $arg)*)
    }};
}
pub(crate) use com;

/// A COM object this module owns one reference to, released when dropped.
pub struct Com(Object);

impl Com {
    /// Takes ownership of the reference `object` carries.
    pub fn from_raw(object: Object) -> Com {
        Com(object)
    }

    pub fn raw(&self) -> Object {
        self.0
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the object is a live COM object whose reference this
            // value owns; slot 2 is IUnknown::Release.
            unsafe { com!(self.0, 2, fn() -> u32) };
        }
    }
}

/// A raw object handed to the recording threads. Direct3D 12 devices, heaps,
/// resources, pipeline states and root signatures are free-threaded; each
/// command list is only ever touched by the one thread that records it.
#[derive(Clone, Copy)]
pub struct Shared(pub Object);

// SAFETY: see the type's documentation: the objects are free-threaded or used
// by one thread at a time.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

pub fn check(result: Hresult, what: &str) -> Result<(), String> {
    if result >= 0 { Ok(()) } else { Err(format!("{what} failed with HRESULT {:#010x}", result as u32)) }
}

/// The object a creating call wrote, owned, or the call's error.
pub fn created(result: Hresult, object: Object, what: &str) -> Result<Com, String> {
    let owned = Com::from_raw(object);
    check(result, what)?;
    if owned.raw().is_null() {
        return Err(format!("{what} returned no object"));
    }
    Ok(owned)
}

#[repr(C)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Guid {
    Guid { data1, data2, data3, data4 }
}

pub const IID_IDXGI_FACTORY1: Guid = guid(0x770a_ae78, 0xf26f, 0x4dba, [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87]);
pub const IID_IDXGI_DEVICE: Guid = guid(0x54ec_77fa, 0x1377, 0x44e6, [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c]);
pub const IID_DEVICE: Guid = guid(0x1898_19f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
pub const IID_COMMAND_QUEUE: Guid = guid(0x0ec8_70a6, 0x5d7e, 0x4c22, [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed]);
pub const IID_COMMAND_ALLOCATOR: Guid = guid(0x6102_dee4, 0xaf59, 0x4b09, [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24]);
pub const IID_GRAPHICS_COMMAND_LIST: Guid = guid(0x5b16_0d0f, 0xac1b, 0x4185, [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55]);
pub const IID_FENCE: Guid = guid(0x0a75_3dcf, 0xc4d8, 0x4b91, [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76]);
pub const IID_RESOURCE: Guid = guid(0x6964_42be, 0xa72e, 0x4059, [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad]);
pub const IID_DESCRIPTOR_HEAP: Guid = guid(0x8efb_471d, 0x616c, 0x4f49, [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51]);
pub const IID_ROOT_SIGNATURE: Guid = guid(0xc54a_6b66, 0x72df, 0x4ee8, [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14]);
pub const IID_PIPELINE_STATE: Guid = guid(0x765a_30f3, 0xf624, 0x4c6f, [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45]);
pub const IID_QUERY_HEAP: Guid = guid(0x0d96_58ae, 0xed45, 0x469e, [0xa6, 0x1d, 0x97, 0x0e, 0xc5, 0x83, 0xca, 0xb4]);
pub const IID_COMMAND_SIGNATURE: Guid = guid(0xc36a_797c, 0xec80, 0x4f0a, [0x89, 0x85, 0xa7, 0xb2, 0x47, 0x50, 0x82, 0xd1]);
pub const IID_DEBUG: Guid = guid(0x3444_88b7, 0x6846, 0x474b, [0xb9, 0x89, 0xf0, 0x27, 0x44, 0x82, 0x45, 0xe0]);
pub const IID_INFO_QUEUE: Guid = guid(0x0742_a90b, 0xc387, 0x483f, [0xb9, 0x46, 0x30, 0xa7, 0xe4, 0xe6, 0x14, 0x58]);
pub const IID_INFO_QUEUE1: Guid = guid(0x2852_dd88, 0xb484, 0x4c0c, [0xb6, 0xb1, 0x67, 0x16, 0x85, 0x00, 0xe6, 0x00]);

// Exported functions.
pub type CreateDxgiFactory1 = unsafe extern "system" fn(*const Guid, *mut Object) -> Hresult;
pub type CreateDevice = unsafe extern "system" fn(Object, u32, *const Guid, *mut Object) -> Hresult;
pub type GetDebugInterface = unsafe extern "system" fn(*const Guid, *mut Object) -> Hresult;
pub type SerializeRootSignature = unsafe extern "system" fn(*const RootSignatureDesc, u32, *mut Object, *mut Object) -> Hresult;
pub type Compile = unsafe extern "system" fn(*const c_void, usize, *const c_char, *const c_void, *mut c_void, *const c_char, *const c_char, u32, u32, *mut Object, *mut Object) -> Hresult;
/// D3D12MessageFunc: category, severity, identifier, description, context.
pub type MessageCallback = unsafe extern "system" fn(u32, u32, u32, *const c_char, *mut c_void);

/// Virtual table slots, after the three of IUnknown.
pub mod slot {
    pub const QUERY_INTERFACE: usize = 0;

    pub mod blob {
        pub const GET_BUFFER_POINTER: usize = 3;
        pub const GET_BUFFER_SIZE: usize = 4;
    }

    pub mod factory {
        /// IDXGIFactory1::EnumAdapters1 (IDXGIObject 3 to 6, IDXGIFactory 7 to 11).
        pub const ENUM_ADAPTERS1: usize = 12;
    }

    pub mod adapter {
        /// IDXGIAdapter::CheckInterfaceSupport.
        pub const CHECK_INTERFACE_SUPPORT: usize = 9;
        /// IDXGIAdapter1::GetDesc1.
        pub const GET_DESC1: usize = 10;
    }

    pub mod debug {
        pub const ENABLE_DEBUG_LAYER: usize = 3;
    }

    /// ID3D12Device (ID3D12Object takes slots 3 to 6).
    pub mod device {
        pub const CREATE_COMMAND_QUEUE: usize = 8;
        pub const CREATE_COMMAND_ALLOCATOR: usize = 9;
        pub const CREATE_GRAPHICS_PIPELINE_STATE: usize = 10;
        pub const CREATE_COMPUTE_PIPELINE_STATE: usize = 11;
        pub const CREATE_COMMAND_LIST: usize = 12;
        pub const CHECK_FEATURE_SUPPORT: usize = 13;
        pub const CREATE_DESCRIPTOR_HEAP: usize = 14;
        pub const GET_DESCRIPTOR_HANDLE_INCREMENT_SIZE: usize = 15;
        pub const CREATE_ROOT_SIGNATURE: usize = 16;
        pub const CREATE_SHADER_RESOURCE_VIEW: usize = 18;
        pub const CREATE_UNORDERED_ACCESS_VIEW: usize = 19;
        pub const CREATE_RENDER_TARGET_VIEW: usize = 20;
        pub const CREATE_DEPTH_STENCIL_VIEW: usize = 21;
        pub const CREATE_SAMPLER: usize = 22;
        pub const CREATE_COMMITTED_RESOURCE: usize = 27;
        pub const CREATE_FENCE: usize = 36;
        pub const GET_DEVICE_REMOVED_REASON: usize = 37;
        pub const GET_COPYABLE_FOOTPRINTS: usize = 38;
        pub const CREATE_QUERY_HEAP: usize = 39;
        pub const CREATE_COMMAND_SIGNATURE: usize = 41;
    }

    /// ID3D12CommandQueue (ID3D12DeviceChild::GetDevice is 7).
    pub mod queue {
        pub const EXECUTE_COMMAND_LISTS: usize = 10;
        pub const SIGNAL: usize = 14;
        pub const GET_TIMESTAMP_FREQUENCY: usize = 16;
    }

    pub mod allocator {
        pub const RESET: usize = 8;
    }

    pub mod fence {
        pub const GET_COMPLETED_VALUE: usize = 8;
        pub const SET_EVENT_ON_COMPLETION: usize = 9;
    }

    pub mod resource {
        pub const MAP: usize = 8;
        pub const UNMAP: usize = 9;
        pub const GET_GPU_VIRTUAL_ADDRESS: usize = 11;
    }

    pub mod descriptor_heap {
        pub const GET_CPU_DESCRIPTOR_HANDLE_FOR_HEAP_START: usize = 9;
        pub const GET_GPU_DESCRIPTOR_HANDLE_FOR_HEAP_START: usize = 10;
    }

    /// ID3D12GraphicsCommandList (ID3D12CommandList::GetType is 8).
    pub mod list {
        pub const CLOSE: usize = 9;
        pub const RESET: usize = 10;
        pub const DRAW_INSTANCED: usize = 12;
        pub const DISPATCH: usize = 14;
        pub const COPY_BUFFER_REGION: usize = 15;
        pub const COPY_TEXTURE_REGION: usize = 16;
        pub const RESOLVE_SUBRESOURCE: usize = 19;
        pub const IA_SET_PRIMITIVE_TOPOLOGY: usize = 20;
        pub const RS_SET_VIEWPORTS: usize = 21;
        pub const RS_SET_SCISSOR_RECTS: usize = 22;
        pub const SET_PIPELINE_STATE: usize = 25;
        pub const RESOURCE_BARRIER: usize = 26;
        pub const SET_DESCRIPTOR_HEAPS: usize = 28;
        pub const SET_COMPUTE_ROOT_SIGNATURE: usize = 29;
        pub const SET_GRAPHICS_ROOT_SIGNATURE: usize = 30;
        pub const SET_COMPUTE_ROOT_DESCRIPTOR_TABLE: usize = 31;
        pub const SET_GRAPHICS_ROOT_DESCRIPTOR_TABLE: usize = 32;
        pub const SET_GRAPHICS_ROOT_32BIT_CONSTANTS: usize = 36;
        pub const SET_GRAPHICS_ROOT_CONSTANT_BUFFER_VIEW: usize = 38;
        pub const SET_COMPUTE_ROOT_SHADER_RESOURCE_VIEW: usize = 39;
        pub const SET_COMPUTE_ROOT_UNORDERED_ACCESS_VIEW: usize = 41;
        pub const IA_SET_VERTEX_BUFFERS: usize = 44;
        pub const OM_SET_RENDER_TARGETS: usize = 46;
        pub const CLEAR_DEPTH_STENCIL_VIEW: usize = 47;
        pub const CLEAR_RENDER_TARGET_VIEW: usize = 48;
        pub const END_QUERY: usize = 53;
        pub const RESOLVE_QUERY_DATA: usize = 54;
        pub const EXECUTE_INDIRECT: usize = 59;
    }

    /// ID3D12InfoQueue, then ID3D12InfoQueue1 after its 35 methods.
    pub mod info_queue {
        pub const CLEAR_STORED_MESSAGES: usize = 4;
        pub const GET_MESSAGE: usize = 5;
        pub const GET_NUM_STORED_MESSAGES_ALLOWED_BY_RETRIEVAL_FILTER: usize = 9;
        pub const REGISTER_MESSAGE_CALLBACK: usize = 38;
        pub const UNREGISTER_MESSAGE_CALLBACK: usize = 39;
    }
}

pub const DXGI_ERROR_NOT_FOUND: Hresult = 0x887a_0002_u32 as i32;
pub const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;
/// The vendor and device identifiers DXGI documents for the Microsoft Basic
/// Render Driver, the rasterizer that runs on the processor.
pub const BASIC_RENDER_DRIVER_IDENTIFIERS: (u32, u32) = (0x1414, 0x8c);

pub const FEATURE_LEVEL_11_0: u32 = 0xb000;
pub const FEATURE_LEVELS: [u32; 5] = [0xb000, 0xb100, 0xc000, 0xc100, 0xc200];
pub const ROOT_SIGNATURE_VERSION_1_0: u32 = 1;
pub const COMPILE_ENABLE_STRICTNESS: u32 = 1 << 11;

// Formats.
pub const FORMAT_R32G32B32A32_FLOAT: u32 = 2;
pub const FORMAT_R32G32B32_FLOAT: u32 = 6;
pub const FORMAT_R16G16B16A16_FLOAT: u32 = 10;
pub const FORMAT_R32G32_FLOAT: u32 = 16;
pub const FORMAT_R8G8B8A8_TYPELESS: u32 = 27;
pub const FORMAT_R8G8B8A8_UNORM: u32 = 28;
pub const FORMAT_R8G8B8A8_UNORM_SRGB: u32 = 29;
pub const FORMAT_D32_FLOAT: u32 = 40;
pub const FORMAT_R32_UINT: u32 = 42;
pub const FORMAT_UNKNOWN: u32 = 0;
/// The block compressed formats, each with its name.
pub const BLOCK_COMPRESSED_FORMATS: [(u32, &str); 14] = [
    (71, "BC1_UNORM"),
    (72, "BC1_UNORM_SRGB"),
    (74, "BC2_UNORM"),
    (75, "BC2_UNORM_SRGB"),
    (77, "BC3_UNORM"),
    (78, "BC3_UNORM_SRGB"),
    (80, "BC4_UNORM"),
    (81, "BC4_SNORM"),
    (83, "BC5_UNORM"),
    (84, "BC5_SNORM"),
    (95, "BC6H_UF16"),
    (96, "BC6H_SF16"),
    (98, "BC7_UNORM"),
    (99, "BC7_UNORM_SRGB"),
];

// Format support bits (D3D12_FORMAT_SUPPORT1).
pub const FORMAT_SUPPORT_SHADER_SAMPLE: u32 = 0x200;
pub const FORMAT_SUPPORT_RENDER_TARGET: u32 = 0x4000;
pub const FORMAT_SUPPORT_DEPTH_STENCIL: u32 = 0x10000;
pub const FORMAT_SUPPORT_MULTISAMPLE_RESOLVE: u32 = 0x40000;
pub const FORMAT_SUPPORT_MULTISAMPLE_RENDER_TARGET: u32 = 0x20_0000;
pub const FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW: u32 = 0x200_0000;

// Features (D3D12_FEATURE).
pub const FEATURE_OPTIONS: u32 = 0;
pub const FEATURE_ARCHITECTURE: u32 = 1;
pub const FEATURE_FEATURE_LEVELS: u32 = 2;
pub const FEATURE_FORMAT_SUPPORT: u32 = 3;
pub const FEATURE_MULTISAMPLE_QUALITY_LEVELS: u32 = 4;
pub const FEATURE_SHADER_MODEL: u32 = 7;
pub const FEATURE_OPTIONS1: u32 = 8;
pub const FEATURE_OPTIONS5: u32 = 27;
pub const FEATURE_OPTIONS7: u32 = 32;
pub const FEATURE_OPTIONS9: u32 = 37;
pub const FEATURE_OPTIONS11: u32 = 40;

pub const COMMAND_LIST_TYPE_DIRECT: u32 = 0;
pub const COMMAND_LIST_TYPE_COMPUTE: u32 = 2;

pub const HEAP_TYPE_DEFAULT: u32 = 1;
pub const HEAP_TYPE_UPLOAD: u32 = 2;
pub const HEAP_TYPE_READBACK: u32 = 3;

pub const RESOURCE_DIMENSION_BUFFER: u32 = 1;
pub const RESOURCE_DIMENSION_TEXTURE2D: u32 = 3;
pub const TEXTURE_LAYOUT_UNKNOWN: u32 = 0;
pub const TEXTURE_LAYOUT_ROW_MAJOR: u32 = 1;
pub const RESOURCE_FLAG_NONE: u32 = 0;
pub const RESOURCE_FLAG_ALLOW_RENDER_TARGET: u32 = 1;
pub const RESOURCE_FLAG_ALLOW_DEPTH_STENCIL: u32 = 2;
pub const RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS: u32 = 4;

// Resource states.
pub const STATE_COMMON: u32 = 0;
pub const STATE_VERTEX_AND_CONSTANT_BUFFER: u32 = 0x1;
pub const STATE_RENDER_TARGET: u32 = 0x4;
pub const STATE_UNORDERED_ACCESS: u32 = 0x8;
pub const STATE_DEPTH_WRITE: u32 = 0x10;
pub const STATE_NON_PIXEL_SHADER_RESOURCE: u32 = 0x40;
pub const STATE_PIXEL_SHADER_RESOURCE: u32 = 0x80;
pub const STATE_INDIRECT_ARGUMENT: u32 = 0x200;
pub const STATE_COPY_DEST: u32 = 0x400;
pub const STATE_COPY_SOURCE: u32 = 0x800;
pub const STATE_RESOLVE_DEST: u32 = 0x1000;
pub const STATE_RESOLVE_SOURCE: u32 = 0x2000;
pub const STATE_GENERIC_READ: u32 = 0xac3;

pub const BARRIER_TYPE_TRANSITION: u32 = 0;
pub const ALL_SUBRESOURCES: u32 = 0xffff_ffff;

pub const DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV: u32 = 0;
pub const DESCRIPTOR_HEAP_TYPE_SAMPLER: u32 = 1;
pub const DESCRIPTOR_HEAP_TYPE_RTV: u32 = 2;
pub const DESCRIPTOR_HEAP_TYPE_DSV: u32 = 3;
pub const DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE: u32 = 1;

pub const SRV_DIMENSION_TEXTURE2D: u32 = 4;
pub const RTV_DIMENSION_TEXTURE2D: u32 = 4;
pub const DEFAULT_SHADER_4_COMPONENT_MAPPING: u32 = 0x1688;

pub const FILTER_MIN_MAG_MIP_POINT: u32 = 0;
pub const FILTER_MIN_MAG_LINEAR_MIP_POINT: u32 = 0x14;
pub const FILTER_MIN_MAG_MIP_LINEAR: u32 = 0x15;
pub const ADDRESS_MODE_WRAP: u32 = 1;
pub const ADDRESS_MODE_CLAMP: u32 = 3;
pub const COMPARISON_NEVER: u32 = 1;
pub const COMPARISON_LESS_EQUAL: u32 = 4;
pub const COMPARISON_ALWAYS: u32 = 8;
pub const FLOAT32_MAX: f32 = 3.402_823_466e38;

pub const ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE: u32 = 0;
pub const ROOT_PARAMETER_TYPE_32BIT_CONSTANTS: u32 = 1;
pub const ROOT_PARAMETER_TYPE_CBV: u32 = 2;
pub const ROOT_PARAMETER_TYPE_SRV: u32 = 3;
pub const ROOT_PARAMETER_TYPE_UAV: u32 = 4;
pub const DESCRIPTOR_RANGE_TYPE_SRV: u32 = 0;
pub const DESCRIPTOR_RANGE_TYPE_UAV: u32 = 1;
pub const DESCRIPTOR_RANGE_TYPE_SAMPLER: u32 = 3;
pub const SHADER_VISIBILITY_ALL: u32 = 0;
pub const SHADER_VISIBILITY_PIXEL: u32 = 5;
pub const ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT: u32 = 1;

pub const BLEND_ZERO: u32 = 1;
pub const BLEND_ONE: u32 = 2;
pub const BLEND_SRC_ALPHA: u32 = 5;
pub const BLEND_INV_SRC_ALPHA: u32 = 6;
pub const BLEND_OP_ADD: u32 = 1;
pub const LOGIC_OP_NOOP: u32 = 4;
pub const COLOR_WRITE_ENABLE_ALL: u8 = 0xf;
pub const FILL_MODE_SOLID: u32 = 3;
pub const CULL_MODE_NONE: u32 = 1;
pub const DEPTH_WRITE_MASK_ZERO: u32 = 0;
pub const DEPTH_WRITE_MASK_ALL: u32 = 1;
pub const STENCIL_OP_KEEP: u32 = 1;
pub const PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE: u32 = 3;
pub const PRIMITIVE_TOPOLOGY_TRIANGLELIST: u32 = 4;
pub const INPUT_PER_VERTEX_DATA: u32 = 0;
pub const CLEAR_FLAG_DEPTH: u32 = 1;

pub const QUERY_HEAP_TYPE_TIMESTAMP: u32 = 1;
pub const QUERY_TYPE_TIMESTAMP: u32 = 2;
pub const INDIRECT_ARGUMENT_TYPE_DRAW: u32 = 0;
pub const TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX: u32 = 0;
pub const TEXTURE_COPY_TYPE_PLACED_FOOTPRINT: u32 = 1;
/// The sample quality that asks for the standard sample positions.
pub const STANDARD_MULTISAMPLE_PATTERN: u32 = 0xffff_ffff;

#[repr(C)]
pub struct AdapterDesc1 {
    pub description: [u16; 128],
    pub vendor_id: u32,
    pub device_id: u32,
    pub sub_sys_id: u32,
    pub revision: u32,
    pub dedicated_video_memory: usize,
    pub dedicated_system_memory: usize,
    pub shared_system_memory: usize,
    pub adapter_luid: [u32; 2],
    pub flags: u32,
}

#[repr(C)]
pub struct CommandQueueDesc {
    pub kind: u32,
    pub priority: i32,
    pub flags: u32,
    pub node_mask: u32,
}

#[repr(C)]
pub struct HeapProperties {
    pub kind: u32,
    pub cpu_page_property: u32,
    pub memory_pool_preference: u32,
    pub creation_node_mask: u32,
    pub visible_node_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SampleDesc {
    pub count: u32,
    pub quality: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResourceDesc {
    pub dimension: u32,
    pub alignment: u64,
    pub width: u64,
    pub height: u32,
    pub depth_or_array_size: u16,
    pub mip_levels: u16,
    pub format: u32,
    pub sample: SampleDesc,
    pub layout: u32,
    pub flags: u32,
}

/// D3D12_CLEAR_VALUE: a color, or a depth in the first value and the stencil
/// in the low byte of the second.
#[repr(C)]
pub struct ClearValue {
    pub format: u32,
    pub values: [f32; 4],
}

#[repr(C)]
pub struct ResourceBarrier {
    pub kind: u32,
    pub flags: u32,
    pub resource: Object,
    pub subresource: u32,
    pub state_before: u32,
    pub state_after: u32,
}

#[repr(C)]
pub struct Range {
    pub begin: usize,
    pub end: usize,
}

#[repr(C)]
pub struct DescriptorHeapDesc {
    pub kind: u32,
    pub count: u32,
    pub flags: u32,
    pub node_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CpuHandle {
    pub pointer: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GpuHandle {
    pub pointer: u64,
}

/// D3D12_SHADER_RESOURCE_VIEW_DESC with its union as a two-dimensional texture.
#[repr(C, align(8))]
pub struct ShaderResourceViewDesc {
    pub format: u32,
    pub dimension: u32,
    pub mapping: u32,
    pub padding: u32,
    pub most_detailed_mip: u32,
    pub mip_levels: u32,
    pub plane_slice: u32,
    pub min_lod_clamp: f32,
    pub rest: [u32; 2],
}

/// D3D12_RENDER_TARGET_VIEW_DESC with its union as a two-dimensional texture.
#[repr(C, align(8))]
pub struct RenderTargetViewDesc {
    pub format: u32,
    pub dimension: u32,
    pub mip_slice: u32,
    pub plane_slice: u32,
    pub rest: [u32; 2],
}

#[repr(C)]
pub struct SamplerDesc {
    pub filter: u32,
    pub address_u: u32,
    pub address_v: u32,
    pub address_w: u32,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border_color: [f32; 4],
    pub min_lod: f32,
    pub max_lod: f32,
}

#[repr(C)]
pub struct StaticSamplerDesc {
    pub filter: u32,
    pub address_u: u32,
    pub address_v: u32,
    pub address_w: u32,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border_color: u32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub register: u32,
    pub space: u32,
    pub visibility: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DescriptorRange {
    pub kind: u32,
    pub count: u32,
    pub base_register: u32,
    pub space: u32,
    pub offset_from_table_start: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RootDescriptorTable {
    pub range_count: u32,
    pub ranges: *const DescriptorRange,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RootConstants {
    pub register: u32,
    pub space: u32,
    pub count: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RootDescriptor {
    pub register: u32,
    pub space: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union RootParameterData {
    pub table: RootDescriptorTable,
    pub constants: RootConstants,
    pub descriptor: RootDescriptor,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RootParameter {
    pub kind: u32,
    pub data: RootParameterData,
    pub visibility: u32,
}

impl RootParameter {
    /// A descriptor table over `ranges`, which must outlive the serialization.
    pub fn table(ranges: &[DescriptorRange], visibility: u32) -> RootParameter {
        let table = RootDescriptorTable { range_count: ranges.len() as u32, ranges: ranges.as_ptr() };
        RootParameter { kind: ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, data: RootParameterData { table }, visibility }
    }

    pub fn constants(register: u32, count: u32, visibility: u32) -> RootParameter {
        RootParameter { kind: ROOT_PARAMETER_TYPE_32BIT_CONSTANTS, data: RootParameterData { constants: RootConstants { register, space: 0, count } }, visibility }
    }

    /// A root descriptor of `kind` (constant buffer, shader resource or unordered access view).
    pub fn descriptor(kind: u32, register: u32, visibility: u32) -> RootParameter {
        RootParameter { kind, data: RootParameterData { descriptor: RootDescriptor { register, space: 0 } }, visibility }
    }
}

#[repr(C)]
pub struct RootSignatureDesc {
    pub parameter_count: u32,
    pub parameters: *const RootParameter,
    pub static_sampler_count: u32,
    pub static_samplers: *const StaticSamplerDesc,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ShaderBytecode {
    pub code: *const c_void,
    pub length: usize,
}

#[repr(C)]
pub struct StreamOutputDesc {
    pub declaration: *const c_void,
    pub entry_count: u32,
    pub buffer_strides: *const u32,
    pub stride_count: u32,
    pub rasterized_stream: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RenderTargetBlendDesc {
    pub blend_enable: i32,
    pub logic_op_enable: i32,
    pub source_blend: u32,
    pub destination_blend: u32,
    pub blend_operation: u32,
    pub source_blend_alpha: u32,
    pub destination_blend_alpha: u32,
    pub blend_operation_alpha: u32,
    pub logic_op: u32,
    pub write_mask: u8,
}

#[repr(C)]
pub struct BlendDesc {
    pub alpha_to_coverage_enable: i32,
    pub independent_blend_enable: i32,
    pub targets: [RenderTargetBlendDesc; 8],
}

#[repr(C)]
pub struct RasterizerDesc {
    pub fill_mode: u32,
    pub cull_mode: u32,
    pub front_counter_clockwise: i32,
    pub depth_bias: i32,
    pub depth_bias_clamp: f32,
    pub slope_scaled_depth_bias: f32,
    pub depth_clip_enable: i32,
    pub multisample_enable: i32,
    pub antialiased_line_enable: i32,
    pub forced_sample_count: u32,
    pub conservative_raster: u32,
}

#[repr(C)]
pub struct DepthStencilOpDesc {
    pub fail: u32,
    pub depth_fail: u32,
    pub pass: u32,
    pub function: u32,
}

#[repr(C)]
pub struct DepthStencilDesc {
    pub depth_enable: i32,
    pub depth_write_mask: u32,
    pub depth_function: u32,
    pub stencil_enable: i32,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
    pub front_face: DepthStencilOpDesc,
    pub back_face: DepthStencilOpDesc,
}

#[repr(C)]
pub struct InputElementDesc {
    pub semantic_name: *const c_char,
    pub semantic_index: u32,
    pub format: u32,
    pub input_slot: u32,
    pub aligned_byte_offset: u32,
    pub input_slot_class: u32,
    pub instance_data_step_rate: u32,
}

#[repr(C)]
pub struct InputLayoutDesc {
    pub elements: *const InputElementDesc,
    pub count: u32,
}

#[repr(C)]
pub struct CachedPipelineState {
    pub blob: *const c_void,
    pub size: usize,
}

#[repr(C)]
pub struct GraphicsPipelineDesc {
    pub root_signature: Object,
    pub vertex_shader: ShaderBytecode,
    pub pixel_shader: ShaderBytecode,
    pub domain_shader: ShaderBytecode,
    pub hull_shader: ShaderBytecode,
    pub geometry_shader: ShaderBytecode,
    pub stream_output: StreamOutputDesc,
    pub blend: BlendDesc,
    pub sample_mask: u32,
    pub rasterizer: RasterizerDesc,
    pub depth_stencil: DepthStencilDesc,
    pub input_layout: InputLayoutDesc,
    pub strip_cut_value: u32,
    pub primitive_topology_type: u32,
    pub render_target_count: u32,
    pub render_target_formats: [u32; 8],
    pub depth_stencil_format: u32,
    pub sample: SampleDesc,
    pub node_mask: u32,
    pub cached: CachedPipelineState,
    pub flags: u32,
}

#[repr(C)]
pub struct ComputePipelineDesc {
    pub root_signature: Object,
    pub compute_shader: ShaderBytecode,
    pub node_mask: u32,
    pub cached: CachedPipelineState,
    pub flags: u32,
}

#[repr(C)]
pub struct Viewport {
    pub top_left_x: f32,
    pub top_left_y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[repr(C)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct VertexBufferView {
    pub location: u64,
    pub size: u32,
    pub stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PlacedFootprint {
    pub offset: u64,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub row_pitch: u32,
}

#[repr(C)]
pub union CopyLocationData {
    pub footprint: PlacedFootprint,
    pub subresource: u32,
}

#[repr(C)]
pub struct TextureCopyLocation {
    pub resource: Object,
    pub kind: u32,
    pub data: CopyLocationData,
}

#[repr(C)]
pub struct QueryHeapDesc {
    pub kind: u32,
    pub count: u32,
    pub node_mask: u32,
}

#[repr(C)]
pub struct IndirectArgumentDesc {
    pub kind: u32,
    pub data: [u32; 3],
}

#[repr(C)]
pub struct CommandSignatureDesc {
    pub byte_stride: u32,
    pub argument_count: u32,
    pub arguments: *const IndirectArgumentDesc,
    pub node_mask: u32,
}

/// D3D12_MESSAGE as the info queue stores it.
#[repr(C)]
pub struct Message {
    pub category: u32,
    pub severity: u32,
    pub id: u32,
    pub description: *const c_char,
    pub description_length: usize,
}

// Feature data (D3D12_FEATURE_DATA_*).

#[repr(C)]
#[derive(Default)]
pub struct OptionsData {
    pub double_precision_float_shader_ops: i32,
    pub output_merger_logic_op: i32,
    pub min_precision_support: u32,
    pub tiled_resources_tier: u32,
    pub resource_binding_tier: u32,
    pub ps_specified_stencil_ref_supported: i32,
    pub typed_uav_load_additional_formats: i32,
    pub rovs_supported: i32,
    pub conservative_rasterization_tier: u32,
    pub max_gpu_virtual_address_bits_per_resource: u32,
    pub standard_swizzle_64kb_supported: i32,
    pub cross_node_sharing_tier: u32,
    pub cross_adapter_row_major_texture_supported: i32,
    pub viewport_and_render_target_array_index_without_geometry_shader: i32,
    pub resource_heap_tier: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct Options1Data {
    pub wave_ops: i32,
    pub wave_lane_count_min: u32,
    pub wave_lane_count_max: u32,
    pub total_lane_count: u32,
    pub expanded_compute_resource_states: i32,
    pub int64_shader_ops: i32,
}

#[repr(C)]
#[derive(Default)]
pub struct Options5Data {
    pub srv_only_tiled_resource_tier3: i32,
    pub render_passes_tier: u32,
    pub raytracing_tier: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct Options7Data {
    pub mesh_shader_tier: u32,
    pub sampler_feedback_tier: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct Options9Data {
    pub mesh_shader_pipeline_stats_supported: i32,
    pub mesh_shader_supports_full_range_render_target_array_index: i32,
    pub atomic_int64_on_typed_resource_supported: i32,
    pub atomic_int64_on_group_shared_supported: i32,
    pub derivatives_in_mesh_and_amplification_shaders_supported: i32,
    pub wave_matrix_tier: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct Options11Data {
    pub atomic_int64_on_descriptor_heap_resource_supported: i32,
}

#[repr(C)]
#[derive(Default)]
pub struct ArchitectureData {
    pub node_index: u32,
    pub tile_based_renderer: i32,
    pub unified_memory: i32,
    pub cache_coherent_unified_memory: i32,
}

#[repr(C)]
pub struct FeatureLevelsData {
    pub count: u32,
    pub requested: *const u32,
    pub highest: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct FormatSupportData {
    pub format: u32,
    pub support1: u32,
    pub support2: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct MultisampleQualityLevelsData {
    pub format: u32,
    pub sample_count: u32,
    pub flags: u32,
    pub quality_levels: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct ShaderModelData {
    pub highest: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The sizes and offsets `d3d12.h` gives on 64-bit Windows: a structure
    /// laid out differently would be read wrongly by the runtime.
    #[test]
    fn the_structures_have_the_layouts_of_the_headers() {
        assert_eq!(size_of::<AdapterDesc1>(), 312);
        assert_eq!(size_of::<ResourceDesc>(), 56);
        assert_eq!(offset_of!(ResourceDesc, format), 32);
        assert_eq!(size_of::<ClearValue>(), 20);
        assert_eq!(size_of::<ResourceBarrier>(), 32);
        assert_eq!(size_of::<ShaderResourceViewDesc>(), 40);
        assert_eq!(size_of::<RenderTargetViewDesc>(), 24);
        assert_eq!(size_of::<SamplerDesc>(), 52);
        assert_eq!(size_of::<StaticSamplerDesc>(), 52);
        assert_eq!(size_of::<RootParameter>(), 32);
        assert_eq!(offset_of!(RootParameter, visibility), 24);
        assert_eq!(size_of::<RootSignatureDesc>(), 40);
        assert_eq!(size_of::<RenderTargetBlendDesc>(), 40);
        assert_eq!(size_of::<BlendDesc>(), 328);
        assert_eq!(size_of::<RasterizerDesc>(), 44);
        assert_eq!(size_of::<DepthStencilDesc>(), 52);
        assert_eq!(offset_of!(GraphicsPipelineDesc, blend), 120);
        assert_eq!(offset_of!(GraphicsPipelineDesc, sample_mask), 448);
        assert_eq!(offset_of!(GraphicsPipelineDesc, input_layout), 552);
        assert_eq!(offset_of!(GraphicsPipelineDesc, render_target_formats), 580);
        assert_eq!(offset_of!(GraphicsPipelineDesc, cached), 632);
        assert_eq!(size_of::<GraphicsPipelineDesc>(), 656);
        assert_eq!(size_of::<ComputePipelineDesc>(), 56);
        assert_eq!(size_of::<InputElementDesc>(), 32);
        assert_eq!(size_of::<PlacedFootprint>(), 32);
        assert_eq!(size_of::<TextureCopyLocation>(), 48);
        assert_eq!(offset_of!(TextureCopyLocation, data), 16);
        assert_eq!(size_of::<CommandSignatureDesc>(), 24);
        assert_eq!(size_of::<IndirectArgumentDesc>(), 16);
        assert_eq!(size_of::<Message>(), 32);
        assert_eq!(offset_of!(Message, description), 16);
        assert_eq!(size_of::<OptionsData>(), 60);
        assert_eq!(size_of::<FeatureLevelsData>(), 24);
    }
}
