//! The instance, the physical devices, the logical device, and a scope that
//! owns the objects one exercise creates and destroys them all, on success
//! as on failure. Throwaway, never product code.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::ffi::*;
use crate::backend::DeviceKind;
use crate::loader::{Library, c_string, text_of};

pub const VALIDATION_LAYER: &str = "VK_LAYER_KHRONOS_validation";
const DEBUG_UTILS_EXTENSION: &str = "VK_EXT_debug_utils";
/// The longest the module waits for the device to finish a submission.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(120);

/// The messages the validation layer delivered to the callback, until drained.
#[derive(Default)]
pub struct MessageSink {
    messages: Mutex<Vec<String>>,
}

impl MessageSink {
    pub fn drain(&self) -> Vec<String> {
        match self.messages.lock() {
            Ok(mut messages) => std::mem::take(&mut *messages),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }
}

/// The messenger's callback: stores each warning and error with its severity
/// and identifier, and lets the call that caused it continue.
unsafe extern "system" fn store_message(severity: u32, _types: u32, data: *const DebugUtilsMessengerCallbackData, user_data: *mut c_void) -> Bool32 {
    if data.is_null() || user_data.is_null() {
        return 0;
    }
    // SAFETY: the layer passes a valid callback structure for the duration of
    // the call, and `user_data` is the `MessageSink` registered with the
    // messenger, which outlives it.
    let (data, sink) = unsafe { (&*data, &*(user_data as *const MessageSink)) };
    let text = |pointer: *const c_char| {
        if pointer.is_null() {
            String::new()
        } else {
            // SAFETY: the layer's strings are NUL-terminated and live for the call.
            unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned()
        }
    };
    let level = if severity & DEBUG_SEVERITY_ERROR != 0 { "error" } else { "warning" };
    let message = format!("{level}: {}: {}", text(data.message_id_name), text(data.message));
    if let Ok(mut messages) = sink.messages.lock() {
        messages.push(message);
    }
    0
}

/// The instance, with its debug messenger when validation is on.
pub struct Instance {
    pub functions: InstanceFunctions,
    /// The physical-device queries of 1.1, where the instance offers them.
    pub functions11: Option<InstanceFunctions11>,
    pub handle: Handle,
    /// The version the instance was created for.
    pub api_version: u32,
    messenger: Option<(Object, DebugUtilsFunctions)>,
}

/// The entries of a two-call enumeration: the count, then the entries.
fn enumerate<T>(call: &str, empty: fn() -> T, get: impl Fn(*mut u32, *mut T) -> VkResult) -> Result<Vec<T>, String> {
    let mut count = 0_u32;
    check(get(&mut count, ptr::null_mut()), call)?;
    let mut entries: Vec<T> = (0..count).map(|_| empty()).collect();
    check(get(&mut count, entries.as_mut_ptr()), call)?;
    entries.truncate(count as usize);
    Ok(entries)
}

fn empty_extension() -> ExtensionProperties {
    ExtensionProperties { extension_name: [0; 256], spec_version: 0 }
}

fn instance_layers(global: &GlobalFunctions) -> Result<Vec<String>, String> {
    let empty = || LayerProperties { layer_name: [0; 256], spec_version: 0, implementation_version: 0, description: [0; 256] };
    // SAFETY: the loader fills at most `count` entries of the array it is given.
    let layers = enumerate("vkEnumerateInstanceLayerProperties", empty, |count, out| unsafe { (global.enumerate_instance_layer_properties)(count, out) })?;
    Ok(layers.iter().map(|layer| text_of(&layer.layer_name)).collect())
}

fn instance_extensions(global: &GlobalFunctions, layer: Option<&str>) -> Result<Vec<String>, String> {
    let layer_name = layer.map(c_string);
    let layer_pointer = layer_name.as_ref().map_or(ptr::null(), |name| name.as_ptr());
    // SAFETY: as above; the layer name is NUL-terminated or null.
    let extensions = enumerate("vkEnumerateInstanceExtensionProperties", empty_extension, |count, out| unsafe { (global.enumerate_instance_extension_properties)(layer_pointer, count, out) })?;
    Ok(extensions.iter().map(|extension| text_of(&extension.extension_name)).collect())
}

/// The highest instance version the loader offers.
fn loader_version(get: GetProcAddr) -> u32 {
    // SAFETY: `vkEnumerateInstanceVersion` has this signature; a 1.0 loader
    // does not provide it.
    let enumerate = unsafe { entry::<unsafe extern "system" fn(*mut u32) -> VkResult>(get, ptr::null_mut(), "vkEnumerateInstanceVersion") };
    let mut version = API_VERSION_1_0;
    if let Ok(enumerate) = enumerate {
        // SAFETY: the function writes one version.
        if unsafe { enumerate(&mut version) } != SUCCESS {
            version = API_VERSION_1_0;
        }
    }
    version
}

impl Instance {
    /// Creates an instance for Vulkan 1.3 where the loader offers 1.1 or
    /// later (a device is then used at its own version), else for 1.0. With
    /// a sink, the validation layer and the debug utils extension are enabled
    /// and a messenger delivers warnings and errors to it.
    pub fn create(library: &Library, sink: Option<&MessageSink>) -> Result<Instance, String> {
        // SAFETY: the loader exports `vkGetInstanceProcAddr` with this signature.
        let get: GetProcAddr = unsafe { library.function("vkGetInstanceProcAddr")? };
        let global = GlobalFunctions::load(get, ptr::null_mut())?;
        let api_version = if loader_version(get) >= API_VERSION_1_1 { API_VERSION_1_3 } else { API_VERSION_1_0 };
        let (layers, extensions) = match sink {
            Some(_) => validation_names(&global)?,
            None => (Vec::new(), Vec::new()),
        };
        let layer_names: Vec<_> = layers.iter().map(|name| c_string(name)).collect();
        let extension_names: Vec<_> = extensions.iter().map(|name| c_string(name)).collect();
        let layer_pointers: Vec<*const c_char> = layer_names.iter().map(|name| name.as_ptr()).collect();
        let extension_pointers: Vec<*const c_char> = extension_names.iter().map(|name| name.as_ptr()).collect();
        let application_name = c_string("render-device-baseline");
        let application = ApplicationInfo {
            s_type: structure::APPLICATION_INFO,
            p_next: ptr::null(),
            application_name: application_name.as_ptr(),
            application_version: 1,
            engine_name: application_name.as_ptr(),
            engine_version: 1,
            api_version,
        };
        let create_info = InstanceCreateInfo {
            s_type: structure::INSTANCE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            application_info: &application,
            enabled_layer_count: layer_pointers.len() as u32,
            enabled_layer_names: layer_pointers.as_ptr(),
            enabled_extension_count: extension_pointers.len() as u32,
            enabled_extension_names: extension_pointers.as_ptr(),
        };
        let mut handle: Handle = ptr::null_mut();
        // SAFETY: every pointer of the structure lives until the call returns.
        check(unsafe { (global.create_instance)(&create_info, ptr::null(), &mut handle) }, "vkCreateInstance")?;
        let functions = match InstanceFunctions::load(get, handle) {
            Ok(functions) => functions,
            Err(reason) => {
                // SAFETY: `vkDestroyInstance` has this signature; the instance
                // was just created and nothing uses it.
                unsafe {
                    if let Ok(destroy) = entry::<unsafe extern "system" fn(Handle, Allocator)>(get, handle, "vkDestroyInstance") {
                        destroy(handle, ptr::null());
                    }
                }
                return Err(reason);
            }
        };
        let functions11 = if api_version >= API_VERSION_1_1 { InstanceFunctions11::load(get, handle).ok() } else { None };
        let mut instance = Instance { functions, functions11, handle, api_version, messenger: None };
        if let Some(sink) = sink {
            instance.messenger = Some(create_messenger(get, handle, sink)?);
        }
        Ok(instance)
    }

