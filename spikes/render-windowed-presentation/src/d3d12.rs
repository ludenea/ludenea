//! Presentation to a window through Direct3D 12 and DXGI, natively:
//! `d3d12.dll` and `dxgi.dll` opened at run time, the COM interfaces called
//! through their virtual tables by slot index, the structures declared here
//! in the field order of `d3d12.h` and `dxgi1_5.h`. A flip-model swapchain is
//! created for the window on the direct queue; a frame clears the back buffer
//! to its color and presents it. Throwaway, never product code.

use std::ffi::c_void;
use std::ptr;

use crate::clock::now_ns;
use crate::host::NativeWindow;
use crate::json::Json;
use crate::loader::Library;
use crate::presenter::{DeviceInfo, FrameOutcome, Mode, Presenter, Request, frame_color};

type Object = *mut c_void;
type Hresult = i32;

/// Calls slot `index` of `object`'s virtual table with the given signature.
macro_rules! com {
    ($object:expr, $index:expr, fn($($arg_type:ty),*) -> $ret:ty $(, $arg:expr)*) => {{
        let object: Object = $object;
        let vtable = *(object as *const *const *const c_void);
        let function: unsafe extern "system" fn(Object $(, $arg_type)*) -> $ret = std::mem::transmute(*vtable.add($index));
        function(object $(, $arg)*)
    }};
}

/// A COM object this module owns one reference to, released when dropped.
struct Com(Object);

impl Com {
    fn raw(&self) -> Object {
        self.0
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the object is live and this value owns one reference;
            // slot 2 is IUnknown::Release.
            unsafe { com!(self.0, 2, fn() -> u32) };
        }
    }
}

fn check(result: Hresult, what: &str) -> Result<(), String> {
    if result >= 0 { Ok(()) } else { Err(format!("{what} failed with HRESULT {:#010x}", result as u32)) }
}

fn created(result: Hresult, object: Object, what: &str) -> Result<Com, String> {
    let owned = Com(object);
    check(result, what)?;
    if owned.raw().is_null() {
        return Err(format!("{what} returned no object"));
    }
    Ok(owned)
}

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Guid {
    Guid { data1, data2, data3, data4 }
}

const IID_FACTORY2: Guid = guid(0x50c8_3a1c, 0xe072, 0x4c48, [0x87, 0xb0, 0x36, 0x30, 0xfa, 0x36, 0xa6, 0xd0]);
const IID_FACTORY5: Guid = guid(0x7632_e1f5, 0xee65, 0x4dca, [0x87, 0xfd, 0x84, 0xcd, 0x75, 0xf8, 0x83, 0x8d]);
const IID_SWAPCHAIN3: Guid = guid(0x94d9_9bdb, 0xf1f8, 0x4ab0, [0xb2, 0x36, 0x7d, 0xa0, 0x17, 0x0e, 0xda, 0xb1]);
const IID_DEVICE: Guid = guid(0x1898_19f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
const IID_COMMAND_QUEUE: Guid = guid(0x0ec8_70a6, 0x5d7e, 0x4c22, [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed]);
const IID_COMMAND_ALLOCATOR: Guid = guid(0x6102_dee4, 0xaf59, 0x4b09, [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24]);
const IID_GRAPHICS_COMMAND_LIST: Guid = guid(0x5b16_0d0f, 0xac1b, 0x4185, [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55]);
const IID_FENCE: Guid = guid(0x0a75_3dcf, 0xc4d8, 0x4b91, [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76]);
const IID_RESOURCE: Guid = guid(0x6964_42be, 0xa72e, 0x4059, [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad]);
const IID_DESCRIPTOR_HEAP: Guid = guid(0x8efb_471d, 0x616c, 0x4f49, [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51]);

type CreateFactory2 = unsafe extern "system" fn(u32, *const Guid, *mut Object) -> Hresult;
type CreateDevice = unsafe extern "system" fn(Object, u32, *const Guid, *mut Object) -> Hresult;

/// Virtual table slots, counted from `IUnknown` through each interface's
/// parents in the order of the headers.
mod slot {
    pub const QUERY_INTERFACE: usize = 0;
    pub mod factory {
        pub const MAKE_WINDOW_ASSOCIATION: usize = 8;
        pub const ENUM_ADAPTERS1: usize = 12;
        pub const CREATE_SWAP_CHAIN_FOR_HWND: usize = 15;
        pub const CHECK_FEATURE_SUPPORT: usize = 28;
    }
    pub mod adapter {
        pub const GET_DESC1: usize = 10;
    }
    pub mod swapchain {
        pub const PRESENT: usize = 8;
        pub const GET_BUFFER: usize = 9;
        pub const RESIZE_BUFFERS: usize = 13;
        pub const GET_FRAME_STATISTICS: usize = 16;
        pub const GET_CURRENT_BACK_BUFFER_INDEX: usize = 36;
    }
    pub mod device {
        pub const CREATE_COMMAND_QUEUE: usize = 8;
        pub const CREATE_COMMAND_ALLOCATOR: usize = 9;
        pub const CREATE_COMMAND_LIST: usize = 12;
        pub const CREATE_DESCRIPTOR_HEAP: usize = 14;
        pub const GET_DESCRIPTOR_HANDLE_INCREMENT_SIZE: usize = 15;
        pub const CREATE_RENDER_TARGET_VIEW: usize = 20;
        pub const CREATE_COMMITTED_RESOURCE: usize = 27;
        pub const CREATE_FENCE: usize = 36;
        pub const GET_DEVICE_REMOVED_REASON: usize = 37;
    }
    pub mod queue {
        pub const EXECUTE_COMMAND_LISTS: usize = 10;
        pub const SIGNAL: usize = 14;
    }
    pub mod allocator {
        pub const RESET: usize = 8;
    }
    pub mod fence {
        pub const GET_COMPLETED_VALUE: usize = 8;
        pub const SET_EVENT_ON_COMPLETION: usize = 9;
    }
    pub mod descriptor_heap {
        pub const GET_CPU_DESCRIPTOR_HANDLE_FOR_HEAP_START: usize = 9;
    }
    pub mod list {
        pub const CLOSE: usize = 9;
        pub const RESET: usize = 10;
        pub const RESOURCE_BARRIER: usize = 26;
        pub const CLEAR_RENDER_TARGET_VIEW: usize = 48;
    }
}

#[repr(C)]
struct AdapterDesc1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_system_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: [u32; 2],
    flags: u32,
}

