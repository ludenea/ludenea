//! The render device port at the spike's size, and the device that draws
//! nothing. Throwaway, never product code.
//!
//! The port is what the renderer speaks to: resources named by typed
//! generational handles and created from descriptions, passes recorded in
//! parts, submissions, read-backs. Behind it stands one adapter of
//! `render_device_baseline::frames::FrameDevice`: the Vulkan, Direct3D 12 or
//! Metal adapter of the device baseline library, or the content device of
//! this file, which accepts every call and executes none.
//!
//! What the port adds to an adapter, in every build:
//!
//! - a handle's generation is checked on every use, and a stale handle is refused;
//! - a released resource is queued with the highest frame number declared so
//!   far and destroyed when every frame up to that number is finished or dropped;
//! - no command may name a resource queued for destruction, and no resource
//!   may be destroyed while an unfinished frame names it: both are assertions
//!   that are counted and reported, never a panic, so that a measurement can
//!   trip them on purpose;
//! - device memory is counted per owner and per class from the descriptions;
//! - a loss is simulated here: the port answers "lost" to every call until
//!   the device is created again, and every handle of before is stale.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use render_device_baseline::backend::{DeviceInfo, DeviceKind};
use render_device_baseline::frames::{DrawConstants, FrameDevice, FrameDraw, PassDescription, Resource, ShaderPayload, TextureSource};
use render_device_baseline::scene::{Image, Texture};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Texture,
    VertexBuffer,
    Target,
    Pipelines,
}

/// A resource of the device as the renderer names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Handle {
    pub kind: Kind,
    index: u32,
    generation: u32,
}

/// The classes device memory is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryClass {
    Textures,
    Geometry,
    Targets,
    Transient,
    Pipelines,
}

/// Who a resource is counted for: the instance, or one world.
pub type Owner = Option<u32>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortError {
    /// The device is lost: the call had no effect.
    Lost,
    /// The handle's generation is not the resource's: refused.
    Stale(Handle),
    /// A command named a resource queued for destruction: the port's assertion.
    QueuedForDestruction(Handle),
    Device(String),
}

impl std::fmt::Display for PortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PortError::Lost => write!(f, "the device is lost"),
            PortError::Stale(handle) => write!(f, "a stale handle was refused: {handle:?}"),
            PortError::QueuedForDestruction(handle) => write!(f, "a command names a resource queued for destruction: {handle:?}"),
            PortError::Device(reason) => write!(f, "{reason}"),
        }
    }
}

pub type PortResult<T> = Result<T, PortError>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Free,
    Live,
    /// Released: destroyed once every frame up to this number is finished or dropped.
    Queued(u64),
}

struct Slot {
    generation: u32,
    kind: Kind,
    native: u64,
    state: SlotState,
    bytes: u64,
    owner: Owner,
    class: MemoryClass,
}

#[derive(Default)]
struct Ledger {
    slots: Vec<Slot>,
    free: Vec<u32>,
    /// The highest frame number declared so far.
    declared: u64,
    /// The frames declared and neither finished by the device nor dropped.
    unfinished: BTreeSet<u64>,
    /// What each unfinished frame names.
    names: BTreeMap<u64, BTreeSet<Handle>>,
    memory: BTreeMap<(Owner, MemoryClass), u64>,
    peak: BTreeMap<(Owner, MemoryClass), u64>,
}

/// One draw as the renderer records it.
#[derive(Clone, Copy, Debug)]
pub struct PortDraw {
    pub buffer: Handle,
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub blended: bool,
    pub constants: DrawConstants,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Texture(Handle),
    Target(Handle),
}

#[derive(Clone, Debug)]
pub struct PassSpec {
    pub target: Handle,
    pub clear: [f32; 4],
    pub pipelines: Handle,
    pub textures: [Source; 5],
    pub parts: usize,
}

/// What the port counted since it was created.
#[derive(Clone, Copy, Debug, Default)]
pub struct PortCounters {
    pub stale_refused: u64,
    pub queued_named: u64,
    pub destroyed_under_a_frame: u64,
    pub created: u64,
    pub destroyed: u64,
    pub losses: u64,
}