    pub fn physical_devices(&self) -> Result<Vec<Handle>, String> {
        // SAFETY: called first with no array to write the count, then with an
        // array of that many handles.
        enumerate("vkEnumeratePhysicalDevices", ptr::null_mut, |count, out| unsafe { (self.functions.enumerate_physical_devices)(self.handle, count, out) })
    }
}

/// The layer and extension that validation needs, or why they are missing.
fn validation_names(global: &GlobalFunctions) -> Result<(Vec<String>, Vec<String>), String> {
    let layers = instance_layers(global)?;
    if !layers.iter().any(|layer| layer == VALIDATION_LAYER) {
        let listed = if layers.is_empty() { "none".to_string() } else { layers.join(", ") };
        return Err(format!("the loader does not list the layer {VALIDATION_LAYER} (layers listed: {listed})"));
    }
    let mut extensions = instance_extensions(global, None)?;
    extensions.extend(instance_extensions(global, Some(VALIDATION_LAYER))?);
    if !extensions.iter().any(|extension| extension == DEBUG_UTILS_EXTENSION) {
        return Err(format!("neither the loader nor {VALIDATION_LAYER} offers the extension {DEBUG_UTILS_EXTENSION}"));
    }
    Ok((vec![VALIDATION_LAYER.to_string()], vec![DEBUG_UTILS_EXTENSION.to_string()]))
}

fn create_messenger(get: GetProcAddr, instance: Handle, sink: &MessageSink) -> Result<(Object, DebugUtilsFunctions), String> {
    let functions = DebugUtilsFunctions::load(get, instance)?;
    let info = DebugUtilsMessengerCreateInfo {
        s_type: structure::DEBUG_UTILS_MESSENGER_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        message_severity: DEBUG_SEVERITY_WARNING | DEBUG_SEVERITY_ERROR,
        message_type: DEBUG_TYPE_GENERAL | DEBUG_TYPE_VALIDATION | DEBUG_TYPE_PERFORMANCE,
        user_callback: store_message,
        user_data: sink as *const MessageSink as *mut c_void,
    };
    let mut messenger: Object = 0;
    // SAFETY: the structure is valid for the call; the sink outlives the
    // messenger (the device that owns both destroys the messenger first).
    check(unsafe { (functions.create_debug_utils_messenger)(instance, &info, ptr::null(), &mut messenger) }, "vkCreateDebugUtilsMessengerEXT")?;
    Ok((messenger, functions))
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the messenger and the instance belong to this value, and
        // every device created from the instance was destroyed before it.
        unsafe {
            if let Some((messenger, functions)) = &self.messenger {
                (functions.destroy_debug_utils_messenger)(self.handle, *messenger, ptr::null());
            }
            (self.functions.destroy_instance)(self.handle, ptr::null());
        }
    }
}

/// What a physical device says about itself.
pub struct PhysicalDevice {
    pub handle: Handle,
    pub properties: PropertiesBytes,
    /// The device's version, as used through this instance.
    pub api_version: u32,
    /// The device's own version.
    pub reported_version: u32,
    pub name: String,
    pub kind: DeviceKind,
    pub vendor_id: u32,
    pub device_id: u32,
    pub driver: String,
    pub extensions: Vec<String>,
    pub families: Vec<QueueFamilyProperties>,
}

impl PhysicalDevice {
    pub fn describe(instance: &Instance, handle: Handle) -> Result<PhysicalDevice, String> {
        let mut properties = PropertiesBytes::zeroed();
        // SAFETY: the structure has the size of `VkPhysicalDeviceProperties`.
        unsafe { (instance.functions.get_physical_device_properties)(handle, &mut properties) };
        let reported_version = properties.u32_at(property::API_VERSION);
        let extensions = device_extensions(instance, handle)?;
        let mut device = PhysicalDevice {
            handle,
            api_version: reported_version.min(instance.api_version),
            reported_version,
            name: text_of(&properties.0[property::DEVICE_NAME..property::DEVICE_NAME + 256]),
            kind: device_kind(properties.u32_at(property::DEVICE_TYPE)),
            vendor_id: properties.u32_at(property::VENDOR_ID),
            device_id: properties.u32_at(property::DEVICE_ID),
            driver: String::new(),
            families: queue_families(instance, handle),
            properties,
            extensions,
        };
        device.driver = device.driver_text(instance);
        Ok(device)
    }

    pub fn has_extension(&self, name: &str) -> bool {
        self.extensions.iter().any(|extension| extension == name)
    }

    /// The driver name and information of the driver properties, where the
    /// device has them, else the raw driver version.
    fn driver_text(&self, instance: &Instance) -> String {
        let raw = format!("driverVersion 0x{:08x}", self.properties.u32_at(property::DRIVER_VERSION));
        let offered = self.api_version >= API_VERSION_1_2 || (self.api_version >= API_VERSION_1_1 && self.has_extension("VK_KHR_driver_properties"));
        let Some(functions) = instance.functions11.as_ref().filter(|_| offered) else { return raw };
        let mut driver = DriverProperties {
            s_type: structure::PHYSICAL_DEVICE_DRIVER_PROPERTIES,
            p_next: ptr::null_mut(),
            driver_id: 0,
            driver_name: [0; 256],
            driver_info: [0; 256],
            conformance_version: [0; 4],
        };
        let mut properties = Properties2 { s_type: structure::PHYSICAL_DEVICE_PROPERTIES_2, p_next: &mut driver as *mut DriverProperties as *mut c_void, properties: PropertiesBytes::zeroed() };
        // SAFETY: the chain holds one structure the device version allows.
        unsafe { (functions.get_physical_device_properties2)(self.handle, &mut properties) };
        let (name, info) = (text_of(&driver.driver_name), text_of(&driver.driver_info));
        if name.is_empty() { raw } else { format!("{name} {info}").trim().to_string() }
    }

    pub fn format_features(&self, instance: &Instance, format: u32) -> FormatProperties {
        let mut features = FormatProperties { linear_tiling_features: 0, optimal_tiling_features: 0, buffer_features: 0 };
        // SAFETY: the function fills one structure.
        unsafe { (instance.functions.get_physical_device_format_properties)(self.handle, format, &mut features) };
        features
    }

    pub fn core_features(&self, instance: &Instance) -> CoreFeatures {
        let mut features: CoreFeatures = [0; 55];
        // SAFETY: the array has the layout of `VkPhysicalDeviceFeatures`.
        unsafe { (instance.functions.get_physical_device_features)(self.handle, &mut features) };
        features
    }
}

fn device_extensions(instance: &Instance, handle: Handle) -> Result<Vec<String>, String> {
    // SAFETY: the driver fills at most `count` entries of the array.
    let extensions = enumerate("vkEnumerateDeviceExtensionProperties", empty_extension, |count, out| unsafe {
        (instance.functions.enumerate_device_extension_properties)(handle, ptr::null(), count, out)
    })?;
    Ok(extensions.iter().map(|extension| text_of(&extension.extension_name)).collect())
}

