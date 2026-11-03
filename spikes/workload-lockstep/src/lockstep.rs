//! The peers, the link and the protocol of the lockstep spike. Throwaway,
//! never product code.
//!
//! Two peers in one process, each a world of the reference workload and one
//! input stream, exchange their inputs over a simulated link and step in
//! deterministic lockstep: a peer steps tick `t` only when it holds both
//! peers' inputs for `t`, merged in the canonical order by source identity,
//! and stalls otherwise. Time is virtual: a frame is the slot of one tick,
//! messages travel in frames, nothing sleeps and no socket opens (the control
//! surface toy spike measured the socket path). Every message carries the
//! inputs the other peer has not acknowledged yet (the usual redundancy, which
//! is the reliable layer), an acknowledgement and the hash of the sender's
//! latest confirmed tick, so that a desync is detected by the hash exchange.
//! One peer may instead run the rollback variant: it predicts the inputs it
//! lacks, steps ahead, and restores a snapshot from its ring and re-simulates
//! when a real input contradicts a prediction.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use reference_workload::{SplitMix64, run_hash};

use crate::world::{
    GROUP_NAMES, Kick, KickOutcome, Snapshot, World, WorldConfig, first_divergent_group, group_hashes,
};

pub const KICKS_PER_TICK: usize = 4;
const KICK_RANGE: f32 = 8.0;
const LOCAL_ONLY_KICK: (f32, f32) = (16.0, 16.0);
const PAYLOAD_HEADER_BYTES: usize = 1 + 4 + 4 + 8 + 2;
const INPUT_HEADER_BYTES: usize = 4 + 1;
const KICK_BYTES: usize = 16;

/// The link's parameters: a latency in frames, a jitter drawn uniformly in
/// `0..=jitter_max` frames per message, and a drop probability per message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkSettings {
    pub latency: u32,
    pub jitter_max: u32,
    pub loss: f32,
}

/// One peer's input for one tick: a value, never changed once issued.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub source: u8,
    pub tick: u32,
    pub kicks: Vec<Kick>,
}

/// What one frame's message carries: the sender's identity, the highest tick
/// up to which it holds the other peer's inputs contiguously, the hash of its
/// latest confirmed tick, and every input of its own the other peer has not
/// acknowledged.
#[derive(Clone, Debug, PartialEq)]
pub struct Payload {
    pub source: u8,
    pub ack: u32,
    pub hash_tick: u32,
    pub hash: u64,
    pub inputs: Vec<Input>,
}

pub fn encode(payload: &Payload) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PAYLOAD_HEADER_BYTES + payload.inputs.iter().map(|i| INPUT_HEADER_BYTES + i.kicks.len() * KICK_BYTES).sum::<usize>());
    bytes.push(payload.source);
    bytes.extend_from_slice(&payload.ack.to_le_bytes());
    bytes.extend_from_slice(&payload.hash_tick.to_le_bytes());
    bytes.extend_from_slice(&payload.hash.to_le_bytes());
    bytes.extend_from_slice(&(payload.inputs.len() as u16).to_le_bytes());
    for input in &payload.inputs {
        bytes.extend_from_slice(&input.tick.to_le_bytes());
        bytes.push(input.kicks.len() as u8);
        for kick in &input.kicks {
            bytes.extend_from_slice(&kick.index.to_le_bytes());
            bytes.extend_from_slice(&kick.generation.to_le_bytes());
            bytes.extend_from_slice(&kick.delta_vx.to_le_bytes());
            bytes.extend_from_slice(&kick.delta_vy.to_le_bytes());
        }
    }
    bytes
}

