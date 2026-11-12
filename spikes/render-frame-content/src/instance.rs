//! What the worlds of an instance share: the device behind its port, the
//! caches of assets and pipelines, the pool of transient targets, the
//! hand-off, and the one presenter that touches the device's queue.
//! Throwaway, never product code.
//!
//! The presenter is a role. By default a dedicated thread takes it, the
//! presentation thread; a host may instead serve it from a stepping thread
//! (`Instance::serve`), the fallback of a machine with too few cores, in
//! which case submission and read-back follow recording in sequence.
//!
//! For each frame it takes, the presenter: builds what a newly resident
//! shader payload asks and publishes the cache entry; submits the frame's
//! passes in the graph's order with the read-back of its target; polls the
//! completion; hands the image to the world's stream. It takes no further
//! frame while the device has not finished three.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use render_device_baseline::frames::{ShaderPayload, vertex_bytes};
use render_device_baseline::scene::{Image, Scene};

use crate::pool::StageTime;
use crate::port::{Handle, MemoryClass, Port, PortError, PortResult};
use crate::visual::SharedGeometry;

/// The frames the device may hold unfinished before the presenter takes another.
pub const FRAMES_IN_FLIGHT: usize = 3;

/// The caches of the instance, as one frame resolves them when it is
/// declared. An entry is never changed in place: a new payload installs a
/// new value under new handles and releases the old ones.
#[derive(Clone, Debug)]
pub struct Caches {
    pub textures: [Handle; 5],
    pub geometry: Handle,
    pub pipelines: Handle,
    /// Which variant of the shader the pipelines were built from.
    pub shader_variant: u32,
}

/// What each stage of a frame cost.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimes {
    pub refresh: StageTime,
    pub prepare: StageTime,
    pub sort: StageTime,
    pub declare_us: f64,
    pub record: StageTime,
    /// The time the frame waited in the hand-off.
    pub hand_off_us: f64,
    pub submit_us: f64,
    /// From the submission's return to the completion seen by a poll.
    pub device_us: f64,
    pub read_back_us: f64,
    /// From the start of the frame to the delivery of its image.
    pub latency_us: f64,
}

/// What a target's image carries beside its bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stamp {
    pub tick: u32,
    pub factor: f32,
    pub complete: bool,
    pub reproducible: bool,
    /// Whether a declared history fed the image.
    pub history: bool,
    pub shader_variant: u32,
    pub draws: u64,
    pub passes: u64,
}

/// A finished frame, from the end of its recording to its delivery.
pub struct FrameRecord {
    pub world: u32,
    pub number: u64,
    pub passes: Vec<u64>,
    pub target: Handle,
    pub transients: Vec<(u32, u32, Handle)>,
    pub stamp: Stamp,
    pub content_hash: Option<u64>,
    pub started: Instant,
    pub placed: Instant,
    pub times: FrameTimes,
}

pub struct Delivered {
    pub number: u64,
    pub stamp: Stamp,
    pub content_hash: Option<u64>,
    /// The target's image; none on the device that draws nothing.
    pub image: Option<Image>,
    pub times: FrameTimes,
}

pub enum StreamEvent {
    Image(Box<Delivered>),
    /// The stream ended: the device was lost, or the instance shut down.
    Ended(String),
}

#[derive(Default)]
struct HandOff {
    queue: VecDeque<FrameRecord>,
    /// The worlds whose place is taken, and those a frame is being recorded for.
    placed: BTreeSet<u32>,
    recording: BTreeSet<u32>,
}

struct InFlight {
    frame: FrameRecord,
    submission: u64,
    submitted: Instant,
}

/// What the presenter measured, summed over the frames it served.
#[derive(Clone, Copy, Debug, Default)]
pub struct PresenterCounters {
    pub frames: u64,
    pub dropped: u64,
    /// The time spent in the presenter's own work: submissions, polls, read-backs, deliveries.
    pub busy_us: f64,
    pub most_in_flight: usize,
}

/// A shader payload that became resident, waiting for the presenter.
struct PendingShader {
    variant: u32,
    payload: ShaderPayload,
    resident: Instant,
}

/// What the path from a resident payload to the cache entry cost.
#[derive(Clone, Copy, Debug)]
pub struct ShaderInstalled {
    pub variant: u32,
    pub resident: Instant,
    pub pipelines_built_us: f64,
    pub published: Instant,
}

