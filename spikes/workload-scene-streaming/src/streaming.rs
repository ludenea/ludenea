//! The streaming machinery the world and the oracle share: the world
//! parameters, the records of the input log (the policy's commands and the
//! arrivals), the residency state of a cell (its meta record), the policy
//! that decides from the point of interest, and the loader thread that
//! delivers a payload after a real delay. Throwaway, never product code.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::grid::{GridConfig, Payload, PayloadStore, PointOfInterest};

/// The deterministic value a reference reads while its target is not
/// present (absent, requested, spawning or unloaded).
pub const NOT_PRESENT: u64 = u64::MAX;
/// What an entity without a reference observes.
pub const NO_REFERENCE: u64 = 0;

/// A read position packed as one word: x bits high, y bits low.
pub fn packed_position(x: f32, y: f32) -> u64 {
    (u64::from(x.to_bits()) << 32) | u64::from(y.to_bits())
}

/// The world parameters of the streaming (the snapshot header's world
/// configuration, never engine constants).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamingParameters {
    /// A cell is requested when its distance to the point is at most this, in cells.
    pub entering: f32,
    /// A cell is released when its distance to the point exceeds this, in cells.
    pub leaving: f32,
    /// The entities the structural fixed point spawns per tick at most; none for an unsliced spawn.
    pub spawn_slice: Option<u32>,
    /// A scripted loader delivers a requested payload this many ticks after
    /// the request; none means the live loader thread with its real delay.
    pub scripted_delay_ticks: Option<u32>,
}

impl StreamingParameters {
    pub const DEFAULT_ENTERING: f32 = 2.0;
    pub const DEFAULT_LEAVING: f32 = 3.0;
    pub const DEFAULT_SLICE: u32 = 500;
    pub const DEFAULT_SCRIPTED_DELAY: u32 = 30;

    pub fn scripted() -> StreamingParameters {
        StreamingParameters {
            entering: Self::DEFAULT_ENTERING,
            leaving: Self::DEFAULT_LEAVING,
            spawn_slice: Some(Self::DEFAULT_SLICE),
            scripted_delay_ticks: Some(Self::DEFAULT_SCRIPTED_DELAY),
        }
    }

    pub fn live() -> StreamingParameters {
        StreamingParameters { scripted_delay_ticks: None, ..Self::scripted() }
    }

    pub fn without_hysteresis(self) -> StreamingParameters {
        StreamingParameters { leaving: self.entering, ..self }
    }

    pub fn unsliced(self) -> StreamingParameters {
        StreamingParameters { spawn_slice: None, ..self }
    }

    pub fn has_hysteresis(&self) -> bool {
        self.leaving > self.entering
    }
}

// ---------------------------------------------------------------------------
// Records and the input log
// ---------------------------------------------------------------------------

/// One element of a tick's input record. A load and an unload are commands
/// the policy writes; an arrival is the availability transition of a
/// requested payload, posted by a loader or scripted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Load { cell: u32, request: u64 },
    Unload { cell: u32 },
    Arrival { request: u64, cell: u32, content_hash: u64 },
}

impl Record {
    pub fn cell(&self) -> u32 {
        match self {
            Record::Load { cell, .. } | Record::Unload { cell } | Record::Arrival { cell, .. } => *cell,
        }
    }
}

/// A record as logged: the tick it was applied at and whether the
/// validation accepted it (a rejection is deterministic and logged too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoggedRecord {
    pub tick: u32,
    pub record: Record,
    pub accepted: bool,
}

const RECORD_BYTES: usize = 26;

/// The input log of a world: every record applied, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputLog {
    pub records: Vec<LoggedRecord>,
}

impl InputLog {
    pub fn accepted_loads(&self) -> usize {
        self.records.iter().filter(|r| r.accepted && matches!(r.record, Record::Load { .. })).count()
    }

    pub fn accepted_unloads(&self) -> usize {
        self.records.iter().filter(|r| r.accepted && matches!(r.record, Record::Unload { .. })).count()
    }

