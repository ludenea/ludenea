//! One open Direct3D 12 device with its direct queue and fence, and the
//! helpers every exercise uses: committed resources, uploads, read-backs,
//! descriptor heaps, root signatures, pipeline states, barriers and command
//! lists. Throwaway, never product code.

use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};

use super::ffi::*;

/// How long a submission may take before the spike gives up on it.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(60);

/// A command allocator and the one graphics command list recorded on it.
pub struct Recorder {
    pub list: Com,
    pub allocator: Com,
}

impl Recorder {
    /// Resets the allocator and the list for a new recording.
    pub fn begin(&self) -> Result<(), String> {
        // SAFETY: both objects are live; the list is closed and its previous
        // submission has completed, which every caller waits for.
        unsafe {
            check(com!(self.allocator.raw(), slot::allocator::RESET, fn() -> Hresult), "ID3D12CommandAllocator::Reset")?;
            check(com!(self.list.raw(), slot::list::RESET, fn(Object, Object) -> Hresult, self.allocator.raw(), ptr::null_mut()), "ID3D12GraphicsCommandList::Reset")
        }
    }
}

/// Closes a recorded list.
pub fn close(list: Object) -> Result<(), String> {
    // SAFETY: `list` is a live command list in the recording state.
    check(unsafe { com!(list, slot::list::CLOSE, fn() -> Hresult) }, "ID3D12GraphicsCommandList::Close")
}

pub fn transition(resource: Object, before: u32, after: u32) -> ResourceBarrier {
    ResourceBarrier { kind: BARRIER_TYPE_TRANSITION, flags: 0, resource, subresource: ALL_SUBRESOURCES, state_before: before, state_after: after }
}

/// Records `barriers` on `list`.
pub fn barriers(list: Object, barriers: &[ResourceBarrier]) {
    // SAFETY: `list` is recording; the barriers name live resources.
    unsafe { com!(list, slot::list::RESOURCE_BARRIER, fn(u32, *const ResourceBarrier) -> (), barriers.len() as u32, barriers.as_ptr()) };
}

pub fn texture_desc(width: u32, height: u32, levels: u16, format: u32, sample: SampleDesc, flags: u32) -> ResourceDesc {
    ResourceDesc { dimension: RESOURCE_DIMENSION_TEXTURE2D, alignment: 0, width: u64::from(width), height, depth_or_array_size: 1, mip_levels: levels, format, sample, layout: TEXTURE_LAYOUT_UNKNOWN, flags }
}

pub fn buffer_desc(size: u64, flags: u32) -> ResourceDesc {
    ResourceDesc {
        dimension: RESOURCE_DIMENSION_BUFFER,
        alignment: 0,
        width: size,
        height: 1,
        depth_or_array_size: 1,
        mip_levels: 1,
        format: FORMAT_UNKNOWN,
        sample: SampleDesc { count: 1, quality: 0 },
        layout: TEXTURE_LAYOUT_ROW_MAJOR,
        flags,
    }
}

pub const ONE_SAMPLE: SampleDesc = SampleDesc { count: 1, quality: 0 };

/// A descriptor heap with the size of one of its descriptors.
pub struct DescriptorHeap {
    pub heap: Com,
    pub increment: u32,
    cpu_start: usize,
    gpu_start: u64,
}

impl DescriptorHeap {
    pub fn cpu(&self, index: u32) -> CpuHandle {
        CpuHandle { pointer: self.cpu_start + (index * self.increment) as usize }
    }

    pub fn gpu(&self, index: u32) -> u64 {
        self.gpu_start + u64::from(index * self.increment)
    }
}

/// A texture's layout in a read-back buffer.
pub struct ReadBack {
    pub buffer: Com,
    pub footprint: PlacedFootprint,
    pub size: u64,
}

pub struct Gpu {
    pub queue: Com,
    pub fence: Com,
    fence_value: u64,
    serialize: SerializeRootSignature,
    /// Dropped last: every other object of the device is released before it.
    pub device: Com,
}

impl Gpu {
    /// Creates the device on `adapter` with its direct queue and its fence.
    pub fn create(create_device: CreateDevice, serialize: SerializeRootSignature, adapter: Object) -> Result<Gpu, String> {
        let mut device: Object = ptr::null_mut();
        // SAFETY: `adapter` is a live DXGI adapter; the call writes an
        // ID3D12Device whose reference becomes owned below.
        let result = unsafe { create_device(adapter, FEATURE_LEVEL_11_0, &IID_DEVICE, &mut device) };
        let device = created(result, device, "D3D12CreateDevice at feature level 11_0")?;
        let queue = create_queue(device.raw(), COMMAND_LIST_TYPE_DIRECT)?;
        let mut fence: Object = ptr::null_mut();
        // SAFETY: the device is live; the call writes an ID3D12Fence.
        let result = unsafe { com!(device.raw(), slot::device::CREATE_FENCE, fn(u64, u32, *const Guid, *mut Object) -> Hresult, 0, 0, &IID_FENCE, &mut fence) };
        let fence = created(result, fence, "ID3D12Device::CreateFence")?;
        Ok(Gpu { queue, fence, fence_value: 0, serialize, device })
    }

