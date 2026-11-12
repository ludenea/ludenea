//! The peers of an agreed game and their relay. Throwaway, never product
//! code.
//!
//! The lockstep spike holds two peers that send each other their inputs. Here
//! any number of peers send their inputs to a relay, which merges the inputs
//! of every peer for a tick, in the order of the peers' identities, and
//! forwards the merged record to each. A peer steps a tick when it holds the
//! tick's merged record. One peer may roll back instead: it predicts the
//! inputs of the others, steps ahead inside a window, and restores a snapshot
//! and steps again when a merged record contradicts a prediction. The relay
//! has no rule for a peer that is gone: it waits.
//!
//! A peer is run once per round by the fixture, its host. It counts a
//! duration for each round on a clock of its own, and may step the ticks its
//! clock has reached, at most [`TICK_BUDGET`] in a round. Every round each
//! side sends everything the other has not acknowledged, which is the
//! reliable layer, as in the lockstep spike.

use std::collections::VecDeque;

use reference_workload::{SplitMix64, run_hash};
use workload_lockstep::world::{Kick, Snapshot, World, WorldConfig};

pub const KICKS_PER_TICK: usize = 4;
const KICK_RANGE: f32 = 8.0;
const LOCAL_ONLY_KICK: (f32, f32) = (16.0, 16.0);
/// The ticks a peer may step in one round when its clock is ahead.
pub const TICK_BUDGET: u32 = 4;
/// A peer's clock counts thousandths of a frame.
pub const FRAME: u64 = 1_000;
/// The source a recording gives a kick the peer applied and never sent.
pub const LOCAL_SOURCE: u32 = u32::MAX;

/// The inputs of every peer for one tick, by peer identity.
pub type Merged = Vec<Vec<Kick>>;

// ---------------------------------------------------------------------------
// The messages
// ---------------------------------------------------------------------------

fn put_kicks(bytes: &mut Vec<u8>, kicks: &[Kick]) {
    bytes.push(kicks.len() as u8);
    for kick in kicks {
        bytes.extend_from_slice(&kick.index.to_le_bytes());
        bytes.extend_from_slice(&kick.generation.to_le_bytes());
        bytes.extend_from_slice(&kick.delta_vx.to_le_bytes());
        bytes.extend_from_slice(&kick.delta_vy.to_le_bytes());
    }
}

struct Cursor<'a>(&'a [u8], usize);

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let end = self.1.checked_add(N).filter(|&end| end <= self.0.len()).ok_or_else(|| format!("message truncated at byte {}", self.1))?;
        let taken = self.0[self.1..end].try_into().expect("the length was checked");
        self.1 = end;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn kicks(&mut self) -> Result<Vec<Kick>, String> {
        let count = self.u8()?;
        (0..count).map(|_| Ok(Kick { index: self.u32()?, generation: self.u32()?, delta_vx: f32::from_le_bytes(self.take()?), delta_vy: f32::from_le_bytes(self.take()?) })).collect()
    }

    fn end(&self) -> Result<(), String> {
        if self.1 == self.0.len() { Ok(()) } else { Err(format!("{} trailing bytes", self.0.len() - self.1)) }
    }
}

/// What a peer sends the relay each round: the highest tick up to which it
/// holds the merged records, and every input of its own the relay has not
/// acknowledged, from `first_tick` on.
#[derive(Clone, Debug, PartialEq)]
pub struct FromPeer {
    pub source: u16,
    pub ack: u32,
    pub first_tick: u32,
    pub inputs: Vec<Vec<Kick>>,
}

/// What the relay sends a peer each round: the highest tick up to which it
/// holds that peer's inputs, and every merged record the peer has not
/// acknowledged, from `first_tick` on.
#[derive(Clone, Debug, PartialEq)]
pub struct FromRelay {
    pub ack: u32,
    pub first_tick: u32,
    pub records: Vec<Merged>,
}