#[repr(C)]
struct SwapChainDesc1 {
    width: u32,
    height: u32,
    format: u32,
    stereo: i32,
    sample_count: u32,
    sample_quality: u32,
    buffer_usage: u32,
    buffer_count: u32,
    scaling: u32,
    swap_effect: u32,
    alpha_mode: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FrameStatistics {
    present_count: u32,
    present_refresh_count: u32,
    sync_refresh_count: u32,
    sync_qpc_time: i64,
    sync_gpu_time: i64,
}

#[repr(C)]
struct CommandQueueDesc {
    kind: u32,
    priority: i32,
    flags: u32,
    node_mask: u32,
}

#[repr(C)]
struct DescriptorHeapDesc {
    kind: u32,
    count: u32,
    flags: u32,
    node_mask: u32,
}

#[repr(C)]
struct HeapProperties {
    kind: u32,
    cpu_page_property: u32,
    memory_pool_preference: u32,
    creation_node_mask: u32,
    visible_node_mask: u32,
}

#[repr(C)]
struct ResourceDesc {
    dimension: u32,
    alignment: u64,
    width: u64,
    height: u32,
    depth_or_array_size: u16,
    mip_levels: u16,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    layout: u32,
    flags: u32,
}

#[repr(C)]
struct ClearValue {
    format: u32,
    color: [f32; 4],
}

#[repr(C)]
struct TransitionBarrier {
    kind: u32,
    flags: u32,
    resource: Object,
    subresource: u32,
    state_before: u32,
    state_after: u32,
}

const DXGI_ERROR_NOT_FOUND: Hresult = 0x887a_0002_u32 as i32;
const DXGI_STATUS_OCCLUDED: Hresult = 0x087a_0001;
const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;
const BASIC_RENDER_DRIVER: (u32, u32) = (0x1414, 0x8c);
const DXGI_FEATURE_PRESENT_ALLOW_TEARING: u32 = 0;
const DXGI_MWA_NO_ALT_ENTER: u32 = 2;
const DXGI_USAGE_RENDER_TARGET_OUTPUT: u32 = 0x20;
const DXGI_SWAP_EFFECT_FLIP_DISCARD: u32 = 4;
const DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING: u32 = 2048;
const DXGI_PRESENT_ALLOW_TEARING: u32 = 0x200;
const FORMAT_R8G8B8A8_UNORM: u32 = 28;
const FEATURE_LEVEL_11_0: u32 = 0xb000;
const COMMAND_LIST_TYPE_DIRECT: u32 = 0;
const DESCRIPTOR_HEAP_TYPE_RTV: u32 = 2;
const HEAP_TYPE_DEFAULT: u32 = 1;
const RESOURCE_DIMENSION_TEXTURE2D: u32 = 3;
const RESOURCE_FLAG_ALLOW_RENDER_TARGET: u32 = 1;
const STATE_PRESENT: u32 = 0;
const STATE_RENDER_TARGET: u32 = 4;
const ALL_SUBRESOURCES: u32 = 0xffff_ffff;
const BUFFER_COUNT: u32 = 3;
const OFFSCREEN_SIZE: u32 = 256;
/// How many presentation statistics a run keeps at most.
const STATISTICS_KEPT: usize = 12_000;

struct Libraries {
    create_factory: CreateFactory2,
    create_device: CreateDevice,
    // Kept open as long as any object they created lives.
    _dxgi: Library,
    _d3d12: Library,
}

fn libraries() -> Result<Libraries, String> {
    let d3d12 = Library::open(&["d3d12.dll"])?;
    let dxgi = Library::open(&["dxgi.dll"])?;
    // SAFETY: the libraries export these functions with these signatures.
    let (create_factory, create_device) = unsafe { (dxgi.function::<CreateFactory2>("CreateDXGIFactory2")?, d3d12.function::<CreateDevice>("D3D12CreateDevice")?) };
    Ok(Libraries { create_factory, create_device, _dxgi: dxgi, _d3d12: d3d12 })
}

/// The factory, at the version that answers the tearing question when the
/// system has it, and whether it has it.
fn factory(libraries: &Libraries) -> Result<(Com, bool), String> {
    let mut object: Object = ptr::null_mut();
    // SAFETY: the function is CreateDXGIFactory2; it writes the interface asked.
    let result = unsafe { (libraries.create_factory)(0, &IID_FACTORY5, &mut object) };
    if result >= 0 && !object.is_null() {
        return Ok((Com(object), true));
    }
    let mut object: Object = ptr::null_mut();
    // SAFETY: as above.
    let result = unsafe { (libraries.create_factory)(0, &IID_FACTORY2, &mut object) };
    Ok((created(result, object, "CreateDXGIFactory2")?, false))
}

struct Adapter {
    name: String,
    software: bool,
    adapter: Com,
}

fn adapters(factory: &Com) -> Result<Vec<Adapter>, String> {
    let mut adapters = Vec::new();
    for index in 0_u32.. {
        let mut adapter: Object = ptr::null_mut();
        // SAFETY: the factory is live; the call writes an IDXGIAdapter1.
        let result = unsafe { com!(factory.raw(), slot::factory::ENUM_ADAPTERS1, fn(u32, *mut Object) -> Hresult, index, &mut adapter) };
        if result == DXGI_ERROR_NOT_FOUND {
            break;
        }
        let adapter = created(result, adapter, "IDXGIFactory1::EnumAdapters1")?;
        // SAFETY: the structure is plain integers, for which zero is valid;
        // the adapter is live and fills it.
        let desc = unsafe {
            let mut desc: AdapterDesc1 = std::mem::zeroed();
            check(com!(adapter.raw(), slot::adapter::GET_DESC1, fn(*mut AdapterDesc1) -> Hresult, &mut desc), "IDXGIAdapter1::GetDesc1")?;
            desc
        };
        let end = desc.description.iter().position(|&unit| unit == 0).unwrap_or(desc.description.len());
        adapters.push(Adapter {
            name: String::from_utf16_lossy(&desc.description[..end]),
            software: desc.flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0 || (desc.vendor_id, desc.device_id) == BASIC_RENDER_DRIVER,
            adapter,
        });
    }
    Ok(adapters)
}

/// The DXGI adapters of the machine, in the order DXGI lists them.
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    let libraries = libraries()?;
    let (factory, _) = factory(&libraries)?;
    Ok(adapters(&factory)?.into_iter().enumerate().map(|(index, adapter)| DeviceInfo { index, name: adapter.name, kind: if adapter.software { "software" } else { "hardware" }.to_string() }).collect())
}