    pub fn raw(&self) -> Object {
        self.device.raw()
    }

    /// Signals the fence on the queue after the work submitted so far.
    pub fn signal(&mut self) -> Result<u64, String> {
        self.fence_value += 1;
        // SAFETY: the queue and the fence are live.
        check(unsafe { com!(self.queue.raw(), slot::queue::SIGNAL, fn(Object, u64) -> Hresult, self.fence.raw(), self.fence_value) }, "ID3D12CommandQueue::Signal")?;
        Ok(self.fence_value)
    }

    pub fn completed_value(&self) -> u64 {
        // SAFETY: the fence is live.
        unsafe { com!(self.fence.raw(), slot::fence::GET_COMPLETED_VALUE, fn() -> u64) }
    }

    /// Blocks until the fence reaches `value`: with no event handle,
    /// SetEventOnCompletion returns once the value is reached.
    pub fn wait(&self, value: u64) -> Result<(), String> {
        // SAFETY: the fence is live; a null event makes the call block.
        check(unsafe { com!(self.fence.raw(), slot::fence::SET_EVENT_ON_COMPLETION, fn(u64, *mut c_void) -> Hresult, value, ptr::null_mut()) }, "ID3D12Fence::SetEventOnCompletion")?;
        self.check_not_removed()
    }

    /// Polls the fence without blocking until it reaches `value`, and returns
    /// how many times it was polled.
    pub fn poll(&self, value: u64) -> Result<u64, String> {
        let started = Instant::now();
        let mut polls = 0;
        loop {
            polls += 1;
            let completed = self.completed_value();
            if completed == u64::MAX {
                self.check_not_removed()?;
            }
            if completed >= value {
                return Ok(polls);
            }
            if started.elapsed() > COMPLETION_TIMEOUT {
                return Err(format!("the fence did not reach {value} within {} seconds (at {completed})", COMPLETION_TIMEOUT.as_secs()));
            }
            std::thread::yield_now();
        }
    }

    fn check_not_removed(&self) -> Result<(), String> {
        // SAFETY: the device is live.
        let reason = unsafe { com!(self.raw(), slot::device::GET_DEVICE_REMOVED_REASON, fn() -> Hresult) };
        check(reason, "the device was removed; ID3D12Device::GetDeviceRemovedReason")
    }

    /// Submits `lists` in their order, signals and returns the fence value.
    pub fn execute(&mut self, lists: &[Object]) -> Result<u64, String> {
        // SAFETY: the lists are live and closed; the queue is live.
        unsafe { com!(self.queue.raw(), slot::queue::EXECUTE_COMMAND_LISTS, fn(u32, *const Object) -> (), lists.len() as u32, lists.as_ptr()) };
        self.signal()
    }

    pub fn wait_idle(&mut self) -> Result<(), String> {
        let value = self.signal()?;
        self.wait(value)
    }

    pub fn recorder(&self) -> Result<Recorder, String> {
        let mut allocator: Object = ptr::null_mut();
        // SAFETY: the device is live; the call writes an ID3D12CommandAllocator.
        let result = unsafe { com!(self.raw(), slot::device::CREATE_COMMAND_ALLOCATOR, fn(u32, *const Guid, *mut Object) -> Hresult, COMMAND_LIST_TYPE_DIRECT, &IID_COMMAND_ALLOCATOR, &mut allocator) };
        let allocator = created(result, allocator, "ID3D12Device::CreateCommandAllocator")?;
        let mut list: Object = ptr::null_mut();
        // SAFETY: the device and the allocator are live; the call writes an
        // ID3D12GraphicsCommandList in the recording state.
        let result = unsafe {
            com!(self.raw(), slot::device::CREATE_COMMAND_LIST, fn(u32, u32, Object, Object, *const Guid, *mut Object) -> Hresult, 0, COMMAND_LIST_TYPE_DIRECT, allocator.raw(), ptr::null_mut(), &IID_GRAPHICS_COMMAND_LIST, &mut list)
        };
        let list = created(result, list, "ID3D12Device::CreateCommandList")?;
        close(list.raw())?;
        Ok(Recorder { list, allocator })
    }