pub fn decode(bytes: &[u8]) -> Result<Payload, String> {
    struct Cursor<'a>(&'a [u8], usize);
    impl Cursor<'_> {
        fn take(&mut self, n: usize) -> Result<&[u8], String> {
            let end = self.1.checked_add(n).filter(|&end| end <= self.0.len()).ok_or_else(|| format!("message truncated at byte {}", self.1))?;
            let slice = &self.0[self.1..end];
            self.1 = end;
            Ok(slice)
        }
        fn u8(&mut self) -> Result<u8, String> {
            Ok(self.take(1)?[0])
        }
        fn u16(&mut self) -> Result<u16, String> {
            Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("two bytes")))
        }
        fn u32(&mut self) -> Result<u32, String> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
        }
        fn u64(&mut self) -> Result<u64, String> {
            Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
        }
        fn f32(&mut self) -> Result<f32, String> {
            Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
        }
    }
    let mut cursor = Cursor(bytes, 0);
    let source = cursor.u8()?;
    if source > 1 {
        return Err(format!("source {source} is not a peer"));
    }
    let ack = cursor.u32()?;
    let hash_tick = cursor.u32()?;
    let hash = cursor.u64()?;
    let count = cursor.u16()?;
    let mut inputs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let tick = cursor.u32()?;
        let kicks = cursor.u8()?;
        let mut record = Vec::with_capacity(kicks as usize);
        for _ in 0..kicks {
            record.push(Kick { index: cursor.u32()?, generation: cursor.u32()?, delta_vx: cursor.f32()?, delta_vy: cursor.f32()? });
        }
        inputs.push(Input { source, tick, kicks: record });
    }
    if cursor.1 != bytes.len() {
        return Err(format!("{} trailing bytes", bytes.len() - cursor.1));
    }
    Ok(Payload { source, ack, hash_tick, hash, inputs })
}

// ---------------------------------------------------------------------------
// The simulated link
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkStats {
    pub sent: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub bytes: u64,
}

struct InFlight {
    delivery: u64,
    to: usize,
    bytes: Vec<u8>,
}

/// The link between the two peers: every message takes a seeded draw for its
/// loss and one for its jitter, so that a run is reproducible for a seed.
pub struct Link {
    settings: LinkSettings,
    rng: SplitMix64,
    in_flight: Vec<InFlight>,
    pub stats: LinkStats,
}

impl Link {
    pub fn new(settings: LinkSettings, seed: u64) -> Link {
        Link { settings, rng: SplitMix64::new(seed), in_flight: Vec::new(), stats: LinkStats::default() }
    }

    pub fn send(&mut self, frame: u64, to: usize, bytes: Vec<u8>) {
        self.stats.sent += 1;
        self.stats.bytes += bytes.len() as u64;
        let loss_draw = self.rng.u01();
        let jitter_draw = self.rng.next();
        if loss_draw < self.settings.loss {
            self.stats.dropped += 1;
            return;
        }
        let jitter = if self.settings.jitter_max == 0 { 0 } else { (jitter_draw % (u64::from(self.settings.jitter_max) + 1)) as u32 };
        self.in_flight.push(InFlight { delivery: frame + u64::from(self.settings.latency) + u64::from(jitter), to, bytes });
    }

    /// The messages for `to` whose delivery frame has come, in delivery order
    /// (and send order within a frame).
    pub fn deliver(&mut self, frame: u64, to: usize) -> Vec<Vec<u8>> {
        let (mut due, rest): (Vec<InFlight>, Vec<InFlight>) = std::mem::take(&mut self.in_flight).into_iter().partition(|m| m.to == to && m.delivery <= frame);
        self.in_flight = rest;
        due.sort_by_key(|m| m.delivery);
        self.stats.delivered += due.len() as u64;
        due.into_iter().map(|m| m.bytes).collect()
    }
}

// ---------------------------------------------------------------------------
// A peer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Lockstep,
    Rollback,
}

/// A hash exchanged for a tick that differs from the local one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Divergence {
    pub tick: u32,
    pub detected_at_frame: u64,
    pub local_hash: u64,
    pub remote_hash: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RollbackStats {
    pub rollbacks: u64,
    pub predicted_ticks: u64,
    pub mispredicted_ticks: u64,
    pub resimulated_ticks: u64,
    pub resimulated_max: u64,
    pub time: Duration,
    pub time_max: Duration,
}

pub struct Peer {
    pub id: usize,
    mode: Mode,
    world: World,
    workers: usize,
    delay: u32,
    target: u32,
    stream: SplitMix64,
    own: Vec<Option<Vec<Kick>>>,
    issued: u32,
    remote: Vec<Option<Vec<Kick>>>,
    remote_contiguous: u32,
    last_remote_tick: Option<u32>,
    acked_by_remote: u32,
    hashes: Vec<u64>,
    remote_hashes: Vec<Option<u64>>,
    pub hashes_compared: u64,
    pub divergence: Option<Divergence>,
    pub stall_frames: u64,
    ring_depth: usize,
    ring: VecDeque<(u32, Snapshot)>,
    pub snapshot_time: Duration,
    pub snapshots_taken: u64,
    injection: Option<u32>,
    pub injected_kick: Option<Kick>,
    used_prediction: Vec<Option<Vec<Kick>>>,
    confirmed: u32,
    pub rollback: RollbackStats,
    pub applied: u64,
    pub stale: u64,
    records: Vec<Vec<Kick>>,
    /// The stepping time of the worlds a rollback replaced.
    stepping_before_restores: Duration,
}