pub struct Direct3d12Presenter {
    mode: Mode,
    swapchain_flags: u32,
    tearing_offered: bool,
    width: u32,
    height: u32,
    device_name: String,
    device_kind: &'static str,
    created_on_thread: String,
    statistics: Vec<FrameStatistics>,
    statistics_errors: u64,
    first_statistics_error: Hresult,
    buffers: Vec<Com>,
    buffer_fences: Vec<u64>,
    render_target_start: usize,
    render_target_increment: usize,
    allocators: Vec<Com>,
    list: Com,
    offscreen_allocator: Com,
    offscreen_list: Com,
    _offscreen_target: Com,
    _render_target_heap: Com,
    fence: Com,
    fence_value: u64,
    swapchain: Com,
    queue: Com,
    device: Com,
    _factory: Com,
    _libraries: Libraries,
}

impl Direct3d12Presenter {
    pub fn open(request: &Request) -> Result<Direct3d12Presenter, String> {
        let NativeWindow::Win32 { hwnd, .. } = request.window else {
            return Err("Direct3D 12 presents to a Windows window only".to_string());
        };
        let libraries = libraries()?;
        let (factory, has_feature_query) = factory(&libraries)?;
        let mut tearing_offered = false;
        if has_feature_query {
            let mut allowed = 0_i32;
            // SAFETY: the factory is an IDXGIFactory5; the call writes one BOOL.
            let result = unsafe { com!(factory.raw(), slot::factory::CHECK_FEATURE_SUPPORT, fn(u32, *mut c_void, u32) -> Hresult, DXGI_FEATURE_PRESENT_ALLOW_TEARING, (&mut allowed as *mut i32).cast(), 4) };
            tearing_offered = result >= 0 && allowed != 0;
        }
        if request.mode == Mode::Unsynchronized && !tearing_offered {
            return Err("not offered: DXGI does not allow tearing on this system".to_string());
        }
        let mut listed = adapters(&factory)?;
        if request.device >= listed.len() {
            return Err(format!("no adapter at index {}", request.device));
        }
        let adapter = listed.swap_remove(request.device);
        let mut device: Object = ptr::null_mut();
        // SAFETY: the adapter is live; the call writes an ID3D12Device.
        let result = unsafe { (libraries.create_device)(adapter.adapter.raw(), FEATURE_LEVEL_11_0, &IID_DEVICE, &mut device) };
        let device = created(result, device, "D3D12CreateDevice at feature level 11_0")?;
        let queue_desc = CommandQueueDesc { kind: COMMAND_LIST_TYPE_DIRECT, priority: 0, flags: 0, node_mask: 0 };
        let mut queue: Object = ptr::null_mut();
        // SAFETY: the device is live; the call writes an ID3D12CommandQueue.
        let result = unsafe { com!(device.raw(), slot::device::CREATE_COMMAND_QUEUE, fn(*const CommandQueueDesc, *const Guid, *mut Object) -> Hresult, &queue_desc, &IID_COMMAND_QUEUE, &mut queue) };
        let queue = created(result, queue, "ID3D12Device::CreateCommandQueue")?;

        let swapchain_flags = if request.mode == Mode::Unsynchronized { DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING } else { 0 };
        let desc = SwapChainDesc1 {
            width: request.width.max(1),
            height: request.height.max(1),
            format: FORMAT_R8G8B8A8_UNORM,
            stereo: 0,
            sample_count: 1,
            sample_quality: 0,
            buffer_usage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            buffer_count: BUFFER_COUNT,
            scaling: 0,
            swap_effect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            alpha_mode: 0,
            flags: swapchain_flags,
        };
        let mut first: Object = ptr::null_mut();
        // SAFETY: the factory, the queue and the window are live; the call
        // writes an IDXGISwapChain1. For Direct3D 12 the device argument is
        // the queue the swapchain presents on.
        let result = unsafe {
            com!(factory.raw(), slot::factory::CREATE_SWAP_CHAIN_FOR_HWND, fn(Object, usize, *const SwapChainDesc1, *const c_void, Object, *mut Object) -> Hresult, queue.raw(), hwnd, &desc, ptr::null(), ptr::null_mut(), &mut first)
        };
        let first = created(result, first, "IDXGIFactory2::CreateSwapChainForHwnd")?;
        let mut swapchain: Object = ptr::null_mut();
        // SAFETY: the swapchain is live; the call writes the interface asked.
        let result = unsafe { com!(first.raw(), slot::QUERY_INTERFACE, fn(*const Guid, *mut Object) -> Hresult, &IID_SWAPCHAIN3, &mut swapchain) };
        let swapchain = created(result, swapchain, "IDXGISwapChain1::QueryInterface for IDXGISwapChain3")?;
        drop(first);
        // The first release plans no exclusive full screen: the key that
        // would switch to it is left to the application.
        // SAFETY: the factory and the window are live.
        check(unsafe { com!(factory.raw(), slot::factory::MAKE_WINDOW_ASSOCIATION, fn(usize, u32) -> Hresult, hwnd, DXGI_MWA_NO_ALT_ENTER) }, "IDXGIFactory::MakeWindowAssociation")?;

        let heap_desc = DescriptorHeapDesc { kind: DESCRIPTOR_HEAP_TYPE_RTV, count: BUFFER_COUNT + 1, flags: 0, node_mask: 0 };
        let mut heap: Object = ptr::null_mut();
        // SAFETY: the device is live; the call writes an ID3D12DescriptorHeap.
        let result = unsafe { com!(device.raw(), slot::device::CREATE_DESCRIPTOR_HEAP, fn(*const DescriptorHeapDesc, *const Guid, *mut Object) -> Hresult, &heap_desc, &IID_DESCRIPTOR_HEAP, &mut heap) };
        let heap = created(result, heap, "ID3D12Device::CreateDescriptorHeap")?;
        let mut render_target_start = 0_usize;
        // SAFETY: the heap is live. The method returns a structure, which the
        // calling convention passes as a hidden pointer.
        unsafe { com!(heap.raw(), slot::descriptor_heap::GET_CPU_DESCRIPTOR_HANDLE_FOR_HEAP_START, fn(*mut usize) -> *mut usize, &mut render_target_start) };
        // SAFETY: the device is live.
        let render_target_increment = unsafe { com!(device.raw(), slot::device::GET_DESCRIPTOR_HANDLE_INCREMENT_SIZE, fn(u32) -> u32, DESCRIPTOR_HEAP_TYPE_RTV) } as usize;

        let mut fence: Object = ptr::null_mut();
        // SAFETY: the device is live; the call writes an ID3D12Fence.
        let result = unsafe { com!(device.raw(), slot::device::CREATE_FENCE, fn(u64, u32, *const Guid, *mut Object) -> Hresult, 0, 0, &IID_FENCE, &mut fence) };
        let fence = created(result, fence, "ID3D12Device::CreateFence")?;

        let allocator = |what: &str| -> Result<Com, String> {
            let mut allocator: Object = ptr::null_mut();
            // SAFETY: the device is live; the call writes an ID3D12CommandAllocator.
            let result = unsafe { com!(device.raw(), slot::device::CREATE_COMMAND_ALLOCATOR, fn(u32, *const Guid, *mut Object) -> Hresult, COMMAND_LIST_TYPE_DIRECT, &IID_COMMAND_ALLOCATOR, &mut allocator) };
            created(result, allocator, what)
        };
        let list_on = |allocator: &Com| -> Result<Com, String> {
            let mut list: Object = ptr::null_mut();
            // SAFETY: the device and the allocator are live; the call writes
            // a command list in the recording state, closed at once.
            unsafe {
                let result = com!(device.raw(), slot::device::CREATE_COMMAND_LIST, fn(u32, u32, Object, Object, *const Guid, *mut Object) -> Hresult, 0, COMMAND_LIST_TYPE_DIRECT, allocator.raw(), ptr::null_mut(), &IID_GRAPHICS_COMMAND_LIST, &mut list);
                let list = created(result, list, "ID3D12Device::CreateCommandList")?;
                check(com!(list.raw(), slot::list::CLOSE, fn() -> Hresult), "ID3D12GraphicsCommandList::Close")?;
                Ok(list)
            }
        };
        let mut allocators = Vec::new();
        for _ in 0..BUFFER_COUNT {
            allocators.push(allocator("ID3D12Device::CreateCommandAllocator")?);
        }
        let list = list_on(&allocators[0])?;
        let offscreen_allocator = allocator("ID3D12Device::CreateCommandAllocator for the offscreen stream")?;
        let offscreen_list = list_on(&offscreen_allocator)?;

        let heap_properties = HeapProperties { kind: HEAP_TYPE_DEFAULT, cpu_page_property: 0, memory_pool_preference: 0, creation_node_mask: 1, visible_node_mask: 1 };
        let target_desc = ResourceDesc {
            dimension: RESOURCE_DIMENSION_TEXTURE2D,
            alignment: 0,
            width: u64::from(OFFSCREEN_SIZE),
            height: OFFSCREEN_SIZE,
            depth_or_array_size: 1,
            mip_levels: 1,
            format: FORMAT_R8G8B8A8_UNORM,
            sample_count: 1,
            sample_quality: 0,
            layout: 0,
            flags: RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        };
        let clear = ClearValue { format: FORMAT_R8G8B8A8_UNORM, color: [0.0, 0.0, 0.0, 1.0] };
        let mut target: Object = ptr::null_mut();
        // SAFETY: the device is live; the structures outlive the call, which
        // writes an ID3D12Resource.
        let result = unsafe {
            com!(device.raw(), slot::device::CREATE_COMMITTED_RESOURCE, fn(*const HeapProperties, u32, *const ResourceDesc, u32, *const ClearValue, *const Guid, *mut Object) -> Hresult, &heap_properties, 0, &target_desc, STATE_RENDER_TARGET, &clear, &IID_RESOURCE, &mut target)
        };
        let offscreen_target = created(result, target, "ID3D12Device::CreateCommittedResource for the offscreen target")?;
        // SAFETY: the device, the target and the heap are live; the handle is
        // the heap's last slot.
        unsafe {
            com!(device.raw(), slot::device::CREATE_RENDER_TARGET_VIEW, fn(Object, *const c_void, usize) -> (), offscreen_target.raw(), ptr::null(), render_target_start + BUFFER_COUNT as usize * render_target_increment)
        };

        let mut presenter = Direct3d12Presenter {
            mode: request.mode,
            swapchain_flags,
            tearing_offered,
            width: desc.width,
            height: desc.height,
            device_name: adapter.name.clone(),
            device_kind: if adapter.software { "software" } else { "hardware" },
            created_on_thread: std::thread::current().name().unwrap_or("unnamed").to_string(),
            statistics: Vec::new(),
            statistics_errors: 0,
            first_statistics_error: 0,
            buffers: Vec::new(),
            buffer_fences: vec![0; BUFFER_COUNT as usize],
            render_target_start,
            render_target_increment,
            allocators,
            list,
            offscreen_allocator,
            offscreen_list,
            _offscreen_target: offscreen_target,
            _render_target_heap: heap,
            fence,
            fence_value: 0,
            swapchain,
            queue,
            device,
            _factory: factory,
            _libraries: libraries,
        };
        presenter.take_buffers()?;
        Ok(presenter)
    }

