//! The Vulkan path: the scene rendered to an offscreen image by whatever device
//! the loader offers (the software device Mesa's lavapipe provides on the Linux
//! cells), read back through a host-visible buffer. Throwaway, never product
//! code.
//!
//! The loader is opened at run time and every entry point is fetched through
//! `vkGetInstanceProcAddr` and `vkGetDeviceProcAddr`: no Vulkan header, no
//! binding crate, the structures declared here with the field order of
//! `vulkan_core.h`. The SPIR-V is produced at run time by the GLSL compiler the
//! setup script installs (glslang), one source per stage.

use std::ffi::{c_char, c_void};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::ptr;
use std::time::Instant;

use crate::loader::{Library, c_string, text_of};
use crate::scene::{Frame, HEIGHT, Scene, VERTEX_STRIDE, WIDTH};
use crate::{FrameResult, Outcome};

type Handle = *mut c_void;
type Object = u64;
type Allocator = *const c_void;
type VkResult = i32;
type Fn = unsafe extern "system" fn();

const API_VERSION_1_1: u32 = (1 << 22) | (1 << 12);
const FORMAT_R8G8B8A8_UNORM: u32 = 37;
const FORMAT_R32G32_SFLOAT: u32 = 103;
const FORMAT_R32G32B32_SFLOAT: u32 = 106;
const IMAGE_TYPE_2D: u32 = 1;
const IMAGE_VIEW_TYPE_2D: u32 = 1;
const SAMPLE_COUNT_1: u32 = 1;
const IMAGE_TILING_OPTIMAL: u32 = 0;
const IMAGE_USAGE_TRANSFER_SRC: u32 = 1;
const IMAGE_USAGE_COLOR_ATTACHMENT: u32 = 0x10;
const SHARING_MODE_EXCLUSIVE: u32 = 0;
const LAYOUT_UNDEFINED: u32 = 0;
const LAYOUT_COLOR_ATTACHMENT_OPTIMAL: u32 = 2;
const LAYOUT_TRANSFER_SRC_OPTIMAL: u32 = 6;
const ASPECT_COLOR: u32 = 1;
const LOAD_OP_CLEAR: u32 = 1;
const LOAD_OP_DONT_CARE: u32 = 2;
const STORE_OP_STORE: u32 = 0;
const STORE_OP_DONT_CARE: u32 = 1;
const BIND_POINT_GRAPHICS: u32 = 0;
const STAGE_VERTEX: u32 = 1;
const STAGE_FRAGMENT: u32 = 0x10;
const INPUT_RATE_VERTEX: u32 = 0;
const TOPOLOGY_TRIANGLE_LIST: u32 = 3;
const POLYGON_MODE_FILL: u32 = 0;
const CULL_MODE_NONE: u32 = 0;
const FRONT_FACE_COUNTER_CLOCKWISE: u32 = 0;
const COLOR_COMPONENTS_RGBA: u32 = 0xf;
const COMMAND_BUFFER_LEVEL_PRIMARY: u32 = 0;
const COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT: u32 = 1;
const SUBPASS_CONTENTS_INLINE: u32 = 0;
const BUFFER_USAGE_TRANSFER_DST: u32 = 2;
const BUFFER_USAGE_VERTEX_BUFFER: u32 = 0x80;
const MEMORY_HOST_VISIBLE: u32 = 2;
const MEMORY_HOST_COHERENT: u32 = 4;
const ACCESS_COLOR_ATTACHMENT_WRITE: u32 = 0x100;
const ACCESS_TRANSFER_READ: u32 = 0x800;
const PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT: u32 = 0x400;
const PIPELINE_STAGE_TRANSFER: u32 = 0x1000;
const QUEUE_GRAPHICS: u32 = 1;
const QUEUE_FAMILY_IGNORED: u32 = !0;
const WHOLE_SIZE: u64 = !0;
const SPIRV_MAGIC: u32 = 0x0723_0203;

/// `VkStructureType` values of the structures the spike fills.
mod stype {
    pub const APPLICATION_INFO: u32 = 0;
    pub const INSTANCE_CREATE_INFO: u32 = 1;
    pub const DEVICE_QUEUE_CREATE_INFO: u32 = 2;
    pub const DEVICE_CREATE_INFO: u32 = 3;
    pub const SUBMIT_INFO: u32 = 4;
    pub const MEMORY_ALLOCATE_INFO: u32 = 5;
    pub const MAPPED_MEMORY_RANGE: u32 = 6;
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
    pub const PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: u32 = 26;
    pub const GRAPHICS_PIPELINE_CREATE_INFO: u32 = 28;
    pub const PIPELINE_LAYOUT_CREATE_INFO: u32 = 30;
    pub const FRAMEBUFFER_CREATE_INFO: u32 = 37;
    pub const RENDER_PASS_CREATE_INFO: u32 = 38;
    pub const COMMAND_POOL_CREATE_INFO: u32 = 39;
    pub const COMMAND_BUFFER_ALLOCATE_INFO: u32 = 40;
    pub const COMMAND_BUFFER_BEGIN_INFO: u32 = 42;
    pub const RENDER_PASS_BEGIN_INFO: u32 = 43;
    pub const IMAGE_MEMORY_BARRIER: u32 = 45;
}

// --- structures, in the field order of vulkan_core.h --------------------------

#[repr(C)]
struct ApplicationInfo {
    s_type: u32,
    p_next: *const c_void,
    application_name: *const c_char,
    application_version: u32,
    engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    application_info: *const ApplicationInfo,
    enabled_layer_count: u32,
    enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    enabled_extension_names: *const *const c_char,
}

#[repr(C)]
struct DeviceQueueCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_family_index: u32,
    queue_count: u32,
    queue_priorities: *const f32,
}

#[repr(C)]
struct DeviceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_create_info_count: u32,
    queue_create_infos: *const DeviceQueueCreateInfo,
    enabled_layer_count: u32,
    enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    enabled_extension_names: *const *const c_char,
    enabled_features: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct QueueFamilyProperties {
    queue_flags: u32,
    queue_count: u32,
    timestamp_valid_bits: u32,
    min_image_transfer_granularity: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryType {
    property_flags: u32,
    heap_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryHeap {
    size: u64,
    flags: u32,
}

#[repr(C)]
struct PhysicalDeviceMemoryProperties {
    memory_type_count: u32,
    memory_types: [MemoryType; 32],
    memory_heap_count: u32,
    memory_heaps: [MemoryHeap; 16],
}

#[repr(C)]
struct MemoryRequirements {
    size: u64,
    alignment: u64,
    memory_type_bits: u32,
}

#[repr(C)]
struct MemoryAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    allocation_size: u64,
    memory_type_index: u32,
}

#[repr(C)]
struct MappedMemoryRange {
    s_type: u32,
    p_next: *const c_void,
    memory: Object,
    offset: u64,
    size: u64,
}

#[repr(C)]
struct BufferCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    size: u64,
    usage: u32,
    sharing_mode: u32,
    queue_family_index_count: u32,
    queue_family_indices: *const u32,
}

#[repr(C)]
struct Extent3D {
    width: u32,
    height: u32,
    depth: u32,
}