pub struct Instance {
    pub port: Port,
    caches: RwLock<Option<Arc<Caches>>>,
    transients: Mutex<Vec<(u32, u32, Handle)>>,
    hand_off: Mutex<HandOff>,
    changed: Condvar,
    in_flight: Mutex<VecDeque<InFlight>>,
    streams: Mutex<BTreeMap<u32, Sender<StreamEvent>>>,
    pending_shader: Mutex<Option<PendingShader>>,
    shader_installed: Mutex<Option<ShaderInstalled>>,
    counters: Mutex<PresenterCounters>,
    stop: AtomicBool,
    /// The frames begun and neither delivered nor dropped.
    open_frames: AtomicU64,
}

impl Instance {
    /// An instance over an open port, its caches filled.
    pub fn new(port: Port) -> PortResult<Arc<Instance>> {
        let instance = Arc::new(Instance {
            port,
            caches: RwLock::new(None),
            transients: Mutex::new(Vec::new()),
            hand_off: Mutex::new(HandOff::default()),
            changed: Condvar::new(),
            in_flight: Mutex::new(VecDeque::new()),
            streams: Mutex::new(BTreeMap::new()),
            pending_shader: Mutex::new(None),
            shader_installed: Mutex::new(None),
            counters: Mutex::new(PresenterCounters::default()),
            stop: AtomicBool::new(false),
            open_frames: AtomicU64::new(0),
        });
        instance.fill_caches(0)?;
        Ok(instance)
    }

    /// The one path that fills the caches of assets and pipelines: at
    /// startup, and again after a device was lost and created again.
    fn fill_caches(&self, shader_variant: u32) -> PortResult<()> {
        let scene = Scene::build();
        let mut textures = Vec::with_capacity(5);
        for texture in &scene.textures {
            textures.push(self.port.create_texture(texture)?);
        }
        let geometry = self.port.create_vertex_buffer(vertex_bytes(&SharedGeometry::build().vertices), None)?;
        let pipelines = self.port.create_pipelines(&self.port.import_shader(shader_variant)?)?;
        *self.caches.write().unwrap() = Some(Arc::new(Caches { textures: [textures[0], textures[1], textures[2], textures[3], textures[4]], geometry, pipelines, shader_variant }));
        Ok(())
    }

    /// The caches as a frame declared now resolves them.
    pub fn caches(&self) -> PortResult<Arc<Caches>> {
        self.caches.read().unwrap().clone().ok_or(PortError::Lost)
    }

    /// Takes a transient target of this size from the pool, or creates one.
    pub fn acquire_transient(&self, width: u32, height: u32) -> PortResult<Handle> {
        let mut pool = self.transients.lock().unwrap();
        if let Some(position) = pool.iter().position(|&(w, h, handle)| (w, h) == (width, height) && self.port.is_live(handle)) {
            return Ok(pool.swap_remove(position).2);
        }
        drop(pool);
        self.port.create_target(width, height, None, MemoryClass::Transient)
    }

    fn give_back(&self, transients: &[(u32, u32, Handle)]) {
        let mut pool = self.transients.lock().unwrap();
        pool.extend(transients.iter().copied().filter(|&(_, _, handle)| self.port.is_live(handle)));
    }

    /// Subscribes to the images of a world's target.
    pub fn subscribe(&self, world: u32) -> Receiver<StreamEvent> {
        let (sender, receiver) = channel();
        self.streams.lock().unwrap().insert(world, sender);
        receiver
    }

    /// Whether a frame of the world may start: none is being recorded and
    /// its place in the hand-off is free. Marks the world as recording.
    pub fn try_begin(&self, world: u32) -> bool {
        let mut hand_off = self.hand_off.lock().unwrap();
        if hand_off.recording.contains(&world) || hand_off.placed.contains(&world) {
            return false;
        }
        hand_off.recording.insert(world);
        self.open_frames.fetch_add(1, Ordering::AcqRel);
        true
    }

    /// Holds the calling stepping thread until a frame of the world may start.
    pub fn begin(&self, world: u32) {
        let mut hand_off = self.hand_off.lock().unwrap();
        while hand_off.recording.contains(&world) || hand_off.placed.contains(&world) {
            hand_off = self.changed.wait_timeout(hand_off, Duration::from_millis(1)).unwrap().0;
        }
        hand_off.recording.insert(world);
        self.open_frames.fetch_add(1, Ordering::AcqRel);
    }

