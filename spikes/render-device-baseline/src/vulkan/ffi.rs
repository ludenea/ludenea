//! The Vulkan declarations the module uses: constants, structures in the field
//! order of `vulkan_core.h`, and the tables of entry points fetched through
//! `vkGetInstanceProcAddr` and `vkGetDeviceProcAddr`. Throwaway, never
//! product code.

use std::ffi::{c_char, c_void};

use crate::loader::c_string;

pub type Handle = *mut c_void;
pub type Object = u64;
pub type Allocator = *const c_void;
pub type VkResult = i32;
pub type Bool32 = u32;
pub type VoidFunction = unsafe extern "system" fn();
pub type GetProcAddr = unsafe extern "system" fn(Handle, *const c_char) -> Option<VoidFunction>;
pub type DebugCallback = unsafe extern "system" fn(u32, u32, *const DebugUtilsMessengerCallbackData, *mut c_void) -> Bool32;

pub const fn make_version(major: u32, minor: u32) -> u32 {
    (major << 22) | (minor << 12)
}

pub const API_VERSION_1_0: u32 = make_version(1, 0);
pub const API_VERSION_1_1: u32 = make_version(1, 1);
pub const API_VERSION_1_2: u32 = make_version(1, 2);
pub const API_VERSION_1_3: u32 = make_version(1, 3);

pub const SUCCESS: VkResult = 0;
pub const NOT_READY: VkResult = 1;

pub const FORMAT_R8G8B8A8_UNORM: u32 = 37;
pub const FORMAT_R8G8B8A8_SRGB: u32 = 43;
pub const FORMAT_R16G16B16A16_SFLOAT: u32 = 97;
pub const FORMAT_R32_UINT: u32 = 98;
pub const FORMAT_R32G32_SFLOAT: u32 = 103;
pub const FORMAT_R32G32B32_SFLOAT: u32 = 106;
pub const FORMAT_R32G32B32A32_SFLOAT: u32 = 109;
pub const FORMAT_D32_SFLOAT: u32 = 126;

pub const FORMAT_FEATURE_SAMPLED_IMAGE: u32 = 0x1;
pub const FORMAT_FEATURE_STORAGE_IMAGE: u32 = 0x2;
pub const FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT: u32 = 0x200;

pub const IMAGE_TYPE_2D: u32 = 1;
pub const IMAGE_VIEW_TYPE_2D: u32 = 1;
pub const IMAGE_TILING_OPTIMAL: u32 = 0;
pub const IMAGE_CREATE_MUTABLE_FORMAT: u32 = 0x8;
pub const IMAGE_USAGE_TRANSFER_SRC: u32 = 0x1;
pub const IMAGE_USAGE_TRANSFER_DST: u32 = 0x2;
pub const IMAGE_USAGE_SAMPLED: u32 = 0x4;
pub const IMAGE_USAGE_STORAGE: u32 = 0x8;
pub const IMAGE_USAGE_COLOR_ATTACHMENT: u32 = 0x10;
pub const IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT: u32 = 0x20;

pub const SAMPLE_COUNT_1: u32 = 0x1;
pub const SAMPLE_COUNT_4: u32 = 0x4;
pub const SHARING_MODE_EXCLUSIVE: u32 = 0;

pub const LAYOUT_UNDEFINED: u32 = 0;
pub const LAYOUT_GENERAL: u32 = 1;
pub const LAYOUT_COLOR_ATTACHMENT_OPTIMAL: u32 = 2;
pub const LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL: u32 = 3;
pub const LAYOUT_SHADER_READ_ONLY_OPTIMAL: u32 = 5;
pub const LAYOUT_TRANSFER_SRC_OPTIMAL: u32 = 6;
pub const LAYOUT_TRANSFER_DST_OPTIMAL: u32 = 7;

pub const ASPECT_COLOR: u32 = 0x1;
pub const ASPECT_DEPTH: u32 = 0x2;

pub const LOAD_OP_LOAD: u32 = 0;
pub const LOAD_OP_CLEAR: u32 = 1;
pub const LOAD_OP_DONT_CARE: u32 = 2;
pub const STORE_OP_STORE: u32 = 0;
pub const STORE_OP_DONT_CARE: u32 = 1;
pub const SUBPASS_EXTERNAL: u32 = !0;

pub const BIND_POINT_GRAPHICS: u32 = 0;
pub const BIND_POINT_COMPUTE: u32 = 1;
pub const STAGE_VERTEX: u32 = 0x1;
pub const STAGE_FRAGMENT: u32 = 0x10;
pub const STAGE_COMPUTE: u32 = 0x20;

pub const INPUT_RATE_VERTEX: u32 = 0;
pub const TOPOLOGY_TRIANGLE_LIST: u32 = 3;
pub const POLYGON_MODE_FILL: u32 = 0;
pub const CULL_MODE_NONE: u32 = 0;
pub const FRONT_FACE_COUNTER_CLOCKWISE: u32 = 0;
pub const COMPARE_OP_LESS_OR_EQUAL: u32 = 3;
pub const COMPARE_OP_ALWAYS: u32 = 7;
pub const BLEND_FACTOR_ONE: u32 = 1;
pub const BLEND_FACTOR_SRC_ALPHA: u32 = 6;
pub const BLEND_FACTOR_ONE_MINUS_SRC_ALPHA: u32 = 7;
pub const BLEND_OP_ADD: u32 = 0;
pub const COLOR_COMPONENTS_RGBA: u32 = 0xf;

pub const FILTER_NEAREST: u32 = 0;
pub const FILTER_LINEAR: u32 = 1;
pub const MIPMAP_MODE_NEAREST: u32 = 0;
pub const MIPMAP_MODE_LINEAR: u32 = 1;
pub const ADDRESS_MODE_REPEAT: u32 = 0;
pub const ADDRESS_MODE_CLAMP_TO_EDGE: u32 = 2;
pub const BORDER_COLOR_FLOAT_TRANSPARENT_BLACK: u32 = 0;
/// `VK_LOD_CLAMP_NONE`: the full range of levels.
pub const LOD_CLAMP_NONE: f32 = 1000.0;