impl Peer {
    /// A peer at tick 0 with its first `delay` inputs issued. `ring_depth` of
    /// zero keeps no snapshots; a rollback peer keeps its confirmed tick and
    /// the `ring_depth` ticks after it, a lockstep peer its last `ring_depth`.
    pub fn new(id: usize, mode: Mode, config: WorldConfig, delay: u32, target: u32, ring_depth: usize, injection: Option<u32>) -> Peer {
        assert!(delay >= 1, "the input delay is at least one tick");
        assert!(mode == Mode::Lockstep || ring_depth > 0, "a rollback peer needs a ring");
        let world = World::new(config);
        let slots = (target + delay + 2) as usize;
        let mut hashes = vec![0; target as usize + 1];
        hashes[0] = world.hash();
        let mut peer = Peer {
            id,
            mode,
            world,
            workers: config.workers,
            delay,
            target,
            stream: SplitMix64::new(config.seed ^ (0x9e37_79b9_7f4a_7c15_u64.wrapping_mul(id as u64 + 1))),
            own: vec![None; slots],
            issued: 0,
            remote: vec![None; slots],
            remote_contiguous: 0,
            last_remote_tick: None,
            acked_by_remote: 0,
            hashes,
            remote_hashes: vec![None; target as usize + 1],
            hashes_compared: 0,
            divergence: None,
            stall_frames: 0,
            ring_depth,
            ring: VecDeque::new(),
            snapshot_time: Duration::ZERO,
            snapshots_taken: 0,
            injection,
            injected_kick: None,
            used_prediction: vec![None; target as usize + 1],
            confirmed: 0,
            rollback: RollbackStats::default(),
            applied: 0,
            stale: 0,
            records: vec![Vec::new(); target as usize + 1],
            stepping_before_restores: Duration::ZERO,
        };
        for tick in 1..=delay {
            peer.issue(tick);
        }
        peer.retain_snapshot();
        peer
    }

    pub fn tick(&self) -> u32 {
        self.world.tick()
    }

    /// The last tick stepped with both peers' real inputs.
    pub fn confirmed(&self) -> u32 {
        self.confirmed
    }

    pub fn hashes(&self) -> &[u64] {
        &self.hashes[..=self.confirmed as usize]
    }

    pub fn run_hash(&self) -> u64 {
        run_hash(self.hashes())
    }

    pub fn final_hash(&self) -> u64 {
        self.hashes[self.confirmed as usize]
    }

    /// The wall time spent stepping, the replaced worlds' included.
    pub fn stepping(&self) -> Duration {
        self.stepping_before_restores + self.world.stepping()
    }

    /// The records applied, by tick, up to the confirmed tick.
    pub fn records(&self) -> Vec<Vec<Kick>> {
        self.records[1..=self.confirmed as usize].to_vec()
    }

    pub fn retained_snapshot(&self, tick: u32) -> Option<&Snapshot> {
        self.ring.iter().find(|(t, _)| *t == tick).map(|(_, s)| s)
    }

    fn ring_capacity(&self) -> usize {
        match self.mode {
            Mode::Lockstep => self.ring_depth,
            Mode::Rollback => self.ring_depth + 1,
        }
    }

    fn retain_snapshot(&mut self) {
        if self.ring_depth == 0 {
            return;
        }
        let taking = Instant::now();
        let snapshot = self.world.snapshot().expect("a healthy world snapshots");
        self.snapshot_time += taking.elapsed();
        self.snapshots_taken += 1;
        self.ring.push_back((self.world.tick(), snapshot));
        while self.ring.len() > self.ring_capacity() {
            self.ring.pop_front();
        }
    }

    /// The peer's input for `tick`, drawn from its stream against its local
    /// state: an index, the generation the peer sees there, and two deltas.
    fn issue(&mut self, tick: u32) {
        let state = self.world.state();
        let n = state.len() as u64;
        let mut kicks = Vec::with_capacity(KICKS_PER_TICK);
        for _ in 0..KICKS_PER_TICK {
            let index = (self.stream.next() % n) as u32;
            let generation = state.generation[index as usize];
            let delta_vx = (self.stream.u01() * (2.0 * KICK_RANGE)) - KICK_RANGE;
            let delta_vy = (self.stream.u01() * (2.0 * KICK_RANGE)) - KICK_RANGE;
            kicks.push(Kick { index, generation, delta_vx, delta_vy });
        }
        self.own[tick as usize] = Some(kicks);
        self.issued = tick;
    }