    /// Records with `record` on a list of its own, submits it and waits.
    pub fn submit_and_wait(&mut self, record: impl FnOnce(Object)) -> Result<(), String> {
        let recorder = self.recorder()?;
        recorder.begin()?;
        record(recorder.list.raw());
        close(recorder.list.raw())?;
        let value = self.execute(&[recorder.list.raw()])?;
        self.wait(value)
    }

    pub fn committed(&self, heap: u32, desc: &ResourceDesc, state: u32, clear: Option<&ClearValue>, what: &str) -> Result<Com, String> {
        let properties = HeapProperties { kind: heap, cpu_page_property: 0, memory_pool_preference: 0, creation_node_mask: 1, visible_node_mask: 1 };
        let clear = clear.map_or(ptr::null(), |clear| clear as *const ClearValue);
        let mut resource: Object = ptr::null_mut();
        // SAFETY: the device is live; the descriptions are complete
        // structures of the header's layout; the call writes an ID3D12Resource.
        let result = unsafe {
            com!(self.raw(), slot::device::CREATE_COMMITTED_RESOURCE, fn(*const HeapProperties, u32, *const ResourceDesc, u32, *const ClearValue, *const Guid, *mut Object) -> Hresult, &properties, 0, desc, state, clear, &IID_RESOURCE, &mut resource)
        };
        created(result, resource, &format!("ID3D12Device::CreateCommittedResource ({what})"))
    }

    /// A buffer of `size` bytes in `heap`, in the state that heap requires.
    pub fn buffer(&self, heap: u32, size: u64, flags: u32, what: &str) -> Result<Com, String> {
        let state = match heap {
            HEAP_TYPE_UPLOAD => STATE_GENERIC_READ,
            HEAP_TYPE_READBACK => STATE_COPY_DEST,
            _ => STATE_COMMON,
        };
        self.committed(heap, &buffer_desc(size, flags), state, None, what)
    }

    /// A buffer of the default heap holding `bytes`, left in `state`.
    pub fn upload_buffer(&mut self, bytes: &[u8], state: u32, flags: u32, what: &str) -> Result<Com, String> {
        let size = bytes.len() as u64;
        let buffer = self.buffer(HEAP_TYPE_DEFAULT, size, flags, what)?;
        let staging = self.buffer(HEAP_TYPE_UPLOAD, size, RESOURCE_FLAG_NONE, &format!("{what}, upload"))?;
        write_buffer(staging.raw(), 0, bytes)?;
        let (target, source) = (buffer.raw(), staging.raw());
        self.submit_and_wait(|list| {
            barriers(list, &[transition(target, STATE_COMMON, STATE_COPY_DEST)]);
            // SAFETY: the list is recording; both buffers hold `size` bytes.
            unsafe { com!(list, slot::list::COPY_BUFFER_REGION, fn(Object, u64, Object, u64, u64) -> (), target, 0, source, 0, size) };
            barriers(list, &[transition(target, STATE_COPY_DEST, state)]);
        })?;
        Ok(buffer)
    }

    /// The placed footprints of the first `count` subresources of `desc`.
    pub fn footprints(&self, desc: &ResourceDesc, count: u32) -> (Vec<PlacedFootprint>, Vec<u32>, u64) {
        let mut layouts = vec![PlacedFootprint::default(); count as usize];
        let mut rows = vec![0_u32; count as usize];
        let mut row_sizes = vec![0_u64; count as usize];
        let mut total = 0_u64;
        // SAFETY: the device is live; the output arrays hold `count` entries.
        unsafe {
            com!(self.raw(), slot::device::GET_COPYABLE_FOOTPRINTS, fn(*const ResourceDesc, u32, u32, u64, *mut PlacedFootprint, *mut u32, *mut u64, *mut u64) -> (), desc, 0, count, 0, layouts.as_mut_ptr(), rows.as_mut_ptr(), row_sizes.as_mut_ptr(), &mut total)
        };
        (layouts, rows, total)
    }

