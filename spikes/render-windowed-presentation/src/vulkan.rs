//! Presentation to a window through Vulkan, natively: the loader opened at
//! run time, every entry point fetched through `vkGetInstanceProcAddr` and
//! `vkGetDeviceProcAddr`, the structures declared here in the field order of
//! `vulkan_core.h`. A frame clears the swapchain's image to its color with a
//! clear command, so that no shader is needed, and presents it. Throwaway,
//! never product code.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;

use crate::clock::now_ns;
use crate::host::NativeWindow;
use crate::json::Json;
use crate::loader::Library;
use crate::presenter::{DeviceInfo, FrameOutcome, Mode, Presenter, Request, frame_color};

type Handle = *mut c_void;
type Object = u64;
type VkResult = i32;
type VoidFunction = unsafe extern "system" fn();
type GetProcAddr = unsafe extern "system" fn(Handle, *const c_char) -> Option<VoidFunction>;

const SUCCESS: VkResult = 0;
const NOT_READY: VkResult = 1;
const TIMEOUT: VkResult = 2;
const SUBOPTIMAL: VkResult = 1_000_001_003;
const ERROR_OUT_OF_DATE: VkResult = -1_000_001_004;

const STRUCTURE_APPLICATION_INFO: u32 = 0;
const STRUCTURE_INSTANCE_CREATE_INFO: u32 = 1;
const STRUCTURE_DEVICE_QUEUE_CREATE_INFO: u32 = 2;
const STRUCTURE_DEVICE_CREATE_INFO: u32 = 3;
const STRUCTURE_SUBMIT_INFO: u32 = 4;
const STRUCTURE_MEMORY_ALLOCATE_INFO: u32 = 5;
const STRUCTURE_FENCE_CREATE_INFO: u32 = 8;
const STRUCTURE_SEMAPHORE_CREATE_INFO: u32 = 9;
const STRUCTURE_IMAGE_CREATE_INFO: u32 = 14;
const STRUCTURE_COMMAND_POOL_CREATE_INFO: u32 = 39;
const STRUCTURE_COMMAND_BUFFER_ALLOCATE_INFO: u32 = 40;
const STRUCTURE_COMMAND_BUFFER_BEGIN_INFO: u32 = 42;
const STRUCTURE_IMAGE_MEMORY_BARRIER: u32 = 45;
const STRUCTURE_SWAPCHAIN_CREATE_INFO: u32 = 1_000_001_000;
const STRUCTURE_PRESENT_INFO: u32 = 1_000_001_001;
const STRUCTURE_XLIB_SURFACE_CREATE_INFO: u32 = 1_000_004_000;
const STRUCTURE_XCB_SURFACE_CREATE_INFO: u32 = 1_000_005_000;
const STRUCTURE_WAYLAND_SURFACE_CREATE_INFO: u32 = 1_000_006_000;
const STRUCTURE_WIN32_SURFACE_CREATE_INFO: u32 = 1_000_009_000;

const FORMAT_R8G8B8A8_UNORM: u32 = 37;
const FORMAT_B8G8R8A8_UNORM: u32 = 44;
const PRESENT_MODE_IMMEDIATE: u32 = 0;
const PRESENT_MODE_MAILBOX: u32 = 1;
const PRESENT_MODE_FIFO: u32 = 2;
const IMAGE_USAGE_TRANSFER_DST: u32 = 0x2;
const IMAGE_USAGE_COLOR_ATTACHMENT: u32 = 0x10;
const LAYOUT_UNDEFINED: u32 = 0;
const LAYOUT_TRANSFER_DST_OPTIMAL: u32 = 7;
const LAYOUT_PRESENT_SRC: u32 = 1_000_001_002;
const ACCESS_TRANSFER_WRITE: u32 = 0x1000;
const STAGE_TOP_OF_PIPE: u32 = 0x1;
const STAGE_TRANSFER: u32 = 0x1000;
const STAGE_BOTTOM_OF_PIPE: u32 = 0x2000;
const QUEUE_GRAPHICS: u32 = 0x1;
const QUEUE_FAMILY_IGNORED: u32 = !0;
const COMMAND_POOL_RESET_COMMAND_BUFFER: u32 = 0x2;
const MEMORY_DEVICE_LOCAL: u32 = 0x1;
const COMPOSITE_ALPHA_OPAQUE: u32 = 0x1;
const COMPOSITE_ALPHA_INHERIT: u32 = 0x8;
const OFFSCREEN_SIZE: u32 = 256;
const COMPLETION_WAIT_NS: u64 = 10_000_000_000;

#[repr(C)]
struct ApplicationInfo {
    kind: u32,
    next: *const c_void,
    application_name: *const c_char,
    application_version: u32,
    engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

#[repr(C)]
struct InstanceCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    application_info: *const ApplicationInfo,
    layer_count: u32,
    layer_names: *const *const c_char,
    extension_count: u32,
    extension_names: *const *const c_char,
}

#[repr(C)]
struct ExtensionProperties {
    name: [c_char; 256],
    version: u32,
}

/// `VkPhysicalDeviceProperties`, read by offset: 824 bytes aligned to eight.
#[repr(C, align(8))]
struct PhysicalDeviceProperties([u8; 824]);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct QueueFamilyProperties {
    flags: u32,
    count: u32,
    timestamp_valid_bits: u32,
    granularity: [u32; 3],
}

#[repr(C)]
struct DeviceQueueCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    family: u32,
    count: u32,
    priorities: *const f32,
}

#[repr(C)]
struct DeviceCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    queue_count: u32,
    queues: *const DeviceQueueCreateInfo,
    layer_count: u32,
    layer_names: *const *const c_char,
    extension_count: u32,
    extension_names: *const *const c_char,
    features: *const c_void,
}

#[repr(C)]
struct SurfaceCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    first: usize,
    second: usize,
}

#[repr(C)]
struct XcbSurfaceCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    connection: usize,
    window: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SurfaceCapabilities {
    min_image_count: u32,
    max_image_count: u32,
    current_extent: [u32; 2],
    min_extent: [u32; 2],
    max_extent: [u32; 2],
    max_array_layers: u32,
    supported_transforms: u32,
    current_transform: u32,
    supported_composite_alpha: u32,
    supported_usage: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SurfaceFormat {
    format: u32,
    color_space: u32,
}

#[repr(C)]
struct SwapchainCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    surface: Object,
    min_image_count: u32,
    format: u32,
    color_space: u32,
    extent: [u32; 2],
    array_layers: u32,
    usage: u32,
    sharing_mode: u32,
    queue_family_count: u32,
    queue_families: *const u32,
    pre_transform: u32,
    composite_alpha: u32,
    present_mode: u32,
    clipped: u32,
    old_swapchain: Object,
}