    fn predicted_remote(&self) -> Vec<Kick> {
        self.last_remote_tick.and_then(|t| self.remote[t as usize].clone()).unwrap_or_default()
    }

    /// The canonical merge of both peers' kicks for `tick`: peer 0's then
    /// peer 1's, each in issue order. The second value is the prediction used
    /// for the remote input, when it was missing.
    fn record_for(&self, tick: u32) -> (Vec<Kick>, Option<Vec<Kick>>) {
        let own = self.own[tick as usize].as_ref().expect("own inputs are issued ahead");
        let (remote, prediction) = match &self.remote[tick as usize] {
            Some(real) => (real.clone(), None),
            None => {
                let prediction = self.predicted_remote();
                (prediction.clone(), Some(prediction))
            }
        };
        let (first, second) = if self.id == 0 { (own, &remote) } else { (&remote, own) };
        let mut record = Vec::with_capacity(first.len() + second.len());
        record.extend_from_slice(first);
        record.extend_from_slice(second);
        (record, prediction)
    }

    /// The injected desync: a kick on the first alive entity, applied locally
    /// and never sent.
    fn local_only_kick(&self) -> Kick {
        let state = self.world.state();
        let index = (0..state.len()).find(|&slot| state.alive[slot]).expect("some entity is alive") as u32;
        Kick { index, generation: state.generation[index as usize], delta_vx: LOCAL_ONLY_KICK.0, delta_vy: LOCAL_ONLY_KICK.1 }
    }

    /// Steps `tick` with its record (and the injection when it is due),
    /// records the hash and the record, and retains the snapshot.
    fn step_tick(&mut self, tick: u32) -> Option<Vec<Kick>> {
        let (mut record, prediction) = self.record_for(tick);
        if self.injection == Some(tick) {
            let kick = self.local_only_kick();
            record.push(kick);
            self.injected_kick = Some(kick);
        }
        let outcomes = self.world.step(vec![record.clone()]).expect("the peer's world steps");
        for outcome in outcomes.iter().flatten() {
            match outcome {
                KickOutcome::Applied => self.applied += 1,
                KickOutcome::StaleHandle | KickOutcome::UnknownIndex => self.stale += 1,
            }
        }
        self.hashes[tick as usize] = self.world.hash();
        self.records[tick as usize] = record;
        self.retain_snapshot();
        prediction
    }

    /// One frame of the peer: steps the next tick when the protocol allows
    /// it, stalls otherwise. Returns whether a tick was stepped.
    pub fn advance(&mut self, frame: u64) -> bool {
        let tick = self.world.tick() + 1;
        if tick > self.target {
            return false;
        }
        let allowed = match self.mode {
            Mode::Lockstep => self.remote[tick as usize].is_some(),
            Mode::Rollback => tick <= self.confirmed + self.ring_depth as u32,
        };
        if !allowed {
            self.stall_frames += 1;
            return false;
        }
        let prediction = self.step_tick(tick);
        if prediction.is_some() {
            self.rollback.predicted_ticks += 1;
        }
        self.used_prediction[tick as usize] = prediction;
        self.issue(tick + self.delay);
        self.confirm_and_compare(frame);
        true
    }

    /// Confirms the ticks stepped with both peers' real inputs, and compares
    /// each newly confirmed tick's hash with the remote one when it is known.
    fn confirm_and_compare(&mut self, frame: u64) {
        while self.confirmed < self.world.tick() {
            let next = self.confirmed as usize + 1;
            if self.remote[next].is_none() || self.used_prediction[next].is_some() {
                break;
            }
            self.confirmed += 1;
            self.compare_hash(self.confirmed, frame);
        }
    }

    /// The message of this frame.
    pub fn outgoing(&self) -> Payload {
        let inputs = (self.acked_by_remote + 1..=self.issued)
            .map(|tick| Input { source: self.id as u8, tick, kicks: self.own[tick as usize].clone().expect("issued") })
            .collect();
        Payload { source: self.id as u8, ack: self.remote_contiguous, hash_tick: self.confirmed, hash: self.hashes[self.confirmed as usize], inputs }
    }