    fn render_target(&self, index: usize) -> usize {
        self.render_target_start + index * self.render_target_increment
    }

    /// Takes the swapchain's buffers and makes a render target view of each.
    fn take_buffers(&mut self) -> Result<(), String> {
        self.buffers.clear();
        for index in 0..BUFFER_COUNT {
            let mut buffer: Object = ptr::null_mut();
            // SAFETY: the swapchain is live; the call writes an ID3D12Resource.
            let result = unsafe { com!(self.swapchain.raw(), slot::swapchain::GET_BUFFER, fn(u32, *const Guid, *mut Object) -> Hresult, index, &IID_RESOURCE, &mut buffer) };
            let buffer = created(result, buffer, "IDXGISwapChain::GetBuffer")?;
            // SAFETY: the device, the buffer and the heap are live.
            unsafe { com!(self.device.raw(), slot::device::CREATE_RENDER_TARGET_VIEW, fn(Object, *const c_void, usize) -> (), buffer.raw(), ptr::null(), self.render_target(index as usize)) };
            self.buffers.push(buffer);
        }
        Ok(())
    }

    fn signal(&mut self) -> Result<u64, String> {
        self.fence_value += 1;
        // SAFETY: the queue and the fence are live.
        check(unsafe { com!(self.queue.raw(), slot::queue::SIGNAL, fn(Object, u64) -> Hresult, self.fence.raw(), self.fence_value) }, "ID3D12CommandQueue::Signal")?;
        Ok(self.fence_value)
    }