impl FromPeer {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.source.to_le_bytes());
        bytes.extend_from_slice(&self.ack.to_le_bytes());
        bytes.extend_from_slice(&self.first_tick.to_le_bytes());
        bytes.extend_from_slice(&(self.inputs.len() as u16).to_le_bytes());
        for kicks in &self.inputs {
            put_kicks(&mut bytes, kicks);
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<FromPeer, String> {
        let mut cursor = Cursor(bytes, 0);
        let (source, ack, first_tick, count) = (cursor.u16()?, cursor.u32()?, cursor.u32()?, cursor.u16()?);
        let inputs = (0..count).map(|_| cursor.kicks()).collect::<Result<_, _>>()?;
        cursor.end()?;
        Ok(FromPeer { source, ack, first_tick, inputs })
    }
}

impl FromRelay {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.ack.to_le_bytes());
        bytes.extend_from_slice(&self.first_tick.to_le_bytes());
        bytes.extend_from_slice(&(self.records.len() as u16).to_le_bytes());
        for record in &self.records {
            bytes.extend_from_slice(&(record.len() as u16).to_le_bytes());
            for kicks in record {
                put_kicks(&mut bytes, kicks);
            }
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<FromRelay, String> {
        let mut cursor = Cursor(bytes, 0);
        let (ack, first_tick, count) = (cursor.u32()?, cursor.u32()?, cursor.u16()?);
        let mut records = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let sources = cursor.u16()?;
            records.push((0..sources).map(|_| cursor.kicks()).collect::<Result<_, _>>()?);
        }
        cursor.end()?;
        Ok(FromRelay { ack, first_tick, records })
    }
}

// ---------------------------------------------------------------------------
// The relay
// ---------------------------------------------------------------------------

/// The relay peer: it holds no world. It keeps each peer's inputs by tick,
/// merges a tick once it holds every peer's inputs for it, and sends each peer
/// the merged records that peer has not acknowledged.
pub struct Relay {
    /// By peer, then by tick.
    inputs: Vec<Vec<Option<Vec<Kick>>>>,
    /// By peer, the highest tick up to which its inputs are held contiguously.
    contiguous: Vec<u32>,
    /// By peer, the highest merged tick it acknowledged.
    acknowledged: Vec<u32>,
    /// The messages a round may carry at most, so that a peer that was away
    /// is not sent the whole history in one.
    window: u32,
}

impl Relay {
    pub fn new(peers: usize, ticks: u32, delay: u32) -> Relay {
        Relay { inputs: vec![vec![None; (ticks + delay + 2) as usize]; peers], contiguous: vec![0; peers], acknowledged: vec![0; peers], window: 16 }
    }

    /// The highest tick every peer's inputs are held for.
    pub fn merged_through(&self) -> u32 {
        self.contiguous.iter().copied().min().unwrap_or(0)
    }