    /// Takes a delivered message: stores the inputs and the acknowledgement,
    /// reconciles predictions (the rollback variant), compares the exchanged
    /// hash with the local one when the tick is confirmed here.
    pub fn receive(&mut self, payload: Payload, frame: u64) {
        assert_eq!(payload.source as usize, 1 - self.id, "a message from the other peer");
        self.acked_by_remote = self.acked_by_remote.max(payload.ack);
        for input in payload.inputs {
            let slot = input.tick as usize;
            match &self.remote[slot] {
                Some(known) => assert_eq!(known, &input.kicks, "an input is a value: a retransmission repeats it"),
                None => {
                    self.remote[slot] = Some(input.kicks);
                    self.last_remote_tick = Some(self.last_remote_tick.map_or(input.tick, |t| t.max(input.tick)));
                }
            }
        }
        while self.remote.get(self.remote_contiguous as usize + 1).is_some_and(Option::is_some) {
            self.remote_contiguous += 1;
        }
        // A stalled sender repeats its confirmed tick's hash: a hash is
        // compared once, when it becomes known if the tick is confirmed here,
        // or when the tick is confirmed otherwise.
        let hash_tick = payload.hash_tick as usize;
        let newly_known = hash_tick >= 1 && hash_tick < self.remote_hashes.len() && self.remote_hashes[hash_tick].is_none();
        if newly_known {
            self.remote_hashes[hash_tick] = Some(payload.hash);
        }
        if self.mode == Mode::Rollback {
            self.reconcile();
        }
        if newly_known && payload.hash_tick <= self.confirmed {
            self.compare_hash(payload.hash_tick, frame);
        }
        self.confirm_and_compare(frame);
    }

    fn compare_hash(&mut self, tick: u32, frame: u64) {
        let Some(remote) = self.remote_hashes[tick as usize] else { return };
        self.hashes_compared += 1;
        let local = self.hashes[tick as usize];
        if local != remote && self.divergence.is_none_or(|d| d.tick > tick) {
            self.divergence = Some(Divergence { tick, detected_at_frame: frame, local_hash: local, remote_hash: remote });
        }
    }

    /// The rollback variant's reconciliation: predictions that the real
    /// inputs confirm are released; the earliest contradicted one restores
    /// the snapshot of the tick before it and re-simulates from there.
    fn reconcile(&mut self) {
        let current = self.world.tick();
        let mut rollback_from = None;
        for tick in self.confirmed + 1..=current {
            let slot = tick as usize;
            let Some(real) = self.remote[slot].as_ref() else { continue };
            let Some(prediction) = self.used_prediction[slot].as_ref() else { continue };
            if prediction == real {
                self.used_prediction[slot] = None;
            } else {
                rollback_from = Some(tick);
                break;
            }
        }
        let Some(from) = rollback_from else { return };
        let started = Instant::now();
        for tick in from..=current {
            if let (Some(prediction), Some(real)) = (&self.used_prediction[tick as usize], &self.remote[tick as usize])
                && prediction != real
            {
                self.rollback.mispredicted_ticks += 1;
            }
        }
        while self.ring.back().is_some_and(|(t, _)| *t >= from) {
            self.ring.pop_back();
        }
        let (restored_tick, snapshot) = self.ring.back().expect("the ring holds the confirmed tick and the ticks after it");
        assert_eq!(*restored_tick, from - 1, "the ring holds the tick before the contradicted one");
        self.stepping_before_restores += self.world.stepping();
        self.world = World::restore(snapshot, self.workers).expect("a retained snapshot restores");
        for tick in from..=current {
            let prediction = self.step_tick(tick);
            self.used_prediction[tick as usize] = prediction;
        }
        let resimulated = u64::from(current - from + 1);
        let elapsed = started.elapsed();
        self.rollback.rollbacks += 1;
        self.rollback.resimulated_ticks += resimulated;
        self.rollback.resimulated_max = self.rollback.resimulated_max.max(resimulated);
        self.rollback.time += elapsed;
        self.rollback.time_max = self.rollback.time_max.max(elapsed);
    }
}

// ---------------------------------------------------------------------------
// A session of two peers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub config: WorldConfig,
    pub ticks: u32,
    pub delay: u32,
    pub link: LinkSettings,
    pub link_seed: u64,
    /// Snapshots retained per peer (0: none); the rollback peer's window.
    pub ring_depth: usize,
    /// The tick at which peer 1 applies a kick it never sends.
    pub injection: Option<u32>,
    pub rollback_peer: bool,
}

/// How a divergence was detected and what the two states of the tick show.
#[derive(Clone, Debug)]
pub struct DivergenceReport {
    pub tick: u32,
    pub detected_at_frame: u64,
    pub detected_by: Vec<usize>,
    pub group_hashes: Option<[[u64; 4]; 2]>,
    pub first_divergent_group: Option<usize>,
    pub snapshot_bytes: Option<[usize; 2]>,
    pub snapshots_identical: Option<bool>,
    pub first_differing_byte: Option<usize>,
}