#[repr(C)]
struct PresentInfo {
    kind: u32,
    next: *const c_void,
    wait_count: u32,
    wait_semaphores: *const Object,
    swapchain_count: u32,
    swapchains: *const Object,
    image_indices: *const u32,
    results: *mut VkResult,
}

#[repr(C)]
struct CommandPoolCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    family: u32,
}

#[repr(C)]
struct CommandBufferAllocateInfo {
    kind: u32,
    next: *const c_void,
    pool: Object,
    level: u32,
    count: u32,
}

#[repr(C)]
struct CommandBufferBeginInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    inheritance: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SubresourceRange {
    aspect: u32,
    base_level: u32,
    level_count: u32,
    base_layer: u32,
    layer_count: u32,
}

const COLOR_RANGE: SubresourceRange = SubresourceRange { aspect: 1, base_level: 0, level_count: 1, base_layer: 0, layer_count: 1 };

#[repr(C)]
struct ImageMemoryBarrier {
    kind: u32,
    next: *const c_void,
    source_access: u32,
    destination_access: u32,
    old_layout: u32,
    new_layout: u32,
    source_family: u32,
    destination_family: u32,
    image: Object,
    range: SubresourceRange,
}

#[repr(C)]
struct SubmitInfo {
    kind: u32,
    next: *const c_void,
    wait_count: u32,
    wait_semaphores: *const Object,
    wait_stages: *const u32,
    command_buffer_count: u32,
    command_buffers: *const Handle,
    signal_count: u32,
    signal_semaphores: *const Object,
}

#[repr(C)]
struct SimpleCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
}

#[repr(C)]
struct ImageCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    image_type: u32,
    format: u32,
    extent: [u32; 3],
    mip_levels: u32,
    array_layers: u32,
    samples: u32,
    tiling: u32,
    usage: u32,
    sharing_mode: u32,
    queue_family_count: u32,
    queue_families: *const u32,
    initial_layout: u32,
}

#[repr(C)]
#[derive(Default)]
struct MemoryRequirements {
    size: u64,
    alignment: u64,
    type_bits: u32,
}

#[repr(C)]
struct MemoryAllocateInfo {
    kind: u32,
    next: *const c_void,
    size: u64,
    type_index: u32,
}

#[repr(C)]
struct MemoryProperties {
    type_count: u32,
    types: [[u32; 2]; 32],
    heap_count: u32,
    heaps: [[u64; 2]; 16],
}

/// Fetches an entry point by name through one of the two lookup functions.
macro_rules! entry {
    ($lookup:expr, $scope:expr, $name:literal, $signature:ty) => {{
        // SAFETY: the lookup function is the loader's; the name is
        // NUL-terminated; the signature is the one Vulkan gives the name.
        match unsafe { ($lookup)($scope, concat!($name, "\0").as_ptr().cast()) } {
            Some(function) => Ok(unsafe { std::mem::transmute::<VoidFunction, $signature>(function) }),
            None => Err(format!("{} is not offered", $name)),
        }
    }};
}

fn check(result: VkResult, call: &str) -> Result<(), String> {
    if result >= 0 { Ok(()) } else { Err(format!("{call} failed with {}", result_name(result))) }
}

fn result_name(result: VkResult) -> String {
    match result {
        SUCCESS => "VK_SUCCESS".to_string(),
        NOT_READY => "VK_NOT_READY".to_string(),
        TIMEOUT => "VK_TIMEOUT".to_string(),
        SUBOPTIMAL => "VK_SUBOPTIMAL_KHR".to_string(),
        ERROR_OUT_OF_DATE => "VK_ERROR_OUT_OF_DATE_KHR".to_string(),
        -1_000_000_000 => "VK_ERROR_SURFACE_LOST_KHR".to_string(),
        -1_000_000_001 => "VK_ERROR_NATIVE_WINDOW_IN_USE_KHR".to_string(),
        -4 => "VK_ERROR_DEVICE_LOST".to_string(),
        -3 => "VK_ERROR_INITIALIZATION_FAILED".to_string(),
        -7 => "VK_ERROR_EXTENSION_NOT_PRESENT".to_string(),
        -9 => "VK_ERROR_INCOMPATIBLE_DRIVER".to_string(),
        other => format!("VkResult {other}"),
    }
}

struct Instance {
    handle: Handle,
    lookup: GetProcAddr,
    extensions: Vec<String>,
    _library: Library,
}

impl Drop for Instance {
    fn drop(&mut self) {
        if let Ok(destroy) = entry!(self.lookup, self.handle, "vkDestroyInstance", unsafe extern "system" fn(Handle, *const c_void)) {
            // SAFETY: the instance is live and every object created from it
            // was destroyed by the presenter that owned it.
            unsafe { destroy(self.handle, ptr::null()) };
        }
    }
}

/// Opens the loader and creates an instance with the surface extensions the
/// loader offers among those the spike can use.
fn instance() -> Result<Instance, String> {
    let library = Library::open(if cfg!(windows) { &["vulkan-1.dll"] } else { &["libvulkan.so.1", "libvulkan.so"] })?;
    // SAFETY: the loader exports this function with this signature.
    let lookup: GetProcAddr = unsafe { library.function("vkGetInstanceProcAddr")? };
    let enumerate = entry!(lookup, ptr::null_mut(), "vkEnumerateInstanceExtensionProperties", unsafe extern "system" fn(*const c_char, *mut u32, *mut ExtensionProperties) -> VkResult)?;
    let create = entry!(lookup, ptr::null_mut(), "vkCreateInstance", unsafe extern "system" fn(*const InstanceCreateInfo, *const c_void, *mut Handle) -> VkResult)?;
    let mut count = 0_u32;
    // SAFETY: the first call writes the count, the second fills that many
    // plain structures.
    let offered: Vec<String> = unsafe {
        check(enumerate(ptr::null(), &mut count, ptr::null_mut()), "vkEnumerateInstanceExtensionProperties")?;
        let mut properties: Vec<ExtensionProperties> = (0..count).map(|_| std::mem::zeroed()).collect();
        check(enumerate(ptr::null(), &mut count, properties.as_mut_ptr()), "vkEnumerateInstanceExtensionProperties")?;
        properties.iter().take(count as usize).map(|property| CStr::from_ptr(property.name.as_ptr()).to_string_lossy().into_owned()).collect()
    };
    let wanted = ["VK_KHR_surface", "VK_KHR_win32_surface", "VK_KHR_xlib_surface", "VK_KHR_xcb_surface", "VK_KHR_wayland_surface"];
    let extensions: Vec<String> = wanted.iter().filter(|name| offered.iter().any(|offered| offered == *name)).map(|name| name.to_string()).collect();
    let names: Vec<std::ffi::CString> = extensions.iter().map(|name| crate::loader::c_string(name)).collect();
    let pointers: Vec<*const c_char> = names.iter().map(|name| name.as_ptr()).collect();
    let application = ApplicationInfo { kind: STRUCTURE_APPLICATION_INFO, next: ptr::null(), application_name: c"render-windowed-presentation".as_ptr(), application_version: 0, engine_name: ptr::null(), engine_version: 0, api_version: 1 << 22 };
    let info = InstanceCreateInfo { kind: STRUCTURE_INSTANCE_CREATE_INFO, next: ptr::null(), flags: 0, application_info: &application, layer_count: 0, layer_names: ptr::null(), extension_count: pointers.len() as u32, extension_names: pointers.as_ptr() };
    let mut handle: Handle = ptr::null_mut();
    // SAFETY: the structures and the names outlive the call, which writes
    // the instance.
    check(unsafe { create(&info, ptr::null(), &mut handle) }, "vkCreateInstance")?;
    Ok(Instance { handle, lookup, extensions, _library: library })
}