    /// One round of the relay: takes what each peer's link delivered, then
    /// answers every peer.
    pub fn round(&mut self, incoming: Vec<Vec<Vec<u8>>>) -> Vec<Vec<u8>> {
        for bytes in incoming.iter().flatten() {
            let message = FromPeer::decode(bytes).expect("the link carries what a peer encoded");
            let peer = message.source as usize;
            self.acknowledged[peer] = self.acknowledged[peer].max(message.ack);
            for (offset, kicks) in message.inputs.into_iter().enumerate() {
                let slot = &mut self.inputs[peer][message.first_tick as usize + offset];
                match slot {
                    Some(known) => assert_eq!(known, &kicks, "an input is a value: a retransmission repeats it"),
                    None => *slot = Some(kicks),
                }
            }
            while self.inputs[peer].get(self.contiguous[peer] as usize + 1).is_some_and(Option::is_some) {
                self.contiguous[peer] += 1;
            }
        }
        let merged_through = self.merged_through();
        (0..self.inputs.len())
            .map(|peer| {
                let first_tick = self.acknowledged[peer] + 1;
                let last_tick = merged_through.min(self.acknowledged[peer] + self.window);
                let records = (first_tick..=last_tick).map(|tick| self.inputs.iter().map(|inputs| inputs[tick as usize].clone().expect("merged ticks hold every peer's inputs")).collect()).collect();
                FromRelay { ack: self.contiguous[peer], first_tick, records }.encode()
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// A peer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Lockstep,
    /// With the window of ticks the peer may step past its confirmed tick.
    Rollback(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RollbackStats {
    pub rollbacks: u64,
    pub predicted_ticks: u64,
    pub resimulated_ticks: u64,
}

pub struct Peer {
    pub id: usize,
    peers: usize,
    mode: Mode,
    config: WorldConfig,
    world: World,
    delay: u32,
    target: u32,
    stream: SplitMix64,
    own: Vec<Option<Vec<Kick>>>,
    issued: u32,
    merged: Vec<Option<Merged>>,
    merged_contiguous: u32,
    acknowledged_by_relay: u32,
    /// The hash of each tick as last stepped; final up to the confirmed tick.
    hashes: Vec<u64>,
    confirmed: u32,
    /// By tick, the record a prediction stepped it with, until it is confirmed.
    predicted: Vec<Option<Merged>>,
    last_known: Option<Merged>,
    ring: VecDeque<(u32, Snapshot)>,
    /// The tick at which the peer applies a kick it never sends.
    injection: Option<u32>,
    pub injected: Option<Kick>,
    /// Thousandths of a frame counted so far.
    clock: u64,
    /// What the clock read one frame before the first tick was stepped: the
    /// rounds the connection took are not time the game is late by.
    origin: Option<u64>,
    /// Thousandths of a frame counted for one round.
    pub rate: u64,
    pub stalled_rounds: u64,
    pub rollback: RollbackStats,
}

impl Peer {
    pub fn new(id: usize, peers: usize, mode: Mode, config: WorldConfig, delay: u32, target: u32, injection: Option<u32>) -> Peer {
        assert!(delay >= 1, "the input delay is at least one tick");
        let world = World::new(config);
        let slots = (target + delay + 2) as usize;
        let mut hashes = vec![0; target as usize + 1];
        hashes[0] = world.hash();
        let mut peer = Peer {
            id,
            peers,
            mode,
            config,
            world,
            delay,
            target,
            stream: SplitMix64::new(config.seed ^ (0x9e37_79b9_7f4a_7c15_u64.wrapping_mul(id as u64 + 1))),
            own: vec![None; slots],
            issued: 0,
            merged: vec![None; slots],
            merged_contiguous: 0,
            acknowledged_by_relay: 0,
            hashes,
            confirmed: 0,
            predicted: vec![None; target as usize + 1],
            last_known: None,
            ring: VecDeque::new(),
            injection,
            injected: None,
            clock: 0,
            origin: None,
            rate: FRAME,
            stalled_rounds: 0,
            rollback: RollbackStats::default(),
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

    /// The last tick stepped with every peer's real inputs.
    pub fn confirmed(&self) -> u32 {
        self.confirmed
    }

    /// The hash of a confirmed tick.
    pub fn confirmed_hash(&self, tick: u32) -> Option<u64> {
        (tick <= self.confirmed).then(|| self.hashes[tick as usize])
    }

    pub fn hashes(&self) -> &[u64] {
        &self.hashes[..=self.confirmed as usize]
    }

    pub fn run_hash(&self) -> u64 {
        run_hash(self.hashes())
    }

    /// The hash of the tick the world stands at, confirmed or not.
    pub fn present_hash(&self) -> u64 {
        self.hashes[self.world.tick() as usize]
    }

    pub fn config(&self) -> WorldConfig {
        self.config
    }

    /// The merged record of a confirmed tick.
    pub fn record(&self, tick: u32) -> &Merged {
        self.merged[tick as usize].as_ref().expect("a confirmed tick holds its merged record")
    }

    /// The tick of the kick the peer applied and never sent, once applied.
    pub fn injection(&self) -> Option<(u32, Kick)> {
        self.injection.zip(self.injected)
    }

    fn retain_snapshot(&mut self) {
        let Mode::Rollback(window) = self.mode else { return };
        self.ring.push_back((self.world.tick(), self.world.snapshot().expect("a healthy world snapshots")));
        while self.ring.len() > window as usize + 1 {
            self.ring.pop_front();
        }
    }

    /// The peer's input for `tick`, drawn from its stream against its local
    /// state, as in the lockstep spike.
    fn issue(&mut self, tick: u32) {
        let state = self.world.state();
        let entities = state.len() as u64;
        let kicks = (0..KICKS_PER_TICK)
            .map(|_| {
                let index = (self.stream.next() % entities) as u32;
                let generation = state.generation[index as usize];
                let delta_vx = (self.stream.u01() * (2.0 * KICK_RANGE)) - KICK_RANGE;
                let delta_vy = (self.stream.u01() * (2.0 * KICK_RANGE)) - KICK_RANGE;
                Kick { index, generation, delta_vx, delta_vy }
            })
            .collect();
        self.own[tick as usize] = Some(kicks);
        self.issued = tick;
    }

    /// The record a tick is stepped with: the merged one where it is held,
    /// otherwise the peer's own input beside the others' last known ones.
    fn record_for(&self, tick: u32) -> (Merged, bool) {
        if let Some(real) = &self.merged[tick as usize] {
            return (real.clone(), false);
        }
        let own = self.own[tick as usize].clone().expect("own inputs are issued ahead");
        let mut record = self.last_known.clone().unwrap_or_else(|| vec![Vec::new(); self.peers]);
        record[self.id] = own;
        (record, true)
    }

    fn local_only_kick(&self) -> Kick {
        let state = self.world.state();
        let index = (0..state.len()).find(|&slot| state.alive[slot]).expect("some entity is alive") as u32;
        Kick { index, generation: state.generation[index as usize], delta_vx: LOCAL_ONLY_KICK.0, delta_vy: LOCAL_ONLY_KICK.1 }
    }

    fn step_tick(&mut self, tick: u32) {
        let (record, predicted) = self.record_for(tick);
        let mut kicks: Vec<Kick> = record.iter().flatten().copied().collect();
        if self.injection == Some(tick) {
            let kick = self.local_only_kick();
            kicks.push(kick);
            self.injected = Some(kick);
        }
        self.world.step(vec![kicks]).expect("the peer's world steps");
        self.hashes[tick as usize] = self.world.hash();
        self.predicted[tick as usize] = predicted.then_some(record);
        self.rollback.predicted_ticks += u64::from(predicted);
        self.retain_snapshot();
    }

    fn confirm(&mut self) {
        while self.confirmed < self.world.tick() {
            let next = self.confirmed as usize + 1;
            if self.merged[next].is_none() || self.predicted[next].is_some() {
                break;
            }
            self.confirmed += 1;
        }
    }

    /// Releases the predictions the merged records confirm, and at the first
    /// one they contradict restores the snapshot of the tick before it and
    /// steps again to the present tick.
    fn reconcile(&mut self) {
        let current = self.world.tick();
        let mut from = None;
        for tick in self.confirmed + 1..=current {
            let slot = tick as usize;
            let (Some(real), Some(prediction)) = (&self.merged[slot], &self.predicted[slot]) else { continue };
            if prediction == real {
                self.predicted[slot] = None;
            } else {
                from = Some(tick);
                break;
            }
        }
        let Some(from) = from else { return };
        while self.ring.back().is_some_and(|(tick, _)| *tick >= from) {
            self.ring.pop_back();
        }
        let (restored, snapshot) = self.ring.back().expect("the ring holds the confirmed tick and the ticks after it");
        assert_eq!(*restored, from - 1, "the ring holds the tick before the contradicted one");
        self.world = World::restore(snapshot, self.config.workers).expect("a retained snapshot restores");
        for tick in from..=current {
            self.step_tick(tick);
        }
        self.rollback.rollbacks += 1;
        self.rollback.resimulated_ticks += u64::from(current - from + 1);
    }

    fn receive(&mut self, message: FromRelay) {
        self.acknowledged_by_relay = self.acknowledged_by_relay.max(message.ack);
        for (offset, record) in message.records.into_iter().enumerate() {
            let slot = message.first_tick as usize + offset;
            match &self.merged[slot] {
                Some(known) => assert_eq!(known, &record, "a merged record is a value: a retransmission repeats it"),
                None => self.merged[slot] = Some(record),
            }
        }
        while self.merged.get(self.merged_contiguous as usize + 1).is_some_and(Option::is_some) {
            self.merged_contiguous += 1;
        }
        if self.merged_contiguous > 0 {
            self.last_known = self.merged[self.merged_contiguous as usize].clone();
        }
        if matches!(self.mode, Mode::Rollback(_)) {
            self.reconcile();
        }
        self.confirm();
    }

    /// A round the peer does not run. Its clock goes on through a stall and
    /// stops through a suspension.
    pub fn skip_round(&mut self, clock_runs: bool) {
        if clock_runs {
            self.clock += self.rate;
        }
    }

    /// One round of the peer's host: it receives, steps the ticks its clock
    /// has reached and the protocol allows, and sends.
    pub fn round(&mut self, incoming: Vec<Vec<u8>>) -> Vec<u8> {
        for bytes in incoming {
            self.receive(FromRelay::decode(&bytes).expect("the link carries what the relay encoded"));
        }
        self.clock += self.rate;
        let due = self.origin.map_or(1, |origin| (self.clock - origin) / FRAME).min(u64::from(self.target)) as u32;
        let mut stepped = 0;
        while stepped < TICK_BUDGET && self.world.tick() < due {
            let tick = self.world.tick() + 1;
            let allowed = match self.mode {
                Mode::Lockstep => self.merged[tick as usize].is_some(),
                Mode::Rollback(window) => tick <= self.confirmed + window,
            };
            if !allowed {
                self.stalled_rounds += u64::from(stepped == 0);
                break;
            }
            self.origin.get_or_insert(self.clock - FRAME);
            self.step_tick(tick);
            self.issue(tick + self.delay);
            self.confirm();
            stepped += 1;
        }
        let first_tick = self.acknowledged_by_relay + 1;
        let inputs = (first_tick..=self.issued).map(|tick| self.own[tick as usize].clone().expect("issued")).collect();
        FromPeer { source: self.id as u16, ack: self.merged_contiguous, first_tick, inputs }.encode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workload_lockstep::world::Size;

    fn kick(index: u32) -> Kick {
        Kick { index, generation: 1, delta_vx: 0.5, delta_vy: -0.25 }
    }

    #[test]
    fn the_messages_read_back_as_written() {
        let from_peer = FromPeer { source: 3, ack: 7, first_tick: 9, inputs: vec![vec![kick(1), kick(2)], vec![], vec![kick(3)]] };
        assert_eq!(FromPeer::decode(&from_peer.encode()), Ok(from_peer.clone()));
        let from_relay = FromRelay { ack: 4, first_tick: 5, records: vec![vec![vec![kick(1)], vec![kick(2), kick(3)]], vec![vec![], vec![]]] };
        assert_eq!(FromRelay::decode(&from_relay.encode()), Ok(from_relay.clone()));
        let mut cut = from_relay.encode();
        cut.pop();
        assert!(FromRelay::decode(&cut).is_err());
        let mut longer = from_peer.encode();
        longer.push(0);
        assert!(FromPeer::decode(&longer).is_err());
    }

    #[test]
    fn the_relay_merges_a_tick_once_it_holds_every_peers_inputs() {
        let mut relay = Relay::new(2, 8, 2);
        let from = |source: u16, ticks: std::ops::RangeInclusive<u32>| FromPeer { source, ack: 0, first_tick: *ticks.start(), inputs: ticks.map(|tick| vec![kick(tick * 10 + u32::from(source))]).collect() }.encode();
        let answers = relay.round(vec![vec![from(0, 1..=2)], vec![]]);
        assert_eq!(relay.merged_through(), 0);
        assert_eq!(FromRelay::decode(&answers[0]), Ok(FromRelay { ack: 2, first_tick: 1, records: vec![] }));
        let answers = relay.round(vec![vec![], vec![from(1, 1..=1)]]);
        assert_eq!(relay.merged_through(), 1);
        let expected = FromRelay { ack: 1, first_tick: 1, records: vec![vec![vec![kick(10)], vec![kick(11)]]] };
        assert_eq!(FromRelay::decode(&answers[1]), Ok(expected));
    }

    #[test]
    fn a_stalled_peer_finds_its_clock_ahead_and_a_suspended_one_does_not() {
        let peer = |clock_runs: bool| {
            let mut peer = Peer::new(0, 1, Mode::Lockstep, WorldConfig::new(Size::Small, 1), 2, 20, None);
            for _ in 0..6 {
                peer.skip_round(clock_runs);
            }
            peer.clock
        };
        assert_eq!(peer(true), 6 * FRAME);
        assert_eq!(peer(false), 0);
    }
}
