//! The Direct3D 12 debug layer: enabling it before the device is created,
//! and collecting its messages through the device's info queue, by the
//! callback of the newer info queue interface where it exists and by polling
//! the stored messages otherwise. Throwaway, never product code.
//!
//! The debug layer ships with the Graphics Tools optional component of
//! Windows; without it `D3D12GetDebugInterface` fails and the spike reports
//! that no layer could be used.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::Mutex;

use super::ffi::*;

/// Enables the debug layer for the devices this process creates from now on.
pub fn enable_layer(get_debug_interface: GetDebugInterface) -> Result<(), String> {
    let mut debug: Object = ptr::null_mut();
    // SAFETY: the function is D3D12GetDebugInterface; it writes an
    // ID3D12Debug owned below.
    let result = unsafe { get_debug_interface(&IID_DEBUG, &mut debug) };
    let debug = created(result, debug, "D3D12GetDebugInterface(ID3D12Debug), which needs the Graphics Tools optional component of Windows,")?;
    // SAFETY: the debug interface is live; EnableDebugLayer takes no argument.
    unsafe { com!(debug.raw(), slot::debug::ENABLE_DEBUG_LAYER, fn() -> ()) };
    Ok(())
}

fn severity_name(severity: u32) -> &'static str {
    match severity {
        0 => "corruption",
        1 => "error",
        2 => "warning",
        3 => "info",
        _ => "message",
    }
}

fn message_text(severity: u32, id: u32, description: &str) -> String {
    format!("{} {}: {}", severity_name(severity), id, description.trim())
}

/// Where the messages are collected, and which are left out.
struct Sink {
    /// The identifiers of the messages that are not collected.
    left_out: &'static [u32],
    messages: Mutex<Vec<String>>,
}

/// The callback the info queue calls for each message, on any thread.
unsafe extern "system" fn on_message(_category: u32, severity: u32, id: u32, description: *const c_char, context: *mut c_void) {
    if context.is_null() || description.is_null() {
        return;
    }
    // SAFETY: the context is the collector's boxed sink, which outlives the
    // registration (the callback is unregistered before the box is freed);
    // the description is a NUL-terminated string for the call's duration.
    let (sink, text) = unsafe { (&*(context as *const Sink), CStr::from_ptr(description).to_string_lossy()) };
    if sink.left_out.contains(&id) {
        return;
    }
    if let Ok(mut messages) = sink.messages.lock() {
        messages.push(message_text(severity, id, &text));
    }
}

enum Delivery {
    /// ID3D12InfoQueue1::RegisterMessageCallback, with the cookie it returned.
    Callback(u32),
    /// ID3D12InfoQueue::GetMessage over the stored messages.
    Polling,
}

/// The messages of the debug layer for one device.
pub struct Collector {
    delivery: Delivery,
    /// Read by the callback through a raw pointer: boxed so that it never moves.
    sink: Box<Sink>,
    queue: Com,
}

impl Collector {
    /// Attaches to the info queue of `device`, created under the layer. The
    /// messages whose identifier is in `left_out` are not collected.
    pub fn attach(device: Object, left_out: &'static [u32]) -> Result<Collector, String> {
        let sink = Box::new(Sink { left_out, messages: Mutex::new(Vec::new()) });
        let mut newer: Object = ptr::null_mut();
        // SAFETY: the device is live; QueryInterface writes an owned reference.
        let result = unsafe { com!(device, slot::QUERY_INTERFACE, fn(*const Guid, *mut Object) -> Hresult, &IID_INFO_QUEUE1, &mut newer) };
        if let Ok(queue) = created(result, newer, "QueryInterface(ID3D12InfoQueue1)") {
            let mut cookie = 0_u32;
            let context = &*sink as *const Sink as *mut c_void;
            let callback: MessageCallback = on_message;
            // SAFETY: the queue is a live ID3D12InfoQueue1; the context stays
            // valid until the callback is unregistered in `drop`.
            let result = unsafe { com!(queue.raw(), slot::info_queue::REGISTER_MESSAGE_CALLBACK, fn(MessageCallback, u32, *mut c_void, *mut u32) -> Hresult, callback, 0, context, &mut cookie) };
            if result >= 0 {
                return Ok(Collector { delivery: Delivery::Callback(cookie), sink, queue });
            }
        }
        let mut older: Object = ptr::null_mut();
        // SAFETY: as above.
        let result = unsafe { com!(device, slot::QUERY_INTERFACE, fn(*const Guid, *mut Object) -> Hresult, &IID_INFO_QUEUE, &mut older) };
        let queue = created(result, older, "QueryInterface(ID3D12InfoQueue) on a device created under the debug layer")?;
        Ok(Collector { delivery: Delivery::Polling, sink, queue })
    }