#[repr(C)]
struct ImageCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    image_type: u32,
    format: u32,
    extent: Extent3D,
    mip_levels: u32,
    array_layers: u32,
    samples: u32,
    tiling: u32,
    usage: u32,
    sharing_mode: u32,
    queue_family_index_count: u32,
    queue_family_indices: *const u32,
    initial_layout: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SubresourceRange {
    aspect_mask: u32,
    base_mip_level: u32,
    level_count: u32,
    base_array_layer: u32,
    layer_count: u32,
}

const COLOR_RANGE: SubresourceRange =
    SubresourceRange { aspect_mask: ASPECT_COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };

#[repr(C)]
struct ImageViewCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    image: Object,
    view_type: u32,
    format: u32,
    components: [u32; 4],
    subresource_range: SubresourceRange,
}

#[repr(C)]
struct ShaderModuleCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    code_size: usize,
    code: *const u32,
}

#[repr(C)]
struct PipelineShaderStageCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    stage: u32,
    module: Object,
    name: *const c_char,
    specialization_info: *const c_void,
}

#[repr(C)]
struct VertexInputBindingDescription {
    binding: u32,
    stride: u32,
    input_rate: u32,
}

#[repr(C)]
struct VertexInputAttributeDescription {
    location: u32,
    binding: u32,
    format: u32,
    offset: u32,
}

#[repr(C)]
struct PipelineVertexInputStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    vertex_binding_description_count: u32,
    vertex_binding_descriptions: *const VertexInputBindingDescription,
    vertex_attribute_description_count: u32,
    vertex_attribute_descriptions: *const VertexInputAttributeDescription,
}

#[repr(C)]
struct PipelineInputAssemblyStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    topology: u32,
    primitive_restart_enable: u32,
}

#[repr(C)]
struct Viewport {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    min_depth: f32,
    max_depth: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect2D {
    offset: [i32; 2],
    extent: [u32; 2],
}

#[repr(C)]
struct PipelineViewportStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    viewport_count: u32,
    viewports: *const Viewport,
    scissor_count: u32,
    scissors: *const Rect2D,
}

#[repr(C)]
struct PipelineRasterizationStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    depth_clamp_enable: u32,
    rasterizer_discard_enable: u32,
    polygon_mode: u32,
    cull_mode: u32,
    front_face: u32,
    depth_bias_enable: u32,
    depth_bias_constant_factor: f32,
    depth_bias_clamp: f32,
    depth_bias_slope_factor: f32,
    line_width: f32,
}

#[repr(C)]
struct PipelineMultisampleStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    rasterization_samples: u32,
    sample_shading_enable: u32,
    min_sample_shading: f32,
    sample_mask: *const u32,
    alpha_to_coverage_enable: u32,
    alpha_to_one_enable: u32,
}

#[repr(C)]
struct PipelineColorBlendAttachmentState {
    blend_enable: u32,
    src_color_blend_factor: u32,
    dst_color_blend_factor: u32,
    color_blend_op: u32,
    src_alpha_blend_factor: u32,
    dst_alpha_blend_factor: u32,
    alpha_blend_op: u32,
    color_write_mask: u32,
}

#[repr(C)]
struct PipelineColorBlendStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    logic_op_enable: u32,
    logic_op: u32,
    attachment_count: u32,
    attachments: *const PipelineColorBlendAttachmentState,
    blend_constants: [f32; 4],
}

#[repr(C)]
struct PipelineLayoutCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    set_layout_count: u32,
    set_layouts: *const Object,
    push_constant_range_count: u32,
    push_constant_ranges: *const c_void,
}

#[repr(C)]
struct GraphicsPipelineCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    stage_count: u32,
    stages: *const PipelineShaderStageCreateInfo,
    vertex_input_state: *const PipelineVertexInputStateCreateInfo,
    input_assembly_state: *const PipelineInputAssemblyStateCreateInfo,
    tessellation_state: *const c_void,
    viewport_state: *const PipelineViewportStateCreateInfo,
    rasterization_state: *const PipelineRasterizationStateCreateInfo,
    multisample_state: *const PipelineMultisampleStateCreateInfo,
    depth_stencil_state: *const c_void,
    color_blend_state: *const PipelineColorBlendStateCreateInfo,
    dynamic_state: *const c_void,
    layout: Object,
    render_pass: Object,
    subpass: u32,
    base_pipeline_handle: Object,
    base_pipeline_index: i32,
}

#[repr(C)]
struct AttachmentDescription {
    flags: u32,
    format: u32,
    samples: u32,
    load_op: u32,
    store_op: u32,
    stencil_load_op: u32,
    stencil_store_op: u32,
    initial_layout: u32,
    final_layout: u32,
}

#[repr(C)]
struct AttachmentReference {
    attachment: u32,
    layout: u32,
}

#[repr(C)]
struct SubpassDescription {
    flags: u32,
    pipeline_bind_point: u32,
    input_attachment_count: u32,
    input_attachments: *const AttachmentReference,
    color_attachment_count: u32,
    color_attachments: *const AttachmentReference,
    resolve_attachments: *const AttachmentReference,
    depth_stencil_attachment: *const AttachmentReference,
    preserve_attachment_count: u32,
    preserve_attachments: *const u32,
}

#[repr(C)]
struct RenderPassCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    attachment_count: u32,
    attachments: *const AttachmentDescription,
    subpass_count: u32,
    subpasses: *const SubpassDescription,
    dependency_count: u32,
    dependencies: *const c_void,
}

#[repr(C)]
struct FramebufferCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    render_pass: Object,
    attachment_count: u32,
    attachments: *const Object,
    width: u32,
    height: u32,
    layers: u32,
}

#[repr(C)]
struct CommandPoolCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_family_index: u32,
}

#[repr(C)]
struct CommandBufferAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    command_pool: Object,
    level: u32,
    command_buffer_count: u32,
}

#[repr(C)]
struct CommandBufferBeginInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    inheritance_info: *const c_void,
}

#[repr(C)]
struct RenderPassBeginInfo {
    s_type: u32,
    p_next: *const c_void,
    render_pass: Object,
    framebuffer: Object,
    render_area: Rect2D,
    clear_value_count: u32,
    clear_values: *const [f32; 4],
}

#[repr(C)]
struct BufferImageCopy {
    buffer_offset: u64,
    buffer_row_length: u32,
    buffer_image_height: u32,
    image_subresource: [u32; 4],
    image_offset: [i32; 3],
    image_extent: Extent3D,
}

#[repr(C)]
struct ImageMemoryBarrier {
    s_type: u32,
    p_next: *const c_void,
    src_access_mask: u32,
    dst_access_mask: u32,
    old_layout: u32,
    new_layout: u32,
    src_queue_family_index: u32,
    dst_queue_family_index: u32,
    image: Object,
    subresource_range: SubresourceRange,
}

#[repr(C)]
struct SubmitInfo {
    s_type: u32,
    p_next: *const c_void,
    wait_semaphore_count: u32,
    wait_semaphores: *const Object,
    wait_dst_stage_mask: *const u32,
    command_buffer_count: u32,
    command_buffers: *const Handle,
    signal_semaphore_count: u32,
    signal_semaphores: *const Object,
}