    /// The task that ends a frame's recording places it here.
    pub fn place(&self, mut frame: FrameRecord) {
        frame.placed = Instant::now();
        let mut hand_off = self.hand_off.lock().unwrap();
        hand_off.recording.remove(&frame.world);
        hand_off.placed.insert(frame.world);
        hand_off.queue.push_back(frame);
        drop(hand_off);
        self.changed.notify_all();
    }

    /// A frame that was begun and will not be placed: its passes are freed.
    pub fn abandon(&self, world: u32, number: u64, passes: &[u64], transients: &[(u32, u32, Handle)]) {
        for &pass in passes {
            self.port.discard_pass(pass);
        }
        self.port.frame_done(number);
        self.give_back(transients);
        self.hand_off.lock().unwrap().recording.remove(&world);
        self.changed.notify_all();
        self.counters.lock().unwrap().dropped += 1;
        self.open_frames.fetch_sub(1, Ordering::AcqRel);
    }

    /// A new payload of the shader is resident: the presenter builds its
    /// pipelines between two submissions and publishes the entry.
    pub fn shader_resident(&self, variant: u32, payload: ShaderPayload) {
        *self.pending_shader.lock().unwrap() = Some(PendingShader { variant, payload, resident: Instant::now() });
        self.changed.notify_all();
    }

    pub fn shader_installed(&self) -> Option<ShaderInstalled> {
        *self.shader_installed.lock().unwrap()
    }

    pub fn presenter_counters(&self) -> PresenterCounters {
        *self.counters.lock().unwrap()
    }

    fn install_pending_shader(&self) -> bool {
        let Some(pending) = self.pending_shader.lock().unwrap().take() else { return false };
        let started = Instant::now();
        let (Ok(pipelines), Ok(old)) = (self.port.create_pipelines(&pending.payload), self.caches()) else { return true };
        let pipelines_built_us = started.elapsed().as_secs_f64() * 1e6;
        *self.caches.write().unwrap() = Some(Arc::new(Caches { pipelines, shader_variant: pending.variant, ..(*old).clone() }));
        let _ = self.port.release(old.pipelines);
        *self.shader_installed.lock().unwrap() = Some(ShaderInstalled { variant: pending.variant, resident: pending.resident, pipelines_built_us, published: Instant::now() });
        true
    }

    fn deliver(&self, world: u32, event: StreamEvent) {
        if let Some(stream) = self.streams.lock().unwrap().get(&world) {
            let _ = stream.send(event);
        }
    }

    /// The device is lost: every frame in flight or in the hand-off is
    /// dropped and every stream ends, whatever its policy.
    fn degrade(&self) {
        let dropped: Vec<FrameRecord> = self.in_flight.lock().unwrap().drain(..).map(|flight| flight.frame).collect();
        let mut hand_off = self.hand_off.lock().unwrap();
        let waiting: Vec<FrameRecord> = hand_off.queue.drain(..).collect();
        hand_off.placed.clear();
        drop(hand_off);
        self.counters.lock().unwrap().dropped += (dropped.len() + waiting.len()) as u64;
        self.open_frames.fetch_sub((dropped.len() + waiting.len()) as u64, Ordering::AcqRel);
        *self.caches.write().unwrap() = None;
        self.transients.lock().unwrap().clear();
        let streams = std::mem::take(&mut *self.streams.lock().unwrap());
        for stream in streams.values() {
            let _ = stream.send(StreamEvent::Ended("degraded: the render device was lost".to_string()));
        }
        self.changed.notify_all();
    }

    /// Tries to create the device again and fills the caches by the path
    /// that filled them. The streams are subscribed anew by their consumers.
    pub fn recover(&self, shader_variant: u32) -> Result<(), String> {
        self.port.recreate()?;
        self.fill_caches(shader_variant).map_err(|error| error.to_string())?;
        Ok(())
    }