fn queue_families(instance: &Instance, handle: Handle) -> Vec<QueueFamilyProperties> {
    let get = |count: *mut u32, out: *mut QueueFamilyProperties| {
        // SAFETY: called first with no array to write the count, then with an
        // array of that many entries.
        unsafe { (instance.functions.get_physical_device_queue_family_properties)(handle, count, out) };
        SUCCESS
    };
    let empty = || QueueFamilyProperties { queue_flags: 0, queue_count: 0, timestamp_valid_bits: 0, min_image_transfer_granularity: [0; 3] };
    enumerate("vkGetPhysicalDeviceQueueFamilyProperties", empty, get).unwrap_or_default()
}

fn device_kind(device_type: u32) -> DeviceKind {
    match device_type {
        1 => DeviceKind::Integrated,
        2 => DeviceKind::Discrete,
        3 => DeviceKind::Virtual,
        4 => DeviceKind::Software,
        _ => DeviceKind::Other,
    }
}

/// An open device: its instance, its one queue, and what the module reads
/// from the physical device.
pub struct Gpu {
    pub functions: DeviceFunctions,
    pub device: Handle,
    pub queue: Handle,
    pub queue_family: u32,
    pub physical: PhysicalDevice,
    pub core_features: CoreFeatures,
    memory: PhysicalDeviceMemoryProperties,
    pub instance: Instance,
    /// Declared after the instance, so that the messenger that writes to it
    /// is destroyed first.
    pub sink: Option<Box<MessageSink>>,
}

impl Gpu {
    /// Creates an instance (under the validation layer when asked), then the
    /// device at `index` with one queue of a family that does graphics and
    /// compute, enabling the one feature the scene's shader needs where offered.
    pub fn open(library: &Library, index: usize, validation: bool) -> Result<Gpu, String> {
        let sink = validation.then(|| Box::new(MessageSink::default()));
        let instance = Instance::create(library, sink.as_deref())?;
        let handles = instance.physical_devices()?;
        let handle = *handles.get(index).ok_or_else(|| format!("no physical device at index {index} ({} listed)", handles.len()))?;
        let physical = PhysicalDevice::describe(&instance, handle)?;
        let queue_family = physical
            .families
            .iter()
            .position(|family| family.queue_flags & (QUEUE_GRAPHICS | QUEUE_COMPUTE) == QUEUE_GRAPHICS | QUEUE_COMPUTE && family.queue_count > 0)
            .ok_or_else(|| format!("{} has no queue family with graphics and compute", physical.name))? as u32;
        let core_features = physical.core_features(&instance);
        let mut enabled: CoreFeatures = [0; 55];
        enabled[feature::SHADER_SAMPLED_IMAGE_ARRAY_DYNAMIC_INDEXING] = core_features[feature::SHADER_SAMPLED_IMAGE_ARRAY_DYNAMIC_INDEXING];
        let priority = 1.0_f32;
        let queue_info = DeviceQueueCreateInfo { s_type: structure::DEVICE_QUEUE_CREATE_INFO, p_next: ptr::null(), flags: 0, queue_family_index: queue_family, queue_count: 1, queue_priorities: &priority };
        let device_info = DeviceCreateInfo {
            s_type: structure::DEVICE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            queue_create_info_count: 1,
            queue_create_infos: &queue_info,
            enabled_layer_count: 0,
            enabled_layer_names: ptr::null(),
            enabled_extension_count: 0,
            enabled_extension_names: ptr::null(),
            enabled_features: &enabled,
        };
        let mut device: Handle = ptr::null_mut();
        // SAFETY: every pointer of the structures lives until the call returns.
        check(unsafe { (instance.functions.create_device)(handle, &device_info, ptr::null(), &mut device) }, "vkCreateDevice")?;
        let functions = match DeviceFunctions::load(instance.functions.get_device_proc_addr, device) {
            Ok(functions) => functions,
            Err(reason) => {
                // SAFETY: `vkDestroyDevice` has this signature; the device was
                // just created and nothing uses it.
                unsafe {
                    if let Ok(destroy) = entry::<unsafe extern "system" fn(Handle, Allocator)>(instance.functions.get_device_proc_addr, device, "vkDestroyDevice") {
                        destroy(device, ptr::null());
                    }
                }
                return Err(reason);
            }
        };
        let mut queue: Handle = ptr::null_mut();
        let mut memory = PhysicalDeviceMemoryProperties {
            memory_type_count: 0,
            memory_types: [MemoryType { property_flags: 0, heap_index: 0 }; 32],
            memory_heap_count: 0,
            memory_heaps: [MemoryHeap { size: 0, flags: 0 }; 16],
        };
        // SAFETY: the queue was requested at device creation; the memory
        // structure has the layout the function fills.
        unsafe {
            (functions.get_device_queue)(device, queue_family, 0, &mut queue);
            (instance.functions.get_physical_device_memory_properties)(handle, &mut memory);
        }
        Ok(Gpu { functions, device, queue, queue_family, physical, core_features, memory, instance, sink })
    }

    pub fn timestamp_bits(&self) -> u32 {
        self.physical.families[self.queue_family as usize].timestamp_valid_bits
    }

    pub fn memory_type(&self, allowed: u32, wanted: u32) -> Option<u32> {
        (0..self.memory.memory_type_count.min(32)).find(|&index| allowed & (1 << index) != 0 && self.memory.memory_types[index as usize].property_flags & wanted == wanted)
    }

    pub fn host_visible_memory(&self) -> Option<(u32, bool)> {
        match self.memory_type(!0, MEMORY_HOST_VISIBLE | MEMORY_HOST_COHERENT) {
            Some(index) => Some((index, true)),
            None => self.memory_type(!0, MEMORY_HOST_VISIBLE).map(|index| (index, false)),
        }
    }

    /// Waits for a fence by polling its status without blocking, and returns
    /// how many times it was polled.
    pub fn poll_fence(&self, fence: Object) -> Result<u64, String> {
        let started = Instant::now();
        let mut polls = 0;
        loop {
            polls += 1;
            // SAFETY: the fence belongs to this device.
            match unsafe { (self.functions.get_fence_status)(self.device, fence) } {
                SUCCESS => return Ok(polls),
                NOT_READY if started.elapsed() < COMPLETION_TIMEOUT => std::thread::yield_now(),
                NOT_READY => return Err(format!("the fence was still unsignaled after {} seconds", COMPLETION_TIMEOUT.as_secs())),
                other => return check(other, "vkGetFenceStatus").map(|()| polls),
            }
        }
    }

    pub fn wait_fence(&self, fence: Object) -> Result<(), String> {
        // SAFETY: the fence belongs to this device.
        check(unsafe { (self.functions.wait_for_fences)(self.device, 1, &fence, 1, COMPLETION_TIMEOUT.as_nanos() as u64) }, "vkWaitForFences")
    }

    pub fn submit(&self, command_buffer: Handle, fence: Object) -> Result<(), String> {
        let submit = SubmitInfo {
            s_type: structure::SUBMIT_INFO,
            p_next: ptr::null(),
            wait_semaphore_count: 0,
            wait_semaphores: ptr::null(),
            wait_dst_stage_mask: ptr::null(),
            command_buffer_count: 1,
            command_buffers: &command_buffer,
            signal_semaphore_count: 0,
            signal_semaphores: ptr::null(),
        };
        // SAFETY: the command buffer has been ended and the fence is unsignaled.
        check(unsafe { (self.functions.queue_submit)(self.queue, 1, &submit, fence) }, "vkQueueSubmit")
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: every object created from the device was destroyed by its
        // scope; waiting first makes sure no work is pending.
        unsafe {
            (self.functions.device_wait_idle)(self.device);
            (self.functions.destroy_device)(self.device, ptr::null());
        }
    }
}