// --- entry points -----------------------------------------------------------------

type GetProcAddr = unsafe extern "system" fn(Handle, *const c_char) -> Option<Fn>;
type CreateInstance = unsafe extern "system" fn(*const InstanceCreateInfo, Allocator, *mut Handle) -> VkResult;
type DestroyInstance = unsafe extern "system" fn(Handle, Allocator);
type EnumeratePhysicalDevices = unsafe extern "system" fn(Handle, *mut u32, *mut Handle) -> VkResult;
type GetPhysicalDeviceProperties = unsafe extern "system" fn(Handle, *mut u8);
type GetPhysicalDeviceQueueFamilyProperties = unsafe extern "system" fn(Handle, *mut u32, *mut QueueFamilyProperties);
type GetPhysicalDeviceMemoryProperties = unsafe extern "system" fn(Handle, *mut PhysicalDeviceMemoryProperties);
type CreateDevice = unsafe extern "system" fn(Handle, *const DeviceCreateInfo, Allocator, *mut Handle) -> VkResult;
type DestroyDevice = unsafe extern "system" fn(Handle, Allocator);
type GetDeviceQueue = unsafe extern "system" fn(Handle, u32, u32, *mut Handle);
type CreateImage = unsafe extern "system" fn(Handle, *const ImageCreateInfo, Allocator, *mut Object) -> VkResult;
type DestroyObject = unsafe extern "system" fn(Handle, Object, Allocator);
type GetImageMemoryRequirements = unsafe extern "system" fn(Handle, Object, *mut MemoryRequirements);
type AllocateMemory = unsafe extern "system" fn(Handle, *const MemoryAllocateInfo, Allocator, *mut Object) -> VkResult;
type BindMemory = unsafe extern "system" fn(Handle, Object, Object, u64) -> VkResult;
type CreateImageView = unsafe extern "system" fn(Handle, *const ImageViewCreateInfo, Allocator, *mut Object) -> VkResult;
type CreateRenderPass = unsafe extern "system" fn(Handle, *const RenderPassCreateInfo, Allocator, *mut Object) -> VkResult;
type CreateFramebuffer = unsafe extern "system" fn(Handle, *const FramebufferCreateInfo, Allocator, *mut Object) -> VkResult;
type CreateShaderModule = unsafe extern "system" fn(Handle, *const ShaderModuleCreateInfo, Allocator, *mut Object) -> VkResult;
type CreatePipelineLayout = unsafe extern "system" fn(Handle, *const PipelineLayoutCreateInfo, Allocator, *mut Object) -> VkResult;
type CreateGraphicsPipelines =
    unsafe extern "system" fn(Handle, Object, u32, *const GraphicsPipelineCreateInfo, Allocator, *mut Object) -> VkResult;
type CreateBuffer = unsafe extern "system" fn(Handle, *const BufferCreateInfo, Allocator, *mut Object) -> VkResult;
type MapMemory = unsafe extern "system" fn(Handle, Object, u64, u64, u32, *mut *mut c_void) -> VkResult;
type UnmapMemory = unsafe extern "system" fn(Handle, Object);
type MappedMemoryRanges = unsafe extern "system" fn(Handle, u32, *const MappedMemoryRange) -> VkResult;
type CreateCommandPool = unsafe extern "system" fn(Handle, *const CommandPoolCreateInfo, Allocator, *mut Object) -> VkResult;
type AllocateCommandBuffers = unsafe extern "system" fn(Handle, *const CommandBufferAllocateInfo, *mut Handle) -> VkResult;
type BeginCommandBuffer = unsafe extern "system" fn(Handle, *const CommandBufferBeginInfo) -> VkResult;
type EndCommandBuffer = unsafe extern "system" fn(Handle) -> VkResult;
type CmdBeginRenderPass = unsafe extern "system" fn(Handle, *const RenderPassBeginInfo, u32);
type CmdEndRenderPass = unsafe extern "system" fn(Handle);
type CmdBindPipeline = unsafe extern "system" fn(Handle, u32, Object);
type CmdBindVertexBuffers = unsafe extern "system" fn(Handle, u32, u32, *const Object, *const u64);
type CmdDraw = unsafe extern "system" fn(Handle, u32, u32, u32, u32);
type CmdPipelineBarrier =
    unsafe extern "system" fn(Handle, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const ImageMemoryBarrier);
type CmdCopyImageToBuffer = unsafe extern "system" fn(Handle, Object, u32, Object, u32, *const BufferImageCopy);
type QueueSubmit = unsafe extern "system" fn(Handle, u32, *const SubmitInfo, Object) -> VkResult;
type QueueWaitIdle = unsafe extern "system" fn(Handle) -> VkResult;
type DeviceWaitIdle = unsafe extern "system" fn(Handle) -> VkResult;

/// The instance-level entry points, fetched from the loader.
struct InstanceApi {
    create_instance: CreateInstance,
    destroy_instance: DestroyInstance,
    enumerate_physical_devices: EnumeratePhysicalDevices,
    get_physical_device_properties: GetPhysicalDeviceProperties,
    get_physical_device_queue_family_properties: GetPhysicalDeviceQueueFamilyProperties,
    get_physical_device_memory_properties: GetPhysicalDeviceMemoryProperties,
    create_device: CreateDevice,
    get_device_proc_addr: GetProcAddr,
}

/// The device-level entry points, fetched through `vkGetDeviceProcAddr`.
struct DeviceApi {
    destroy_device: DestroyDevice,
    get_device_queue: GetDeviceQueue,
    create_image: CreateImage,
    destroy_image: DestroyObject,
    get_image_memory_requirements: GetImageMemoryRequirements,
    allocate_memory: AllocateMemory,
    free_memory: DestroyObject,
    bind_image_memory: BindMemory,
    create_image_view: CreateImageView,
    destroy_image_view: DestroyObject,
    create_render_pass: CreateRenderPass,
    destroy_render_pass: DestroyObject,
    create_framebuffer: CreateFramebuffer,
    destroy_framebuffer: DestroyObject,
    create_shader_module: CreateShaderModule,
    destroy_shader_module: DestroyObject,
    create_pipeline_layout: CreatePipelineLayout,
    destroy_pipeline_layout: DestroyObject,
    create_graphics_pipelines: CreateGraphicsPipelines,
    destroy_pipeline: DestroyObject,
    create_buffer: CreateBuffer,
    destroy_buffer: DestroyObject,
    get_buffer_memory_requirements: GetImageMemoryRequirements,
    bind_buffer_memory: BindMemory,
    map_memory: MapMemory,
    unmap_memory: UnmapMemory,
    flush_mapped_memory_ranges: MappedMemoryRanges,
    invalidate_mapped_memory_ranges: MappedMemoryRanges,
    create_command_pool: CreateCommandPool,
    destroy_command_pool: DestroyObject,
    allocate_command_buffers: AllocateCommandBuffers,
    begin_command_buffer: BeginCommandBuffer,
    end_command_buffer: EndCommandBuffer,
    cmd_begin_render_pass: CmdBeginRenderPass,
    cmd_end_render_pass: CmdEndRenderPass,
    cmd_bind_pipeline: CmdBindPipeline,
    cmd_bind_vertex_buffers: CmdBindVertexBuffers,
    cmd_draw: CmdDraw,
    cmd_pipeline_barrier: CmdPipelineBarrier,
    cmd_copy_image_to_buffer: CmdCopyImageToBuffer,
    queue_submit: QueueSubmit,
    queue_wait_idle: QueueWaitIdle,
    device_wait_idle: DeviceWaitIdle,
}