struct PhysicalDevice {
    handle: Handle,
    name: String,
    kind: &'static str,
}

fn physical_devices(instance: &Instance) -> Result<Vec<PhysicalDevice>, String> {
    let enumerate = entry!(instance.lookup, instance.handle, "vkEnumeratePhysicalDevices", unsafe extern "system" fn(Handle, *mut u32, *mut Handle) -> VkResult)?;
    let properties = entry!(instance.lookup, instance.handle, "vkGetPhysicalDeviceProperties", unsafe extern "system" fn(Handle, *mut PhysicalDeviceProperties))?;
    let mut count = 0_u32;
    // SAFETY: the first call writes the count, the second fills that many
    // handles; the properties call fills 824 bytes.
    unsafe {
        check(enumerate(instance.handle, &mut count, ptr::null_mut()), "vkEnumeratePhysicalDevices")?;
        let mut handles = vec![ptr::null_mut(); count as usize];
        check(enumerate(instance.handle, &mut count, handles.as_mut_ptr()), "vkEnumeratePhysicalDevices")?;
        handles.truncate(count as usize);
        Ok(handles
            .into_iter()
            .map(|handle| {
                let mut bytes = PhysicalDeviceProperties([0; 824]);
                properties(handle, &mut bytes);
                let device_type = u32::from_le_bytes([bytes.0[16], bytes.0[17], bytes.0[18], bytes.0[19]]);
                let kind = match device_type {
                    1 => "integrated",
                    2 => "discrete",
                    3 => "virtual",
                    4 => "software",
                    _ => "other",
                };
                PhysicalDevice { handle, name: crate::loader::text_of(&bytes.0[20..276]), kind }
            })
            .collect())
    }
}

/// The physical devices the loader lists, in its order.
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    let instance = instance()?;
    Ok(physical_devices(&instance)?.into_iter().enumerate().map(|(index, device)| DeviceInfo { index, name: device.name, kind: device.kind.to_string() }).collect())
}

/// The device-level entry points the presenter calls.
struct Calls {
    destroy_device: unsafe extern "system" fn(Handle, *const c_void),
    wait_idle: unsafe extern "system" fn(Handle) -> VkResult,
    create_swapchain: unsafe extern "system" fn(Handle, *const SwapchainCreateInfo, *const c_void, *mut Object) -> VkResult,
    destroy_swapchain: unsafe extern "system" fn(Handle, Object, *const c_void),
    swapchain_images: unsafe extern "system" fn(Handle, Object, *mut u32, *mut Object) -> VkResult,
    acquire: unsafe extern "system" fn(Handle, Object, u64, Object, Object, *mut u32) -> VkResult,
    present: unsafe extern "system" fn(Handle, *const PresentInfo) -> VkResult,
    destroy_pool: unsafe extern "system" fn(Handle, Object, *const c_void),
    begin: unsafe extern "system" fn(Handle, *const CommandBufferBeginInfo) -> VkResult,
    end: unsafe extern "system" fn(Handle) -> VkResult,
    barrier: unsafe extern "system" fn(Handle, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const ImageMemoryBarrier),
    clear: unsafe extern "system" fn(Handle, Object, u32, *const [f32; 4], u32, *const SubresourceRange),
    submit: unsafe extern "system" fn(Handle, u32, *const SubmitInfo, Object) -> VkResult,
    destroy_fence: unsafe extern "system" fn(Handle, Object, *const c_void),
    wait_fences: unsafe extern "system" fn(Handle, u32, *const Object, u32, u64) -> VkResult,
    reset_fences: unsafe extern "system" fn(Handle, u32, *const Object) -> VkResult,
    destroy_semaphore: unsafe extern "system" fn(Handle, Object, *const c_void),
    destroy_image: unsafe extern "system" fn(Handle, Object, *const c_void),
    free_memory: unsafe extern "system" fn(Handle, Object, *const c_void),
}

pub struct VulkanPresenter {
    present_mode: u32,
    offered_modes: Vec<u32>,
    acquire_wait_ns: u64,
    device_name: String,
    device_kind: &'static str,
    surface_extension: &'static str,
    created_on_thread: String,
    timing_extensions: Vec<String>,
    wanted_extent: [u32; 2],
    extent: [u32; 2],
    surface_format: SurfaceFormat,
    images: Vec<Object>,
    swapchain: Object,
    surface: Object,
    command_buffers: [Handle; 2],
    pool: Object,
    fences: [Object; 2],
    acquired: Object,
    rendered: Object,
    offscreen_image: Object,
    offscreen_memory: Object,
    queue: Handle,
    physical: Handle,
    device: Handle,
    calls: Calls,
    capabilities: unsafe extern "system" fn(Handle, Object, *mut SurfaceCapabilities) -> VkResult,
    destroy_surface: unsafe extern "system" fn(Handle, Object, *const c_void),
    instance: Instance,
}