    /// A texture of the default heap with every level of `levels` (tight rows
    /// of `bytes_per_texel`) uploaded, left in `state`.
    pub fn upload_texture(&mut self, desc: &ResourceDesc, levels: &[Vec<u8>], bytes_per_texel: u32, state: u32, what: &str) -> Result<Com, String> {
        let texture = self.committed(HEAP_TYPE_DEFAULT, desc, STATE_COPY_DEST, None, what)?;
        let (layouts, rows, total) = self.footprints(desc, levels.len() as u32);
        let staging = self.buffer(HEAP_TYPE_UPLOAD, total, RESOURCE_FLAG_NONE, &format!("{what}, upload"))?;
        let mut staged = vec![0_u8; total as usize];
        for ((level, layout), row_count) in levels.iter().zip(&layouts).zip(&rows) {
            let tight = (layout.width * bytes_per_texel) as usize;
            for row in 0..*row_count as usize {
                let start = layout.offset as usize + row * layout.row_pitch as usize;
                staged[start..start + tight].copy_from_slice(&level[row * tight..(row + 1) * tight]);
            }
        }
        write_buffer(staging.raw(), 0, &staged)?;
        let (target, source) = (texture.raw(), staging.raw());
        self.submit_and_wait(|list| {
            for (index, layout) in layouts.iter().enumerate() {
                copy_buffer_to_texture(list, target, index as u32, source, *layout);
            }
            barriers(list, &[transition(target, STATE_COPY_DEST, state)]);
        })?;
        Ok(texture)
    }

    /// A read-back buffer sized for the first subresource of `desc`.
    pub fn read_back_for(&self, desc: &ResourceDesc, what: &str) -> Result<ReadBack, String> {
        let (layouts, _, total) = self.footprints(desc, 1);
        let buffer = self.buffer(HEAP_TYPE_READBACK, total, RESOURCE_FLAG_NONE, what)?;
        Ok(ReadBack { buffer, footprint: layouts[0], size: total })
    }

    pub fn descriptor_heap(&self, kind: u32, count: u32, shader_visible: bool) -> Result<DescriptorHeap, String> {
        let desc = DescriptorHeapDesc { kind, count, flags: if shader_visible { DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE } else { 0 }, node_mask: 0 };
        let mut heap: Object = ptr::null_mut();
        // SAFETY: the device is live; the call writes an ID3D12DescriptorHeap.
        let result = unsafe { com!(self.raw(), slot::device::CREATE_DESCRIPTOR_HEAP, fn(*const DescriptorHeapDesc, *const Guid, *mut Object) -> Hresult, &desc, &IID_DESCRIPTOR_HEAP, &mut heap) };
        let heap = created(result, heap, "ID3D12Device::CreateDescriptorHeap")?;
        let mut cpu = CpuHandle { pointer: 0 };
        let mut gpu = GpuHandle { pointer: 0 };
        // SAFETY: the device and the heap are live; the two handle getters
        // return a structure, through the hidden pointer of the convention.
        let increment = unsafe {
            com!(heap.raw(), slot::descriptor_heap::GET_CPU_DESCRIPTOR_HANDLE_FOR_HEAP_START, fn(*mut CpuHandle) -> *mut CpuHandle, &mut cpu);
            if shader_visible {
                com!(heap.raw(), slot::descriptor_heap::GET_GPU_DESCRIPTOR_HANDLE_FOR_HEAP_START, fn(*mut GpuHandle) -> *mut GpuHandle, &mut gpu);
            }
            com!(self.raw(), slot::device::GET_DESCRIPTOR_HANDLE_INCREMENT_SIZE, fn(u32) -> u32, kind)
        };
        Ok(DescriptorHeap { heap, increment, cpu_start: cpu.pointer, gpu_start: gpu.pointer })
    }

    /// A shader resource view of a two-dimensional texture, every level, in `format`.
    pub fn texture_view(&self, resource: Object, format: u32, levels: u32, handle: CpuHandle) {
        let desc = ShaderResourceViewDesc {
            format,
            dimension: SRV_DIMENSION_TEXTURE2D,
            mapping: DEFAULT_SHADER_4_COMPONENT_MAPPING,
            padding: 0,
            most_detailed_mip: 0,
            mip_levels: levels,
            plane_slice: 0,
            min_lod_clamp: 0.0,
            rest: [0; 2],
        };
        // SAFETY: the device and the resource are live; the handle is a slot
        // of a live heap.
        unsafe { com!(self.raw(), slot::device::CREATE_SHADER_RESOURCE_VIEW, fn(Object, *const ShaderResourceViewDesc, CpuHandle) -> (), resource, &desc, handle) };
    }

    /// A render target view of `resource` in `format`, or in the resource's
    /// own format and dimension when `format` is unknown.
    pub fn render_target_view(&self, resource: Object, format: u32, handle: CpuHandle) {
        let desc = RenderTargetViewDesc { format, dimension: RTV_DIMENSION_TEXTURE2D, mip_slice: 0, plane_slice: 0, rest: [0; 2] };
        let desc_pointer = if format == FORMAT_UNKNOWN { ptr::null() } else { &desc as *const RenderTargetViewDesc };
        // SAFETY: the device and the resource are live; the handle is a slot
        // of a live heap; a null description is allowed for a typed resource.
        unsafe { com!(self.raw(), slot::device::CREATE_RENDER_TARGET_VIEW, fn(Object, *const RenderTargetViewDesc, CpuHandle) -> (), resource, desc_pointer, handle) };
    }