/// A command buffer handle that may move to the thread that records it: each
/// is allocated from a pool that only that thread uses.
#[derive(Clone, Copy)]
pub struct CommandBuffer(pub Handle);

// SAFETY: Vulkan lets a command buffer be recorded on any thread provided its
// pool is used by one thread at a time, which the module guarantees by giving
// each recording thread its own pool.
unsafe impl Send for CommandBuffer {}

/// A buffer in host-visible memory, mapped for the scope's lifetime.
pub struct HostBuffer {
    pub buffer: Object,
    memory: Object,
    mapped: *mut u8,
    coherent: bool,
    pub size: usize,
}

enum Owned {
    Buffer(Object),
    Memory(Object),
    Image(Object),
    ImageView(Object),
    Sampler(Object),
    ShaderModule(Object),
    DescriptorSetLayout(Object),
    DescriptorPool(Object),
    PipelineLayout(Object),
    Pipeline(Object),
    RenderPass(Object),
    Framebuffer(Object),
    CommandPool(Object),
    Fence(Object),
    QueryPool(Object),
}

/// Every object one exercise creates, destroyed in reverse order when the
/// scope ends, after the device has finished its work.
pub struct Scope<'g> {
    pub gpu: &'g Gpu,
    owned: Vec<Owned>,
}

/// One image to upload: its levels, largest first, tightly packed RGBA8.
pub struct ImageUpload<'a> {
    pub image: Object,
    pub width: u32,
    pub height: u32,
    pub levels: Vec<&'a [u8]>,
}

/// What a graphics pipeline of the module varies.
pub struct GraphicsPipelineDescription<'a> {
    pub vertex: Object,
    pub fragment: Object,
    /// Zero when the vertex shader reads no vertex buffer.
    pub vertex_stride: u32,
    pub attributes: &'a [VertexInputAttributeDescription],
    pub viewport: Viewport,
    pub samples: u32,
    /// Depth test less-or-equal, with or without depth write; none without depth.
    pub depth_write: Option<bool>,
    pub blends: &'a [PipelineColorBlendAttachmentState],
    pub layout: Object,
    pub render_pass: Object,
}