type Opener = Box<dyn Fn() -> Result<Box<dyn FrameDevice>, String> + Send + Sync>;

pub struct Port {
    device: RwLock<Option<Box<dyn FrameDevice>>>,
    opener: Opener,
    ledger: Mutex<Ledger>,
    lost: AtomicBool,
    stale_refused: AtomicU64,
    queued_named: AtomicU64,
    destroyed_under_a_frame: AtomicU64,
    created: AtomicU64,
    destroyed: AtomicU64,
    losses: AtomicU64,
}

fn native_resource(kind: Kind, native: u64) -> Resource {
    match kind {
        Kind::Texture => Resource::Texture(native),
        Kind::VertexBuffer => Resource::VertexBuffer(native),
        Kind::Target => Resource::Target(native),
        Kind::Pipelines => Resource::Pipelines(native),
    }
}

/// The bytes a target takes: color and depth at 4 samples, and the resolved image.
pub fn target_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * (4 * 4 + 4 * 4 + 4)
}

impl Port {
    /// Opens the device through `opener`, which a recovery calls again.
    pub fn open(opener: impl Fn() -> Result<Box<dyn FrameDevice>, String> + Send + Sync + 'static) -> Result<Port, String> {
        let device = opener()?;
        Ok(Port {
            device: RwLock::new(Some(device)),
            opener: Box::new(opener),
            ledger: Mutex::new(Ledger::default()),
            lost: AtomicBool::new(false),
            stale_refused: AtomicU64::new(0),
            queued_named: AtomicU64::new(0),
            destroyed_under_a_frame: AtomicU64::new(0),
            created: AtomicU64::new(0),
            destroyed: AtomicU64::new(0),
            losses: AtomicU64::new(0),
        })
    }

    fn with<T>(&self, call: impl FnOnce(&dyn FrameDevice) -> Result<T, String>) -> PortResult<T> {
        if self.lost.load(Ordering::Acquire) {
            return Err(PortError::Lost);
        }
        match self.device.read().unwrap().as_deref() {
            Some(device) => call(device).map_err(PortError::Device),
            None => Err(PortError::Lost),
        }
    }