    /// Blocks until the fence reaches `value`: with no event handle,
    /// SetEventOnCompletion returns once the value is reached.
    fn wait(&self, value: u64) -> Result<(), String> {
        // SAFETY: the fence is live.
        let completed = unsafe { com!(self.fence.raw(), slot::fence::GET_COMPLETED_VALUE, fn() -> u64) };
        if completed >= value && completed != u64::MAX {
            return Ok(());
        }
        // SAFETY: the fence is live; a null event makes the call block.
        check(unsafe { com!(self.fence.raw(), slot::fence::SET_EVENT_ON_COMPLETION, fn(u64, *mut c_void) -> Hresult, value, ptr::null_mut()) }, "ID3D12Fence::SetEventOnCompletion")?;
        // SAFETY: the device is live.
        check(unsafe { com!(self.device.raw(), slot::device::GET_DEVICE_REMOVED_REASON, fn() -> Hresult) }, "the device was removed; ID3D12Device::GetDeviceRemovedReason")
    }

    /// Records a clear of one render target on `list` and submits it.
    fn clear_and_submit(&self, allocator: Object, list: Object, target: usize, buffer: Option<Object>, color: [f32; 4]) -> Result<(), String> {
        // SAFETY: every object is live; the allocator's previous submission
        // has completed, which each caller waits for; the barriers name a
        // live swapchain buffer.
        unsafe {
            check(com!(allocator, slot::allocator::RESET, fn() -> Hresult), "ID3D12CommandAllocator::Reset")?;
            check(com!(list, slot::list::RESET, fn(Object, Object) -> Hresult, allocator, ptr::null_mut()), "ID3D12GraphicsCommandList::Reset")?;
            let barrier = |before: u32, after: u32| {
                if let Some(resource) = buffer {
                    let barrier = TransitionBarrier { kind: 0, flags: 0, resource, subresource: ALL_SUBRESOURCES, state_before: before, state_after: after };
                    com!(list, slot::list::RESOURCE_BARRIER, fn(u32, *const TransitionBarrier) -> (), 1, &barrier);
                }
            };
            barrier(STATE_PRESENT, STATE_RENDER_TARGET);
            com!(list, slot::list::CLEAR_RENDER_TARGET_VIEW, fn(usize, *const f32, u32, *const c_void) -> (), target, color.as_ptr(), 0, ptr::null());
            barrier(STATE_RENDER_TARGET, STATE_PRESENT);
            check(com!(list, slot::list::CLOSE, fn() -> Hresult), "ID3D12GraphicsCommandList::Close")?;
            com!(self.queue.raw(), slot::queue::EXECUTE_COMMAND_LISTS, fn(u32, *const Object) -> (), 1, &list);
        }
        Ok(())
    }
}