impl<'g> Scope<'g> {
    pub fn new(gpu: &'g Gpu) -> Scope<'g> {
        Scope { gpu, owned: Vec::new() }
    }

    fn device(&self) -> Handle {
        self.gpu.device
    }

    fn functions(&self) -> &DeviceFunctions {
        &self.gpu.functions
    }

    fn allocate(&mut self, requirements: &MemoryRequirements, memory_type: u32, what: &str) -> Result<Object, String> {
        let info = MemoryAllocateInfo { s_type: structure::MEMORY_ALLOCATE_INFO, p_next: ptr::null(), allocation_size: requirements.size, memory_type_index: memory_type };
        let mut memory: Object = 0;
        // SAFETY: the structure is valid for the call.
        check(unsafe { (self.functions().allocate_memory)(self.device(), &info, ptr::null(), &mut memory) }, &format!("vkAllocateMemory ({what})"))?;
        self.owned.push(Owned::Memory(memory));
        Ok(memory)
    }

    /// A buffer bound to host-visible memory, mapped.
    pub fn host_buffer(&mut self, size: usize, usage: u32) -> Result<HostBuffer, String> {
        let info = BufferCreateInfo {
            s_type: structure::BUFFER_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            size: size as u64,
            usage,
            sharing_mode: SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            queue_family_indices: ptr::null(),
        };
        let mut buffer: Object = 0;
        // SAFETY: the structure is valid for the call.
        check(unsafe { (self.functions().create_buffer)(self.device(), &info, ptr::null(), &mut buffer) }, "vkCreateBuffer")?;
        self.owned.push(Owned::Buffer(buffer));
        let mut requirements = MemoryRequirements { size: 0, alignment: 0, memory_type_bits: 0 };
        // SAFETY: the buffer belongs to this device.
        unsafe { (self.functions().get_buffer_memory_requirements)(self.device(), buffer, &mut requirements) };
        let (memory_type, coherent) = match self.gpu.memory_type(requirements.memory_type_bits, MEMORY_HOST_VISIBLE | MEMORY_HOST_COHERENT) {
            Some(index) => (index, true),
            None => (self.gpu.memory_type(requirements.memory_type_bits, MEMORY_HOST_VISIBLE).ok_or("no host-visible memory type for a buffer")?, false),
        };
        let memory = self.allocate(&requirements, memory_type, "buffer")?;
        let mut mapped: *mut c_void = ptr::null_mut();
        // SAFETY: the memory is host-visible, unbound until now and mapped once.
        unsafe {
            check((self.functions().bind_buffer_memory)(self.device(), buffer, memory, 0), "vkBindBufferMemory")?;
            check((self.functions().map_memory)(self.device(), memory, 0, WHOLE_SIZE, 0, &mut mapped), "vkMapMemory")?;
        }
        Ok(HostBuffer { buffer, memory, mapped: mapped as *mut u8, coherent, size })
    }

    /// A host-visible buffer holding `bytes`, visible to the device.
    pub fn buffer_with(&mut self, bytes: &[u8], usage: u32) -> Result<HostBuffer, String> {
        let buffer = self.host_buffer(bytes.len().max(4), usage)?;
        // SAFETY: the mapping covers at least `bytes.len()` bytes.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.mapped, bytes.len()) };
        if !buffer.coherent {
            let range = MappedMemoryRange { s_type: structure::MAPPED_MEMORY_RANGE, p_next: ptr::null(), memory: buffer.memory, offset: 0, size: WHOLE_SIZE };
            // SAFETY: the range covers the mapped memory.
            check(unsafe { (self.functions().flush_mapped_memory_ranges)(self.device(), 1, &range) }, "vkFlushMappedMemoryRanges")?;
        }
        Ok(buffer)
    }

    /// The bytes of a host-visible buffer the device wrote.
    pub fn read(&self, buffer: &HostBuffer) -> Result<Vec<u8>, String> {
        if !buffer.coherent {
            let range = MappedMemoryRange { s_type: structure::MAPPED_MEMORY_RANGE, p_next: ptr::null(), memory: buffer.memory, offset: 0, size: WHOLE_SIZE };
            // SAFETY: the range covers the mapped memory.
            check(unsafe { (self.functions().invalidate_mapped_memory_ranges)(self.device(), 1, &range) }, "vkInvalidateMappedMemoryRanges")?;
        }
        // SAFETY: the mapping covers `size` bytes and the device's writes are complete.
        Ok(unsafe { std::slice::from_raw_parts(buffer.mapped, buffer.size) }.to_vec())
    }

    /// An image in device memory (device-local where offered).
    pub fn image(&mut self, info: &ImageCreateInfo) -> Result<Object, String> {
        let mut image: Object = 0;
        // SAFETY: the structure is valid for the call.
        check(unsafe { (self.functions().create_image)(self.device(), info, ptr::null(), &mut image) }, "vkCreateImage")?;
        self.owned.push(Owned::Image(image));
        let mut requirements = MemoryRequirements { size: 0, alignment: 0, memory_type_bits: 0 };
        // SAFETY: the image belongs to this device.
        unsafe { (self.functions().get_image_memory_requirements)(self.device(), image, &mut requirements) };
        let memory_type = self
            .gpu
            .memory_type(requirements.memory_type_bits, MEMORY_DEVICE_LOCAL)
            .or_else(|| self.gpu.memory_type(requirements.memory_type_bits, 0))
            .ok_or("no memory type for an image")?;
        let memory = self.allocate(&requirements, memory_type, "image")?;
        // SAFETY: the memory was allocated for this image's requirements.
        check(unsafe { (self.functions().bind_image_memory)(self.device(), image, memory, 0) }, "vkBindImageMemory")?;
        Ok(image)
    }

    pub fn image_view(&mut self, image: Object, format: u32, aspect: u32, levels: u32) -> Result<Object, String> {
        let info = ImageViewCreateInfo {
            s_type: structure::IMAGE_VIEW_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            image,
            view_type: IMAGE_VIEW_TYPE_2D,
            format,
            components: [0; 4],
            subresource_range: range(aspect, levels),
        };
        let mut view: Object = 0;
        // SAFETY: the structure is valid for the call.
        check(unsafe { (self.functions().create_image_view)(self.device(), &info, ptr::null(), &mut view) }, "vkCreateImageView")?;
        self.owned.push(Owned::ImageView(view));
        Ok(view)
    }

    pub fn sampler(&mut self, info: &SamplerCreateInfo) -> Result<Object, String> {
        let mut sampler: Object = 0;
        // SAFETY: the structure is valid for the call.
        check(unsafe { (self.functions().create_sampler)(self.device(), info, ptr::null(), &mut sampler) }, "vkCreateSampler")?;
        self.owned.push(Owned::Sampler(sampler));
        Ok(sampler)
    }

    pub fn shader_module(&mut self, words: &[u32]) -> Result<Object, String> {
        let info = ShaderModuleCreateInfo { s_type: structure::SHADER_MODULE_CREATE_INFO, p_next: ptr::null(), flags: 0, code_size: words.len() * 4, code: words.as_ptr() };
        let mut module: Object = 0;
        // SAFETY: the code is a SPIR-V module of `code_size` bytes.
        check(unsafe { (self.functions().create_shader_module)(self.device(), &info, ptr::null(), &mut module) }, "vkCreateShaderModule")?;
        self.owned.push(Owned::ShaderModule(module));
        Ok(module)
    }

    /// A descriptor set layout of the given (type, count, stages) bindings,
    /// numbered from zero.
    pub fn descriptor_set_layout(&mut self, bindings: &[(u32, u32, u32)]) -> Result<Object, String> {
        let bindings: Vec<DescriptorSetLayoutBinding> = bindings
            .iter()
            .enumerate()
            .map(|(binding, &(descriptor_type, descriptor_count, stage_flags))| DescriptorSetLayoutBinding { binding: binding as u32, descriptor_type, descriptor_count, stage_flags, immutable_samplers: ptr::null() })
            .collect();
        let info = DescriptorSetLayoutCreateInfo { s_type: structure::DESCRIPTOR_SET_LAYOUT_CREATE_INFO, p_next: ptr::null(), flags: 0, binding_count: bindings.len() as u32, bindings: bindings.as_ptr() };
        let mut layout: Object = 0;
        // SAFETY: the bindings live until the call returns.
        check(unsafe { (self.functions().create_descriptor_set_layout)(self.device(), &info, ptr::null(), &mut layout) }, "vkCreateDescriptorSetLayout")?;
        self.owned.push(Owned::DescriptorSetLayout(layout));
        Ok(layout)
    }

    /// One descriptor set of `layout`, from a pool sized for it.
    pub fn descriptor_set(&mut self, layout: Object, bindings: &[(u32, u32, u32)]) -> Result<Object, String> {
        let sizes: Vec<DescriptorPoolSize> = bindings.iter().map(|&(descriptor_type, descriptor_count, _)| DescriptorPoolSize { descriptor_type, descriptor_count }).collect();
        let info = DescriptorPoolCreateInfo { s_type: structure::DESCRIPTOR_POOL_CREATE_INFO, p_next: ptr::null(), flags: 0, max_sets: 1, pool_size_count: sizes.len() as u32, pool_sizes: sizes.as_ptr() };
        let mut pool: Object = 0;
        // SAFETY: the sizes live until the call returns.
        check(unsafe { (self.functions().create_descriptor_pool)(self.device(), &info, ptr::null(), &mut pool) }, "vkCreateDescriptorPool")?;
        self.owned.push(Owned::DescriptorPool(pool));
        let allocate = DescriptorSetAllocateInfo { s_type: structure::DESCRIPTOR_SET_ALLOCATE_INFO, p_next: ptr::null(), descriptor_pool: pool, descriptor_set_count: 1, set_layouts: &layout };
        let mut set: Object = 0;
        // SAFETY: the pool has room for one set of this layout.
        check(unsafe { (self.functions().allocate_descriptor_sets)(self.device(), &allocate, &mut set) }, "vkAllocateDescriptorSets")?;
        Ok(set)
    }

    pub fn write_images(&self, set: Object, binding: u32, descriptor_type: u32, images: &[DescriptorImageInfo]) {
        let write = WriteDescriptorSet {
            s_type: structure::WRITE_DESCRIPTOR_SET,
            p_next: ptr::null(),
            dst_set: set,
            dst_binding: binding,
            dst_array_element: 0,
            descriptor_count: images.len() as u32,
            descriptor_type,
            image_info: images.as_ptr(),
            buffer_info: ptr::null(),
            texel_buffer_view: ptr::null(),
        };
        // SAFETY: the image descriptions live until the call returns.
        unsafe { (self.functions().update_descriptor_sets)(self.device(), 1, &write, 0, ptr::null()) };
    }

    pub fn write_buffer(&self, set: Object, binding: u32, descriptor_type: u32, buffer: &HostBuffer) {
        let info = DescriptorBufferInfo { buffer: buffer.buffer, offset: 0, range: WHOLE_SIZE };
        let write = WriteDescriptorSet {
            s_type: structure::WRITE_DESCRIPTOR_SET,
            p_next: ptr::null(),
            dst_set: set,
            dst_binding: binding,
            dst_array_element: 0,
            descriptor_count: 1,
            descriptor_type,
            image_info: ptr::null(),
            buffer_info: &info,
            texel_buffer_view: ptr::null(),
        };
        // SAFETY: the buffer description lives until the call returns.
        unsafe { (self.functions().update_descriptor_sets)(self.device(), 1, &write, 0, ptr::null()) };
    }

    pub fn pipeline_layout(&mut self, set_layouts: &[Object], push_constants: &[PushConstantRange]) -> Result<Object, String> {
        let info = PipelineLayoutCreateInfo {
            s_type: structure::PIPELINE_LAYOUT_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            set_layout_count: set_layouts.len() as u32,
            set_layouts: set_layouts.as_ptr(),
            push_constant_range_count: push_constants.len() as u32,
            push_constant_ranges: push_constants.as_ptr(),
        };
        let mut layout: Object = 0;
        // SAFETY: the arrays live until the call returns.
        check(unsafe { (self.functions().create_pipeline_layout)(self.device(), &info, ptr::null(), &mut layout) }, "vkCreatePipelineLayout")?;
        self.owned.push(Owned::PipelineLayout(layout));
        Ok(layout)
    }

    /// A render pass of one subpass writing `colors` (with `resolves`, one per
    /// color, where given) and the optional depth attachment.
    pub fn render_pass(&mut self, attachments: &[AttachmentDescription], colors: &[u32], resolves: &[u32], depth: Option<u32>, dependencies: &[SubpassDependency]) -> Result<Object, String> {
        let color_references: Vec<AttachmentReference> = colors.iter().map(|&attachment| AttachmentReference { attachment, layout: LAYOUT_COLOR_ATTACHMENT_OPTIMAL }).collect();
        let resolve_references: Vec<AttachmentReference> = resolves.iter().map(|&attachment| AttachmentReference { attachment, layout: LAYOUT_COLOR_ATTACHMENT_OPTIMAL }).collect();
        let depth_reference = depth.map(|attachment| AttachmentReference { attachment, layout: LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL });
        let subpass = SubpassDescription {
            flags: 0,
            pipeline_bind_point: BIND_POINT_GRAPHICS,
            input_attachment_count: 0,
            input_attachments: ptr::null(),
            color_attachment_count: color_references.len() as u32,
            color_attachments: color_references.as_ptr(),
            resolve_attachments: if resolve_references.is_empty() { ptr::null() } else { resolve_references.as_ptr() },
            depth_stencil_attachment: depth_reference.as_ref().map_or(ptr::null(), |reference| reference as *const AttachmentReference),
            preserve_attachment_count: 0,
            preserve_attachments: ptr::null(),
        };
        let info = RenderPassCreateInfo {
            s_type: structure::RENDER_PASS_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            attachment_count: attachments.len() as u32,
            attachments: attachments.as_ptr(),
            subpass_count: 1,
            subpasses: &subpass,
            dependency_count: dependencies.len() as u32,
            dependencies: dependencies.as_ptr(),
        };
        let mut render_pass: Object = 0;
        // SAFETY: every array lives until the call returns.
        check(unsafe { (self.functions().create_render_pass)(self.device(), &info, ptr::null(), &mut render_pass) }, "vkCreateRenderPass")?;
        self.owned.push(Owned::RenderPass(render_pass));
        Ok(render_pass)
    }

    pub fn framebuffer(&mut self, render_pass: Object, views: &[Object], width: u32, height: u32) -> Result<Object, String> {
        let info = FramebufferCreateInfo {
            s_type: structure::FRAMEBUFFER_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            render_pass,
            attachment_count: views.len() as u32,
            attachments: views.as_ptr(),
            width,
            height,
            layers: 1,
        };
        let mut framebuffer: Object = 0;
        // SAFETY: the views live until the call returns.
        check(unsafe { (self.functions().create_framebuffer)(self.device(), &info, ptr::null(), &mut framebuffer) }, "vkCreateFramebuffer")?;
        self.owned.push(Owned::Framebuffer(framebuffer));
        Ok(framebuffer)
    }

    pub fn graphics_pipeline(&mut self, description: &GraphicsPipelineDescription) -> Result<Object, String> {
        let entry_name = c_string("main");
        let stage = |stage: u32, module: Object| PipelineShaderStageCreateInfo {
            s_type: structure::PIPELINE_SHADER_STAGE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            stage,
            module,
            name: entry_name.as_ptr(),
            specialization_info: ptr::null(),
        };
        let stages = [stage(STAGE_VERTEX, description.vertex), stage(STAGE_FRAGMENT, description.fragment)];
        let binding = VertexInputBindingDescription { binding: 0, stride: description.vertex_stride, input_rate: INPUT_RATE_VERTEX };
        let has_vertices = description.vertex_stride > 0;
        let vertex_input = PipelineVertexInputStateCreateInfo {
            s_type: structure::PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            vertex_binding_description_count: u32::from(has_vertices),
            vertex_binding_descriptions: &binding,
            vertex_attribute_description_count: description.attributes.len() as u32,
            vertex_attribute_descriptions: description.attributes.as_ptr(),
        };
        let input_assembly = PipelineInputAssemblyStateCreateInfo { s_type: structure::PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO, p_next: ptr::null(), flags: 0, topology: TOPOLOGY_TRIANGLE_LIST, primitive_restart_enable: 0 };
        let viewport = description.viewport;
        let scissor = Rect2D { offset: [viewport.x as i32, viewport.y as i32], extent: [viewport.width as u32, viewport.height as u32] };
        let viewport_state = PipelineViewportStateCreateInfo { s_type: structure::PIPELINE_VIEWPORT_STATE_CREATE_INFO, p_next: ptr::null(), flags: 0, viewport_count: 1, viewports: &viewport, scissor_count: 1, scissors: &scissor };
        let rasterization = PipelineRasterizationStateCreateInfo {
            s_type: structure::PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
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
            s_type: structure::PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            rasterization_samples: description.samples,
            sample_shading_enable: 0,
            min_sample_shading: 0.0,
            sample_mask: ptr::null(),
            alpha_to_coverage_enable: 0,
            alpha_to_one_enable: 0,
        };
        let keep = StencilOpState { fail_op: 0, pass_op: 0, depth_fail_op: 0, compare_op: COMPARE_OP_ALWAYS, compare_mask: 0, write_mask: 0, reference: 0 };
        let depth_stencil = PipelineDepthStencilStateCreateInfo {
            s_type: structure::PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            depth_test_enable: 1,
            depth_write_enable: u32::from(description.depth_write == Some(true)),
            depth_compare_op: COMPARE_OP_LESS_OR_EQUAL,
            depth_bounds_test_enable: 0,
            stencil_test_enable: 0,
            front: keep,
            back: keep,
            min_depth_bounds: 0.0,
            max_depth_bounds: 1.0,
        };
        let color_blend = PipelineColorBlendStateCreateInfo {
            s_type: structure::PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            logic_op_enable: 0,
            logic_op: 0,
            attachment_count: description.blends.len() as u32,
            attachments: description.blends.as_ptr(),
            blend_constants: [0.0; 4],
        };
        let info = GraphicsPipelineCreateInfo {
            s_type: structure::GRAPHICS_PIPELINE_CREATE_INFO,
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
            depth_stencil_state: if description.depth_write.is_some() { &depth_stencil } else { ptr::null() },
            color_blend_state: &color_blend,
            dynamic_state: ptr::null(),
            layout: description.layout,
            render_pass: description.render_pass,
            subpass: 0,
            base_pipeline_handle: 0,
            base_pipeline_index: -1,
        };
        let mut pipeline: Object = 0;
        // SAFETY: every structure the description points to lives until the call returns.
        check(unsafe { (self.functions().create_graphics_pipelines)(self.device(), 0, 1, &info, ptr::null(), &mut pipeline) }, "vkCreateGraphicsPipelines")?;
        self.owned.push(Owned::Pipeline(pipeline));
        Ok(pipeline)
    }

    pub fn compute_pipeline(&mut self, module: Object, layout: Object) -> Result<Object, String> {
        let entry_name = c_string("main");
        let info = ComputePipelineCreateInfo {
            s_type: structure::COMPUTE_PIPELINE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            stage: PipelineShaderStageCreateInfo {
                s_type: structure::PIPELINE_SHADER_STAGE_CREATE_INFO,
                p_next: ptr::null(),
                flags: 0,
                stage: STAGE_COMPUTE,
                module,
                name: entry_name.as_ptr(),
                specialization_info: ptr::null(),
            },
            layout,
            base_pipeline_handle: 0,
            base_pipeline_index: -1,
        };
        let mut pipeline: Object = 0;
        // SAFETY: the structure and the entry name live until the call returns.
        check(unsafe { (self.functions().create_compute_pipelines)(self.device(), 0, 1, &info, ptr::null(), &mut pipeline) }, "vkCreateComputePipelines")?;
        self.owned.push(Owned::Pipeline(pipeline));
        Ok(pipeline)
    }

    pub fn command_pool(&mut self) -> Result<Object, String> {
        let info = CommandPoolCreateInfo { s_type: structure::COMMAND_POOL_CREATE_INFO, p_next: ptr::null(), flags: 0, queue_family_index: self.gpu.queue_family };
        let mut pool: Object = 0;
        // SAFETY: the structure is valid for the call.
        check(unsafe { (self.functions().create_command_pool)(self.device(), &info, ptr::null(), &mut pool) }, "vkCreateCommandPool")?;
        self.owned.push(Owned::CommandPool(pool));
        Ok(pool)
    }

    /// One command buffer of `level`, freed with its pool.
    pub fn command_buffer(&self, pool: Object, level: u32) -> Result<CommandBuffer, String> {
        let info = CommandBufferAllocateInfo { s_type: structure::COMMAND_BUFFER_ALLOCATE_INFO, p_next: ptr::null(), command_pool: pool, level, command_buffer_count: 1 };
        let mut command_buffer: Handle = ptr::null_mut();
        // SAFETY: the pool belongs to this device and is used by this thread.
        check(unsafe { (self.functions().allocate_command_buffers)(self.device(), &info, &mut command_buffer) }, "vkAllocateCommandBuffers")?;
        Ok(CommandBuffer(command_buffer))
    }

    pub fn fence(&mut self) -> Result<Object, String> {
        let info = FenceCreateInfo { s_type: structure::FENCE_CREATE_INFO, p_next: ptr::null(), flags: 0 };
        let mut fence: Object = 0;
        // SAFETY: the structure is valid for the call.
        check(unsafe { (self.functions().create_fence)(self.device(), &info, ptr::null(), &mut fence) }, "vkCreateFence")?;
        self.owned.push(Owned::Fence(fence));
        Ok(fence)
    }

    pub fn timestamp_pool(&mut self, count: u32) -> Result<Object, String> {
        let info = QueryPoolCreateInfo { s_type: structure::QUERY_POOL_CREATE_INFO, p_next: ptr::null(), flags: 0, query_type: QUERY_TYPE_TIMESTAMP, query_count: count, pipeline_statistics: 0 };
        let mut pool: Object = 0;
        // SAFETY: the structure is valid for the call.
        check(unsafe { (self.functions().create_query_pool)(self.device(), &info, ptr::null(), &mut pool) }, "vkCreateQueryPool")?;
        self.owned.push(Owned::QueryPool(pool));
        Ok(pool)
    }

    /// Records commands into a new primary command buffer, submits it and
    /// waits for the device to finish.
    pub fn run_once(&mut self, record: impl FnOnce(&DeviceFunctions, Handle)) -> Result<(), String> {
        let pool = self.command_pool()?;
        let command_buffer = self.command_buffer(pool, COMMAND_BUFFER_LEVEL_PRIMARY)?.0;
        let fence = self.fence()?;
        begin(self.functions(), command_buffer, COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT, None)?;
        record(self.functions(), command_buffer);
        end(self.functions(), command_buffer)?;
        self.gpu.submit(command_buffer, fence)?;
        self.gpu.wait_fence(fence)
    }

    /// Uploads every level of each image through one staging buffer and
    /// leaves the images ready for sampling.
    pub fn upload_images(&mut self, uploads: &[ImageUpload]) -> Result<(), String> {
        let total: usize = uploads.iter().flat_map(|upload| upload.levels.iter()).map(|level| level.len()).sum();
        let mut bytes = Vec::with_capacity(total);
        let mut copies: Vec<Vec<BufferImageCopy>> = Vec::new();
        for upload in uploads {
            let mut regions = Vec::new();
            for (level, texels) in upload.levels.iter().enumerate() {
                regions.push(BufferImageCopy {
                    buffer_offset: bytes.len() as u64,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: [ASPECT_COLOR, level as u32, 0, 1],
                    image_offset: [0, 0, 0],
                    image_extent: Extent3D { width: (upload.width >> level).max(1), height: (upload.height >> level).max(1), depth: 1 },
                });
                bytes.extend_from_slice(texels);
            }
            copies.push(regions);
        }
        let staging = self.buffer_with(&bytes, BUFFER_USAGE_TRANSFER_SRC)?;
        self.run_once(|functions, command_buffer| {
            for (upload, regions) in uploads.iter().zip(&copies) {
                let levels = upload.levels.len() as u32;
                image_barrier(functions, command_buffer, upload.image, ASPECT_COLOR, levels, (LAYOUT_UNDEFINED, LAYOUT_TRANSFER_DST_OPTIMAL), (ACCESS_NONE, ACCESS_TRANSFER_WRITE), (PIPELINE_STAGE_TOP_OF_PIPE, PIPELINE_STAGE_TRANSFER));
                // SAFETY: the regions lie inside the staging buffer and the image.
                unsafe { (functions.cmd_copy_buffer_to_image)(command_buffer, staging.buffer, upload.image, LAYOUT_TRANSFER_DST_OPTIMAL, regions.len() as u32, regions.as_ptr()) };
                image_barrier(
                    functions,
                    command_buffer,
                    upload.image,
                    ASPECT_COLOR,
                    levels,
                    (LAYOUT_TRANSFER_DST_OPTIMAL, LAYOUT_SHADER_READ_ONLY_OPTIMAL),
                    (ACCESS_TRANSFER_WRITE, ACCESS_SHADER_READ),
                    (PIPELINE_STAGE_TRANSFER, PIPELINE_STAGE_FRAGMENT_SHADER),
                );
            }
        })
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        let (functions, device) = (&self.gpu.functions, self.gpu.device);
        // SAFETY: every object below was created from this device by this
        // scope and is destroyed once, after the device has finished with it;
        // memory is freed last, once nothing bound to it remains.
        unsafe {
            (functions.device_wait_idle)(device);
            for owned in self.owned.iter().rev() {
                match *owned {
                    Owned::Buffer(object) => (functions.destroy_buffer)(device, object, ptr::null()),
                    Owned::Image(object) => (functions.destroy_image)(device, object, ptr::null()),
                    Owned::ImageView(object) => (functions.destroy_image_view)(device, object, ptr::null()),
                    Owned::Sampler(object) => (functions.destroy_sampler)(device, object, ptr::null()),
                    Owned::ShaderModule(object) => (functions.destroy_shader_module)(device, object, ptr::null()),
                    Owned::DescriptorSetLayout(object) => (functions.destroy_descriptor_set_layout)(device, object, ptr::null()),
                    Owned::DescriptorPool(object) => (functions.destroy_descriptor_pool)(device, object, ptr::null()),
                    Owned::PipelineLayout(object) => (functions.destroy_pipeline_layout)(device, object, ptr::null()),
                    Owned::Pipeline(object) => (functions.destroy_pipeline)(device, object, ptr::null()),
                    Owned::RenderPass(object) => (functions.destroy_render_pass)(device, object, ptr::null()),
                    Owned::Framebuffer(object) => (functions.destroy_framebuffer)(device, object, ptr::null()),
                    Owned::CommandPool(object) => (functions.destroy_command_pool)(device, object, ptr::null()),
                    Owned::Fence(object) => (functions.destroy_fence)(device, object, ptr::null()),
                    Owned::QueryPool(object) => (functions.destroy_query_pool)(device, object, ptr::null()),
                    Owned::Memory(_) => {}
                }
            }
            for owned in &self.owned {
                if let Owned::Memory(memory) = *owned {
                    (functions.free_memory)(device, memory, ptr::null());
                }
            }
        }
    }
}

// --- small builders shared by the exercises -----------------------------------

pub fn range(aspect: u32, levels: u32) -> SubresourceRange {
    SubresourceRange { aspect_mask: aspect, base_mip_level: 0, level_count: levels, base_array_layer: 0, layer_count: 1 }
}

/// A 2D image of one layer with optimal tiling.
pub fn image_info(format: u32, width: u32, height: u32, levels: u32, samples: u32, usage: u32, flags: u32) -> ImageCreateInfo {
    ImageCreateInfo {
        s_type: structure::IMAGE_CREATE_INFO,
        p_next: ptr::null(),
        flags,
        image_type: IMAGE_TYPE_2D,
        format,
        extent: Extent3D { width, height, depth: 1 },
        mip_levels: levels,
        array_layers: 1,
        samples,
        tiling: IMAGE_TILING_OPTIMAL,
        usage,
        sharing_mode: SHARING_MODE_EXCLUSIVE,
        queue_family_index_count: 0,
        queue_family_indices: ptr::null(),
        initial_layout: LAYOUT_UNDEFINED,
    }
}

/// An attachment with no stencil; `layouts` are the initial and final ones.
pub fn attachment(format: u32, samples: u32, load_op: u32, store_op: u32, layouts: (u32, u32)) -> AttachmentDescription {
    AttachmentDescription {
        flags: 0,
        format,
        samples,
        load_op,
        store_op,
        stencil_load_op: LOAD_OP_DONT_CARE,
        stencil_store_op: STORE_OP_DONT_CARE,
        initial_layout: layouts.0,
        final_layout: layouts.1,
    }
}

/// What follows the pass waits for its color writes: `stage` and `access`
/// are what reads them next.
pub fn after_color_writes(stage: u32, access: u32) -> SubpassDependency {
    SubpassDependency {
        src_subpass: 0,
        dst_subpass: SUBPASS_EXTERNAL,
        src_stage_mask: PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT,
        dst_stage_mask: stage,
        src_access_mask: ACCESS_COLOR_ATTACHMENT_WRITE,
        dst_access_mask: access,
        dependency_flags: 0,
    }
}

pub fn opaque_blend() -> PipelineColorBlendAttachmentState {
    PipelineColorBlendAttachmentState {
        blend_enable: 0,
        src_color_blend_factor: BLEND_FACTOR_ONE,
        dst_color_blend_factor: 0,
        color_blend_op: BLEND_OP_ADD,
        src_alpha_blend_factor: BLEND_FACTOR_ONE,
        dst_alpha_blend_factor: 0,
        alpha_blend_op: BLEND_OP_ADD,
        color_write_mask: COLOR_COMPONENTS_RGBA,
    }
}

pub fn full_viewport(width: u32, height: u32) -> Viewport {
    Viewport { x: 0.0, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 }
}

pub fn begin(functions: &DeviceFunctions, command_buffer: Handle, flags: u32, inheritance: Option<&CommandBufferInheritanceInfo>) -> Result<(), String> {
    let info = CommandBufferBeginInfo {
        s_type: structure::COMMAND_BUFFER_BEGIN_INFO,
        p_next: ptr::null(),
        flags,
        inheritance_info: inheritance.map_or(ptr::null(), |inheritance| inheritance as *const CommandBufferInheritanceInfo),
    };
    // SAFETY: the command buffer is in the initial state and its pool is used by this thread.
    check(unsafe { (functions.begin_command_buffer)(command_buffer, &info) }, "vkBeginCommandBuffer")
}

pub fn end(functions: &DeviceFunctions, command_buffer: Handle) -> Result<(), String> {
    // SAFETY: the command buffer is recording.
    check(unsafe { (functions.end_command_buffer)(command_buffer) }, "vkEndCommandBuffer")
}

/// One image barrier: `layouts`, `accesses` and `stages` are each a pair of
/// before and after.
pub fn image_barrier(functions: &DeviceFunctions, command_buffer: Handle, image: Object, aspect: u32, levels: u32, layouts: (u32, u32), accesses: (u32, u32), stages: (u32, u32)) {
    let barrier = ImageMemoryBarrier {
        s_type: structure::IMAGE_MEMORY_BARRIER,
        p_next: ptr::null(),
        src_access_mask: accesses.0,
        dst_access_mask: accesses.1,
        old_layout: layouts.0,
        new_layout: layouts.1,
        src_queue_family_index: QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: QUEUE_FAMILY_IGNORED,
        image,
        subresource_range: range(aspect, levels),
    };
    // SAFETY: the barrier lives until the call returns; the command buffer is recording.
    unsafe { (functions.cmd_pipeline_barrier)(command_buffer, stages.0, stages.1, 0, 0, ptr::null(), 0, ptr::null(), 1, &barrier) };
}

/// Makes the device's writes to a buffer, made by `stage`, visible to the host.
pub fn host_read_barrier(functions: &DeviceFunctions, command_buffer: Handle, buffer: Object, stage: u32, access: u32) {
    let barrier = BufferMemoryBarrier {
        s_type: structure::BUFFER_MEMORY_BARRIER,
        p_next: ptr::null(),
        src_access_mask: access,
        dst_access_mask: ACCESS_HOST_READ,
        src_queue_family_index: QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: QUEUE_FAMILY_IGNORED,
        buffer,
        offset: 0,
        size: WHOLE_SIZE,
    };
    // SAFETY: the barrier lives until the call returns; the command buffer is recording.
    unsafe { (functions.cmd_pipeline_barrier)(command_buffer, stage, PIPELINE_STAGE_HOST, 0, 0, ptr::null(), 1, &barrier, 0, ptr::null()) };
}

/// Copies one color image, in the transfer source layout, to a buffer of
/// tight rows, and makes the copy visible to the host.
pub fn copy_to_host(functions: &DeviceFunctions, command_buffer: Handle, image: Object, width: u32, height: u32, buffer: Object) {
    let copy = BufferImageCopy {
        buffer_offset: 0,
        buffer_row_length: 0,
        buffer_image_height: 0,
        image_subresource: [ASPECT_COLOR, 0, 0, 1],
        image_offset: [0, 0, 0],
        image_extent: Extent3D { width, height, depth: 1 },
    };
    // SAFETY: the region lies inside the image and the buffer; the command buffer is recording.
    unsafe { (functions.cmd_copy_image_to_buffer)(command_buffer, image, LAYOUT_TRANSFER_SRC_OPTIMAL, buffer, 1, &copy) };
    host_read_barrier(functions, command_buffer, buffer, PIPELINE_STAGE_TRANSFER, ACCESS_TRANSFER_WRITE);
}

/// Begins a render pass over the whole framebuffer.
pub fn begin_render_pass(functions: &DeviceFunctions, command_buffer: Handle, render_pass: Object, framebuffer: Object, extent: [u32; 2], clears: &[ClearValue], contents: u32) {
    let info = RenderPassBeginInfo {
        s_type: structure::RENDER_PASS_BEGIN_INFO,
        p_next: ptr::null(),
        render_pass,
        framebuffer,
        render_area: Rect2D { offset: [0, 0], extent },
        clear_value_count: clears.len() as u32,
        clear_values: clears.as_ptr(),
    };
    // SAFETY: the clear values live until the call returns; the command buffer is recording.
    unsafe { (functions.cmd_begin_render_pass)(command_buffer, &info, contents) };
}