impl VulkanPresenter {
    pub fn open(request: &Request) -> Result<VulkanPresenter, String> {
        let instance = instance()?;
        let (lookup, scope) = (instance.lookup, instance.handle);
        let has = |name: &str| instance.extensions.iter().any(|extension| extension == name);
        if !has("VK_KHR_surface") {
            return Err("the loader offers no VK_KHR_surface".to_string());
        }
        // The surface, through the extension of the window's display system.
        let mut surface: Object = 0;
        let surface_extension = match request.window {
            NativeWindow::Win32 { hwnd, hinstance } => {
                let create = entry!(lookup, scope, "vkCreateWin32SurfaceKHR", unsafe extern "system" fn(Handle, *const SurfaceCreateInfo, *const c_void, *mut Object) -> VkResult)?;
                let info = SurfaceCreateInfo { kind: STRUCTURE_WIN32_SURFACE_CREATE_INFO, next: ptr::null(), flags: 0, first: hinstance, second: hwnd };
                // SAFETY: the instance and the window are live.
                check(unsafe { create(scope, &info, ptr::null(), &mut surface) }, "vkCreateWin32SurfaceKHR")?;
                "VK_KHR_win32_surface"
            }
            NativeWindow::Xlib { display, window } => {
                let create = entry!(lookup, scope, "vkCreateXlibSurfaceKHR", unsafe extern "system" fn(Handle, *const SurfaceCreateInfo, *const c_void, *mut Object) -> VkResult)?;
                let info = SurfaceCreateInfo { kind: STRUCTURE_XLIB_SURFACE_CREATE_INFO, next: ptr::null(), flags: 0, first: display, second: window as usize };
                // SAFETY: the instance, the display and the window are live.
                check(unsafe { create(scope, &info, ptr::null(), &mut surface) }, "vkCreateXlibSurfaceKHR")?;
                "VK_KHR_xlib_surface"
            }
            NativeWindow::Xcb { connection, window } => {
                let create = entry!(lookup, scope, "vkCreateXcbSurfaceKHR", unsafe extern "system" fn(Handle, *const XcbSurfaceCreateInfo, *const c_void, *mut Object) -> VkResult)?;
                let info = XcbSurfaceCreateInfo { kind: STRUCTURE_XCB_SURFACE_CREATE_INFO, next: ptr::null(), flags: 0, connection, window };
                // SAFETY: the instance, the connection and the window are live.
                check(unsafe { create(scope, &info, ptr::null(), &mut surface) }, "vkCreateXcbSurfaceKHR")?;
                "VK_KHR_xcb_surface"
            }
            NativeWindow::Wayland { display, surface: wayland_surface } => {
                let create = entry!(lookup, scope, "vkCreateWaylandSurfaceKHR", unsafe extern "system" fn(Handle, *const SurfaceCreateInfo, *const c_void, *mut Object) -> VkResult)?;
                let info = SurfaceCreateInfo { kind: STRUCTURE_WAYLAND_SURFACE_CREATE_INFO, next: ptr::null(), flags: 0, first: display, second: wayland_surface };
                // SAFETY: the instance, the display and the surface are live.
                check(unsafe { create(scope, &info, ptr::null(), &mut surface) }, "vkCreateWaylandSurfaceKHR")?;
                "VK_KHR_wayland_surface"
            }
            NativeWindow::AppKit { .. } => return Err("the spike presents through Vulkan on Windows and Linux only".to_string()),
        };
        let destroy_surface = entry!(lookup, scope, "vkDestroySurfaceKHR", unsafe extern "system" fn(Handle, Object, *const c_void))?;
        // From here an early return would leak the surface and the instance's
        // children; the process of a failed run ends at once, which is the
        // spike's only cleanup on that path.
        let listed = physical_devices(&instance)?;
        let Some(physical) = listed.get(request.device) else {
            return Err(format!("no physical device at index {}", request.device));
        };
        let families = entry!(lookup, scope, "vkGetPhysicalDeviceQueueFamilyProperties", unsafe extern "system" fn(Handle, *mut u32, *mut QueueFamilyProperties))?;
        let surface_support = entry!(lookup, scope, "vkGetPhysicalDeviceSurfaceSupportKHR", unsafe extern "system" fn(Handle, u32, Object, *mut u32) -> VkResult)?;
        let capabilities = entry!(lookup, scope, "vkGetPhysicalDeviceSurfaceCapabilitiesKHR", unsafe extern "system" fn(Handle, Object, *mut SurfaceCapabilities) -> VkResult)?;
        let surface_formats = entry!(lookup, scope, "vkGetPhysicalDeviceSurfaceFormatsKHR", unsafe extern "system" fn(Handle, Object, *mut u32, *mut SurfaceFormat) -> VkResult)?;
        let surface_modes = entry!(lookup, scope, "vkGetPhysicalDeviceSurfacePresentModesKHR", unsafe extern "system" fn(Handle, Object, *mut u32, *mut u32) -> VkResult)?;
        let device_extensions = entry!(lookup, scope, "vkEnumerateDeviceExtensionProperties", unsafe extern "system" fn(Handle, *const c_char, *mut u32, *mut ExtensionProperties) -> VkResult)?;
        let memory_properties = entry!(lookup, scope, "vkGetPhysicalDeviceMemoryProperties", unsafe extern "system" fn(Handle, *mut MemoryProperties))?;
        let create_device = entry!(lookup, scope, "vkCreateDevice", unsafe extern "system" fn(Handle, *const DeviceCreateInfo, *const c_void, *mut Handle) -> VkResult)?;
        let device_lookup = entry!(lookup, scope, "vkGetDeviceProcAddr", GetProcAddr)?;

        // SAFETY: each pair of calls writes a count and then fills that many
        // plain structures; the handles are live.
        let (family, formats, modes, extensions, memory) = unsafe {
            let mut count = 0_u32;
            families(physical.handle, &mut count, ptr::null_mut());
            let mut listed_families = vec![QueueFamilyProperties::default(); count as usize];
            families(physical.handle, &mut count, listed_families.as_mut_ptr());
            let mut family = None;
            for (index, properties) in listed_families.iter().enumerate().take(count as usize) {
                let mut supported = 0_u32;
                check(surface_support(physical.handle, index as u32, surface, &mut supported), "vkGetPhysicalDeviceSurfaceSupportKHR")?;
                if properties.flags & QUEUE_GRAPHICS != 0 && supported != 0 {
                    family = Some(index as u32);
                    break;
                }
            }
            let family = family.ok_or("no queue family of the device both draws and presents to the surface")?;
            let mut count = 0_u32;
            check(surface_formats(physical.handle, surface, &mut count, ptr::null_mut()), "vkGetPhysicalDeviceSurfaceFormatsKHR")?;
            let mut formats = vec![SurfaceFormat::default(); count as usize];
            check(surface_formats(physical.handle, surface, &mut count, formats.as_mut_ptr()), "vkGetPhysicalDeviceSurfaceFormatsKHR")?;
            let mut count = 0_u32;
            check(surface_modes(physical.handle, surface, &mut count, ptr::null_mut()), "vkGetPhysicalDeviceSurfacePresentModesKHR")?;
            let mut modes = vec![0_u32; count as usize];
            check(surface_modes(physical.handle, surface, &mut count, modes.as_mut_ptr()), "vkGetPhysicalDeviceSurfacePresentModesKHR")?;
            let mut count = 0_u32;
            check(device_extensions(physical.handle, ptr::null(), &mut count, ptr::null_mut()), "vkEnumerateDeviceExtensionProperties")?;
            let mut properties: Vec<ExtensionProperties> = (0..count).map(|_| std::mem::zeroed()).collect();
            check(device_extensions(physical.handle, ptr::null(), &mut count, properties.as_mut_ptr()), "vkEnumerateDeviceExtensionProperties")?;
            let extensions: Vec<String> = properties.iter().take(count as usize).map(|property| CStr::from_ptr(property.name.as_ptr()).to_string_lossy().into_owned()).collect();
            let mut memory: MemoryProperties = std::mem::zeroed();
            memory_properties(physical.handle, &mut memory);
            (family, formats, modes, extensions, memory)
        };
        if !extensions.iter().any(|name| name == "VK_KHR_swapchain") {
            return Err("the device offers no VK_KHR_swapchain".to_string());
        }
        let present_mode = match request.mode {
            Mode::Synchronized => PRESENT_MODE_FIFO,
            Mode::Latest => PRESENT_MODE_MAILBOX,
            Mode::Unsynchronized => PRESENT_MODE_IMMEDIATE,
        };
        if !modes.contains(&present_mode) {
            return Err(format!("not offered: the surface lists the presentation modes {}", modes.iter().map(|&mode| mode_name(mode)).collect::<Vec<_>>().join(", ")));
        }
        let surface_format = formats
            .iter()
            .copied()
            .find(|format| format.format == FORMAT_B8G8R8A8_UNORM || format.format == FORMAT_R8G8B8A8_UNORM)
            .or(formats.first().copied())
            .ok_or("the surface lists no format")?;
        // The extensions through which a platform gives presentation times.
        let timing_extensions: Vec<String> = ["VK_GOOGLE_display_timing", "VK_EXT_present_timing"].iter().filter(|name| extensions.iter().any(|offered| offered == *name)).map(|name| name.to_string()).collect();

        let priority = 1.0_f32;
        let queue_info = DeviceQueueCreateInfo { kind: STRUCTURE_DEVICE_QUEUE_CREATE_INFO, next: ptr::null(), flags: 0, family, count: 1, priorities: &priority };
        let swapchain_name = c"VK_KHR_swapchain".as_ptr();
        let device_info = DeviceCreateInfo { kind: STRUCTURE_DEVICE_CREATE_INFO, next: ptr::null(), flags: 0, queue_count: 1, queues: &queue_info, layer_count: 0, layer_names: ptr::null(), extension_count: 1, extension_names: &swapchain_name, features: ptr::null() };
        let mut device: Handle = ptr::null_mut();
        // SAFETY: the structures outlive the call, which writes the device.
        check(unsafe { create_device(physical.handle, &device_info, ptr::null(), &mut device) }, "vkCreateDevice")?;

        let calls = Calls {
            destroy_device: entry!(device_lookup, device, "vkDestroyDevice", unsafe extern "system" fn(Handle, *const c_void))?,
            wait_idle: entry!(device_lookup, device, "vkDeviceWaitIdle", unsafe extern "system" fn(Handle) -> VkResult)?,
            create_swapchain: entry!(device_lookup, device, "vkCreateSwapchainKHR", unsafe extern "system" fn(Handle, *const SwapchainCreateInfo, *const c_void, *mut Object) -> VkResult)?,
            destroy_swapchain: entry!(device_lookup, device, "vkDestroySwapchainKHR", unsafe extern "system" fn(Handle, Object, *const c_void))?,
            swapchain_images: entry!(device_lookup, device, "vkGetSwapchainImagesKHR", unsafe extern "system" fn(Handle, Object, *mut u32, *mut Object) -> VkResult)?,
            acquire: entry!(device_lookup, device, "vkAcquireNextImageKHR", unsafe extern "system" fn(Handle, Object, u64, Object, Object, *mut u32) -> VkResult)?,
            present: entry!(device_lookup, device, "vkQueuePresentKHR", unsafe extern "system" fn(Handle, *const PresentInfo) -> VkResult)?,
            destroy_pool: entry!(device_lookup, device, "vkDestroyCommandPool", unsafe extern "system" fn(Handle, Object, *const c_void))?,
            begin: entry!(device_lookup, device, "vkBeginCommandBuffer", unsafe extern "system" fn(Handle, *const CommandBufferBeginInfo) -> VkResult)?,
            end: entry!(device_lookup, device, "vkEndCommandBuffer", unsafe extern "system" fn(Handle) -> VkResult)?,
            barrier: entry!(device_lookup, device, "vkCmdPipelineBarrier", unsafe extern "system" fn(Handle, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const ImageMemoryBarrier))?,
            clear: entry!(device_lookup, device, "vkCmdClearColorImage", unsafe extern "system" fn(Handle, Object, u32, *const [f32; 4], u32, *const SubresourceRange))?,
            submit: entry!(device_lookup, device, "vkQueueSubmit", unsafe extern "system" fn(Handle, u32, *const SubmitInfo, Object) -> VkResult)?,
            destroy_fence: entry!(device_lookup, device, "vkDestroyFence", unsafe extern "system" fn(Handle, Object, *const c_void))?,
            wait_fences: entry!(device_lookup, device, "vkWaitForFences", unsafe extern "system" fn(Handle, u32, *const Object, u32, u64) -> VkResult)?,
            reset_fences: entry!(device_lookup, device, "vkResetFences", unsafe extern "system" fn(Handle, u32, *const Object) -> VkResult)?,
            destroy_semaphore: entry!(device_lookup, device, "vkDestroySemaphore", unsafe extern "system" fn(Handle, Object, *const c_void))?,
            destroy_image: entry!(device_lookup, device, "vkDestroyImage", unsafe extern "system" fn(Handle, Object, *const c_void))?,
            free_memory: entry!(device_lookup, device, "vkFreeMemory", unsafe extern "system" fn(Handle, Object, *const c_void))?,
        };
        let get_queue = entry!(device_lookup, device, "vkGetDeviceQueue", unsafe extern "system" fn(Handle, u32, u32, *mut Handle))?;
        let create_pool = entry!(device_lookup, device, "vkCreateCommandPool", unsafe extern "system" fn(Handle, *const CommandPoolCreateInfo, *const c_void, *mut Object) -> VkResult)?;
        let allocate_buffers = entry!(device_lookup, device, "vkAllocateCommandBuffers", unsafe extern "system" fn(Handle, *const CommandBufferAllocateInfo, *mut Handle) -> VkResult)?;
        let create_fence = entry!(device_lookup, device, "vkCreateFence", unsafe extern "system" fn(Handle, *const SimpleCreateInfo, *const c_void, *mut Object) -> VkResult)?;
        let create_semaphore = entry!(device_lookup, device, "vkCreateSemaphore", unsafe extern "system" fn(Handle, *const SimpleCreateInfo, *const c_void, *mut Object) -> VkResult)?;
        let create_image = entry!(device_lookup, device, "vkCreateImage", unsafe extern "system" fn(Handle, *const ImageCreateInfo, *const c_void, *mut Object) -> VkResult)?;
        let image_requirements = entry!(device_lookup, device, "vkGetImageMemoryRequirements", unsafe extern "system" fn(Handle, Object, *mut MemoryRequirements))?;
        let allocate_memory = entry!(device_lookup, device, "vkAllocateMemory", unsafe extern "system" fn(Handle, *const MemoryAllocateInfo, *const c_void, *mut Object) -> VkResult)?;
        let bind_image = entry!(device_lookup, device, "vkBindImageMemory", unsafe extern "system" fn(Handle, Object, Object, u64) -> VkResult)?;

        let mut queue: Handle = ptr::null_mut();
        let mut pool: Object = 0;
        let mut command_buffers = [ptr::null_mut(); 2];
        let mut fences = [0_u64; 2];
        let (mut acquired, mut rendered) = (0_u64, 0_u64);
        let (mut offscreen_image, mut offscreen_memory) = (0_u64, 0_u64);
        // SAFETY: the device is live; every structure outlives its call, and
        // each call writes the object named.
        unsafe {
            get_queue(device, family, 0, &mut queue);
            let pool_info = CommandPoolCreateInfo { kind: STRUCTURE_COMMAND_POOL_CREATE_INFO, next: ptr::null(), flags: COMMAND_POOL_RESET_COMMAND_BUFFER, family };
            check(create_pool(device, &pool_info, ptr::null(), &mut pool), "vkCreateCommandPool")?;
            let buffer_info = CommandBufferAllocateInfo { kind: STRUCTURE_COMMAND_BUFFER_ALLOCATE_INFO, next: ptr::null(), pool, level: 0, count: 2 };
            check(allocate_buffers(device, &buffer_info, command_buffers.as_mut_ptr()), "vkAllocateCommandBuffers")?;
            let fence_info = SimpleCreateInfo { kind: STRUCTURE_FENCE_CREATE_INFO, next: ptr::null(), flags: 0 };
            for fence in &mut fences {
                check(create_fence(device, &fence_info, ptr::null(), fence), "vkCreateFence")?;
            }
            let semaphore_info = SimpleCreateInfo { kind: STRUCTURE_SEMAPHORE_CREATE_INFO, next: ptr::null(), flags: 0 };
            check(create_semaphore(device, &semaphore_info, ptr::null(), &mut acquired), "vkCreateSemaphore")?;
            check(create_semaphore(device, &semaphore_info, ptr::null(), &mut rendered), "vkCreateSemaphore")?;
            let image_info = ImageCreateInfo {
                kind: STRUCTURE_IMAGE_CREATE_INFO,
                next: ptr::null(),
                flags: 0,
                image_type: 1,
                format: FORMAT_R8G8B8A8_UNORM,
                extent: [OFFSCREEN_SIZE, OFFSCREEN_SIZE, 1],
                mip_levels: 1,
                array_layers: 1,
                samples: 1,
                tiling: 0,
                usage: IMAGE_USAGE_TRANSFER_DST | IMAGE_USAGE_COLOR_ATTACHMENT,
                sharing_mode: 0,
                queue_family_count: 0,
                queue_families: ptr::null(),
                initial_layout: LAYOUT_UNDEFINED,
            };
            check(create_image(device, &image_info, ptr::null(), &mut offscreen_image), "vkCreateImage for the offscreen target")?;
            let mut requirements = MemoryRequirements::default();
            image_requirements(device, offscreen_image, &mut requirements);
            let type_index = (0..memory.type_count).find(|&index| requirements.type_bits & (1 << index) != 0 && memory.types[index as usize][0] & MEMORY_DEVICE_LOCAL != 0).or((0..memory.type_count).find(|&index| requirements.type_bits & (1 << index) != 0)).ok_or("no memory type fits the offscreen target")?;
            let allocation = MemoryAllocateInfo { kind: STRUCTURE_MEMORY_ALLOCATE_INFO, next: ptr::null(), size: requirements.size, type_index };
            check(allocate_memory(device, &allocation, ptr::null(), &mut offscreen_memory), "vkAllocateMemory for the offscreen target")?;
            check(bind_image(device, offscreen_image, offscreen_memory, 0), "vkBindImageMemory")?;
        }

        let mut presenter = VulkanPresenter {
            present_mode,
            offered_modes: modes,
            acquire_wait_ns: u64::from(request.acquire_wait_ms) * 1_000_000,
            device_name: physical.name.clone(),
            device_kind: physical.kind,
            surface_extension,
            created_on_thread: std::thread::current().name().unwrap_or("unnamed").to_string(),
            timing_extensions,
            wanted_extent: [request.width, request.height],
            extent: [0, 0],
            surface_format,
            images: Vec::new(),
            swapchain: 0,
            surface,
            command_buffers,
            pool,
            fences,
            acquired,
            rendered,
            offscreen_image,
            offscreen_memory,
            queue,
            physical: physical.handle,
            device,
            calls,
            capabilities,
            destroy_surface,
            instance,
        };
        presenter.build_swapchain()?;
        Ok(presenter)
    }