    pub fn depth_view(&self, resource: Object, handle: CpuHandle) {
        // SAFETY: the device and the resource (typed D32_FLOAT) are live; the
        // handle is a slot of a live heap.
        unsafe { com!(self.raw(), slot::device::CREATE_DEPTH_STENCIL_VIEW, fn(Object, *const c_void, CpuHandle) -> (), resource, ptr::null(), handle) };
    }

    pub fn root_signature(&self, parameters: &[RootParameter], static_samplers: &[StaticSamplerDesc], flags: u32, what: &str) -> Result<Com, String> {
        let desc = RootSignatureDesc {
            parameter_count: parameters.len() as u32,
            parameters: parameters.as_ptr(),
            static_sampler_count: static_samplers.len() as u32,
            static_samplers: static_samplers.as_ptr(),
            flags,
        };
        let (mut blob, mut errors): (Object, Object) = (ptr::null_mut(), ptr::null_mut());
        // SAFETY: the description and the arrays it points to are live for
        // the call; the function writes two blobs owned below.
        let result = unsafe { (self.serialize)(&desc, ROOT_SIGNATURE_VERSION_1_0, &mut blob, &mut errors) };
        let messages = if errors.is_null() { String::new() } else { blob_text(&Com::from_raw(errors)) };
        let blob = created(result, blob, &format!("D3D12SerializeRootSignature ({what}) {messages}"))?;
        let bytes = blob_bytes(&blob);
        let mut signature: Object = ptr::null_mut();
        // SAFETY: the device is live; the bytes are a serialized root signature.
        let result = unsafe {
            com!(self.raw(), slot::device::CREATE_ROOT_SIGNATURE, fn(u32, *const c_void, usize, *const Guid, *mut Object) -> Hresult, 0, bytes.as_ptr() as *const c_void, bytes.len(), &IID_ROOT_SIGNATURE, &mut signature)
        };
        created(result, signature, &format!("ID3D12Device::CreateRootSignature ({what})"))
    }

    pub fn graphics_pipeline(&self, desc: &GraphicsPipelineDesc, what: &str) -> Result<Com, String> {
        let mut pipeline: Object = ptr::null_mut();
        // SAFETY: the device is live; the description is complete and every
        // pointer in it is live for the call.
        let result = unsafe { com!(self.raw(), slot::device::CREATE_GRAPHICS_PIPELINE_STATE, fn(*const GraphicsPipelineDesc, *const Guid, *mut Object) -> Hresult, desc, &IID_PIPELINE_STATE, &mut pipeline) };
        created(result, pipeline, &format!("ID3D12Device::CreateGraphicsPipelineState ({what})"))
    }

    pub fn compute_pipeline(&self, root_signature: Object, shader: &[u8], what: &str) -> Result<Com, String> {
        let desc = ComputePipelineDesc { root_signature, compute_shader: bytecode(shader), node_mask: 0, cached: CachedPipelineState { blob: ptr::null(), size: 0 }, flags: 0 };
        let mut pipeline: Object = ptr::null_mut();
        // SAFETY: the device is live; the description and the bytecode are
        // live for the call.
        let result = unsafe { com!(self.raw(), slot::device::CREATE_COMPUTE_PIPELINE_STATE, fn(*const ComputePipelineDesc, *const Guid, *mut Object) -> Hresult, &desc, &IID_PIPELINE_STATE, &mut pipeline) };
        created(result, pipeline, &format!("ID3D12Device::CreateComputePipelineState ({what})"))
    }

    /// Reads one feature structure; the error carries the call's result.
    pub fn feature<T>(&self, feature: u32, data: &mut T, what: &str) -> Result<(), String> {
        // SAFETY: the device is live; `data` is the structure of the header
        // for `feature`, with its exact size.
        let result = unsafe { com!(self.raw(), slot::device::CHECK_FEATURE_SUPPORT, fn(u32, *mut c_void, u32) -> Hresult, feature, data as *mut T as *mut c_void, std::mem::size_of::<T>() as u32) };
        check(result, &format!("CheckFeatureSupport({what})"))
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // The queue may still hold work if an exercise failed half way; the
        // objects are released only once it has drained. A failure here has
        // no caller to report to, and releasing anyway is the only option.
        let _ = self.wait_idle();
    }
}