pub const DESCRIPTOR_SAMPLER: u32 = 0;
pub const DESCRIPTOR_COMBINED_IMAGE_SAMPLER: u32 = 1;
pub const DESCRIPTOR_SAMPLED_IMAGE: u32 = 2;
pub const DESCRIPTOR_STORAGE_IMAGE: u32 = 3;
pub const DESCRIPTOR_UNIFORM_BUFFER: u32 = 6;
pub const DESCRIPTOR_STORAGE_BUFFER: u32 = 7;

pub const BUFFER_USAGE_TRANSFER_SRC: u32 = 0x1;
pub const BUFFER_USAGE_TRANSFER_DST: u32 = 0x2;
pub const BUFFER_USAGE_UNIFORM: u32 = 0x10;
pub const BUFFER_USAGE_STORAGE: u32 = 0x20;
pub const BUFFER_USAGE_VERTEX: u32 = 0x80;
pub const BUFFER_USAGE_INDIRECT: u32 = 0x100;

pub const MEMORY_DEVICE_LOCAL: u32 = 0x1;
pub const MEMORY_HOST_VISIBLE: u32 = 0x2;
pub const MEMORY_HOST_COHERENT: u32 = 0x4;

pub const PIPELINE_STAGE_TOP_OF_PIPE: u32 = 0x1;
pub const PIPELINE_STAGE_FRAGMENT_SHADER: u32 = 0x80;
pub const PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT: u32 = 0x400;
pub const PIPELINE_STAGE_COMPUTE_SHADER: u32 = 0x800;
pub const PIPELINE_STAGE_TRANSFER: u32 = 0x1000;
pub const PIPELINE_STAGE_BOTTOM_OF_PIPE: u32 = 0x2000;
pub const PIPELINE_STAGE_HOST: u32 = 0x4000;

pub const ACCESS_NONE: u32 = 0;
pub const ACCESS_SHADER_READ: u32 = 0x20;
pub const ACCESS_SHADER_WRITE: u32 = 0x40;
pub const ACCESS_COLOR_ATTACHMENT_READ: u32 = 0x80;
pub const ACCESS_COLOR_ATTACHMENT_WRITE: u32 = 0x100;
pub const ACCESS_TRANSFER_READ: u32 = 0x800;
pub const ACCESS_TRANSFER_WRITE: u32 = 0x1000;
pub const ACCESS_HOST_READ: u32 = 0x2000;

pub const COMMAND_BUFFER_LEVEL_PRIMARY: u32 = 0;
pub const COMMAND_BUFFER_LEVEL_SECONDARY: u32 = 1;
pub const COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT: u32 = 0x1;
pub const COMMAND_BUFFER_USAGE_RENDER_PASS_CONTINUE: u32 = 0x2;
pub const SUBPASS_CONTENTS_INLINE: u32 = 0;
pub const SUBPASS_CONTENTS_SECONDARY_COMMAND_BUFFERS: u32 = 1;

pub const QUERY_TYPE_TIMESTAMP: u32 = 2;
pub const QUERY_RESULT_64: u32 = 0x1;
pub const QUERY_RESULT_WAIT: u32 = 0x2;

pub const QUEUE_GRAPHICS: u32 = 0x1;
pub const QUEUE_COMPUTE: u32 = 0x2;
pub const QUEUE_FAMILY_IGNORED: u32 = !0;
pub const WHOLE_SIZE: u64 = !0;

pub const DEBUG_SEVERITY_WARNING: u32 = 0x100;
pub const DEBUG_SEVERITY_ERROR: u32 = 0x1000;
pub const DEBUG_TYPE_GENERAL: u32 = 0x1;
pub const DEBUG_TYPE_VALIDATION: u32 = 0x2;
pub const DEBUG_TYPE_PERFORMANCE: u32 = 0x4;