    pub fn accepted_arrivals(&self) -> usize {
        self.records.iter().filter(|r| r.accepted && matches!(r.record, Record::Arrival { .. })).count()
    }

    pub fn rejected(&self) -> usize {
        self.records.iter().filter(|r| !r.accepted).count()
    }

    /// Rejected loads and unloads: the policy issued a command the record
    /// refused (expected never).
    pub fn rejected_commands(&self) -> usize {
        self.records.iter().filter(|r| !r.accepted && !matches!(r.record, Record::Arrival { .. })).count()
    }

    /// Rejected arrivals: the payload of a request abandoned by an unload
    /// before it landed (expected whenever the delay exceeds the dwell time).
    pub fn rejected_arrivals(&self) -> usize {
        self.records.iter().filter(|r| !r.accepted && matches!(r.record, Record::Arrival { .. })).count()
    }

    /// The canonical bytes: the count, then per record the tick, the kind,
    /// the cell, the request, the content hash and the accepted flag,
    /// little-endian (a kind without a field writes zero for it).
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 + self.records.len() * RECORD_BYTES);
        bytes.extend_from_slice(&(self.records.len() as u32).to_le_bytes());
        for logged in &self.records {
            let (kind, cell, request, hash) = match logged.record {
                Record::Load { cell, request } => (0_u8, cell, request, 0_u64),
                Record::Unload { cell } => (1, cell, 0, 0),
                Record::Arrival { request, cell, content_hash } => (2, cell, request, content_hash),
            };
            bytes.extend_from_slice(&logged.tick.to_le_bytes());
            bytes.push(kind);
            bytes.extend_from_slice(&cell.to_le_bytes());
            bytes.extend_from_slice(&request.to_le_bytes());
            bytes.extend_from_slice(&hash.to_le_bytes());
            bytes.push(u8::from(logged.accepted));
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<InputLog, String> {
        let read = |at: usize, len: usize| -> Result<&[u8], String> {
            bytes.get(at..at + len).ok_or_else(|| format!("log truncated at byte {at}"))
        };
        let u32_at = |at: usize| read(at, 4).map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")));
        let u64_at = |at: usize| read(at, 8).map(|b| u64::from_le_bytes(b.try_into().expect("eight bytes")));
        let count = u32_at(0)? as usize;
        let mut records = Vec::with_capacity(count);
        let mut at = 4;
        for _ in 0..count {
            let tick = u32_at(at)?;
            let kind = read(at + 4, 1)?[0];
            let cell = u32_at(at + 5)?;
            let request = u64_at(at + 9)?;
            let content_hash = u64_at(at + 17)?;
            let accepted = match read(at + 25, 1)?[0] {
                0 => false,
                1 => true,
                other => return Err(format!("accepted flag {other} at byte {}", at + 25)),
            };
            let record = match kind {
                0 => Record::Load { cell, request },
                1 => Record::Unload { cell },
                2 => Record::Arrival { request, cell, content_hash },
                other => return Err(format!("record kind {other} at byte {}", at + 4)),
            };
            records.push(LoggedRecord { tick, record, accepted });
            at += RECORD_BYTES;
        }
        if at != bytes.len() {
            return Err(format!("{} trailing bytes", bytes.len() - at));
        }
        Ok(InputLog { records })
    }
}

// ---------------------------------------------------------------------------
// Residency: the meta record of an instance
// ---------------------------------------------------------------------------

/// The residency state of a cell, read by the policy and the queries. The
/// record outlives the payload: an unloaded cell stays known and a reload is
/// a transition of the same record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residency {
    Unloaded,
    /// A load command was accepted; the payload is on its way under this request.
    Requested(u64),
    /// The payload arrived; the spawn is pending at the structural fixed point.
    Arrived(u64),
    /// The spawn is in progress, sliced across ticks; the instance reads as absent.
    Spawning(u64),
    Loaded,
    /// An unload command was accepted; the despawn is pending at the structural fixed point.
    Unloading,
}