    /// Creates the swapchain at the surface's current extent, or leaves none
    /// when the surface has no extent (a minimized window).
    fn build_swapchain(&mut self) -> Result<(), String> {
        // SAFETY: the device, the surface and the old swapchain are live;
        // the device is idle when the old swapchain is destroyed.
        unsafe {
            check((self.calls.wait_idle)(self.device), "vkDeviceWaitIdle")?;
            let mut capabilities = SurfaceCapabilities::default();
            check((self.capabilities)(self.physical, self.surface, &mut capabilities), "vkGetPhysicalDeviceSurfaceCapabilitiesKHR")?;
            let mut extent = capabilities.current_extent;
            if extent[0] == u32::MAX {
                // The surface takes the size of the swapchain (Wayland).
                extent = [self.wanted_extent[0].clamp(capabilities.min_extent[0], capabilities.max_extent[0].max(1)), self.wanted_extent[1].clamp(capabilities.min_extent[1], capabilities.max_extent[1].max(1))];
            }
            let old = self.swapchain;
            if extent[0] == 0 || extent[1] == 0 {
                if old != 0 {
                    (self.calls.destroy_swapchain)(self.device, old, ptr::null());
                }
                self.swapchain = 0;
                self.images.clear();
                self.extent = [0, 0];
                return Ok(());
            }
            let mut image_count = (capabilities.min_image_count + 1).max(3);
            if capabilities.max_image_count != 0 {
                image_count = image_count.min(capabilities.max_image_count);
            }
            if capabilities.supported_usage & IMAGE_USAGE_TRANSFER_DST == 0 {
                return Err("the surface's images cannot be the destination of a clear command".to_string());
            }
            let composite_alpha = if capabilities.supported_composite_alpha & COMPOSITE_ALPHA_OPAQUE != 0 { COMPOSITE_ALPHA_OPAQUE } else { COMPOSITE_ALPHA_INHERIT };
            let info = SwapchainCreateInfo {
                kind: STRUCTURE_SWAPCHAIN_CREATE_INFO,
                next: ptr::null(),
                flags: 0,
                surface: self.surface,
                min_image_count: image_count,
                format: self.surface_format.format,
                color_space: self.surface_format.color_space,
                extent,
                array_layers: 1,
                usage: IMAGE_USAGE_TRANSFER_DST | IMAGE_USAGE_COLOR_ATTACHMENT,
                sharing_mode: 0,
                queue_family_count: 0,
                queue_families: ptr::null(),
                pre_transform: capabilities.current_transform,
                composite_alpha,
                present_mode: self.present_mode,
                clipped: 1,
                old_swapchain: old,
            };
            let mut swapchain: Object = 0;
            let result = (self.calls.create_swapchain)(self.device, &info, ptr::null(), &mut swapchain);
            if old != 0 {
                (self.calls.destroy_swapchain)(self.device, old, ptr::null());
            }
            self.swapchain = 0;
            self.images.clear();
            check(result, "vkCreateSwapchainKHR")?;
            let mut count = 0_u32;
            check((self.calls.swapchain_images)(self.device, swapchain, &mut count, ptr::null_mut()), "vkGetSwapchainImagesKHR")?;
            let mut images = vec![0_u64; count as usize];
            check((self.calls.swapchain_images)(self.device, swapchain, &mut count, images.as_mut_ptr()), "vkGetSwapchainImagesKHR")?;
            images.truncate(count as usize);
            self.swapchain = swapchain;
            self.images = images;
            self.extent = extent;
        }
        Ok(())
    }