/// `VkStructureType` values of the structures the module fills.
pub mod structure {
    pub const APPLICATION_INFO: u32 = 0;
    pub const INSTANCE_CREATE_INFO: u32 = 1;
    pub const DEVICE_QUEUE_CREATE_INFO: u32 = 2;
    pub const DEVICE_CREATE_INFO: u32 = 3;
    pub const SUBMIT_INFO: u32 = 4;
    pub const MEMORY_ALLOCATE_INFO: u32 = 5;
    pub const MAPPED_MEMORY_RANGE: u32 = 6;
    pub const FENCE_CREATE_INFO: u32 = 8;
    pub const QUERY_POOL_CREATE_INFO: u32 = 11;
    pub const BUFFER_CREATE_INFO: u32 = 12;
    pub const IMAGE_CREATE_INFO: u32 = 14;
    pub const IMAGE_VIEW_CREATE_INFO: u32 = 15;
    pub const SHADER_MODULE_CREATE_INFO: u32 = 16;
    pub const PIPELINE_SHADER_STAGE_CREATE_INFO: u32 = 18;
    pub const PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO: u32 = 19;
    pub const PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO: u32 = 20;
    pub const PIPELINE_VIEWPORT_STATE_CREATE_INFO: u32 = 22;
    pub const PIPELINE_RASTERIZATION_STATE_CREATE_INFO: u32 = 23;
    pub const PIPELINE_MULTISAMPLE_STATE_CREATE_INFO: u32 = 24;
    pub const PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO: u32 = 25;
    pub const PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: u32 = 26;
    pub const GRAPHICS_PIPELINE_CREATE_INFO: u32 = 28;
    pub const COMPUTE_PIPELINE_CREATE_INFO: u32 = 29;
    pub const PIPELINE_LAYOUT_CREATE_INFO: u32 = 30;
    pub const SAMPLER_CREATE_INFO: u32 = 31;
    pub const DESCRIPTOR_SET_LAYOUT_CREATE_INFO: u32 = 32;
    pub const DESCRIPTOR_POOL_CREATE_INFO: u32 = 33;
    pub const DESCRIPTOR_SET_ALLOCATE_INFO: u32 = 34;
    pub const WRITE_DESCRIPTOR_SET: u32 = 35;
    pub const FRAMEBUFFER_CREATE_INFO: u32 = 37;
    pub const RENDER_PASS_CREATE_INFO: u32 = 38;
    pub const COMMAND_POOL_CREATE_INFO: u32 = 39;
    pub const COMMAND_BUFFER_ALLOCATE_INFO: u32 = 40;
    pub const COMMAND_BUFFER_INHERITANCE_INFO: u32 = 41;
    pub const COMMAND_BUFFER_BEGIN_INFO: u32 = 42;
    pub const RENDER_PASS_BEGIN_INFO: u32 = 43;
    pub const BUFFER_MEMORY_BARRIER: u32 = 44;
    pub const IMAGE_MEMORY_BARRIER: u32 = 45;
    pub const PHYSICAL_DEVICE_VULKAN_1_1_FEATURES: u32 = 49;
    pub const PHYSICAL_DEVICE_VULKAN_1_2_FEATURES: u32 = 51;
    pub const PHYSICAL_DEVICE_VULKAN_1_3_FEATURES: u32 = 53;
    pub const PHYSICAL_DEVICE_FEATURES_2: u32 = 1_000_059_000;
    pub const PHYSICAL_DEVICE_PROPERTIES_2: u32 = 1_000_059_001;
    pub const DEBUG_UTILS_MESSENGER_CREATE_INFO: u32 = 1_000_128_004;
    pub const PHYSICAL_DEVICE_ACCELERATION_STRUCTURE_FEATURES: u32 = 1_000_150_013;
    pub const PHYSICAL_DEVICE_DESCRIPTOR_INDEXING_FEATURES: u32 = 1_000_161_001;
    pub const PHYSICAL_DEVICE_SHADER_ATOMIC_INT64_FEATURES: u32 = 1_000_180_000;
    pub const PHYSICAL_DEVICE_DRIVER_PROPERTIES: u32 = 1_000_196_000;
    pub const PHYSICAL_DEVICE_MESH_SHADER_FEATURES: u32 = 1_000_328_000;
    pub const PHYSICAL_DEVICE_RAY_QUERY_FEATURES: u32 = 1_000_348_013;
}

// --- structures, in the field order of vulkan_core.h ---------------------------

#[repr(C)]
pub struct ApplicationInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub application_name: *const c_char,
    pub application_version: u32,
    pub engine_name: *const c_char,
    pub engine_version: u32,
    pub api_version: u32,
}

#[repr(C)]
pub struct InstanceCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub application_info: *const ApplicationInfo,
    pub enabled_layer_count: u32,
    pub enabled_layer_names: *const *const c_char,
    pub enabled_extension_count: u32,
    pub enabled_extension_names: *const *const c_char,
}

#[repr(C)]
pub struct LayerProperties {
    pub layer_name: [u8; 256],
    pub spec_version: u32,
    pub implementation_version: u32,
    pub description: [u8; 256],
}

#[repr(C)]
pub struct ExtensionProperties {
    pub extension_name: [u8; 256],
    pub spec_version: u32,
}

#[repr(C)]
pub struct DebugUtilsMessengerCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub message_severity: u32,
    pub message_type: u32,
    pub user_callback: DebugCallback,
    pub user_data: *mut c_void,
}

/// The head of `VkDebugUtilsMessengerCallbackDataEXT`: the fields up to the
/// message, which is all the callback reads.
#[repr(C)]
pub struct DebugUtilsMessengerCallbackData {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub message_id_name: *const c_char,
    pub message_id_number: i32,
    pub message: *const c_char,
}

#[repr(C)]
pub struct DeviceQueueCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub queue_family_index: u32,
    pub queue_count: u32,
    pub queue_priorities: *const f32,
}

#[repr(C)]
pub struct DeviceCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub queue_create_info_count: u32,
    pub queue_create_infos: *const DeviceQueueCreateInfo,
    pub enabled_layer_count: u32,
    pub enabled_layer_names: *const *const c_char,
    pub enabled_extension_count: u32,
    pub enabled_extension_names: *const *const c_char,
    pub enabled_features: *const CoreFeatures,
}

/// `VkPhysicalDeviceFeatures`: 55 booleans, indexed by the constants of
/// [`feature`].
pub type CoreFeatures = [Bool32; 55];

/// Positions in `VkPhysicalDeviceFeatures`.
pub mod feature {
    pub const MULTI_DRAW_INDIRECT: usize = 9;
    pub const DRAW_INDIRECT_FIRST_INSTANCE: usize = 10;
    pub const TEXTURE_COMPRESSION_ETC2: usize = 20;
    pub const TEXTURE_COMPRESSION_ASTC_LDR: usize = 21;
    pub const TEXTURE_COMPRESSION_BC: usize = 22;
    pub const SHADER_SAMPLED_IMAGE_ARRAY_DYNAMIC_INDEXING: usize = 34;
}

/// Positions in `VkPhysicalDeviceVulkan12Features` after its header.
pub mod feature12 {
    pub const DRAW_INDIRECT_COUNT: usize = 1;
    pub const SHADER_BUFFER_INT64_ATOMICS: usize = 5;
    pub const SHADER_SHARED_INT64_ATOMICS: usize = 6;
    pub const DESCRIPTOR_INDEXING: usize = 9;
    pub const SHADER_SAMPLED_IMAGE_ARRAY_NON_UNIFORM_INDEXING: usize = 14;
    pub const DESCRIPTOR_BINDING_PARTIALLY_BOUND: usize = 27;
    pub const DESCRIPTOR_BINDING_VARIABLE_DESCRIPTOR_COUNT: usize = 28;
    pub const RUNTIME_DESCRIPTOR_ARRAY: usize = 29;
    /// `VkPhysicalDeviceDescriptorIndexingFeatures` holds the same fields
    /// from `shaderInputAttachmentArrayDynamicIndexing` on, which sits at 10.
    pub const DESCRIPTOR_INDEXING_STRUCTURE_OFFSET: usize = 10;
}