impl Residency {
    /// Whether the policy may release the cell.
    pub fn releasable(&self) -> bool {
        matches!(self, Residency::Requested(_) | Residency::Arrived(_) | Residency::Spawning(_) | Residency::Loaded)
    }

}

// ---------------------------------------------------------------------------
// The policy
// ---------------------------------------------------------------------------

/// A command the policy writes into the next tick's input record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Load(u32),
    Unload(u32),
}

/// The default policy: every cell whose centre lies within the entering
/// distance of the point and that is unloaded is requested, the requests
/// ordered by the direction of travel (the cells ahead first, by the
/// projection of the cell's offset on the velocity, then by index); every
/// cell farther than the leaving distance that is requested, arrived,
/// spawning or loaded is released, by index. Only the square of cells around
/// the point that the leaving distance can reach is visited, plus the cells
/// known to be resident, so that the work follows the loaded set and not the
/// world.
pub fn decide(config: &GridConfig, parameters: &StreamingParameters, point: &PointOfInterest, residency: &[Residency]) -> Vec<Command> {
    let reach = parameters.leaving.max(parameters.entering).ceil() as i64 + 1;
    let (px, py) = ((point.x / config.cell_side).floor() as i64, (point.y / config.cell_side).floor() as i64);
    let mut loads: Vec<(f32, u32)> = Vec::new();
    let mut unloads: Vec<u32> = Vec::new();
    let speed = ((point.vx * point.vx) + (point.vy * point.vy)).sqrt();
    for cy in (py - reach)..=(py + reach) {
        for cx in (px - reach)..=(px + reach) {
            if !config.contains(cx, cy) {
                continue;
            }
            let cell = config.cell_index(cx as u32, cy as u32);
            let distance = config.distance_in_cells(cell, point.x, point.y);
            match residency[cell as usize] {
                Residency::Unloaded if distance <= parameters.entering => {
                    let (centre_x, centre_y) = config.centre(cell);
                    let ahead = if speed > 0.0 {
                        (((centre_x - point.x) * point.vx) + ((centre_y - point.y) * point.vy)) / speed
                    } else {
                        -distance
                    };
                    loads.push((ahead, cell));
                }
                state if state.releasable() && distance > parameters.leaving => unloads.push(cell),
                _ => {}
            }
        }
    }
    // A resident cell outside the visited square (a teleport would leave
    // some) is released too.
    for (index, state) in residency.iter().enumerate() {
        let cell = index as u32;
        let (cx, cy) = config.coordinate(cell);
        let outside = (i64::from(cx) - px).abs() > reach || (i64::from(cy) - py).abs() > reach;
        if outside && state.releasable() && !unloads.contains(&cell) {
            unloads.push(cell);
        }
    }
    loads.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("finite projections").then(a.1.cmp(&b.1)));
    unloads.sort_unstable();
    let mut commands: Vec<Command> = loads.into_iter().map(|(_, cell)| Command::Load(cell)).collect();
    commands.extend(unloads.into_iter().map(Command::Unload));
    commands
}

// ---------------------------------------------------------------------------
// The loader
// ---------------------------------------------------------------------------

/// What a loader posts to a world's arrival queue.
#[derive(Clone, Debug)]
pub struct Arrival {
    pub request: u64,
    pub cell: u32,
    pub payload: Arc<Payload>,
}

pub type ArrivalQueue = Arc<Mutex<Vec<Arrival>>>;

/// A request a world sends to the live loader: the payload of `cell` under
/// `request`, to be posted to `queue`.
pub struct LoadRequest {
    pub request: u64,
    pub cell: u32,
    pub queue: ArrivalQueue,
}