    /// Records a clear of `image` on one of the two command buffers, submits
    /// it with the semaphores given, and returns once it was submitted.
    fn clear_and_submit(&self, slot: usize, image: Object, final_layout: u32, color: [f32; 4], wait: Option<Object>, signal: Option<Object>) -> Result<(), String> {
        let buffer = self.command_buffers[slot];
        let fence = self.fences[slot];
        let barrier = |old: u32, new: u32, source: u32, destination: u32| ImageMemoryBarrier { kind: STRUCTURE_IMAGE_MEMORY_BARRIER, next: ptr::null(), source_access: source, destination_access: destination, old_layout: old, new_layout: new, source_family: QUEUE_FAMILY_IGNORED, destination_family: QUEUE_FAMILY_IGNORED, image, range: COLOR_RANGE };
        // SAFETY: the device and its objects are live; the command buffer's
        // previous submission has completed, which each caller waits for;
        // every structure outlives its call.
        unsafe {
            check((self.calls.reset_fences)(self.device, 1, &fence), "vkResetFences")?;
            let begin = CommandBufferBeginInfo { kind: STRUCTURE_COMMAND_BUFFER_BEGIN_INFO, next: ptr::null(), flags: 1, inheritance: ptr::null() };
            check((self.calls.begin)(buffer, &begin), "vkBeginCommandBuffer")?;
            let to_clear = barrier(LAYOUT_UNDEFINED, LAYOUT_TRANSFER_DST_OPTIMAL, 0, ACCESS_TRANSFER_WRITE);
            (self.calls.barrier)(buffer, if wait.is_some() { STAGE_TRANSFER } else { STAGE_TOP_OF_PIPE }, STAGE_TRANSFER, 0, 0, ptr::null(), 0, ptr::null(), 1, &to_clear);
            (self.calls.clear)(buffer, image, LAYOUT_TRANSFER_DST_OPTIMAL, &color, 1, &COLOR_RANGE);
            if final_layout != LAYOUT_TRANSFER_DST_OPTIMAL {
                let to_final = barrier(LAYOUT_TRANSFER_DST_OPTIMAL, final_layout, ACCESS_TRANSFER_WRITE, 0);
                (self.calls.barrier)(buffer, STAGE_TRANSFER, STAGE_BOTTOM_OF_PIPE, 0, 0, ptr::null(), 0, ptr::null(), 1, &to_final);
            }
            check((self.calls.end)(buffer), "vkEndCommandBuffer")?;
            let wait_stage = STAGE_TRANSFER;
            let wait_semaphore = wait.unwrap_or(0);
            let signal_semaphore = signal.unwrap_or(0);
            let submit = SubmitInfo {
                kind: STRUCTURE_SUBMIT_INFO,
                next: ptr::null(),
                wait_count: u32::from(wait.is_some()),
                wait_semaphores: &wait_semaphore,
                wait_stages: &wait_stage,
                command_buffer_count: 1,
                command_buffers: &buffer,
                signal_count: u32::from(signal.is_some()),
                signal_semaphores: &signal_semaphore,
            };
            check((self.calls.submit)(self.queue, 1, &submit, fence), "vkQueueSubmit")
        }
    }