/// Any `VkPhysicalDevice*Features` structure: the header, then booleans.
/// Sixty booleans hold the largest structure read here.
#[repr(C)]
pub struct FeatureBlock {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub bits: [Bool32; 60],
}

impl FeatureBlock {
    pub fn new(s_type: u32) -> FeatureBlock {
        FeatureBlock { s_type, p_next: std::ptr::null_mut(), bits: [0; 60] }
    }
}

/// `VkPhysicalDeviceProperties`, read by offset: 824 bytes.
#[repr(C, align(8))]
pub struct PropertiesBytes(pub [u8; 824]);

/// Offsets in `VkPhysicalDeviceProperties`; the limits start at 296.
pub mod property {
    pub const API_VERSION: usize = 0;
    pub const DRIVER_VERSION: usize = 4;
    pub const VENDOR_ID: usize = 8;
    pub const DEVICE_ID: usize = 12;
    pub const DEVICE_TYPE: usize = 16;
    pub const DEVICE_NAME: usize = 20;
    const LIMITS: usize = 296;
    pub const MAX_STORAGE_BUFFER_RANGE: usize = LIMITS + 28;
    pub const MAX_PER_STAGE_DESCRIPTOR_STORAGE_BUFFERS: usize = LIMITS + 76;
    pub const MAX_PER_STAGE_DESCRIPTOR_SAMPLED_IMAGES: usize = LIMITS + 80;
    pub const MAX_PER_STAGE_DESCRIPTOR_STORAGE_IMAGES: usize = LIMITS + 84;
    pub const MAX_COMPUTE_WORK_GROUP_INVOCATIONS: usize = LIMITS + 232;
    pub const MAX_DRAW_INDIRECT_COUNT: usize = LIMITS + 264;
    pub const FRAMEBUFFER_COLOR_SAMPLE_COUNTS: usize = LIMITS + 376;
    pub const FRAMEBUFFER_DEPTH_SAMPLE_COUNTS: usize = LIMITS + 380;
    pub const MAX_COLOR_ATTACHMENTS: usize = LIMITS + 392;
    pub const TIMESTAMP_COMPUTE_AND_GRAPHICS: usize = LIMITS + 420;
    pub const TIMESTAMP_PERIOD: usize = LIMITS + 424;
    pub const STANDARD_SAMPLE_LOCATIONS: usize = LIMITS + 472;
}

impl PropertiesBytes {
    pub fn zeroed() -> PropertiesBytes {
        PropertiesBytes([0; 824])
    }

    pub fn u32_at(&self, offset: usize) -> u32 {
        u32::from_le_bytes([self.0[offset], self.0[offset + 1], self.0[offset + 2], self.0[offset + 3]])
    }

    pub fn f32_at(&self, offset: usize) -> f32 {
        f32::from_bits(self.u32_at(offset))
    }
}

#[repr(C)]
pub struct Properties2 {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub properties: PropertiesBytes,
}

#[repr(C)]
pub struct DriverProperties {
    pub s_type: u32,
    pub p_next: *mut c_void,
    pub driver_id: u32,
    pub driver_name: [u8; 256],
    pub driver_info: [u8; 256],
    pub conformance_version: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct QueueFamilyProperties {
    pub queue_flags: u32,
    pub queue_count: u32,
    pub timestamp_valid_bits: u32,
    pub min_image_transfer_granularity: [u32; 3],
}

#[repr(C)]
pub struct FormatProperties {
    pub linear_tiling_features: u32,
    pub optimal_tiling_features: u32,
    pub buffer_features: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemoryType {
    pub property_flags: u32,
    pub heap_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemoryHeap {
    pub size: u64,
    pub flags: u32,
}

#[repr(C)]
pub struct PhysicalDeviceMemoryProperties {
    pub memory_type_count: u32,
    pub memory_types: [MemoryType; 32],
    pub memory_heap_count: u32,
    pub memory_heaps: [MemoryHeap; 16],
}

#[repr(C)]
pub struct MemoryRequirements {
    pub size: u64,
    pub alignment: u64,
    pub memory_type_bits: u32,
}

#[repr(C)]
pub struct MemoryAllocateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub allocation_size: u64,
    pub memory_type_index: u32,
}

#[repr(C)]
pub struct MappedMemoryRange {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub memory: Object,
    pub offset: u64,
    pub size: u64,
}

#[repr(C)]
pub struct BufferCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub size: u64,
    pub usage: u32,
    pub sharing_mode: u32,
    pub queue_family_index_count: u32,
    pub queue_family_indices: *const u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Extent3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

#[repr(C)]
pub struct ImageCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub image_type: u32,
    pub format: u32,
    pub extent: Extent3D,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub samples: u32,
    pub tiling: u32,
    pub usage: u32,
    pub sharing_mode: u32,
    pub queue_family_index_count: u32,
    pub queue_family_indices: *const u32,
    pub initial_layout: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubresourceRange {
    pub aspect_mask: u32,
    pub base_mip_level: u32,
    pub level_count: u32,
    pub base_array_layer: u32,
    pub layer_count: u32,
}

#[repr(C)]
pub struct ImageViewCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub image: Object,
    pub view_type: u32,
    pub format: u32,
    pub components: [u32; 4],
    pub subresource_range: SubresourceRange,
}

#[repr(C)]
pub struct SamplerCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub mag_filter: u32,
    pub min_filter: u32,
    pub mipmap_mode: u32,
    pub address_mode_u: u32,
    pub address_mode_v: u32,
    pub address_mode_w: u32,
    pub mip_lod_bias: f32,
    pub anisotropy_enable: Bool32,
    pub max_anisotropy: f32,
    pub compare_enable: Bool32,
    pub compare_op: u32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub border_color: u32,
    pub unnormalized_coordinates: Bool32,
}

#[repr(C)]
pub struct ShaderModuleCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub code_size: usize,
    pub code: *const u32,
}

#[repr(C)]
pub struct PipelineShaderStageCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub stage: u32,
    pub module: Object,
    pub name: *const c_char,
    pub specialization_info: *const c_void,
}