/// The live loader: a dispatcher thread that gives every request its own
/// short-lived thread (the late asset arrival spike's loader per arrival, so
/// that the delays of outstanding requests overlap as an I/O executor's
/// reads do), each sleeping a real delay drawn from the clock, then fetching
/// the payload from the shared store (importing it on the first request of
/// that cell) and posting it to the world's queue. Several worlds may share
/// one loader. Dropping it closes the channel and joins every thread.
pub struct LiveLoader {
    sender: Option<Sender<LoadRequest>>,
    thread: Option<JoinHandle<u64>>,
}

impl LiveLoader {
    /// `delay_ms` bounds the per-request delay drawn from the clock.
    pub fn spawn(store: Arc<PayloadStore>, delay_ms: (u64, u64)) -> LiveLoader {
        let (sender, receiver) = mpsc::channel::<LoadRequest>();
        let thread = thread::Builder::new()
            .name("scene-loader".to_string())
            .spawn(move || serve(&receiver, &store, delay_ms))
            .expect("spawn the loader");
        LiveLoader { sender: Some(sender), thread: Some(thread) }
    }

    pub fn sender(&self) -> Sender<LoadRequest> {
        self.sender.clone().expect("the loader is alive")
    }

    /// Closes the channel and waits for the thread; returns the requests it served.
    pub fn finish(mut self) -> u64 {
        self.sender.take();
        self.thread.take().map_or(0, |thread| thread.join().expect("the loader never panics"))
    }
}