    fn wait_for(&self, slot: usize) -> Result<(), String> {
        // SAFETY: the device and the fence are live.
        let result = unsafe { (self.calls.wait_fences)(self.device, 1, &self.fences[slot], 1, COMPLETION_WAIT_NS) };
        if result == TIMEOUT {
            return Err("the device did not finish a frame within ten seconds".to_string());
        }
        check(result, "vkWaitForFences")
    }
}

fn mode_name(mode: u32) -> &'static str {
    match mode {
        PRESENT_MODE_IMMEDIATE => "immediate",
        PRESENT_MODE_MAILBOX => "mailbox",
        PRESENT_MODE_FIFO => "first in first out",
        3 => "first in first out, relaxed",
        _ => "another mode",
    }
}

impl Presenter for VulkanPresenter {
    fn describe(&self) -> Json {
        Json::object()
            .with("interface", format!("Vulkan with {} and VK_KHR_swapchain, natively", self.surface_extension))
            .with("device", self.device_name.clone())
            .with("device_kind", self.device_kind)
            .with("swapchain", format!("{} images, presentation mode {}", self.images.len(), mode_name(self.present_mode)))
            .with("presentation_modes_offered", self.offered_modes.iter().map(|&mode| mode_name(mode)).collect::<Vec<_>>().join(", "))
            .with("presentation_time_extensions_offered", if self.timing_extensions.is_empty() { "none".to_string() } else { self.timing_extensions.join(", ") })
            .with("created_on_thread", self.created_on_thread.clone())
            .with("acquisition", format!("vkAcquireNextImageKHR with a wait of at most {} milliseconds", self.acquire_wait_ns / 1_000_000))
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        self.wanted_extent = [width, height];
        if [width, height] != self.extent {
            self.build_swapchain()?;
        }
        Ok(())
    }