impl Presenter for Direct3d12Presenter {
    fn describe(&self) -> Json {
        Json::object()
            .with("interface", "Direct3D 12 with a DXGI flip-model swapchain, natively")
            .with("device", self.device_name.clone())
            .with("device_kind", self.device_kind)
            .with("swapchain", format!("{BUFFER_COUNT} buffers, flip discard, {}", if self.swapchain_flags & DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING != 0 { "tearing allowed" } else { "no tearing flag" }))
            .with("unsynchronized_offered", self.tearing_offered)
            .with("created_on_thread", self.created_on_thread.clone())
            .with("acquisition", "reads the index of the current back buffer, then waits for the work last submitted for that buffer; no call of the interface may give nothing")
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 || (width, height) == (self.width, self.height) {
            return Ok(());
        }
        let value = self.signal()?;
        self.wait(value)?;
        self.buffers.clear();
        // SAFETY: the swapchain is live and no reference to its buffers is held.
        check(unsafe { com!(self.swapchain.raw(), slot::swapchain::RESIZE_BUFFERS, fn(u32, u32, u32, u32, u32) -> Hresult, 0, width, height, 0, self.swapchain_flags) }, "IDXGISwapChain::ResizeBuffers")?;
        self.width = width;
        self.height = height;
        self.buffer_fences.fill(0);
        self.take_buffers()
    }