/// Fetches an entry point through a `vkGet*ProcAddr` and casts it to its type.
///
/// # Safety
///
/// `F` must be the entry point's signature.
unsafe fn entry<F: Copy>(get: GetProcAddr, owner: Handle, name: &str) -> Result<F, String> {
    let c_name = c_string(name);
    let address = unsafe { get(owner, c_name.as_ptr()) }.ok_or_else(|| format!("{name} not provided"))?;
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<Fn>());
    // SAFETY: the caller states the signature.
    Ok(unsafe { std::mem::transmute_copy::<Fn, F>(&address) })
}

macro_rules! instance_entry {
    ($get:expr, $instance:expr, $name:literal) => {
        unsafe { entry($get, $instance, $name)? }
    };
}

impl InstanceApi {
    /// Creates the instance through the loader's global entry point, then
    /// fetches the instance-level entry points from it.
    fn create(library: &Library, create_info: &InstanceCreateInfo) -> Result<(InstanceApi, Handle), String> {
        // SAFETY: the loader exports `vkGetInstanceProcAddr` with this signature.
        let get: GetProcAddr = unsafe { library.function("vkGetInstanceProcAddr")? };
        let create_instance: CreateInstance = instance_entry!(get, ptr::null_mut(), "vkCreateInstance");
        let mut instance: Handle = ptr::null_mut();
        check(unsafe { create_instance(create_info, ptr::null(), &mut instance) }, "vkCreateInstance")?;
        let api = InstanceApi {
            create_instance,
            destroy_instance: instance_entry!(get, instance, "vkDestroyInstance"),
            enumerate_physical_devices: instance_entry!(get, instance, "vkEnumeratePhysicalDevices"),
            get_physical_device_properties: instance_entry!(get, instance, "vkGetPhysicalDeviceProperties"),
            get_physical_device_queue_family_properties: instance_entry!(get, instance, "vkGetPhysicalDeviceQueueFamilyProperties"),
            get_physical_device_memory_properties: instance_entry!(get, instance, "vkGetPhysicalDeviceMemoryProperties"),
            create_device: instance_entry!(get, instance, "vkCreateDevice"),
            get_device_proc_addr: instance_entry!(get, instance, "vkGetDeviceProcAddr"),
        };
        Ok((api, instance))
    }
}

impl DeviceApi {
    fn load(get: GetProcAddr, device: Handle) -> Result<DeviceApi, String> {
        Ok(DeviceApi {
            destroy_device: instance_entry!(get, device, "vkDestroyDevice"),
            get_device_queue: instance_entry!(get, device, "vkGetDeviceQueue"),
            create_image: instance_entry!(get, device, "vkCreateImage"),
            destroy_image: instance_entry!(get, device, "vkDestroyImage"),
            get_image_memory_requirements: instance_entry!(get, device, "vkGetImageMemoryRequirements"),
            allocate_memory: instance_entry!(get, device, "vkAllocateMemory"),
            free_memory: instance_entry!(get, device, "vkFreeMemory"),
            bind_image_memory: instance_entry!(get, device, "vkBindImageMemory"),
            create_image_view: instance_entry!(get, device, "vkCreateImageView"),
            destroy_image_view: instance_entry!(get, device, "vkDestroyImageView"),
            create_render_pass: instance_entry!(get, device, "vkCreateRenderPass"),
            destroy_render_pass: instance_entry!(get, device, "vkDestroyRenderPass"),
            create_framebuffer: instance_entry!(get, device, "vkCreateFramebuffer"),
            destroy_framebuffer: instance_entry!(get, device, "vkDestroyFramebuffer"),
            create_shader_module: instance_entry!(get, device, "vkCreateShaderModule"),
            destroy_shader_module: instance_entry!(get, device, "vkDestroyShaderModule"),
            create_pipeline_layout: instance_entry!(get, device, "vkCreatePipelineLayout"),
            destroy_pipeline_layout: instance_entry!(get, device, "vkDestroyPipelineLayout"),
            create_graphics_pipelines: instance_entry!(get, device, "vkCreateGraphicsPipelines"),
            destroy_pipeline: instance_entry!(get, device, "vkDestroyPipeline"),
            create_buffer: instance_entry!(get, device, "vkCreateBuffer"),
            destroy_buffer: instance_entry!(get, device, "vkDestroyBuffer"),
            get_buffer_memory_requirements: instance_entry!(get, device, "vkGetBufferMemoryRequirements"),
            bind_buffer_memory: instance_entry!(get, device, "vkBindBufferMemory"),
            map_memory: instance_entry!(get, device, "vkMapMemory"),
            unmap_memory: instance_entry!(get, device, "vkUnmapMemory"),
            flush_mapped_memory_ranges: instance_entry!(get, device, "vkFlushMappedMemoryRanges"),
            invalidate_mapped_memory_ranges: instance_entry!(get, device, "vkInvalidateMappedMemoryRanges"),
            create_command_pool: instance_entry!(get, device, "vkCreateCommandPool"),
            destroy_command_pool: instance_entry!(get, device, "vkDestroyCommandPool"),
            allocate_command_buffers: instance_entry!(get, device, "vkAllocateCommandBuffers"),
            begin_command_buffer: instance_entry!(get, device, "vkBeginCommandBuffer"),
            end_command_buffer: instance_entry!(get, device, "vkEndCommandBuffer"),
            cmd_begin_render_pass: instance_entry!(get, device, "vkCmdBeginRenderPass"),
            cmd_end_render_pass: instance_entry!(get, device, "vkCmdEndRenderPass"),
            cmd_bind_pipeline: instance_entry!(get, device, "vkCmdBindPipeline"),
            cmd_bind_vertex_buffers: instance_entry!(get, device, "vkCmdBindVertexBuffers"),
            cmd_draw: instance_entry!(get, device, "vkCmdDraw"),
            cmd_pipeline_barrier: instance_entry!(get, device, "vkCmdPipelineBarrier"),
            cmd_copy_image_to_buffer: instance_entry!(get, device, "vkCmdCopyImageToBuffer"),
            queue_submit: instance_entry!(get, device, "vkQueueSubmit"),
            queue_wait_idle: instance_entry!(get, device, "vkQueueWaitIdle"),
            device_wait_idle: instance_entry!(get, device, "vkDeviceWaitIdle"),
        })
    }
}

fn check(result: VkResult, what: &str) -> Result<(), String> {
    if result == 0 { Ok(()) } else { Err(format!("{what} failed with VkResult {result}")) }
}

fn device_type_name(kind: u32) -> &'static str {
    match kind {
        1 => "integrated gpu",
        2 => "discrete gpu",
        3 => "virtual gpu",
        4 => "cpu",
        _ => "other",
    }
}

fn version_string(version: u32) -> String {
    format!("{}.{}.{}", version >> 22, (version >> 12) & 0x3ff, version & 0xfff)
}