#[repr(C)]
pub struct VertexInputBindingDescription {
    pub binding: u32,
    pub stride: u32,
    pub input_rate: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VertexInputAttributeDescription {
    pub location: u32,
    pub binding: u32,
    pub format: u32,
    pub offset: u32,
}

#[repr(C)]
pub struct PipelineVertexInputStateCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub vertex_binding_description_count: u32,
    pub vertex_binding_descriptions: *const VertexInputBindingDescription,
    pub vertex_attribute_description_count: u32,
    pub vertex_attribute_descriptions: *const VertexInputAttributeDescription,
}

#[repr(C)]
pub struct PipelineInputAssemblyStateCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub topology: u32,
    pub primitive_restart_enable: Bool32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect2D {
    pub offset: [i32; 2],
    pub extent: [u32; 2],
}

#[repr(C)]
pub struct PipelineViewportStateCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub viewport_count: u32,
    pub viewports: *const Viewport,
    pub scissor_count: u32,
    pub scissors: *const Rect2D,
}

#[repr(C)]
pub struct PipelineRasterizationStateCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub depth_clamp_enable: Bool32,
    pub rasterizer_discard_enable: Bool32,
    pub polygon_mode: u32,
    pub cull_mode: u32,
    pub front_face: u32,
    pub depth_bias_enable: Bool32,
    pub depth_bias_constant_factor: f32,
    pub depth_bias_clamp: f32,
    pub depth_bias_slope_factor: f32,
    pub line_width: f32,
}

#[repr(C)]
pub struct PipelineMultisampleStateCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub rasterization_samples: u32,
    pub sample_shading_enable: Bool32,
    pub min_sample_shading: f32,
    pub sample_mask: *const u32,
    pub alpha_to_coverage_enable: Bool32,
    pub alpha_to_one_enable: Bool32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StencilOpState {
    pub fail_op: u32,
    pub pass_op: u32,
    pub depth_fail_op: u32,
    pub compare_op: u32,
    pub compare_mask: u32,
    pub write_mask: u32,
    pub reference: u32,
}

#[repr(C)]
pub struct PipelineDepthStencilStateCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub depth_test_enable: Bool32,
    pub depth_write_enable: Bool32,
    pub depth_compare_op: u32,
    pub depth_bounds_test_enable: Bool32,
    pub stencil_test_enable: Bool32,
    pub front: StencilOpState,
    pub back: StencilOpState,
    pub min_depth_bounds: f32,
    pub max_depth_bounds: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PipelineColorBlendAttachmentState {
    pub blend_enable: Bool32,
    pub src_color_blend_factor: u32,
    pub dst_color_blend_factor: u32,
    pub color_blend_op: u32,
    pub src_alpha_blend_factor: u32,
    pub dst_alpha_blend_factor: u32,
    pub alpha_blend_op: u32,
    pub color_write_mask: u32,
}

#[repr(C)]
pub struct PipelineColorBlendStateCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub logic_op_enable: Bool32,
    pub logic_op: u32,
    pub attachment_count: u32,
    pub attachments: *const PipelineColorBlendAttachmentState,
    pub blend_constants: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PushConstantRange {
    pub stage_flags: u32,
    pub offset: u32,
    pub size: u32,
}

#[repr(C)]
pub struct PipelineLayoutCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub set_layout_count: u32,
    pub set_layouts: *const Object,
    pub push_constant_range_count: u32,
    pub push_constant_ranges: *const PushConstantRange,
}

#[repr(C)]
pub struct GraphicsPipelineCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub stage_count: u32,
    pub stages: *const PipelineShaderStageCreateInfo,
    pub vertex_input_state: *const PipelineVertexInputStateCreateInfo,
    pub input_assembly_state: *const PipelineInputAssemblyStateCreateInfo,
    pub tessellation_state: *const c_void,
    pub viewport_state: *const PipelineViewportStateCreateInfo,
    pub rasterization_state: *const PipelineRasterizationStateCreateInfo,
    pub multisample_state: *const PipelineMultisampleStateCreateInfo,
    pub depth_stencil_state: *const PipelineDepthStencilStateCreateInfo,
    pub color_blend_state: *const PipelineColorBlendStateCreateInfo,
    pub dynamic_state: *const c_void,
    pub layout: Object,
    pub render_pass: Object,
    pub subpass: u32,
    pub base_pipeline_handle: Object,
    pub base_pipeline_index: i32,
}

#[repr(C)]
pub struct ComputePipelineCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub stage: PipelineShaderStageCreateInfo,
    pub layout: Object,
    pub base_pipeline_handle: Object,
    pub base_pipeline_index: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AttachmentDescription {
    pub flags: u32,
    pub format: u32,
    pub samples: u32,
    pub load_op: u32,
    pub store_op: u32,
    pub stencil_load_op: u32,
    pub stencil_store_op: u32,
    pub initial_layout: u32,
    pub final_layout: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AttachmentReference {
    pub attachment: u32,
    pub layout: u32,
}

#[repr(C)]
pub struct SubpassDescription {
    pub flags: u32,
    pub pipeline_bind_point: u32,
    pub input_attachment_count: u32,
    pub input_attachments: *const AttachmentReference,
    pub color_attachment_count: u32,
    pub color_attachments: *const AttachmentReference,
    pub resolve_attachments: *const AttachmentReference,
    pub depth_stencil_attachment: *const AttachmentReference,
    pub preserve_attachment_count: u32,
    pub preserve_attachments: *const u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubpassDependency {
    pub src_subpass: u32,
    pub dst_subpass: u32,
    pub src_stage_mask: u32,
    pub dst_stage_mask: u32,
    pub src_access_mask: u32,
    pub dst_access_mask: u32,
    pub dependency_flags: u32,
}

#[repr(C)]
pub struct RenderPassCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub attachment_count: u32,
    pub attachments: *const AttachmentDescription,
    pub subpass_count: u32,
    pub subpasses: *const SubpassDescription,
    pub dependency_count: u32,
    pub dependencies: *const SubpassDependency,
}

#[repr(C)]
pub struct FramebufferCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub render_pass: Object,
    pub attachment_count: u32,
    pub attachments: *const Object,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

#[repr(C)]
pub struct DescriptorSetLayoutBinding {
    pub binding: u32,
    pub descriptor_type: u32,
    pub descriptor_count: u32,
    pub stage_flags: u32,
    pub immutable_samplers: *const Object,
}

#[repr(C)]
pub struct DescriptorSetLayoutCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub binding_count: u32,
    pub bindings: *const DescriptorSetLayoutBinding,
}