pub fn create_queue(device: Object, kind: u32) -> Result<Com, String> {
    let desc = CommandQueueDesc { kind, priority: 0, flags: 0, node_mask: 0 };
    let mut queue: Object = ptr::null_mut();
    // SAFETY: the device is live; the call writes an ID3D12CommandQueue.
    let result = unsafe { com!(device, slot::device::CREATE_COMMAND_QUEUE, fn(*const CommandQueueDesc, *const Guid, *mut Object) -> Hresult, &desc, &IID_COMMAND_QUEUE, &mut queue) };
    created(result, queue, "ID3D12Device::CreateCommandQueue")
}

pub fn bytecode(code: &[u8]) -> ShaderBytecode {
    ShaderBytecode { code: code.as_ptr() as *const c_void, length: code.len() }
}

pub fn blob_bytes(blob: &Com) -> Vec<u8> {
    // SAFETY: the blob is live; its pointer and size describe its bytes.
    unsafe {
        let pointer = com!(blob.raw(), slot::blob::GET_BUFFER_POINTER, fn() -> *const u8);
        let size = com!(blob.raw(), slot::blob::GET_BUFFER_SIZE, fn() -> usize);
        if pointer.is_null() { Vec::new() } else { std::slice::from_raw_parts(pointer, size).to_vec() }
    }
}

pub fn blob_text(blob: &Com) -> String {
    String::from_utf8_lossy(&blob_bytes(blob)).trim_end_matches('\0').trim().to_string()
}

/// Copies `bytes` into a mapped upload buffer at `offset`.
pub fn write_buffer(buffer: Object, offset: usize, bytes: &[u8]) -> Result<(), String> {
    let mut data: *mut c_void = ptr::null_mut();
    let nothing_read = Range { begin: 0, end: 0 };
    // SAFETY: the buffer is a live upload buffer of at least `offset +
    // bytes.len()` bytes; it is unmapped before returning.
    unsafe {
        check(com!(buffer, slot::resource::MAP, fn(u32, *const Range, *mut *mut c_void) -> Hresult, 0, &nothing_read, &mut data), "ID3D12Resource::Map (upload)")?;
        ptr::copy_nonoverlapping(bytes.as_ptr(), (data as *mut u8).add(offset), bytes.len());
        com!(buffer, slot::resource::UNMAP, fn(u32, *const Range) -> (), 0, ptr::null());
    }
    Ok(())
}

/// The first `size` bytes of a read-back buffer whose writes have completed.
pub fn read_buffer(buffer: Object, size: u64) -> Result<Vec<u8>, String> {
    let mut data: *mut c_void = ptr::null_mut();
    let read = Range { begin: 0, end: size as usize };
    let nothing_written = Range { begin: 0, end: 0 };
    // SAFETY: the buffer is a live read-back buffer of at least `size` bytes
    // and the device's writes to it have completed; it is unmapped before
    // returning.
    unsafe {
        check(com!(buffer, slot::resource::MAP, fn(u32, *const Range, *mut *mut c_void) -> Hresult, 0, &read, &mut data), "ID3D12Resource::Map (read-back)")?;
        let bytes = std::slice::from_raw_parts(data as *const u8, size as usize).to_vec();
        com!(buffer, slot::resource::UNMAP, fn(u32, *const Range) -> (), 0, &nothing_written);
        Ok(bytes)
    }
}

pub fn gpu_address(resource: Object) -> u64 {
    // SAFETY: the resource is a live buffer.
    unsafe { com!(resource, slot::resource::GET_GPU_VIRTUAL_ADDRESS, fn() -> u64) }
}

fn copy_buffer_to_texture(list: Object, texture: Object, subresource: u32, buffer: Object, footprint: PlacedFootprint) {
    let destination = TextureCopyLocation { resource: texture, kind: TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, data: CopyLocationData { subresource } };
    let source = TextureCopyLocation { resource: buffer, kind: TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, data: CopyLocationData { footprint } };
    // SAFETY: the list is recording; both locations name live resources.
    unsafe { com!(list, slot::list::COPY_TEXTURE_REGION, fn(*const TextureCopyLocation, u32, u32, u32, *const TextureCopyLocation, *const c_void) -> (), &destination, 0, 0, 0, &source, ptr::null()) };
}

/// Records the copy of subresource 0 of `texture` to its read-back buffer.
pub fn copy_texture_to_read_back(list: Object, texture: Object, read_back: &ReadBack) {
    let destination = TextureCopyLocation { resource: read_back.buffer.raw(), kind: TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, data: CopyLocationData { footprint: read_back.footprint } };
    let source = TextureCopyLocation { resource: texture, kind: TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, data: CopyLocationData { subresource: 0 } };
    // SAFETY: the list is recording; both locations name live resources.
    unsafe { com!(list, slot::list::COPY_TEXTURE_REGION, fn(*const TextureCopyLocation, u32, u32, u32, *const TextureCopyLocation, *const c_void) -> (), &destination, 0, 0, 0, &source, ptr::null()) };
}