// --- the GLSL compiler ----------------------------------------------------------

/// The GLSL compiler the setup script installs, with its version line.
pub fn find_glsl_compiler() -> Result<(String, String), String> {
    for name in ["glslangValidator", "glslang"] {
        if let Ok(output) = Command::new(name).arg("--version").output() {
            let text = String::from_utf8_lossy(&output.stdout);
            let version = text.lines().next().unwrap_or("").trim().to_string();
            return Ok((name.to_string(), version));
        }
    }
    Err("no GLSL compiler on PATH (glslangValidator or glslang)".to_string())
}

/// Compiles one GLSL stage to SPIR-V words with the compiler found on PATH.
fn compile_glsl(compiler: &str, source: &Path, work: &Path) -> Result<Vec<u32>, String> {
    let output_path = work.join(format!("{}.spv", source.file_name().and_then(|n| n.to_str()).unwrap_or("stage")));
    let output = Command::new(compiler)
        .arg("-V")
        .arg(source)
        .arg("-o")
        .arg(&output_path)
        .output()
        .map_err(|error| format!("{compiler}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{compiler} failed on {}: {}{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let bytes = fs::read(&output_path).map_err(|error| format!("{}: {error}", output_path.display()))?;
    if bytes.len() % 4 != 0 || bytes.len() < 20 {
        return Err(format!("{} is not a SPIR-V module ({} bytes)", output_path.display(), bytes.len()));
    }
    let words: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect();
    if words[0] != SPIRV_MAGIC {
        return Err(format!("{} has no SPIR-V magic", output_path.display()));
    }
    Ok(words)
}

// --- the renderer ---------------------------------------------------------------

struct Gpu {
    api: InstanceApi,
    dev: DeviceApi,
    instance: Handle,
    physical: Handle,
    device: Handle,
    queue: Handle,
    queue_family: u32,
    memory: PhysicalDeviceMemoryProperties,
}

impl Gpu {
    fn open(library: &Library, prefer_cpu: bool) -> Result<(Gpu, String, String, String), String> {
        let application_name = c_string("render-offscreen");
        let application_info = ApplicationInfo {
            s_type: stype::APPLICATION_INFO,
            p_next: ptr::null(),
            application_name: application_name.as_ptr(),
            application_version: 1,
            engine_name: application_name.as_ptr(),
            engine_version: 1,
            api_version: API_VERSION_1_1,
        };
        let create_info = InstanceCreateInfo {
            s_type: stype::INSTANCE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            application_info: &application_info,
            enabled_layer_count: 0,
            enabled_layer_names: ptr::null(),
            enabled_extension_count: 0,
            enabled_extension_names: ptr::null(),
        };
        let (api, instance) = InstanceApi::create(library, &create_info)?;
        let _ = api.create_instance;

        let mut count = 0_u32;
        check(unsafe { (api.enumerate_physical_devices)(instance, &mut count, ptr::null_mut()) }, "vkEnumeratePhysicalDevices")?;
        if count == 0 {
            unsafe { (api.destroy_instance)(instance, ptr::null()) };
            return Err("the loader found no physical device (no installable client driver)".to_string());
        }
        let mut devices: Vec<Handle> = vec![ptr::null_mut(); count as usize];
        check(unsafe { (api.enumerate_physical_devices)(instance, &mut count, devices.as_mut_ptr()) }, "vkEnumeratePhysicalDevices")?;
        // The properties structure is read by offset: device type at 16, name at 20 (256 bytes).
        let described: Vec<(Handle, u32, String, String, String)> = devices
            .iter()
            .map(|&physical| {
                let mut properties = [0_u8; 1024];
                unsafe { (api.get_physical_device_properties)(physical, properties.as_mut_ptr()) };
                let api_version = u32::from_le_bytes(properties[0..4].try_into().expect("four bytes"));
                let driver_version = u32::from_le_bytes(properties[4..8].try_into().expect("four bytes"));
                let kind = u32::from_le_bytes(properties[16..20].try_into().expect("four bytes"));
                let name = text_of(&properties[20..276]);
                (physical, kind, name, version_string(api_version), format!("{driver_version:#x}"))
            })
            .collect();
        let chosen = described
            .iter()
            .position(|(_, kind, _, _, _)| prefer_cpu && *kind == 4)
            .unwrap_or(0);
        let (physical, kind, name, api_version, driver_version) = described[chosen].clone();

        let mut family_count = 0_u32;
        unsafe { (api.get_physical_device_queue_family_properties)(physical, &mut family_count, ptr::null_mut()) };
        let mut families = vec![QueueFamilyProperties { queue_flags: 0, queue_count: 0, timestamp_valid_bits: 0, min_image_transfer_granularity: [0; 3] }; family_count as usize];
        unsafe { (api.get_physical_device_queue_family_properties)(physical, &mut family_count, families.as_mut_ptr()) };
        let queue_family = families
            .iter()
            .position(|family| family.queue_flags & QUEUE_GRAPHICS != 0 && family.queue_count > 0)
            .ok_or_else(|| "no graphics queue family".to_string())? as u32;

        let priority = 1.0_f32;
        let queue_info = DeviceQueueCreateInfo {
            s_type: stype::DEVICE_QUEUE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            queue_family_index: queue_family,
            queue_count: 1,
            queue_priorities: &priority,
        };
        let device_info = DeviceCreateInfo {
            s_type: stype::DEVICE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            queue_create_info_count: 1,
            queue_create_infos: &queue_info,
            enabled_layer_count: 0,
            enabled_layer_names: ptr::null(),
            enabled_extension_count: 0,
            enabled_extension_names: ptr::null(),
            enabled_features: ptr::null(),
        };
        let mut device: Handle = ptr::null_mut();
        check(unsafe { (api.create_device)(physical, &device_info, ptr::null(), &mut device) }, "vkCreateDevice")?;
        let dev = DeviceApi::load(api.get_device_proc_addr, device)?;
        let mut queue: Handle = ptr::null_mut();
        unsafe { (dev.get_device_queue)(device, queue_family, 0, &mut queue) };
        let mut memory = PhysicalDeviceMemoryProperties {
            memory_type_count: 0,
            memory_types: [MemoryType { property_flags: 0, heap_index: 0 }; 32],
            memory_heap_count: 0,
            memory_heaps: [MemoryHeap { size: 0, flags: 0 }; 16],
        };
        unsafe { (api.get_physical_device_memory_properties)(physical, &mut memory) };
        let devices_seen = described.iter().map(|(_, kind, name, _, _)| format!("{name} ({})", device_type_name(*kind))).collect::<Vec<_>>().join(", ");
        Ok((
            Gpu { api, dev, instance, physical, device, queue, queue_family, memory },
            name,
            device_type_name(kind).to_string(),
            format!("api {api_version}, driver {driver_version}; devices seen: {devices_seen}"),
        ))
    }

    fn memory_type(&self, allowed: u32, wanted: u32) -> Option<u32> {
        (0..self.memory.memory_type_count).find(|&index| {
            allowed & (1 << index) != 0 && self.memory.memory_types[index as usize].property_flags & wanted == wanted
        })
    }

    /// A buffer bound to host-visible memory, mapped; `coherent` says whether
    /// the host sees the device's writes without an invalidation.
    fn host_buffer(&self, size: u64, usage: u32) -> Result<(Object, Object, *mut c_void, bool), String> {
        let info = BufferCreateInfo {
            s_type: stype::BUFFER_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            size,
            usage,
            sharing_mode: SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            queue_family_indices: ptr::null(),
        };
        let mut buffer: Object = 0;
        check(unsafe { (self.dev.create_buffer)(self.device, &info, ptr::null(), &mut buffer) }, "vkCreateBuffer")?;
        let mut requirements = MemoryRequirements { size: 0, alignment: 0, memory_type_bits: 0 };
        unsafe { (self.dev.get_buffer_memory_requirements)(self.device, buffer, &mut requirements) };
        let (memory_type, coherent) = match self.memory_type(requirements.memory_type_bits, MEMORY_HOST_VISIBLE | MEMORY_HOST_COHERENT) {
            Some(index) => (index, true),
            None => (self.memory_type(requirements.memory_type_bits, MEMORY_HOST_VISIBLE).ok_or("no host-visible memory type")?, false),
        };
        let allocate = MemoryAllocateInfo {
            s_type: stype::MEMORY_ALLOCATE_INFO,
            p_next: ptr::null(),
            allocation_size: requirements.size,
            memory_type_index: memory_type,
        };
        let mut memory: Object = 0;
        check(unsafe { (self.dev.allocate_memory)(self.device, &allocate, ptr::null(), &mut memory) }, "vkAllocateMemory")?;
        check(unsafe { (self.dev.bind_buffer_memory)(self.device, buffer, memory, 0) }, "vkBindBufferMemory")?;
        let mut mapped: *mut c_void = ptr::null_mut();
        check(unsafe { (self.dev.map_memory)(self.device, memory, 0, WHOLE_SIZE, 0, &mut mapped) }, "vkMapMemory")?;
        Ok((buffer, memory, mapped, coherent))
    }

    fn range(memory: Object) -> MappedMemoryRange {
        MappedMemoryRange { s_type: stype::MAPPED_MEMORY_RANGE, p_next: ptr::null(), memory, offset: 0, size: WHOLE_SIZE }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            (self.dev.device_wait_idle)(self.device);
            (self.dev.destroy_device)(self.device, ptr::null());
            (self.api.destroy_instance)(self.instance, ptr::null());
        }
    }
}

/// One complete render: instance, device, pipeline, one frame, read back.
fn render_once(library: &Library, scene: &Scene, spirv: &(Vec<u32>, Vec<u32>), outcome: &mut Outcome) -> Result<Frame, String> {
    let started = Instant::now();
    let (gpu, name, kind, driver) = Gpu::open(library, true)?;
    outcome.device_name = name;
    outcome.device_type = kind;
    outcome.driver = driver;
    outcome.device_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    let _ = gpu.physical;
    let dev = &gpu.dev;
    let device = gpu.device;
    let none: Allocator = ptr::null();

    let pipeline_started = Instant::now();
    // The color image, its memory and its view.
    let image_info = ImageCreateInfo {
        s_type: stype::IMAGE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        image_type: IMAGE_TYPE_2D,
        format: FORMAT_R8G8B8A8_UNORM,
        extent: Extent3D { width: WIDTH, height: HEIGHT, depth: 1 },
        mip_levels: 1,
        array_layers: 1,
        samples: SAMPLE_COUNT_1,
        tiling: IMAGE_TILING_OPTIMAL,
        usage: IMAGE_USAGE_COLOR_ATTACHMENT | IMAGE_USAGE_TRANSFER_SRC,
        sharing_mode: SHARING_MODE_EXCLUSIVE,
        queue_family_index_count: 0,
        queue_family_indices: ptr::null(),
        initial_layout: LAYOUT_UNDEFINED,
    };
    let mut image: Object = 0;
    check(unsafe { (dev.create_image)(device, &image_info, none, &mut image) }, "vkCreateImage")?;
    let mut requirements = MemoryRequirements { size: 0, alignment: 0, memory_type_bits: 0 };
    unsafe { (dev.get_image_memory_requirements)(device, image, &mut requirements) };
    let image_memory_type = gpu.memory_type(requirements.memory_type_bits, 0).ok_or("no memory type for the image")?;
    let allocate = MemoryAllocateInfo {
        s_type: stype::MEMORY_ALLOCATE_INFO,
        p_next: ptr::null(),
        allocation_size: requirements.size,
        memory_type_index: image_memory_type,
    };
    let mut image_memory: Object = 0;
    check(unsafe { (dev.allocate_memory)(device, &allocate, none, &mut image_memory) }, "vkAllocateMemory (image)")?;
    check(unsafe { (dev.bind_image_memory)(device, image, image_memory, 0) }, "vkBindImageMemory")?;
    let view_info = ImageViewCreateInfo {
        s_type: stype::IMAGE_VIEW_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        image,
        view_type: IMAGE_VIEW_TYPE_2D,
        format: FORMAT_R8G8B8A8_UNORM,
        components: [0; 4],
        subresource_range: COLOR_RANGE,
    };
    let mut view: Object = 0;
    check(unsafe { (dev.create_image_view)(device, &view_info, none, &mut view) }, "vkCreateImageView")?;

    // The render pass: one color attachment cleared on load, kept on store,
    // left in the color attachment layout; the copy transitions it afterwards.
    let attachment = AttachmentDescription {
        flags: 0,
        format: FORMAT_R8G8B8A8_UNORM,
        samples: SAMPLE_COUNT_1,
        load_op: LOAD_OP_CLEAR,
        store_op: STORE_OP_STORE,
        stencil_load_op: LOAD_OP_DONT_CARE,
        stencil_store_op: STORE_OP_DONT_CARE,
        initial_layout: LAYOUT_UNDEFINED,
        final_layout: LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
    };
    let reference = AttachmentReference { attachment: 0, layout: LAYOUT_COLOR_ATTACHMENT_OPTIMAL };
    let subpass = SubpassDescription {
        flags: 0,
        pipeline_bind_point: BIND_POINT_GRAPHICS,
        input_attachment_count: 0,
        input_attachments: ptr::null(),
        color_attachment_count: 1,
        color_attachments: &reference,
        resolve_attachments: ptr::null(),
        depth_stencil_attachment: ptr::null(),
        preserve_attachment_count: 0,
        preserve_attachments: ptr::null(),
    };
    let render_pass_info = RenderPassCreateInfo {
        s_type: stype::RENDER_PASS_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        attachment_count: 1,
        attachments: &attachment,
        subpass_count: 1,
        subpasses: &subpass,
        dependency_count: 0,
        dependencies: ptr::null(),
    };
    let mut render_pass: Object = 0;
    check(unsafe { (dev.create_render_pass)(device, &render_pass_info, none, &mut render_pass) }, "vkCreateRenderPass")?;
    let framebuffer_info = FramebufferCreateInfo {
        s_type: stype::FRAMEBUFFER_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        render_pass,
        attachment_count: 1,
        attachments: &view,
        width: WIDTH,
        height: HEIGHT,
        layers: 1,
    };
    let mut framebuffer: Object = 0;
    check(unsafe { (dev.create_framebuffer)(device, &framebuffer_info, none, &mut framebuffer) }, "vkCreateFramebuffer")?;

    // The shaders and the pipeline.
    let mut modules = [0_u64; 2];
    for (index, words) in [&spirv.0, &spirv.1].into_iter().enumerate() {
        let info = ShaderModuleCreateInfo {
            s_type: stype::SHADER_MODULE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            code_size: words.len() * 4,
            code: words.as_ptr(),
        };
        check(unsafe { (dev.create_shader_module)(device, &info, none, &mut modules[index]) }, "vkCreateShaderModule")?;
    }
    let entry_name = c_string("main");
    let stages = [
        PipelineShaderStageCreateInfo {
            s_type: stype::PIPELINE_SHADER_STAGE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            stage: STAGE_VERTEX,
            module: modules[0],
            name: entry_name.as_ptr(),
            specialization_info: ptr::null(),
        },
        PipelineShaderStageCreateInfo {
            s_type: stype::PIPELINE_SHADER_STAGE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            stage: STAGE_FRAGMENT,
            module: modules[1],
            name: entry_name.as_ptr(),
            specialization_info: ptr::null(),
        },
    ];
    let binding = VertexInputBindingDescription { binding: 0, stride: VERTEX_STRIDE, input_rate: INPUT_RATE_VERTEX };
    let attributes = [
        VertexInputAttributeDescription { location: 0, binding: 0, format: FORMAT_R32G32_SFLOAT, offset: 0 },
        VertexInputAttributeDescription { location: 1, binding: 0, format: FORMAT_R32G32B32_SFLOAT, offset: 8 },
    ];
    let vertex_input = PipelineVertexInputStateCreateInfo {
        s_type: stype::PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        vertex_binding_description_count: 1,
        vertex_binding_descriptions: &binding,
        vertex_attribute_description_count: 2,
        vertex_attribute_descriptions: attributes.as_ptr(),
    };
    let input_assembly = PipelineInputAssemblyStateCreateInfo {
        s_type: stype::PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        topology: TOPOLOGY_TRIANGLE_LIST,
        primitive_restart_enable: 0,
    };
    let viewport = Viewport { x: 0.0, y: 0.0, width: WIDTH as f32, height: HEIGHT as f32, min_depth: 0.0, max_depth: 1.0 };
    let scissor = Rect2D { offset: [0, 0], extent: [WIDTH, HEIGHT] };
    let viewport_state = PipelineViewportStateCreateInfo {
        s_type: stype::PIPELINE_VIEWPORT_STATE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        viewport_count: 1,
        viewports: &viewport,
        scissor_count: 1,
        scissors: &scissor,
    };
    let rasterization = PipelineRasterizationStateCreateInfo {
        s_type: stype::PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        depth_clamp_enable: 0,
        rasterizer_discard_enable: 0,
        polygon_mode: POLYGON_MODE_FILL,
        cull_mode: CULL_MODE_NONE,
        front_face: FRONT_FACE_COUNTER_CLOCKWISE,
        depth_bias_enable: 0,
        depth_bias_constant_factor: 0.0,
        depth_bias_clamp: 0.0,
        depth_bias_slope_factor: 0.0,
        line_width: 1.0,
    };
    let multisample = PipelineMultisampleStateCreateInfo {
        s_type: stype::PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        rasterization_samples: SAMPLE_COUNT_1,
        sample_shading_enable: 0,
        min_sample_shading: 0.0,
        sample_mask: ptr::null(),
        alpha_to_coverage_enable: 0,
        alpha_to_one_enable: 0,
    };
    let blend_attachment = PipelineColorBlendAttachmentState {
        blend_enable: 0,
        src_color_blend_factor: 0,
        dst_color_blend_factor: 0,
        color_blend_op: 0,
        src_alpha_blend_factor: 0,
        dst_alpha_blend_factor: 0,
        alpha_blend_op: 0,
        color_write_mask: COLOR_COMPONENTS_RGBA,
    };
    let color_blend = PipelineColorBlendStateCreateInfo {
        s_type: stype::PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        logic_op_enable: 0,
        logic_op: 0,
        attachment_count: 1,
        attachments: &blend_attachment,
        blend_constants: [0.0; 4],
    };
    let layout_info = PipelineLayoutCreateInfo {
        s_type: stype::PIPELINE_LAYOUT_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        set_layout_count: 0,
        set_layouts: ptr::null(),
        push_constant_range_count: 0,
        push_constant_ranges: ptr::null(),
    };
    let mut layout: Object = 0;
    check(unsafe { (dev.create_pipeline_layout)(device, &layout_info, none, &mut layout) }, "vkCreatePipelineLayout")?;
    let pipeline_info = GraphicsPipelineCreateInfo {
        s_type: stype::GRAPHICS_PIPELINE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        stage_count: 2,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        tessellation_state: ptr::null(),
        viewport_state: &viewport_state,
        rasterization_state: &rasterization,
        multisample_state: &multisample,
        depth_stencil_state: ptr::null(),
        color_blend_state: &color_blend,
        dynamic_state: ptr::null(),
        layout,
        render_pass,
        subpass: 0,
        base_pipeline_handle: 0,
        base_pipeline_index: -1,
    };
    let mut pipeline: Object = 0;
    check(unsafe { (dev.create_graphics_pipelines)(device, 0, 1, &pipeline_info, none, &mut pipeline) }, "vkCreateGraphicsPipelines")?;

    // The vertex buffer, written through a mapping.
    let vertex_bytes = scene.vertex_bytes();
    let (vertex_buffer, vertex_memory, vertex_mapped, vertex_coherent) =
        gpu.host_buffer(vertex_bytes.len() as u64, BUFFER_USAGE_VERTEX_BUFFER)?;
    unsafe { ptr::copy_nonoverlapping(vertex_bytes.as_ptr(), vertex_mapped as *mut u8, vertex_bytes.len()) };
    if !vertex_coherent {
        check(unsafe { (dev.flush_mapped_memory_ranges)(device, 1, &Gpu::range(vertex_memory)) }, "vkFlushMappedMemoryRanges")?;
    }
    // The readback buffer.
    let readback_size = u64::from(WIDTH) * u64::from(HEIGHT) * 4;
    let (readback_buffer, readback_memory, readback_mapped, readback_coherent) =
        gpu.host_buffer(readback_size, BUFFER_USAGE_TRANSFER_DST)?;
    outcome.pipeline_us = pipeline_started.elapsed().as_secs_f64() * 1_000_000.0;

    // The command buffer: clear, draw, transition, copy.
    let render_started = Instant::now();
    let pool_info = CommandPoolCreateInfo { s_type: stype::COMMAND_POOL_CREATE_INFO, p_next: ptr::null(), flags: 0, queue_family_index: gpu.queue_family };
    let mut pool: Object = 0;
    check(unsafe { (dev.create_command_pool)(device, &pool_info, none, &mut pool) }, "vkCreateCommandPool")?;
    let allocate_info = CommandBufferAllocateInfo {
        s_type: stype::COMMAND_BUFFER_ALLOCATE_INFO,
        p_next: ptr::null(),
        command_pool: pool,
        level: COMMAND_BUFFER_LEVEL_PRIMARY,
        command_buffer_count: 1,
    };
    let mut commands: Handle = ptr::null_mut();
    check(unsafe { (dev.allocate_command_buffers)(device, &allocate_info, &mut commands) }, "vkAllocateCommandBuffers")?;
    let begin = CommandBufferBeginInfo {
        s_type: stype::COMMAND_BUFFER_BEGIN_INFO,
        p_next: ptr::null(),
        flags: COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT,
        inheritance_info: ptr::null(),
    };
    check(unsafe { (dev.begin_command_buffer)(commands, &begin) }, "vkBeginCommandBuffer")?;
    let clear = [0.0_f32, 0.0, 0.0, 1.0];
    let pass_begin = RenderPassBeginInfo {
        s_type: stype::RENDER_PASS_BEGIN_INFO,
        p_next: ptr::null(),
        render_pass,
        framebuffer,
        render_area: scissor,
        clear_value_count: 1,
        clear_values: &clear,
    };
    unsafe {
        (dev.cmd_begin_render_pass)(commands, &pass_begin, SUBPASS_CONTENTS_INLINE);
        (dev.cmd_bind_pipeline)(commands, BIND_POINT_GRAPHICS, pipeline);
        let offset = 0_u64;
        (dev.cmd_bind_vertex_buffers)(commands, 0, 1, &vertex_buffer, &offset);
        (dev.cmd_draw)(commands, scene.vertices.len() as u32, 1, 0, 0);
        (dev.cmd_end_render_pass)(commands);
    }
    let barrier = ImageMemoryBarrier {
        s_type: stype::IMAGE_MEMORY_BARRIER,
        p_next: ptr::null(),
        src_access_mask: ACCESS_COLOR_ATTACHMENT_WRITE,
        dst_access_mask: ACCESS_TRANSFER_READ,
        old_layout: LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
        new_layout: LAYOUT_TRANSFER_SRC_OPTIMAL,
        src_queue_family_index: QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: QUEUE_FAMILY_IGNORED,
        image,
        subresource_range: COLOR_RANGE,
    };
    let copy = BufferImageCopy {
        buffer_offset: 0,
        buffer_row_length: 0,
        buffer_image_height: 0,
        image_subresource: [ASPECT_COLOR, 0, 0, 1],
        image_offset: [0, 0, 0],
        image_extent: Extent3D { width: WIDTH, height: HEIGHT, depth: 1 },
    };
    unsafe {
        (dev.cmd_pipeline_barrier)(
            commands,
            PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT,
            PIPELINE_STAGE_TRANSFER,
            0,
            0,
            ptr::null(),
            0,
            ptr::null(),
            1,
            &barrier,
        );
        (dev.cmd_copy_image_to_buffer)(commands, image, LAYOUT_TRANSFER_SRC_OPTIMAL, readback_buffer, 1, &copy);
    }
    check(unsafe { (dev.end_command_buffer)(commands) }, "vkEndCommandBuffer")?;
    let submit = SubmitInfo {
        s_type: stype::SUBMIT_INFO,
        p_next: ptr::null(),
        wait_semaphore_count: 0,
        wait_semaphores: ptr::null(),
        wait_dst_stage_mask: ptr::null(),
        command_buffer_count: 1,
        command_buffers: &commands,
        signal_semaphore_count: 0,
        signal_semaphores: ptr::null(),
    };
    check(unsafe { (dev.queue_submit)(gpu.queue, 1, &submit, 0) }, "vkQueueSubmit")?;
    check(unsafe { (dev.queue_wait_idle)(gpu.queue) }, "vkQueueWaitIdle")?;
    let render_us = render_started.elapsed().as_secs_f64() * 1_000_000.0;

    let readback_started = Instant::now();
    if !readback_coherent {
        check(unsafe { (dev.invalidate_mapped_memory_ranges)(device, 1, &Gpu::range(readback_memory)) }, "vkInvalidateMappedMemoryRanges")?;
    }
    let pixels = unsafe { std::slice::from_raw_parts(readback_mapped as *const u8, readback_size as usize) }.to_vec();
    let frame = Frame::from_rows(WIDTH, HEIGHT, WIDTH as usize * 4, &pixels);
    let readback_us = readback_started.elapsed().as_secs_f64() * 1_000_000.0;
    outcome.frames.push(FrameResult { hash: frame.hash(), lit_pixels: frame.lit_pixels(), render_us, readback_us });

    unsafe {
        (dev.destroy_command_pool)(device, pool, none);
        (dev.unmap_memory)(device, readback_memory);
        (dev.destroy_buffer)(device, readback_buffer, none);
        (dev.free_memory)(device, readback_memory, none);
        (dev.unmap_memory)(device, vertex_memory);
        (dev.destroy_buffer)(device, vertex_buffer, none);
        (dev.free_memory)(device, vertex_memory, none);
        (dev.destroy_pipeline)(device, pipeline, none);
        (dev.destroy_pipeline_layout)(device, layout, none);
        for module in modules {
            (dev.destroy_shader_module)(device, module, none);
        }
        (dev.destroy_framebuffer)(device, framebuffer, none);
        (dev.destroy_render_pass)(device, render_pass, none);
        (dev.destroy_image_view)(device, view, none);
        (dev.destroy_image)(device, image, none);
        (dev.free_memory)(device, image_memory, none);
    }
    Ok(frame)
}

const LOADER_NAMES: &[&str] = if cfg!(target_os = "windows") {
    &["vulkan-1.dll"]
} else if cfg!(target_os = "macos") {
    &["libvulkan.1.dylib", "libvulkan.dylib", "libMoltenVK.dylib"]
} else {
    &["libvulkan.so.1", "libvulkan.so"]
};

/// Renders the scene `runs` times from scratch (instance, device, pipeline,
/// frame) and reports every frame; the first frame is kept for the image.
pub fn render(scene: &Scene, shaders: &Path, work: &Path, runs: usize) -> Outcome {
    let mut outcome = Outcome::new("vulkan");
    let library = match Library::open(LOADER_NAMES) {
        Ok(library) => library,
        Err(error) => {
            outcome.reason = format!("no Vulkan loader: {error}");
            return outcome;
        }
    };
    outcome.loader = library.name.clone();
    let (compiler, version) = match find_glsl_compiler() {
        Ok(found) => found,
        Err(error) => {
            outcome.reason = error;
            return outcome;
        }
    };
    outcome.shader_compiler = format!("{compiler} ({version})");
    let compile_started = Instant::now();
    let spirv = match (compile_glsl(&compiler, &shaders.join("scene.vert"), work), compile_glsl(&compiler, &shaders.join("scene.frag"), work)) {
        (Ok(vertex), Ok(fragment)) => (vertex, fragment),
        (Err(error), _) | (_, Err(error)) => {
            outcome.reason = error;
            return outcome;
        }
    };
    outcome.shader_compile_us = compile_started.elapsed().as_secs_f64() * 1_000_000.0;
    outcome.notes.push(format!("SPIR-V words: vertex {}, fragment {}", spirv.0.len(), spirv.1.len()));
    for _ in 0..runs {
        match render_once(&library, scene, &spirv, &mut outcome) {
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