impl Drop for LiveLoader {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(receiver: &Receiver<LoadRequest>, store: &Arc<PayloadStore>, delay_ms: (u64, u64)) -> u64 {
    let mut deliveries: Vec<JoinHandle<()>> = Vec::new();
    let mut served = 0;
    while let Ok(request) = receiver.recv() {
        served += 1;
        let delay = clock_delay(delay_ms);
        let store = store.clone();
        deliveries.push(
            thread::Builder::new()
                .name(format!("scene-load-{}", request.request))
                .spawn(move || {
                    thread::sleep(delay);
                    let payload = store.fetch(request.cell);
                    request.queue.lock().expect("the queue is never poisoned").push(Arrival { request: request.request, cell: request.cell, payload });
                })
                .expect("spawn a delivery"),
        );
        // Deliveries long done are joined as the loader goes, so that a long
        // run does not hold thousands of finished threads.
        if deliveries.len() >= 64 {
            deliveries.retain(|delivery| !delivery.is_finished());
        }
    }
    for delivery in deliveries {
        delivery.join().expect("a delivery never panics");
    }
    served
}

/// A real delay drawn from the clock, different between runs.
fn clock_delay((low, high): (u64, u64)) -> Duration {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| u64::from(since.subsec_nanos()));
    Duration::from_millis(low + nanos % (high - low + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_round_trips_and_refuses_corruption() {
        let log = InputLog {
            records: vec![
                LoggedRecord { tick: 1, record: Record::Load { cell: 4, request: 1 }, accepted: true },
                LoggedRecord { tick: 1, record: Record::Load { cell: 4, request: 2 }, accepted: false },
                LoggedRecord { tick: 7, record: Record::Arrival { request: 1, cell: 4, content_hash: 0xabcd }, accepted: true },
                LoggedRecord { tick: 9, record: Record::Unload { cell: 4 }, accepted: true },
            ],
        };
        let bytes = log.encode();
        assert_eq!(bytes.len(), 4 + 4 * RECORD_BYTES);
        assert_eq!(InputLog::decode(&bytes).unwrap(), log);
        assert_eq!((log.accepted_loads(), log.accepted_unloads(), log.accepted_arrivals(), log.rejected()), (1, 1, 1, 1));
        assert!(InputLog::decode(&bytes[..30]).unwrap_err().contains("truncated"));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(InputLog::decode(&trailing).unwrap_err().contains("trailing"));
        let mut bad_kind = bytes.clone();
        bad_kind[4 + 4] = 9;
        assert!(InputLog::decode(&bad_kind).unwrap_err().contains("record kind"));
        let mut bad_flag = bytes;
        bad_flag[4 + 25] = 2;
        assert!(InputLog::decode(&bad_flag).unwrap_err().contains("accepted flag"));
    }

    #[test]
    fn the_policy_requests_ahead_first_and_releases_beyond_the_leaving_distance() {
        let config = GridConfig::new(7, 10, 1);
        let parameters = StreamingParameters { entering: 1.0, leaving: 2.0, spawn_slice: None, scripted_delay_ticks: None };
        let mut residency = vec![Residency::Unloaded; config.cell_count() as usize];
        let (x, y) = config.centre(config.cell_index(3, 3));
        let point = PointOfInterest { x, y, vx: 1.0, vy: 0.0 };
        let commands = decide(&config, &parameters, &point, &residency);
        let loads: Vec<u32> = commands.iter().filter_map(|c| if let Command::Load(cell) = c { Some(*cell) } else { None }).collect();
        // Within one cell of the centre: the cell itself and its four edge
        // neighbours (the diagonals are at 1.41). The cell ahead comes first,
        // the one behind last.
        assert_eq!(loads.len(), 5);
        assert_eq!(loads[0], config.cell_index(4, 3));
        assert_eq!(loads[4], config.cell_index(2, 3));
        assert!(commands.iter().all(|c| matches!(c, Command::Load(_))));

        residency[config.cell_index(0, 0) as usize] = Residency::Loaded;
        residency[config.cell_index(6, 6) as usize] = Residency::Requested(3);
        residency[config.cell_index(4, 3) as usize] = Residency::Loaded;
        residency[config.cell_index(3, 3) as usize] = Residency::Unloading;
        let commands = decide(&config, &parameters, &point, &residency);
        let unloads: Vec<u32> = commands.iter().filter_map(|c| if let Command::Unload(cell) = c { Some(*cell) } else { None }).collect();
        assert_eq!(unloads, vec![config.cell_index(0, 0), config.cell_index(6, 6)], "the far cells, resident or requested, by index");
        assert!(!commands.contains(&Command::Load(config.cell_index(4, 3))), "a loaded cell is not requested again");
        assert!(!commands.contains(&Command::Load(config.cell_index(3, 3))), "an unloading cell waits");
    }

    #[test]
    fn the_live_loader_posts_the_payload_of_the_cell_after_its_delay() {
        let store = PayloadStore::new(GridConfig::new(3, 10, 5));
        let loader = LiveLoader::spawn(store.clone(), (1, 2));
        let queue: ArrivalQueue = Arc::new(Mutex::new(Vec::new()));
        loader.sender().send(LoadRequest { request: 7, cell: 4, queue: queue.clone() }).unwrap();
        loader.sender().send(LoadRequest { request: 8, cell: 0, queue: queue.clone() }).unwrap();
        assert_eq!(loader.finish(), 2);
        let mut arrivals = queue.lock().unwrap().clone();
        // The deliveries overlap, so the landing order is the delays'; the
        // world sorts by request at its fixed point.
        arrivals.sort_by_key(|arrival| arrival.request);
        assert_eq!(arrivals.len(), 2);
        assert_eq!((arrivals[0].request, arrivals[0].cell), (7, 4));
        assert_eq!((arrivals[1].request, arrivals[1].cell), (8, 0));
        assert_eq!(arrivals[0].payload.content_hash, Payload::import(store.config(), 4).content_hash);
        assert_eq!(store.imports(), 2);
    }

    #[test]
    fn the_parameters_derive_their_variants() {
        let scripted = StreamingParameters::scripted();
        assert!(scripted.has_hysteresis());
        assert!(!scripted.without_hysteresis().has_hysteresis());
        assert_eq!(scripted.unsliced().spawn_slice, None);
        assert_eq!(StreamingParameters::live().scripted_delay_ticks, None);
        assert_eq!(packed_position(1.0, 2.0), (u64::from(1.0_f32.to_bits()) << 32) | u64::from(2.0_f32.to_bits()));
    }
}