/// The tight rows of a read-back texture of `bytes_per_texel`.
pub fn tight_rows(read_back: &ReadBack, bytes_per_texel: u32) -> Result<Vec<u8>, String> {
    let bytes = read_buffer(read_back.buffer.raw(), read_back.size)?;
    let footprint = read_back.footprint;
    let tight = (footprint.width * bytes_per_texel) as usize;
    let mut rows = Vec::with_capacity(tight * footprint.height as usize);
    for row in 0..footprint.height as usize {
        let start = footprint.offset as usize + row * footprint.row_pitch as usize;
        rows.extend_from_slice(&bytes[start..start + tight]);
    }
    Ok(rows)
}

/// A pipeline description with every field zero or off, blending off for
/// every target, the full sample mask, solid fill with no culling and depth
/// clipping on, depth and stencil off, and triangles.
pub fn default_graphics_desc(root_signature: Object, vertex: &[u8], pixel: &[u8]) -> GraphicsPipelineDesc {
    // SAFETY: every field of the description is an integer, a float or a
    // pointer, for which zero is a valid value.
    let mut desc: GraphicsPipelineDesc = unsafe { std::mem::zeroed() };
    desc.root_signature = root_signature;
    desc.vertex_shader = bytecode(vertex);
    desc.pixel_shader = bytecode(pixel);
    let opaque = RenderTargetBlendDesc {
        blend_enable: 0,
        logic_op_enable: 0,
        source_blend: BLEND_ONE,
        destination_blend: BLEND_ZERO,
        blend_operation: BLEND_OP_ADD,
        source_blend_alpha: BLEND_ONE,
        destination_blend_alpha: BLEND_ZERO,
        blend_operation_alpha: BLEND_OP_ADD,
        logic_op: LOGIC_OP_NOOP,
        write_mask: COLOR_WRITE_ENABLE_ALL,
    };
    desc.blend.targets = [opaque; 8];
    desc.sample_mask = u32::MAX;
    desc.rasterizer = RasterizerDesc {
        fill_mode: FILL_MODE_SOLID,
        cull_mode: CULL_MODE_NONE,
        front_counter_clockwise: 0,
        depth_bias: 0,
        depth_bias_clamp: 0.0,
        slope_scaled_depth_bias: 0.0,
        depth_clip_enable: 1,
        multisample_enable: 0,
        antialiased_line_enable: 0,
        forced_sample_count: 0,
        conservative_raster: 0,
    };
    let keep = || DepthStencilOpDesc { fail: STENCIL_OP_KEEP, depth_fail: STENCIL_OP_KEEP, pass: STENCIL_OP_KEEP, function: COMPARISON_ALWAYS };
    desc.depth_stencil = DepthStencilDesc {
        depth_enable: 0,
        depth_write_mask: DEPTH_WRITE_MASK_ZERO,
        depth_function: COMPARISON_LESS_EQUAL,
        stencil_enable: 0,
        stencil_read_mask: 0xff,
        stencil_write_mask: 0xff,
        front_face: keep(),
        back_face: keep(),
    };
    desc.primitive_topology_type = PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    desc.sample = ONE_SAMPLE;
    desc
}

/// Sets the render target formats of a pipeline description.
pub fn set_targets(desc: &mut GraphicsPipelineDesc, formats: &[u32]) {
    desc.render_target_count = formats.len() as u32;
    desc.render_target_formats[..formats.len()].copy_from_slice(formats);
}

/// Turns on the depth test (less or equal) with or without depth writes.
pub fn set_depth(desc: &mut GraphicsPipelineDesc, write: bool) {
    desc.depth_stencil.depth_enable = 1;
    desc.depth_stencil.depth_write_mask = if write { DEPTH_WRITE_MASK_ALL } else { DEPTH_WRITE_MASK_ZERO };
    desc.depth_stencil_format = FORMAT_D32_FLOAT;
}

pub fn input_element(name: &std::ffi::CStr, format: u32, offset: u32) -> InputElementDesc {
    InputElementDesc { semantic_name: name.as_ptr(), semantic_index: 0, format, input_slot: 0, aligned_byte_offset: offset, input_slot_class: INPUT_PER_VERTEX_DATA, instance_data_step_rate: 0 }
}