impl DivergenceReport {
    pub fn group_name(&self) -> &'static str {
        self.first_divergent_group.map_or("none", |g| GROUP_NAMES[g])
    }
}

pub struct PeerSummary {
    pub final_tick: u32,
    pub confirmed: u32,
    pub final_hash: u64,
    pub run_hash: u64,
    pub stall_frames: u64,
    pub applied: u64,
    pub stale: u64,
    pub hashes_compared: u64,
    pub rollback: RollbackStats,
    pub snapshot_time: Duration,
    pub snapshots_taken: u64,
    pub stepping: Duration,
    pub injected_kick: Option<Kick>,
}

pub struct Outcome {
    pub frames: u64,
    pub elapsed: Duration,
    pub link: LinkStats,
    pub peers: [PeerSummary; 2],
    /// Both peers' hashes at every tick both confirmed, compared after the run.
    pub hashes_identical: bool,
    pub first_divergent_tick: Option<u32>,
    pub divergence: Option<DivergenceReport>,
    /// Peer 0's records, for the sequential oracle.
    pub records: Vec<Vec<Kick>>,
}

fn summarize(peer: &Peer) -> PeerSummary {
    PeerSummary {
        final_tick: peer.tick(),
        confirmed: peer.confirmed(),
        final_hash: peer.final_hash(),
        run_hash: peer.run_hash(),
        stall_frames: peer.stall_frames,
        applied: peer.applied,
        stale: peer.stale,
        hashes_compared: peer.hashes_compared,
        rollback: peer.rollback,
        snapshot_time: peer.snapshot_time,
        snapshots_taken: peer.snapshots_taken,
        stepping: peer.stepping(),
        injected_kick: peer.injected_kick,
    }
}

fn divergence_report(peers: &[Peer; 2]) -> Option<DivergenceReport> {
    let detected: Vec<&Divergence> = peers.iter().filter_map(|p| p.divergence.as_ref()).collect();
    let first = detected.iter().min_by_key(|d| (d.tick, d.detected_at_frame))?;
    let tick = first.tick;
    let snapshots = [peers[0].retained_snapshot(tick), peers[1].retained_snapshot(tick)];
    let mut report = DivergenceReport {
        tick,
        detected_at_frame: first.detected_at_frame,
        detected_by: peers.iter().filter(|p| p.divergence.is_some()).map(|p| p.id).collect(),
        group_hashes: None,
        first_divergent_group: None,
        snapshot_bytes: None,
        snapshots_identical: None,
        first_differing_byte: None,
    };
    if let [Some(a), Some(b)] = snapshots {
        let hashes = [group_hashes(&a.state().expect("a retained snapshot decodes")), group_hashes(&b.state().expect("a retained snapshot decodes"))];
        report.first_divergent_group = first_divergent_group(&hashes[0], &hashes[1]);
        report.group_hashes = Some(hashes);
        report.snapshot_bytes = Some([a.bytes.len(), b.bytes.len()]);
        report.snapshots_identical = Some(a.bytes == b.bytes);
        report.first_differing_byte = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y).or_else(|| (a.bytes.len() != b.bytes.len()).then_some(a.bytes.len().min(b.bytes.len())));
    }
    Some(report)
}