    pub fn settings(&self) -> PortResult<String> {
        self.with(|device| Ok(device.settings()))
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    pub fn counters(&self) -> PortCounters {
        PortCounters {
            stale_refused: self.stale_refused.load(Ordering::Relaxed),
            queued_named: self.queued_named.load(Ordering::Relaxed),
            destroyed_under_a_frame: self.destroyed_under_a_frame.load(Ordering::Relaxed),
            created: self.created.load(Ordering::Relaxed),
            destroyed: self.destroyed.load(Ordering::Relaxed),
            losses: self.losses.load(Ordering::Relaxed),
        }
    }

    fn register(&self, kind: Kind, native: u64, bytes: u64, owner: Owner, class: MemoryClass) -> Handle {
        let mut ledger = self.ledger.lock().unwrap();
        let index = match ledger.free.pop() {
            Some(index) => index,
            None => {
                ledger.slots.push(Slot { generation: 0, kind, native: 0, state: SlotState::Free, bytes: 0, owner, class });
                ledger.slots.len() as u32 - 1
            }
        };
        let slot = &mut ledger.slots[index as usize];
        slot.generation += 1;
        (slot.kind, slot.native, slot.state, slot.bytes, slot.owner, slot.class) = (kind, native, SlotState::Live, bytes, owner, class);
        let generation = slot.generation;
        let total = ledger.memory.entry((owner, class)).or_insert(0);
        *total += bytes;
        let total = *total;
        let peak = ledger.peak.entry((owner, class)).or_insert(0);
        *peak = (*peak).max(total);
        self.created.fetch_add(1, Ordering::Relaxed);
        Handle { kind, index, generation }
    }

    pub fn import_shader(&self, variant: u32) -> PortResult<ShaderPayload> {
        self.with(|device| device.import_shader(variant))
    }

    pub fn create_pipelines(&self, payload: &ShaderPayload) -> PortResult<Handle> {
        let native = self.with(|device| device.create_pipelines(payload))?;
        Ok(self.register(Kind::Pipelines, native, (payload.vertex.len() + payload.fragment.len()) as u64, None, MemoryClass::Pipelines))
    }

    pub fn create_texture(&self, texture: &Texture) -> PortResult<Handle> {
        let native = self.with(|device| device.create_texture(texture))?;
        Ok(self.register(Kind::Texture, native, texture.levels.iter().map(|level| level.len() as u64).sum(), None, MemoryClass::Textures))
    }

    pub fn create_vertex_buffer(&self, bytes: &[u8], owner: Owner) -> PortResult<Handle> {
        let native = self.with(|device| device.create_vertex_buffer(bytes))?;
        Ok(self.register(Kind::VertexBuffer, native, bytes.len() as u64, owner, MemoryClass::Geometry))
    }

    pub fn create_target(&self, width: u32, height: u32, owner: Owner, class: MemoryClass) -> PortResult<Handle> {
        let native = self.with(|device| device.create_target(width, height))?;
        Ok(self.register(Kind::Target, native, target_bytes(width, height), owner, class))
    }

    /// Numbers a frame, across the instance, when it is declared.
    pub fn declare_frame(&self) -> u64 {
        let mut ledger = self.ledger.lock().unwrap();
        ledger.declared += 1;
        let number = ledger.declared;
        ledger.unfinished.insert(number);
        number
    }

    /// The frame was finished by the device, or dropped: what waited for it
    /// is destroyed.
    pub fn frame_done(&self, frame: u64) {
        let mut ledger = self.ledger.lock().unwrap();
        ledger.unfinished.remove(&frame);
        ledger.names.remove(&frame);
        self.collect(&mut ledger);
    }

    /// Destroys every queued resource whose frames are all finished or dropped.
    fn collect(&self, ledger: &mut Ledger) {
        let oldest_unfinished = ledger.unfinished.first().copied().unwrap_or(u64::MAX);
        let device = self.device.read().unwrap();
        for index in 0..ledger.slots.len() {
            if let SlotState::Queued(after) = ledger.slots[index].state
                && after < oldest_unfinished
            {
                let (kind, native) = (ledger.slots[index].kind, ledger.slots[index].native);
                if let Some(device) = device.as_deref() {
                    device.destroy(native_resource(kind, native));
                }
                self.free_slot(ledger, index);
            }
        }
    }

    fn free_slot(&self, ledger: &mut Ledger, index: usize) {
        let slot = &mut ledger.slots[index];
        slot.state = SlotState::Free;
        let (key, bytes) = ((slot.owner, slot.class), slot.bytes);
        *ledger.memory.entry(key).or_insert(0) -= bytes;
        ledger.free.push(index as u32);
        self.destroyed.fetch_add(1, Ordering::Relaxed);
    }

    fn slot_of<'a>(&self, ledger: &'a Ledger, handle: Handle) -> PortResult<&'a Slot> {
        match ledger.slots.get(handle.index as usize) {
            Some(slot) if slot.generation == handle.generation && slot.kind == handle.kind && slot.state != SlotState::Free => Ok(slot),
            _ => {
                self.stale_refused.fetch_add(1, Ordering::Relaxed);
                Err(PortError::Stale(handle))
            }
        }
    }

    /// Releases a resource: it is destroyed after every frame declared so far.
    pub fn release(&self, handle: Handle) -> PortResult<()> {
        let mut ledger = self.ledger.lock().unwrap();
        self.slot_of(&ledger, handle)?;
        let declared = ledger.declared;
        ledger.slots[handle.index as usize].state = SlotState::Queued(declared);
        self.collect(&mut ledger);
        Ok(())
    }

    /// Destroys a resource at once, whatever frames name it: the wrongly
    /// ordered release a measurement commits on purpose. Returns whether the
    /// port's assertion tripped, an unfinished frame naming the resource.
    pub fn destroy_now(&self, handle: Handle) -> PortResult<bool> {
        let mut ledger = self.ledger.lock().unwrap();
        let (kind, native) = {
            let slot = self.slot_of(&ledger, handle)?;
            (slot.kind, slot.native)
        };
        let named = ledger.names.values().any(|names| names.contains(&handle));
        if named {
            self.destroyed_under_a_frame.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(device) = self.device.read().unwrap().as_deref() {
            device.destroy(native_resource(kind, native));
        }
        self.free_slot(&mut ledger, handle.index as usize);
        Ok(named)
    }

    /// The adapter's identifier of a resource a command of `frame` names.
    fn resolve(&self, ledger: &mut Ledger, frame: u64, handle: Handle) -> PortResult<u64> {
        let slot = self.slot_of(ledger, handle)?;
        if matches!(slot.state, SlotState::Queued(_)) {
            self.queued_named.fetch_add(1, Ordering::Relaxed);
            return Err(PortError::QueuedForDestruction(handle));
        }
        let native = slot.native;
        ledger.names.entry(frame).or_default().insert(handle);
        Ok(native)
    }

    /// Whether a handle still names a live resource.
    pub fn is_live(&self, handle: Handle) -> bool {
        let ledger = self.ledger.lock().unwrap();
        matches!(ledger.slots.get(handle.index as usize), Some(slot) if slot.generation == handle.generation && slot.state == SlotState::Live)
    }

    pub fn open_pass(&self, frame: u64, pass: &PassSpec) -> PortResult<u64> {
        let description = {
            let mut ledger = self.ledger.lock().unwrap();
            let mut textures = [TextureSource::Texture(0); 5];
            for (native, source) in textures.iter_mut().zip(pass.textures) {
                *native = match source {
                    Source::Texture(handle) => TextureSource::Texture(self.resolve(&mut ledger, frame, handle)?),
                    Source::Target(handle) => TextureSource::Target(self.resolve(&mut ledger, frame, handle)?),
                };
            }
            PassDescription { target: self.resolve(&mut ledger, frame, pass.target)?, clear: pass.clear, pipelines: self.resolve(&mut ledger, frame, pass.pipelines)?, textures, parts: pass.parts }
        };
        self.with(|device| device.open_pass(&description))
    }

    pub fn record_part(&self, frame: u64, pass: u64, part: usize, draws: &[PortDraw]) -> PortResult<()> {
        thread_local! {
            /// The part a thread is recording, as the adapter takes it.
            static NATIVE: std::cell::RefCell<Vec<FrameDraw>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        NATIVE.with_borrow_mut(|native_draws| {
            native_draws.clear();
            let mut last: Option<(Handle, u64)> = None;
            for draw in draws {
                let vertex_buffer = match last {
                    Some((handle, native)) if handle == draw.buffer => native,
                    _ => {
                        let native = self.resolve(&mut self.ledger.lock().unwrap(), frame, draw.buffer)?;
                        last = Some((draw.buffer, native));
                        native
                    }
                };
                native_draws.push(FrameDraw { vertex_buffer, first_vertex: draw.first_vertex, vertex_count: draw.vertex_count, blended: draw.blended, constants: draw.constants });
            }
            self.with(|device| device.record_part(pass, part, native_draws))
        })
    }

    pub fn close_pass(&self, pass: u64) -> PortResult<()> {
        self.with(|device| device.close_pass(pass))
    }

    pub fn discard_pass(&self, pass: u64) {
        let _ = self.with(|device| {
            device.discard_pass(pass);
            Ok(())
        });
    }

    pub fn submit(&self, frame: u64, passes: &[u64], read_back: &[Handle]) -> PortResult<u64> {
        let targets = {
            let mut ledger = self.ledger.lock().unwrap();
            read_back.iter().map(|&handle| self.resolve(&mut ledger, frame, handle)).collect::<PortResult<Vec<u64>>>()?
        };
        self.with(|device| device.submit(passes, &targets))
    }

    pub fn finished(&self, submission: u64) -> PortResult<bool> {
        self.with(|device| device.finished(submission))
    }

    pub fn take_images(&self, submission: u64) -> PortResult<Vec<Image>> {
        self.with(|device| device.take_images(submission))
    }

    pub fn wait_idle(&self) -> PortResult<()> {
        self.with(|device| device.wait_idle())
    }

    /// Asks the interface behind the port to remove its device for real.
    pub fn remove_device(&self) -> PortResult<String> {
        self.with(|device| device.remove_device())
    }

    /// What the interface says of a removed device, if it says anything.
    pub fn removed_reason(&self) -> Option<String> {
        self.device.read().unwrap().as_deref().and_then(|device| device.removed_reason())
    }

    pub fn validation_delivery(&self) -> PortResult<String> {
        self.with(|device| Ok(device.validation_delivery()))
    }

    pub fn validation_messages(&self) -> PortResult<Vec<String>> {
        self.with(|device| Ok(device.validation_messages()))
    }

    /// The bytes counted now and at their peak, per owner and class.
    pub fn memory(&self) -> Vec<(Owner, MemoryClass, u64, u64)> {
        let ledger = self.ledger.lock().unwrap();
        ledger.peak.iter().map(|(&(owner, class), &peak)| (owner, class, ledger.memory.get(&(owner, class)).copied().unwrap_or(0), peak)).collect()
    }

    /// Answers "lost" from now on, as an adapter whose device was removed
    /// would: the device behind the port is dropped and every handle is stale.
    pub fn simulate_loss(&self) {
        self.lost.store(true, Ordering::Release);
        self.losses.fetch_add(1, Ordering::Relaxed);
        let device = self.device.write().unwrap().take();
        // The adapter waits for its device to be idle and destroys what is left.
        drop(device);
        let mut ledger = self.ledger.lock().unwrap();
        for index in 0..ledger.slots.len() {
            if ledger.slots[index].state != SlotState::Free {
                self.free_slot(&mut ledger, index);
            }
        }
        ledger.unfinished.clear();
        ledger.names.clear();
    }

    /// Creates the device again, with the preference it was first asked with.
    pub fn recreate(&self) -> Result<(), String> {
        let device = (self.opener)()?;
        *self.device.write().unwrap() = Some(device);
        self.lost.store(false, Ordering::Release);
        Ok(())
    }
}

/// The device that draws nothing: the port with nothing behind it. It
/// accepts every call, executes none and produces no pixel.
#[derive(Default)]
pub struct ContentDevice {
    next: AtomicU64,
    pub draws_recorded: AtomicU64,
    pub passes_opened: AtomicU64,
    pub submissions: AtomicU64,
}

impl ContentDevice {
    pub fn new() -> ContentDevice {
        ContentDevice::default()
    }

    fn identifier(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }
}

pub const CONTENT_DEVICE_NAME: &str = "content device (draws nothing)";

impl FrameDevice for ContentDevice {
    fn info(&self) -> DeviceInfo {
        DeviceInfo { index: 0, name: CONTENT_DEVICE_NAME.to_string(), kind: DeviceKind::Other, vendor_id: 0, device_id: 0, driver: "the port with nothing behind it".to_string(), interface_version: "none".to_string() }
    }

    fn settings(&self) -> String {
        "none".to_string()
    }

    fn import_shader(&self, variant: u32) -> Result<ShaderPayload, String> {
        Ok(ShaderPayload { vertex: Vec::new(), fragment: vec![variant as u8], form: "none: the content device binds no payload" })
    }

    fn create_pipelines(&self, _payload: &ShaderPayload) -> Result<u64, String> {
        Ok(self.identifier())
    }

    fn create_texture(&self, _texture: &Texture) -> Result<u64, String> {
        Ok(self.identifier())
    }

    fn create_vertex_buffer(&self, _bytes: &[u8]) -> Result<u64, String> {
        Ok(self.identifier())
    }

    fn create_target(&self, _width: u32, _height: u32) -> Result<u64, String> {
        Ok(self.identifier())
    }

    fn destroy(&self, _resource: Resource) {}

    fn open_pass(&self, _pass: &PassDescription) -> Result<u64, String> {
        self.passes_opened.fetch_add(1, Ordering::Relaxed);
        Ok(self.identifier())
    }

    fn record_part(&self, _pass: u64, _part: usize, draws: &[FrameDraw]) -> Result<(), String> {
        self.draws_recorded.fetch_add(draws.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    fn close_pass(&self, _pass: u64) -> Result<(), String> {
        Ok(())
    }

    fn discard_pass(&self, _pass: u64) {}

    fn submit(&self, _passes: &[u64], _read_back: &[u64]) -> Result<u64, String> {
        self.submissions.fetch_add(1, Ordering::Relaxed);
        Ok(self.identifier())
    }

    fn finished(&self, _submission: u64) -> Result<bool, String> {
        Ok(true)
    }

    fn take_images(&self, _submission: u64) -> Result<Vec<Image>, String> {
        Ok(Vec::new())
    }

    fn wait_idle(&self) -> Result<(), String> {
        Ok(())
    }

    fn validation_delivery(&self) -> String {
        "the content device has no validation layer".to_string()
    }

    fn validation_messages(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_port() -> Port {
        Port::open(|| Ok(Box::new(ContentDevice::new()) as Box<dyn FrameDevice>)).unwrap()
    }

    fn spec(target: Handle, pipelines: Handle, texture: Handle) -> PassSpec {
        PassSpec { target, clear: [0.0; 4], pipelines, textures: [Source::Texture(texture); 5], parts: 1 }
    }

    fn resources(port: &Port) -> (Handle, Handle, Handle, Handle) {
        let texture = port.create_texture(&Texture { name: "t", width: 1, height: 1, encoding: render_device_baseline::scene::Encoding::Unorm, levels: vec![vec![0; 4]] }).unwrap();
        let pipelines = port.create_pipelines(&port.import_shader(0).unwrap()).unwrap();
        (port.create_target(8, 8, Some(0), MemoryClass::Targets).unwrap(), pipelines, texture, port.create_vertex_buffer(&[0; 240], Some(0)).unwrap())
    }

    #[test]
    fn a_released_resource_lives_until_the_frames_declared_before_its_release_are_done() {
        let port = content_port();
        let (target, pipelines, texture, buffer) = resources(&port);
        let first = port.declare_frame();
        let second = port.declare_frame();
        port.open_pass(first, &spec(target, pipelines, texture)).unwrap();
        port.release(buffer).unwrap();
        assert_eq!(port.counters().destroyed, 0);
        port.frame_done(first);
        assert_eq!(port.counters().destroyed, 0, "the second frame was declared before the release");
        port.frame_done(second);
        assert_eq!(port.counters().destroyed, 1);
        assert_eq!(port.release(buffer), Err(PortError::Stale(buffer)));
    }

    #[test]
    fn the_assertions_trip_on_a_command_that_names_a_queued_resource_and_on_a_destruction_under_a_frame() {
        let port = content_port();
        let (target, pipelines, texture, buffer) = resources(&port);
        let frame = port.declare_frame();
        let pass = port.open_pass(frame, &spec(target, pipelines, texture)).unwrap();
        let draw = PortDraw { buffer, first_vertex: 0, vertex_count: 6, blended: false, constants: DrawConstants { transform: [0.0; 16], tint: [1.0; 4], texture: 0, sampler: 0 } };
        port.record_part(frame, pass, 0, &[draw]).unwrap();
        assert_eq!(port.destroy_now(buffer), Ok(true));
        let other = port.create_vertex_buffer(&[0; 240], Some(0)).unwrap();
        port.release(other).unwrap();
        let named = port.record_part(frame, pass, 0, &[PortDraw { buffer: other, ..draw }]);
        assert_eq!(named, Err(PortError::QueuedForDestruction(other)));
        let counters = port.counters();
        assert_eq!((counters.destroyed_under_a_frame, counters.queued_named), (1, 1));
    }

    #[test]
    fn a_loss_makes_every_handle_stale_and_every_call_answer_lost_until_the_device_is_created_again() {
        let port = content_port();
        let (target, _, _, buffer) = resources(&port);
        port.simulate_loss();
        assert_eq!(port.create_target(8, 8, None, MemoryClass::Targets), Err(PortError::Lost));
        port.recreate().unwrap();
        assert_eq!(port.release(buffer), Err(PortError::Stale(buffer)));
        assert!(!port.is_live(target));
        assert!(port.memory().iter().all(|&(_, _, now, _)| now == 0));
        assert!(port.create_target(8, 8, None, MemoryClass::Targets).is_ok());
    }
}