/// Sets the viewport and the scissor to a rectangle of the target.
pub fn set_viewport(list: Object, left: u32, width: u32, height: u32) {
    let viewport = Viewport { top_left_x: left as f32, top_left_y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 };
    let scissor = Rect { left: left as i32, top: 0, right: (left + width) as i32, bottom: height as i32 };
    // SAFETY: the list is recording; the structures are live for the calls.
    unsafe {
        com!(list, slot::list::RS_SET_VIEWPORTS, fn(u32, *const Viewport) -> (), 1, &viewport);
        com!(list, slot::list::RS_SET_SCISSOR_RECTS, fn(u32, *const Rect) -> (), 1, &scissor);
    }
}

pub fn set_triangle_list(list: Object) {
    // SAFETY: the list is recording.
    unsafe { com!(list, slot::list::IA_SET_PRIMITIVE_TOPOLOGY, fn(u32) -> (), PRIMITIVE_TOPOLOGY_TRIANGLELIST) };
}

/// Binds a vertex buffer of triangles.
pub fn set_vertex_buffer(list: Object, buffer: Object, size: u32, stride: u32) {
    set_triangle_list(list);
    let view = VertexBufferView { location: gpu_address(buffer), size, stride };
    // SAFETY: the list is recording; the view names a live buffer.
    unsafe { com!(list, slot::list::IA_SET_VERTEX_BUFFERS, fn(u32, u32, *const VertexBufferView) -> (), 0, 1, &view) };
}

pub fn set_render_targets(list: Object, targets: &[CpuHandle], depth: Option<CpuHandle>) {
    let depth = depth.as_ref().map_or(ptr::null(), |handle| handle as *const CpuHandle);
    // SAFETY: the list is recording; the handles are slots of live heaps.
    unsafe { com!(list, slot::list::OM_SET_RENDER_TARGETS, fn(u32, *const CpuHandle, i32, *const CpuHandle) -> (), targets.len() as u32, targets.as_ptr(), 0, depth) };
}

pub fn clear_target(list: Object, target: CpuHandle, color: [f32; 4]) {
    // SAFETY: the list is recording; the handle is a slot of a live heap.
    unsafe { com!(list, slot::list::CLEAR_RENDER_TARGET_VIEW, fn(CpuHandle, *const f32, u32, *const Rect) -> (), target, color.as_ptr(), 0, ptr::null()) };
}

pub fn clear_depth(list: Object, depth: CpuHandle, value: f32) {
    // SAFETY: the list is recording; the handle is a slot of a live heap.
    unsafe { com!(list, slot::list::CLEAR_DEPTH_STENCIL_VIEW, fn(CpuHandle, u32, f32, u8, u32, *const Rect) -> (), depth, CLEAR_FLAG_DEPTH, value, 0, 0, ptr::null()) };
}

pub fn set_pipeline(list: Object, pipeline: Object) {
    // SAFETY: the list is recording; the pipeline state is live.
    unsafe { com!(list, slot::list::SET_PIPELINE_STATE, fn(Object) -> (), pipeline) };
}

pub fn set_graphics_root_signature(list: Object, signature: Object) {
    // SAFETY: the list is recording; the root signature is live.
    unsafe { com!(list, slot::list::SET_GRAPHICS_ROOT_SIGNATURE, fn(Object) -> (), signature) };
}

pub fn set_descriptor_heaps(list: Object, heaps: &[Object]) {
    // SAFETY: the list is recording; the heaps are live and shader visible.
    unsafe { com!(list, slot::list::SET_DESCRIPTOR_HEAPS, fn(u32, *const Object) -> (), heaps.len() as u32, heaps.as_ptr()) };
}

pub fn set_graphics_table(list: Object, parameter: u32, handle: u64) {
    // SAFETY: the list is recording; the handle is a slot of the bound heap.
    unsafe { com!(list, slot::list::SET_GRAPHICS_ROOT_DESCRIPTOR_TABLE, fn(u32, u64) -> (), parameter, handle) };
}

pub fn draw(list: Object, vertex_count: u32, first_vertex: u32) {
    // SAFETY: the list is recording with a complete pipeline bound.
    unsafe { com!(list, slot::list::DRAW_INSTANCED, fn(u32, u32, u32, u32) -> (), vertex_count, 1, first_vertex, 0) };
}

/// A clear value of a color format.
pub fn clear_color(format: u32, values: [f32; 4]) -> ClearValue {
    ClearValue { format, values }
}

/// The clear value of a D32_FLOAT depth target.
pub fn clear_depth_value(depth: f32) -> ClearValue {
    ClearValue { format: FORMAT_D32_FLOAT, values: [depth, 0.0, 0.0, 0.0] }
}