/// Runs the two peers frame by frame until both confirmed every tick, or a
/// divergence was detected (the run stops there, the states of the tick kept).
pub fn run(settings: &Settings) -> Outcome {
    let mode = if settings.rollback_peer { Mode::Rollback } else { Mode::Lockstep };
    let mut peers = [
        Peer::new(0, Mode::Lockstep, settings.config, settings.delay, settings.ticks, settings.ring_depth, None),
        Peer::new(1, mode, settings.config, settings.delay, settings.ticks, settings.ring_depth, settings.injection),
    ];
    let mut link = Link::new(settings.link, settings.link_seed);
    let guard = u64::from(settings.ticks) * 40 + 1_000;
    let started = Instant::now();
    // Frame 0 is the exchange of the pre-issued inputs: both peers send before
    // anyone steps, and a peer steps its first tick when they have arrived.
    let mut frame = 0_u64;
    for id in 0..2 {
        let bytes = encode(&peers[id].outgoing());
        link.send(frame, 1 - id, bytes);
    }
    loop {
        frame += 1;
        for id in 0..2 {
            for bytes in link.deliver(frame, id) {
                let payload = decode(&bytes).expect("the link carries what was encoded");
                peers[id].receive(payload, frame);
            }
            peers[id].advance(frame);
            let bytes = encode(&peers[id].outgoing());
            link.send(frame, 1 - id, bytes);
        }
        let detected = peers.iter().any(|p| p.divergence.is_some());
        let finished = peers.iter().all(|p| p.confirmed() >= settings.ticks);
        if detected || finished || frame >= guard {
            break;
        }
    }
    let elapsed = started.elapsed();
    let compared = peers[0].confirmed().min(peers[1].confirmed()) as usize;
    let first_divergent_tick = reference_workload::first_divergent_tick(&peers[0].hashes()[..=compared], &peers[1].hashes()[..=compared]).map(|t| t as u32);
    Outcome {
        frames: frame + 1,
        elapsed,
        link: link.stats,
        hashes_identical: first_divergent_tick.is_none(),
        first_divergent_tick,
        divergence: divergence_report(&peers),
        records: peers[0].records(),
        peers: [summarize(&peers[0]), summarize(&peers[1])],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Size, oracle};

    const TICKS: u32 = 40;

    fn session(workers: usize, delay: u32, link: LinkSettings) -> Settings {
        Settings { config: WorldConfig::new(Size::Small, workers), ticks: TICKS, delay, link, link_seed: 7, ring_depth: 0, injection: None, rollback_peer: false }
    }

    fn perfect() -> LinkSettings {
        LinkSettings { latency: 0, jitter_max: 0, loss: 0.0 }
    }

    #[test]
    fn two_peers_over_a_perfect_link_run_the_oracles_run() {
        for workers in [1, 3] {
            let outcome = run(&session(workers, 2, perfect()));
            assert!(outcome.hashes_identical, "{workers} workers");
            assert_eq!(outcome.peers[0].final_tick, TICKS);
            assert_eq!(outcome.peers[1].final_tick, TICKS);
            assert_eq!(outcome.peers[0].stall_frames + outcome.peers[1].stall_frames, 0);
            assert!(outcome.divergence.is_none());
            assert_eq!(outcome.records.len(), TICKS as usize);
            assert!(outcome.records.iter().all(|r| r.len() == 2 * KICKS_PER_TICK));
            let oracle_hashes = oracle(&WorldConfig::new(Size::Small, 1), &outcome.records);
            assert_eq!(oracle_hashes.last(), Some(&outcome.peers[0].final_hash));
            assert_eq!(run_hash(&oracle_hashes), outcome.peers[1].run_hash);
            assert!(outcome.peers[0].applied > 0);
            assert!(outcome.peers[0].hashes_compared > 30, "{}", outcome.peers[0].hashes_compared);
            assert_eq!(outcome.link.dropped, 0);
        }
    }

    #[test]
    fn a_latency_above_the_input_delay_stalls_and_keeps_the_hashes_identical() {
        let fast = run(&session(2, 2, LinkSettings { latency: 2, jitter_max: 0, loss: 0.0 }));
        assert!(fast.peers[0].stall_frames <= 1 && fast.peers[1].stall_frames <= 1, "latency at the delay stalls for the first inputs' travel only: {:?}", (fast.peers[0].stall_frames, fast.peers[1].stall_frames));
        assert_eq!(fast.frames, TICKS as u64 + 2, "the handshake frame, the first inputs' travel, then one tick per frame");
        let slow = run(&session(2, 2, LinkSettings { latency: 4, jitter_max: 0, loss: 0.0 }));
        assert!(slow.hashes_identical);
        assert!(slow.peers[0].stall_frames > 10 && slow.peers[1].stall_frames > 10, "{:?}", (slow.peers[0].stall_frames, slow.peers[1].stall_frames));
        assert!(slow.frames > fast.frames);
        assert_eq!(slow.peers[0].final_hash, fast.peers[0].final_hash, "the link changes the timing, never the result");
    }

    #[test]
    fn a_lossy_link_delivers_every_input_through_the_redundancy() {
        let outcome = run(&session(2, 2, LinkSettings { latency: 1, jitter_max: 1, loss: 0.2 }));
        assert!(outcome.link.dropped > 5, "{}", outcome.link.dropped);
        assert!(outcome.hashes_identical);
        assert_eq!(outcome.peers[0].confirmed, TICKS);
        assert_eq!(outcome.peers[1].confirmed, TICKS);
        let perfect = run(&session(2, 2, perfect()));
        assert_eq!(outcome.peers[0].final_hash, perfect.peers[0].final_hash);
    }

    #[test]
    fn a_local_only_kick_is_detected_at_its_tick_with_the_velocities_first() {
        let mut settings = session(2, 2, LinkSettings { latency: 1, jitter_max: 0, loss: 0.0 });
        settings.ring_depth = 16;
        settings.injection = Some(20);
        let outcome = run(&settings);
        assert!(!outcome.hashes_identical);
        assert_eq!(outcome.first_divergent_tick, Some(20));
        let report = outcome.divergence.expect("detected");
        assert_eq!(report.tick, 20);
        assert!(report.detected_at_frame >= 20 && report.detected_at_frame <= 24, "{}", report.detected_at_frame);
        assert!(!report.detected_by.is_empty());
        assert_eq!(report.first_divergent_group, Some(0));
        assert_eq!(report.group_name(), "input_and_repulsion_velocities");
        assert_eq!(report.snapshots_identical, Some(false));
        assert!(report.first_differing_byte.is_some());
        assert!(outcome.peers[1].injected_kick.is_some());
        assert!(outcome.peers[0].injected_kick.is_none());
        settings.injection = None;
        let clean = run(&settings);
        assert!(clean.hashes_identical && clean.divergence.is_none());
    }

    #[test]
    fn the_rollback_peer_rolls_back_and_its_confirmed_hashes_equal_the_lockstep_peers() {
        let mut settings = session(2, 2, LinkSettings { latency: 3, jitter_max: 1, loss: 0.0 });
        settings.ring_depth = 8;
        settings.rollback_peer = true;
        let outcome = run(&settings);
        let rollback = &outcome.peers[1].rollback;
        assert!(rollback.rollbacks > 5, "{rollback:?}");
        assert!(rollback.predicted_ticks > 5 && rollback.mispredicted_ticks >= rollback.rollbacks, "{rollback:?}");
        assert!(rollback.resimulated_max >= 1 && rollback.resimulated_max <= 8);
        assert_eq!(outcome.peers[1].confirmed, TICKS);
        assert!(outcome.hashes_identical);
        assert!(outcome.divergence.is_none());
        let perfect = run(&session(2, 2, perfect()));
        assert_eq!(outcome.peers[0].run_hash, perfect.peers[0].run_hash, "the inputs are the same values whatever the link");
        assert!(outcome.peers[1].snapshots_taken as u32 > TICKS);
        let eager = run(&Settings { link: LinkSettings { latency: 1, jitter_max: 0, loss: 0.0 }, ..settings });
        assert_eq!(eager.peers[1].rollback.rollbacks, 0, "inputs arrive before they are needed");
        assert!(eager.hashes_identical);
    }

    #[test]
    fn the_link_is_deterministic_for_a_seed_and_the_encoding_round_trips() {
        let settings = LinkSettings { latency: 2, jitter_max: 3, loss: 0.3 };
        let pattern = |seed| {
            let mut link = Link::new(settings, seed);
            for frame in 0..50 {
                link.send(frame, (frame % 2) as usize, vec![frame as u8]);
            }
            let mut delivered = Vec::new();
            for frame in 0..60 {
                delivered.push(link.deliver(frame, 0));
                delivered.push(link.deliver(frame, 1));
            }
            (link.stats, delivered)
        };
        assert_eq!(pattern(1), pattern(1));
        assert_ne!(pattern(1).0, pattern(2).0);
        let (stats, _) = pattern(1);
        assert_eq!(stats.sent, 50);
        assert_eq!(stats.delivered + stats.dropped, 50);
        assert!(stats.dropped > 5);

        let payload = Payload {
            source: 1,
            ack: 7,
            hash_tick: 9,
            hash: 0x0123_4567_89ab_cdef,
            inputs: vec![
                Input { source: 1, tick: 10, kicks: vec![Kick { index: 3, generation: 1, delta_vx: -2.5, delta_vy: 8.0 }] },
                Input { source: 1, tick: 11, kicks: Vec::new() },
            ],
        };
        let bytes = encode(&payload);
        assert_eq!(bytes.len(), PAYLOAD_HEADER_BYTES + 2 * INPUT_HEADER_BYTES + KICK_BYTES);
        assert_eq!(decode(&bytes).unwrap(), payload);
        assert!(decode(&bytes[..bytes.len() - 1]).unwrap_err().contains("truncated"));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing).unwrap_err().contains("trailing"));
    }
}