#[repr(C)]
pub struct DescriptorPoolSize {
    pub descriptor_type: u32,
    pub descriptor_count: u32,
}

#[repr(C)]
pub struct DescriptorPoolCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub max_sets: u32,
    pub pool_size_count: u32,
    pub pool_sizes: *const DescriptorPoolSize,
}

#[repr(C)]
pub struct DescriptorSetAllocateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub descriptor_pool: Object,
    pub descriptor_set_count: u32,
    pub set_layouts: *const Object,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DescriptorImageInfo {
    pub sampler: Object,
    pub image_view: Object,
    pub image_layout: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DescriptorBufferInfo {
    pub buffer: Object,
    pub offset: u64,
    pub range: u64,
}

#[repr(C)]
pub struct WriteDescriptorSet {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub dst_set: Object,
    pub dst_binding: u32,
    pub dst_array_element: u32,
    pub descriptor_count: u32,
    pub descriptor_type: u32,
    pub image_info: *const DescriptorImageInfo,
    pub buffer_info: *const DescriptorBufferInfo,
    pub texel_buffer_view: *const Object,
}

#[repr(C)]
pub struct CommandPoolCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub queue_family_index: u32,
}

#[repr(C)]
pub struct CommandBufferAllocateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub command_pool: Object,
    pub level: u32,
    pub command_buffer_count: u32,
}

#[repr(C)]
pub struct CommandBufferInheritanceInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub render_pass: Object,
    pub subpass: u32,
    pub framebuffer: Object,
    pub occlusion_query_enable: Bool32,
    pub query_flags: u32,
    pub pipeline_statistics: u32,
}

#[repr(C)]
pub struct CommandBufferBeginInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub inheritance_info: *const CommandBufferInheritanceInfo,
}

/// `VkClearValue`: a color as four floats, or a depth in the first float.
pub type ClearValue = [f32; 4];

#[repr(C)]
pub struct RenderPassBeginInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub render_pass: Object,
    pub framebuffer: Object,
    pub render_area: Rect2D,
    pub clear_value_count: u32,
    pub clear_values: *const ClearValue,
}

#[repr(C)]
pub struct BufferImageCopy {
    pub buffer_offset: u64,
    pub buffer_row_length: u32,
    pub buffer_image_height: u32,
    /// Aspect, level, first layer, layer count.
    pub image_subresource: [u32; 4],
    pub image_offset: [i32; 3],
    pub image_extent: Extent3D,
}

#[repr(C)]
pub struct ImageMemoryBarrier {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub src_access_mask: u32,
    pub dst_access_mask: u32,
    pub old_layout: u32,
    pub new_layout: u32,
    pub src_queue_family_index: u32,
    pub dst_queue_family_index: u32,
    pub image: Object,
    pub subresource_range: SubresourceRange,
}

#[repr(C)]
pub struct BufferMemoryBarrier {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub src_access_mask: u32,
    pub dst_access_mask: u32,
    pub src_queue_family_index: u32,
    pub dst_queue_family_index: u32,
    pub buffer: Object,
    pub offset: u64,
    pub size: u64,
}

#[repr(C)]
pub struct SubmitInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub wait_semaphore_count: u32,
    pub wait_semaphores: *const Object,
    pub wait_dst_stage_mask: *const u32,
    pub command_buffer_count: u32,
    pub command_buffers: *const Handle,
    pub signal_semaphore_count: u32,
    pub signal_semaphores: *const Object,
}

#[repr(C)]
pub struct FenceCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
}

#[repr(C)]
pub struct QueryPoolCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub query_type: u32,
    pub query_count: u32,
    pub pipeline_statistics: u32,
}

// --- entry points --------------------------------------------------------------

/// Fetches an entry point through a `vkGet*ProcAddr` and casts it to its type.
///
/// # Safety
///
/// `F` must be the entry point's signature.
pub unsafe fn entry<F: Copy>(get: GetProcAddr, owner: Handle, name: &str) -> Result<F, String> {
    let c_name = c_string(name);
    // SAFETY: `get` is the loader's or the device's lookup function and the
    // name is NUL-terminated.
    let address = unsafe { get(owner, c_name.as_ptr()) }.ok_or_else(|| format!("{name} is not provided"))?;
    if std::mem::size_of::<F>() != std::mem::size_of::<VoidFunction>() {
        return Err(format!("{name}: the requested type is not a function pointer"));
    }
    // SAFETY: the caller states that `F` is the entry point's signature, and
    // both are function pointers of one size.
    Ok(unsafe { std::mem::transmute_copy::<VoidFunction, F>(&address) })
}

/// Declares a table of entry points and the function that fetches them.
macro_rules! entry_points {
    ($table:ident { $($field:ident = $name:literal : fn($($argument:ty),* $(,)?) $(-> $result:ty)?;)* }) => {
        pub struct $table {
            $(pub $field: unsafe extern "system" fn($($argument),*) $(-> $result)?,)*
        }

        impl $table {
            pub fn load(get: GetProcAddr, owner: Handle) -> Result<$table, String> {
                Ok($table {
                    $(
                        // SAFETY: the field's type is the signature
                        // vulkan_core.h declares for the entry point.
                        $field: unsafe { entry::<unsafe extern "system" fn($($argument),*) $(-> $result)?>(get, owner, $name)? },
                    )*
                })
            }
        }
    };
}