    fn frame(&mut self, index: u64) -> Result<FrameOutcome, String> {
        let started = now_ns();
        // SAFETY: the swapchain is live.
        let back = unsafe { com!(self.swapchain.raw(), slot::swapchain::GET_CURRENT_BACK_BUFFER_INDEX, fn() -> u32) } as usize;
        let indexed = now_ns();
        self.wait(self.buffer_fences[back])?;
        let acquired = now_ns();
        self.clear_and_submit(self.allocators[back].raw(), self.list.raw(), self.render_target(back), Some(self.buffers[back].raw()), frame_color(index))?;
        let submitted = now_ns();
        let (interval, flags) = match self.mode {
            Mode::Synchronized => (1, 0),
            Mode::Latest => (0, 0),
            Mode::Unsynchronized => (0, DXGI_PRESENT_ALLOW_TEARING),
        };
        // SAFETY: the swapchain is live.
        let result = unsafe { com!(self.swapchain.raw(), slot::swapchain::PRESENT, fn(u32, u32) -> Hresult, interval, flags) };
        let presented_at = now_ns();
        self.buffer_fences[back] = self.signal()?;
        if result < 0 {
            return Err(format!("IDXGISwapChain::Present failed with HRESULT {:#010x}", result as u32));
        }
        if self.mode == Mode::Synchronized && self.statistics.len() < STATISTICS_KEPT {
            let mut statistics = FrameStatistics::default();
            // SAFETY: the swapchain is live; the call fills the structure.
            let got = unsafe { com!(self.swapchain.raw(), slot::swapchain::GET_FRAME_STATISTICS, fn(*mut FrameStatistics) -> Hresult, &mut statistics) };
            if got >= 0 {
                self.statistics.push(statistics);
            } else {
                if self.statistics_errors == 0 {
                    self.first_statistics_error = got;
                }
                self.statistics_errors += 1;
            }
        }
        let (presented, status) = match result {
            0 => (true, "presented".to_string()),
            DXGI_STATUS_OCCLUDED => (false, "presented nothing: the call returned the status occluded".to_string()),
            other => (true, format!("presented with status {:#010x}", other as u32)),
        };
        Ok(FrameOutcome { acquire_ns: indexed - started, wait_ns: acquired - indexed, submit_ns: submitted - acquired, present_ns: presented_at - submitted, presented, status })
    }