    fn frame(&mut self, index: u64) -> Result<FrameOutcome, String> {
        let started = now_ns();
        if self.swapchain == 0 {
            // No swapchain can exist on a surface with no extent: look again.
            self.build_swapchain()?;
            if self.swapchain == 0 {
                return Ok(FrameOutcome::nothing(now_ns() - started, "no image: the surface has no extent"));
            }
        }
        crate::presenter::in_call(crate::presenter::ACQUISITION);
        let mut image = 0_u32;
        // SAFETY: the device, the swapchain and the semaphore are live; the
        // semaphore has no pending signal, since every frame that acquired
        // an image waited for its submission.
        let result = unsafe { (self.calls.acquire)(self.device, self.swapchain, self.acquire_wait_ns, self.acquired, 0, &mut image) };
        let acquired = now_ns();
        match result {
            SUCCESS | SUBOPTIMAL => {}
            NOT_READY | TIMEOUT => return Ok(FrameOutcome::nothing(acquired - started, format!("no image: {}", result_name(result)))),
            ERROR_OUT_OF_DATE => {
                self.build_swapchain()?;
                return Ok(FrameOutcome::nothing(now_ns() - started, "no image: VK_ERROR_OUT_OF_DATE_KHR, the swapchain was made again"));
            }
            other => return Err(format!("vkAcquireNextImageKHR failed with {}", result_name(other))),
        }
        crate::presenter::in_call(crate::presenter::SUBMISSION);
        self.clear_and_submit(0, self.images[image as usize], LAYOUT_PRESENT_SRC, frame_color(index), Some(self.acquired), Some(self.rendered))?;
        let submitted = now_ns();
        let info = PresentInfo { kind: STRUCTURE_PRESENT_INFO, next: ptr::null(), wait_count: 1, wait_semaphores: &self.rendered, swapchain_count: 1, swapchains: &self.swapchain, image_indices: &image, results: ptr::null_mut() };
        // SAFETY: the queue, the swapchain and the semaphore are live.
        crate::presenter::in_call(crate::presenter::PRESENTATION);
        let presented = unsafe { (self.calls.present)(self.queue, &info) };
        let presented_at = now_ns();
        crate::presenter::in_call(crate::presenter::WAIT_FOR_THE_DEVICE);
        self.wait_for(0)?;
        crate::presenter::in_call(crate::presenter::BETWEEN_CALLS);
        let waited = now_ns() - presented_at;
        let status = match presented {
            SUCCESS => "presented".to_string(),
            SUBOPTIMAL => "presented with VK_SUBOPTIMAL_KHR".to_string(),
            ERROR_OUT_OF_DATE => {
                self.build_swapchain()?;
                return Ok(FrameOutcome { acquire_ns: acquired - started, submit_ns: submitted - acquired, wait_ns: waited, present_ns: presented_at - submitted, presented: false, status: "presented nothing: VK_ERROR_OUT_OF_DATE_KHR, the swapchain was made again".to_string() });
            }
            other => return Err(format!("vkQueuePresentKHR failed with {}", result_name(other))),
        };
        if result == SUBOPTIMAL || presented == SUBOPTIMAL {
            self.build_swapchain()?;
        }
        Ok(FrameOutcome { acquire_ns: acquired - started, submit_ns: submitted - acquired, wait_ns: waited, present_ns: presented_at - submitted, presented: true, status })
    }

    fn offscreen(&mut self, index: u64) -> Result<(), String> {
        self.clear_and_submit(1, self.offscreen_image, LAYOUT_TRANSFER_DST_OPTIMAL, frame_color(index), None, None)?;
        self.wait_for(1)
    }

    fn presentation_times(&mut self) -> Json {
        Json::object()
            .with("source", "none read: the spike calls no presentation time extension of Vulkan")
            .with("presentation_time_extensions_offered", if self.timing_extensions.is_empty() { "none".to_string() } else { self.timing_extensions.join(", ") })
    }
}

impl Drop for VulkanPresenter {
    fn drop(&mut self) {
        // SAFETY: every object is live and destroyed once, children before
        // the device, the device and the surface before the instance, which
        // the field's own drop destroys last.
        unsafe {
            (self.calls.wait_idle)(self.device);
            if self.swapchain != 0 {
                (self.calls.destroy_swapchain)(self.device, self.swapchain, ptr::null());
            }
            (self.calls.destroy_image)(self.device, self.offscreen_image, ptr::null());
            (self.calls.free_memory)(self.device, self.offscreen_memory, ptr::null());
            (self.calls.destroy_semaphore)(self.device, self.acquired, ptr::null());
            (self.calls.destroy_semaphore)(self.device, self.rendered, ptr::null());
            for fence in self.fences {
                (self.calls.destroy_fence)(self.device, fence, ptr::null());
            }
            (self.calls.destroy_pool)(self.device, self.pool, ptr::null());
            (self.calls.destroy_device)(self.device, ptr::null());
            (self.destroy_surface)(self.instance.handle, self.surface, ptr::null());
        }
    }
}