entry_points!(GlobalFunctions {
    create_instance = "vkCreateInstance": fn(*const InstanceCreateInfo, Allocator, *mut Handle) -> VkResult;
    enumerate_instance_layer_properties = "vkEnumerateInstanceLayerProperties": fn(*mut u32, *mut LayerProperties) -> VkResult;
    enumerate_instance_extension_properties = "vkEnumerateInstanceExtensionProperties": fn(*const c_char, *mut u32, *mut ExtensionProperties) -> VkResult;
});

entry_points!(InstanceFunctions {
    destroy_instance = "vkDestroyInstance": fn(Handle, Allocator);
    enumerate_physical_devices = "vkEnumeratePhysicalDevices": fn(Handle, *mut u32, *mut Handle) -> VkResult;
    get_physical_device_properties = "vkGetPhysicalDeviceProperties": fn(Handle, *mut PropertiesBytes);
    get_physical_device_queue_family_properties = "vkGetPhysicalDeviceQueueFamilyProperties": fn(Handle, *mut u32, *mut QueueFamilyProperties);
    get_physical_device_memory_properties = "vkGetPhysicalDeviceMemoryProperties": fn(Handle, *mut PhysicalDeviceMemoryProperties);
    get_physical_device_features = "vkGetPhysicalDeviceFeatures": fn(Handle, *mut CoreFeatures);
    get_physical_device_format_properties = "vkGetPhysicalDeviceFormatProperties": fn(Handle, u32, *mut FormatProperties);
    enumerate_device_extension_properties = "vkEnumerateDeviceExtensionProperties": fn(Handle, *const c_char, *mut u32, *mut ExtensionProperties) -> VkResult;
    create_device = "vkCreateDevice": fn(Handle, *const DeviceCreateInfo, Allocator, *mut Handle) -> VkResult;
    get_device_proc_addr = "vkGetDeviceProcAddr": fn(Handle, *const c_char) -> Option<VoidFunction>;
});

// The physical-device queries of Vulkan 1.1, fetched only from an instance
// created for 1.1 or later.
entry_points!(InstanceFunctions11 {
    get_physical_device_features2 = "vkGetPhysicalDeviceFeatures2": fn(Handle, *mut FeatureBlock);
    get_physical_device_properties2 = "vkGetPhysicalDeviceProperties2": fn(Handle, *mut Properties2);
});