    fn offscreen(&mut self, index: u64) -> Result<(), String> {
        self.clear_and_submit(self.offscreen_allocator.raw(), self.offscreen_list.raw(), self.render_target(BUFFER_COUNT as usize), None, frame_color(index))?;
        let value = self.signal()?;
        self.wait(value)
    }

    fn presentation_times(&mut self) -> Json {
        let mut times = Json::object()
            .with("source", "IDXGISwapChain::GetFrameStatistics after each synchronized presentation: the count of the refresh at which the last frame was shown and the performance counter at a recent refresh")
            .with("readings", self.statistics.len())
            .with("readings_refused", self.statistics_errors);
        if self.statistics_errors > 0 {
            times.set("first_refusal", format!("HRESULT {:#010x}", self.first_statistics_error as u32));
        }
        // The refresh period the statistics imply: the performance counter
        // against the count of refreshes, between the first and the last
        // reading that differ.
        if let (Some(first), Some(last)) = (self.statistics.first(), self.statistics.last()) {
            let refreshes = last.sync_refresh_count.wrapping_sub(first.sync_refresh_count);
            if refreshes > 0 {
                let ticks = (last.sync_qpc_time - first.sync_qpc_time) as f64;
                let period_ns = ticks * 1e9 / crate::win32::performance_frequency() as f64 / f64::from(refreshes);
                times.set("refresh_period_from_the_statistics_us", Json::Float(period_ns / 1000.0));
                times.set("refreshes_spanned", refreshes);
            }
            // By how many refreshes the shown frame advanced from one
            // presentation to the next.
            let mut steps: std::collections::BTreeMap<u32, u64> = std::collections::BTreeMap::new();
            for pair in self.statistics.windows(2) {
                *steps.entry(pair[1].present_refresh_count.wrapping_sub(pair[0].present_refresh_count)).or_default() += 1;
            }
            times.set("refreshes_between_two_presentations", Json::List(steps.into_iter().map(|(refreshes, count)| Json::object().with("refreshes", refreshes).with("presentations", count)).collect()));
        }
        times
    }
}

impl Drop for Direct3d12Presenter {
    fn drop(&mut self) {
        // The device must have finished every submission before the objects
        // it uses are released.
        if let Ok(value) = self.signal() {
            let _ = self.wait(value);
        }
    }
}