    pub fn delivery(&self) -> String {
        match self.delivery {
            Delivery::Callback(_) => "a callback registered with ID3D12InfoQueue1::RegisterMessageCallback, called by the layer for each message".to_string(),
            Delivery::Polling => "polling: ID3D12InfoQueue1 or its callback registration was not available, so the stored messages are read with ID3D12InfoQueue::GetMessage and cleared".to_string(),
        }
    }

    /// The messages delivered since the last call.
    pub fn drain(&mut self) -> Vec<String> {
        if let Delivery::Polling = self.delivery {
            let stored = self.stored_messages();
            if let Ok(mut messages) = self.sink.messages.lock() {
                messages.extend(stored);
            }
        }
        match self.sink.messages.lock() {
            Ok(mut messages) => std::mem::take(&mut *messages),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }

    /// Reads and clears the messages the info queue stored.
    fn stored_messages(&self) -> Vec<String> {
        let queue = self.queue.raw();
        // SAFETY: the queue is a live ID3D12InfoQueue.
        let count = unsafe { com!(queue, slot::info_queue::GET_NUM_STORED_MESSAGES_ALLOWED_BY_RETRIEVAL_FILTER, fn() -> u64) };
        let mut messages = Vec::new();
        for index in 0..count {
            let mut length = 0_usize;
            // SAFETY: a null message asks for the length the message needs.
            let result = unsafe { com!(queue, slot::info_queue::GET_MESSAGE, fn(u64, *mut Message, *mut usize) -> Hresult, index, ptr::null_mut(), &mut length) };
            if result < 0 || length < std::mem::size_of::<Message>() {
                messages.push(format!("ID3D12InfoQueue::GetMessage({index}) for the length failed with HRESULT {:#010x}", result as u32));
                continue;
            }
            // Eight-byte words keep the structure at the start aligned.
            let mut storage = vec![0_u64; length.div_ceil(8)];
            let message = storage.as_mut_ptr() as *mut Message;
            // SAFETY: the storage holds `length` bytes, aligned for Message;
            // the description the call writes points inside it.
            let text = unsafe {
                let result = com!(queue, slot::info_queue::GET_MESSAGE, fn(u64, *mut Message, *mut usize) -> Hresult, index, message, &mut length);
                if result < 0 || (*message).description.is_null() {
                    Some(format!("ID3D12InfoQueue::GetMessage({index}) failed with HRESULT {:#010x}", result as u32))
                } else if self.sink.left_out.contains(&(*message).id) {
                    None
                } else {
                    Some(message_text((*message).severity, (*message).id, &CStr::from_ptr((*message).description).to_string_lossy()))
                }
            };
            messages.extend(text);
        }
        // SAFETY: the queue is live; ClearStoredMessages takes no argument.
        unsafe { com!(queue, slot::info_queue::CLEAR_STORED_MESSAGES, fn() -> ()) };
        messages
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        if let Delivery::Callback(cookie) = self.delivery {
            // SAFETY: the queue is the live ID3D12InfoQueue1 the callback was
            // registered with; after this call the sink is no longer used.
            // A failure leaves nothing to do but release.
            let _ = unsafe { com!(self.queue.raw(), slot::info_queue::UNREGISTER_MESSAGE_CALLBACK, fn(u32) -> Hresult, cookie) };
        }
    }
}

/// One invalid call the device survives: a committed texture of width zero,
/// which the runtime refuses with an error code and the layer reports.
pub fn misuse(device: Object) -> String {
    let properties = HeapProperties { kind: HEAP_TYPE_DEFAULT, cpu_page_property: 0, memory_pool_preference: 0, creation_node_mask: 1, visible_node_mask: 1 };
    let desc = ResourceDesc {
        dimension: RESOURCE_DIMENSION_TEXTURE2D,
        alignment: 0,
        width: 0,
        height: 16,
        depth_or_array_size: 1,
        mip_levels: 1,
        format: FORMAT_R8G8B8A8_UNORM,
        sample: SampleDesc { count: 1, quality: 0 },
        layout: TEXTURE_LAYOUT_UNKNOWN,
        flags: RESOURCE_FLAG_NONE,
    };
    let mut resource: Object = ptr::null_mut();
    // SAFETY: the device is live; the description is complete (its width of
    // zero is the deliberate error); any object written is released below.
    let result = unsafe {
        com!(device, slot::device::CREATE_COMMITTED_RESOURCE, fn(*const HeapProperties, u32, *const ResourceDesc, u32, *const ClearValue, *const Guid, *mut Object) -> Hresult, &properties, 0, &desc, STATE_COMMON, ptr::null(), &IID_RESOURCE, &mut resource)
    };
    drop(Com::from_raw(resource));
    format!("ID3D12Device::CreateCommittedResource of a two-dimensional texture of width 0 returned HRESULT {:#010x}", result as u32)
}