entry_points!(DebugUtilsFunctions {
    create_debug_utils_messenger = "vkCreateDebugUtilsMessengerEXT": fn(Handle, *const DebugUtilsMessengerCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_debug_utils_messenger = "vkDestroyDebugUtilsMessengerEXT": fn(Handle, Object, Allocator);
});

entry_points!(DeviceFunctions {
    destroy_device = "vkDestroyDevice": fn(Handle, Allocator);
    get_device_queue = "vkGetDeviceQueue": fn(Handle, u32, u32, *mut Handle);
    device_wait_idle = "vkDeviceWaitIdle": fn(Handle) -> VkResult;
    queue_submit = "vkQueueSubmit": fn(Handle, u32, *const SubmitInfo, Object) -> VkResult;
    allocate_memory = "vkAllocateMemory": fn(Handle, *const MemoryAllocateInfo, Allocator, *mut Object) -> VkResult;
    free_memory = "vkFreeMemory": fn(Handle, Object, Allocator);
    map_memory = "vkMapMemory": fn(Handle, Object, u64, u64, u32, *mut *mut c_void) -> VkResult;
    flush_mapped_memory_ranges = "vkFlushMappedMemoryRanges": fn(Handle, u32, *const MappedMemoryRange) -> VkResult;
    invalidate_mapped_memory_ranges = "vkInvalidateMappedMemoryRanges": fn(Handle, u32, *const MappedMemoryRange) -> VkResult;
    create_buffer = "vkCreateBuffer": fn(Handle, *const BufferCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_buffer = "vkDestroyBuffer": fn(Handle, Object, Allocator);
    get_buffer_memory_requirements = "vkGetBufferMemoryRequirements": fn(Handle, Object, *mut MemoryRequirements);
    bind_buffer_memory = "vkBindBufferMemory": fn(Handle, Object, Object, u64) -> VkResult;
    create_image = "vkCreateImage": fn(Handle, *const ImageCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_image = "vkDestroyImage": fn(Handle, Object, Allocator);
    get_image_memory_requirements = "vkGetImageMemoryRequirements": fn(Handle, Object, *mut MemoryRequirements);
    bind_image_memory = "vkBindImageMemory": fn(Handle, Object, Object, u64) -> VkResult;
    create_image_view = "vkCreateImageView": fn(Handle, *const ImageViewCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_image_view = "vkDestroyImageView": fn(Handle, Object, Allocator);
    create_sampler = "vkCreateSampler": fn(Handle, *const SamplerCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_sampler = "vkDestroySampler": fn(Handle, Object, Allocator);
    create_shader_module = "vkCreateShaderModule": fn(Handle, *const ShaderModuleCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_shader_module = "vkDestroyShaderModule": fn(Handle, Object, Allocator);
    create_descriptor_set_layout = "vkCreateDescriptorSetLayout": fn(Handle, *const DescriptorSetLayoutCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_descriptor_set_layout = "vkDestroyDescriptorSetLayout": fn(Handle, Object, Allocator);
    create_descriptor_pool = "vkCreateDescriptorPool": fn(Handle, *const DescriptorPoolCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_descriptor_pool = "vkDestroyDescriptorPool": fn(Handle, Object, Allocator);
    allocate_descriptor_sets = "vkAllocateDescriptorSets": fn(Handle, *const DescriptorSetAllocateInfo, *mut Object) -> VkResult;
    update_descriptor_sets = "vkUpdateDescriptorSets": fn(Handle, u32, *const WriteDescriptorSet, u32, *const c_void);
    create_pipeline_layout = "vkCreatePipelineLayout": fn(Handle, *const PipelineLayoutCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_pipeline_layout = "vkDestroyPipelineLayout": fn(Handle, Object, Allocator);
    create_graphics_pipelines = "vkCreateGraphicsPipelines": fn(Handle, Object, u32, *const GraphicsPipelineCreateInfo, Allocator, *mut Object) -> VkResult;
    create_compute_pipelines = "vkCreateComputePipelines": fn(Handle, Object, u32, *const ComputePipelineCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_pipeline = "vkDestroyPipeline": fn(Handle, Object, Allocator);
    create_render_pass = "vkCreateRenderPass": fn(Handle, *const RenderPassCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_render_pass = "vkDestroyRenderPass": fn(Handle, Object, Allocator);
    create_framebuffer = "vkCreateFramebuffer": fn(Handle, *const FramebufferCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_framebuffer = "vkDestroyFramebuffer": fn(Handle, Object, Allocator);
    create_command_pool = "vkCreateCommandPool": fn(Handle, *const CommandPoolCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_command_pool = "vkDestroyCommandPool": fn(Handle, Object, Allocator);
    reset_command_pool = "vkResetCommandPool": fn(Handle, Object, u32) -> VkResult;
    allocate_command_buffers = "vkAllocateCommandBuffers": fn(Handle, *const CommandBufferAllocateInfo, *mut Handle) -> VkResult;
    begin_command_buffer = "vkBeginCommandBuffer": fn(Handle, *const CommandBufferBeginInfo) -> VkResult;
    end_command_buffer = "vkEndCommandBuffer": fn(Handle) -> VkResult;
    create_fence = "vkCreateFence": fn(Handle, *const FenceCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_fence = "vkDestroyFence": fn(Handle, Object, Allocator);
    get_fence_status = "vkGetFenceStatus": fn(Handle, Object) -> VkResult;
    wait_for_fences = "vkWaitForFences": fn(Handle, u32, *const Object, Bool32, u64) -> VkResult;
    reset_fences = "vkResetFences": fn(Handle, u32, *const Object) -> VkResult;
    create_query_pool = "vkCreateQueryPool": fn(Handle, *const QueryPoolCreateInfo, Allocator, *mut Object) -> VkResult;
    destroy_query_pool = "vkDestroyQueryPool": fn(Handle, Object, Allocator);
    get_query_pool_results = "vkGetQueryPoolResults": fn(Handle, Object, u32, u32, usize, *mut c_void, u64, u32) -> VkResult;
    cmd_begin_render_pass = "vkCmdBeginRenderPass": fn(Handle, *const RenderPassBeginInfo, u32);
    cmd_end_render_pass = "vkCmdEndRenderPass": fn(Handle);
    cmd_execute_commands = "vkCmdExecuteCommands": fn(Handle, u32, *const Handle);
    cmd_bind_pipeline = "vkCmdBindPipeline": fn(Handle, u32, Object);
    cmd_bind_descriptor_sets = "vkCmdBindDescriptorSets": fn(Handle, u32, Object, u32, u32, *const Object, u32, *const u32);
    cmd_bind_vertex_buffers = "vkCmdBindVertexBuffers": fn(Handle, u32, u32, *const Object, *const u64);
    cmd_push_constants = "vkCmdPushConstants": fn(Handle, Object, u32, u32, u32, *const c_void);
    cmd_draw = "vkCmdDraw": fn(Handle, u32, u32, u32, u32);
    cmd_draw_indirect = "vkCmdDrawIndirect": fn(Handle, Object, u64, u32, u32);
    cmd_dispatch = "vkCmdDispatch": fn(Handle, u32, u32, u32);
    cmd_pipeline_barrier = "vkCmdPipelineBarrier": fn(Handle, u32, u32, u32, u32, *const c_void, u32, *const BufferMemoryBarrier, u32, *const ImageMemoryBarrier);
    cmd_copy_buffer_to_image = "vkCmdCopyBufferToImage": fn(Handle, Object, Object, u32, u32, *const BufferImageCopy);
    cmd_copy_image_to_buffer = "vkCmdCopyImageToBuffer": fn(Handle, Object, u32, Object, u32, *const BufferImageCopy);
    cmd_clear_color_image = "vkCmdClearColorImage": fn(Handle, Object, u32, *const ClearValue, u32, *const SubresourceRange);
    cmd_reset_query_pool = "vkCmdResetQueryPool": fn(Handle, Object, u32, u32);
    cmd_write_timestamp = "vkCmdWriteTimestamp": fn(Handle, u32, Object, u32);
});

/// The name of a `VkResult`, for error messages.
pub fn result_name(result: VkResult) -> &'static str {
    match result {
        0 => "VK_SUCCESS",
        1 => "VK_NOT_READY",
        2 => "VK_TIMEOUT",
        5 => "VK_INCOMPLETE",
        -1 => "VK_ERROR_OUT_OF_HOST_MEMORY",
        -2 => "VK_ERROR_OUT_OF_DEVICE_MEMORY",
        -3 => "VK_ERROR_INITIALIZATION_FAILED",
        -4 => "VK_ERROR_DEVICE_LOST",
        -5 => "VK_ERROR_MEMORY_MAP_FAILED",
        -6 => "VK_ERROR_LAYER_NOT_PRESENT",
        -7 => "VK_ERROR_EXTENSION_NOT_PRESENT",
        -8 => "VK_ERROR_FEATURE_NOT_PRESENT",
        -9 => "VK_ERROR_INCOMPATIBLE_DRIVER",
        -10 => "VK_ERROR_TOO_MANY_OBJECTS",
        -11 => "VK_ERROR_FORMAT_NOT_SUPPORTED",
        _ => "an unlisted result",
    }
}

/// `Ok` for `VK_SUCCESS`, otherwise the call and its result.
pub fn check(result: VkResult, call: &str) -> Result<(), String> {
    if result == SUCCESS { Ok(()) } else { Err(format!("{call} failed with {} ({result})", result_name(result))) }
}

/// `major.minor.patch` of a packed Vulkan version.
pub fn version_text(version: u32) -> String {
    format!("{}.{}.{}", (version >> 22) & 0x7f, (version >> 12) & 0x3ff, version & 0xfff)
}
