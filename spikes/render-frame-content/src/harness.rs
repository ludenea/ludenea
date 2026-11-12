//! What the measurements share: an instance over a device, a world with its
//! renderer and its stream, and the summary of a series of figures.
//! Throwaway, never product code.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use reference_workload::Json;
use render_device_baseline::backend::Api;
use render_device_baseline::frames::FrameDevice;
use skeleton_core::{Capabilities, Size, World, WorldConfig};

use crate::instance::{Delivered, Instance, PresentationThread, StreamEvent};
use crate::math::Arithmetic;
use crate::port::{ContentDevice, Port};
use crate::renderer::{Request, WorldRenderer};
use crate::slices::RenderExtraction;

/// Which device stands behind the port.
#[derive(Clone, Copy)]
pub enum Backend {
    /// The device that draws nothing.
    Content,
    /// A device of a graphics interface, by its index, with or without the
    /// interface's validation layer.
    Native { api: SharedApi, index: usize, validation: bool },
}

/// A graphics interface kept for the life of the process, so that a device
/// can be created again from any thread.
#[derive(Clone, Copy)]
pub struct SharedApi(pub &'static dyn Api);

// SAFETY: the interface object holds the loaded library and what was read
// from it at start; a device is opened from it by one thread at a time, which
// the port's lock on its device ensures.
unsafe impl Send for SharedApi {}
// SAFETY: as above.
unsafe impl Sync for SharedApi {}

impl Backend {
    pub fn draws_pixels(self) -> bool {
        matches!(self, Backend::Native { .. })
    }

    pub fn open(self) -> Result<Port, String> {
        match self {
            Backend::Content => Port::open(|| Ok(Box::new(ContentDevice::new()) as Box<dyn FrameDevice>)),
            Backend::Native { api, index, validation } => Port::open(move || {
                let api = api;
                api.0.open_frames(index, validation)
            }),
        }
    }
}

/// How the presenter's role is taken.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Presenter {
    /// A dedicated thread: recording, submission and read-back overlap the
    /// stepping loop.
    Thread,
    /// The stepping thread itself: submission and read-back follow recording
    /// in sequence, and the loop goes on when the image is read back.
    SteppingThread,
}

pub struct Rig {
    pub instance: Arc<Instance>,
    pub presenter: Presenter,
    _thread: Option<PresentationThread>,
}

impl Rig {
    pub fn new(backend: Backend, presenter: Presenter) -> Result<Rig, String> {
        let instance = Instance::new(backend.open()?).map_err(|error| error.to_string())?;
        let thread = (presenter == Presenter::Thread).then(|| instance.start_presentation_thread());
        Ok(Rig { instance, presenter, _thread: thread })
    }
}

/// The first point of shutdown: the streams end before the presentation thread is stopped and joined.
impl Drop for Rig {
    fn drop(&mut self) {
        self.instance.end_streams();
    }
}

pub struct RenderedWorld {
    pub world: World,
    pub renderer: WorldRenderer,
    pub stream: Receiver<StreamEvent>,
    instance: Arc<Instance>,
    presenter: Presenter,
}

impl RenderedWorld {
    /// A world of the reference workload at its default size, with its
    /// extraction, its renderer and a subscription to its target's images.
    pub fn new(rig: &Rig, id: u32, size: Size, world_workers: usize, worker_budget: usize) -> RenderedWorld {
        let extraction = Arc::new(RenderExtraction::new());
        let world = World::new(WorldConfig::new(size, world_workers), Capabilities { extraction: Some(extraction.clone()) });
        let renderer = WorldRenderer::new(id, Arc::clone(&rig.instance), extraction, worker_budget);
        RenderedWorld { world, renderer, stream: rig.instance.subscribe(id), instance: Arc::clone(&rig.instance), presenter: rig.presenter }
    }

    pub fn step(&mut self, ticks: u32) {
        self.world.step(ticks).expect("the reference workload steps");
    }

    /// Subscribes anew, after the stream ended with a lost device.
    pub fn subscribe(&mut self) {
        self.stream = self.instance.subscribe(self.renderer.id);
    }

    /// Starts a frame, holding the stepping thread until one may start.
    pub fn start<A: Arithmetic>(&mut self, request: &Request) -> Result<u64, String> {
        self.instance.begin(self.renderer.id);
        self.renderer.frame::<A>(request).map_err(|error| error.to_string())
    }

    /// The next event of the stream, the calling thread serving the
    /// presenter when no thread does.
    pub fn next_event(&mut self, limit: Duration) -> Result<StreamEvent, String> {
        let deadline = Instant::now() + limit;
        loop {
            // Read before the stream: a frame delivered since then is in the stream.
            let nothing_on_its_way = !self.instance.busy();
            match self.stream.try_recv() {
                Ok(event) => return Ok(event),
                Err(TryRecvError::Disconnected) => return Err("the stream was closed".to_string()),
                Err(TryRecvError::Empty) => {}
            }
            if nothing_on_its_way {
                return Err("the frame was dropped".to_string());
            }
            let served = self.presenter == Presenter::SteppingThread && self.instance.serve();
            if !served {
                if Instant::now() > deadline {
                    return Err("no image arrived in time".to_string());
                }
                std::thread::yield_now();
            }
        }
    }

    /// Draws a few frames as fast as they may start and waits for the last,
    /// so that the pool of transient targets and the world's buffers hold
    /// what frames in flight at once need before anything is timed.
    pub fn warm_up<A: Arithmetic>(&mut self, request: &Request) -> Result<(), String> {
        for _ in 0..6 {
            self.instance.begin(self.renderer.id);
            self.renderer.frame::<A>(request).map_err(|error| error.to_string())?;
            if self.presenter == Presenter::SteppingThread {
                self.instance.drain();
            }
        }
        while self.instance.busy() {
            if self.presenter == Presenter::SteppingThread {
                self.instance.serve();
            }
            std::thread::yield_now();
        }
        while self.stream.try_recv().is_ok() {}
        Ok(())
    }

    /// Makes one frame and waits for its image.
    pub fn capture<A: Arithmetic>(&mut self, request: &Request) -> Result<Delivered, String> {
        let number = self.start::<A>(request)?;
        loop {
            match self.next_event(Duration::from_secs(120))? {
                StreamEvent::Image(delivered) if delivered.number == number => return Ok(*delivered),
                StreamEvent::Image(_) => {}
                StreamEvent::Ended(reason) => return Err(reason),
            }
        }
    }
}

/// The smallest, the median and the largest of a series.
pub fn summary(values: &[f64]) -> [f64; 3] {
    if values.is_empty() {
        return [0.0; 3];
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    [sorted[0], sorted[sorted.len() / 2], sorted[sorted.len() - 1]]
}

/// The value below which `fraction` of a series falls.
pub fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

pub fn write_summary(out: &mut Json, key: &str, values: &[f64]) {
    let [min, median, max] = summary(values);
    out.key(key);
    out.object_start();
    out.decimal("min", min);
    out.decimal("median", median);
    out.decimal("max", max);
    out.u64("count", values.len() as u64);
    out.object_end();
}