    /// One turn of the presenter. Returns whether it did anything.
    pub fn serve(&self) -> bool {
        let started = Instant::now();
        if self.port.is_lost() {
            let pending = !self.in_flight.lock().unwrap().is_empty() || !self.hand_off.lock().unwrap().queue.is_empty() || self.caches.read().unwrap().is_some();
            if pending {
                self.degrade();
            }
            return pending;
        }
        let mut worked = self.install_pending_shader();

        // The submissions the device has finished, oldest first.
        loop {
            let mut in_flight = self.in_flight.lock().unwrap();
            let Some(oldest) = in_flight.front() else { break };
            match self.port.finished(oldest.submission) {
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => break,
            }
            let flight = in_flight.pop_front().unwrap();
            drop(in_flight);
            let mut frame = flight.frame;
            frame.times.device_us = flight.submitted.elapsed().as_secs_f64() * 1e6;
            let reading = Instant::now();
            let image = self.port.take_images(flight.submission).ok().and_then(|mut images| if images.is_empty() { None } else { Some(images.remove(0)) });
            frame.times.read_back_us = reading.elapsed().as_secs_f64() * 1e6;
            frame.times.latency_us = frame.started.elapsed().as_secs_f64() * 1e6;
            self.port.frame_done(frame.number);
            self.give_back(&frame.transients);
            self.deliver(frame.world, StreamEvent::Image(Box::new(Delivered { number: frame.number, stamp: frame.stamp, content_hash: frame.content_hash, image, times: frame.times })));
            self.counters.lock().unwrap().frames += 1;
            self.open_frames.fetch_sub(1, Ordering::AcqRel);
            worked = true;
        }

        // One more frame from the hand-off, in the order they arrived, under
        // the bound on the frames in flight.
        if self.in_flight.lock().unwrap().len() < FRAMES_IN_FLIGHT {
            let mut hand_off = self.hand_off.lock().unwrap();
            if let Some(mut frame) = hand_off.queue.pop_front() {
                hand_off.placed.remove(&frame.world);
                drop(hand_off);
                self.changed.notify_all();
                frame.times.hand_off_us = frame.placed.elapsed().as_secs_f64() * 1e6;
                let submitting = Instant::now();
                match self.port.submit(frame.number, &frame.passes, &[frame.target]) {
                    Ok(submission) => {
                        frame.times.submit_us = submitting.elapsed().as_secs_f64() * 1e6;
                        let mut in_flight = self.in_flight.lock().unwrap();
                        in_flight.push_back(InFlight { frame, submission, submitted: Instant::now() });
                        let count = in_flight.len();
                        drop(in_flight);
                        let mut counters = self.counters.lock().unwrap();
                        counters.most_in_flight = counters.most_in_flight.max(count);
                    }
                    Err(_) => {
                        // A frame the device refused is dropped; nothing waits for it.
                        for &pass in &frame.passes {
                            self.port.discard_pass(pass);
                        }
                        self.port.frame_done(frame.number);
                        self.give_back(&frame.transients);
                        self.counters.lock().unwrap().dropped += 1;
                        self.open_frames.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                worked = true;
            }
        }
        if worked {
            self.counters.lock().unwrap().busy_us += started.elapsed().as_secs_f64() * 1e6;
        }
        worked
    }

    /// Whether a frame is on its way: begun, and neither delivered nor dropped.
    pub fn busy(&self) -> bool {
        self.open_frames.load(Ordering::Acquire) > 0
    }

    /// Serves the presenter from the calling thread until nothing is left.
    pub fn drain(&self) {
        while self.busy() {
            if !self.serve() {
                std::thread::yield_now();
            }
        }
    }

    /// Starts the presentation thread, which the instance joins at shutdown.
    pub fn start_presentation_thread(self: &Arc<Instance>) -> PresentationThread {
        let instance = Arc::clone(self);
        let thread = std::thread::Builder::new()
            .name("presentation".to_string())
            .spawn(move || {
                while !instance.stop.load(Ordering::Acquire) {
                    if instance.serve() {
                        continue;
                    }
                    if instance.in_flight.lock().unwrap().is_empty() {
                        // Nothing in flight: wait for a frame to be placed.
                        let hand_off = instance.hand_off.lock().unwrap();
                        if hand_off.queue.is_empty() {
                            let _ = instance.changed.wait_timeout(hand_off, Duration::from_millis(1)).unwrap();
                        }
                    } else {
                        // The device is working: poll again shortly.
                        std::thread::yield_now();
                    }
                }
            })
            .expect("the presentation thread starts");
        PresentationThread { instance: Arc::clone(self), thread: Some(thread) }
    }

    /// Ends every stream: the first point of shutdown.
    pub fn end_streams(&self) {
        let streams = std::mem::take(&mut *self.streams.lock().unwrap());
        for stream in streams.values() {
            let _ = stream.send(StreamEvent::Ended("the instance shut down".to_string()));
        }
    }
}

/// The presentation thread, stopped and joined when dropped.
pub struct PresentationThread {
    instance: Arc<Instance>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for PresentationThread {
    fn drop(&mut self) {
        self.instance.stop.store(true, Ordering::Release);
        self.instance.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.instance.stop.store(false, Ordering::Release);
    }
}
