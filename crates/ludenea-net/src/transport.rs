//! How packets move between peers: the [`Transport`] trait, an in-memory network for tests, and
//! a seeded [`Conditioned`] wrapper that adds latency, jitter, loss, duplication and reordering
//! deterministically.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use ludenea_app::Rng;
use thiserror::Error;

use crate::settings::{MAX_PACKET_BYTES, PeerId};

/// Why a transport refused to send a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TransportError {
    /// The destination is not a peer this transport can reach.
    #[error("{peer} is not reachable from this transport")]
    UnknownPeer { peer: PeerId },
    /// The packet is longer than [`MAX_PACKET_BYTES`].
    #[error("a packet of {len} bytes exceeds the {MAX_PACKET_BYTES} byte limit")]
    TooLarge { len: usize },
}

/// Refuses a packet longer than [`MAX_PACKET_BYTES`]; the one place the size rule is written.
fn check_size(len: usize) -> Result<(), TransportError> {
    if len > MAX_PACKET_BYTES {
        return Err(TransportError::TooLarge { len });
    }
    Ok(())
}

/// A datagram channel to the other peers of a match: unreliable and unordered by contract.
///
/// The caller passes the current time in monotonic microseconds; a transport reads no clock.
pub trait Transport {
    /// Sends one packet to `to`.
    fn send(&mut self, to: PeerId, packet: Vec<u8>, now_micros: u64) -> Result<(), TransportError>;

    /// The next packet that has arrived by `now_micros`, with its sender.
    fn receive(&mut self, now_micros: u64) -> Option<(PeerId, Vec<u8>)>;
}

type Inbox = VecDeque<(PeerId, Vec<u8>)>;

/// The endpoint of one peer on a [`MemoryNetwork`]. Delivery is immediate and in send order.
/// A peer never sends to itself: its own id is an unknown peer.
#[derive(Debug)]
pub struct MemoryTransport {
    local: PeerId,
    inboxes: Rc<RefCell<Vec<Inbox>>>,
}

/// An in-process network of peers `0..peers`, single-threaded. It is a helper for tests and
/// local play: any count up to 255 is accepted, and [`crate::MAX_PEERS`] is a limit of
/// match settings, enforced there.
#[derive(Debug)]
pub struct MemoryNetwork;

impl MemoryNetwork {
    /// One transport per peer, indexed by peer number.
    #[allow(clippy::new_ret_no_self, reason = "a network is only its endpoints")]
    pub fn new(peers: u8) -> Vec<MemoryTransport> {
        let inboxes = Rc::new(RefCell::new(vec![Inbox::new(); usize::from(peers)]));
        (0..peers)
            .map(|id| MemoryTransport {
                local: PeerId(id),
                inboxes: Rc::clone(&inboxes),
            })
            .collect()
    }
}

impl Transport for MemoryTransport {
    fn send(
        &mut self,
        to: PeerId,
        packet: Vec<u8>,
        _now_micros: u64,
    ) -> Result<(), TransportError> {
        if to == self.local || usize::from(to.0) >= self.inboxes.borrow().len() {
            return Err(TransportError::UnknownPeer { peer: to });
        }
        check_size(packet.len())?;
        self.inboxes.borrow_mut()[usize::from(to.0)].push_back((self.local, packet));
        Ok(())
    }

    fn receive(&mut self, _now_micros: u64) -> Option<(PeerId, Vec<u8>)> {
        self.inboxes.borrow_mut()[usize::from(self.local.0)].pop_front()
    }
}

/// The impairments of a link. Integers only: 5% loss is `loss_per_mille: 50`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkConditions {
    /// Fixed delay of every packet.
    pub latency_micros: u32,
    /// Extra delay drawn uniformly from `0..=jitter_micros`.
    pub jitter_micros: u32,
    /// Chance, in thousandths, that a packet is lost.
    pub loss_per_mille: u16,
    /// Chance, in thousandths, that a packet is delivered twice.
    pub duplicate_per_mille: u16,
    /// Chance, in thousandths, that a copy is delayed by `reorder_extra_micros` more.
    pub reorder_per_mille: u16,
    /// The extra delay of a reordered copy.
    pub reorder_extra_micros: u32,
    /// Seeds the draws; each destination has its own stream.
    pub seed: u64,
}

/// Why [`LinkConditions`] were refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ConditionsError {
    /// A per-mille value above 1000.
    #[error("{field} is {value} per mille, at most 1000 is allowed")]
    PerMilleTooLarge { field: &'static str, value: u16 },
    /// `jitter_micros` must stay below `u32::MAX` so the draw range is representable.
    #[error("jitter_micros must be below u32::MAX")]
    JitterTooLarge,
}

/// A transport that holds each sent packet back, loses it, duplicates it or reorders it,
/// deterministically for a seed, then hands it to the inner transport when it is due.
///
/// Outbound packets are released only when [`Transport::receive`] is called, so a peer must keep
/// calling `receive` for its sends to leave. `send` refuses an oversized packet at once; it cannot
/// know the inner transport's peers, so an unknown destination surfaces at release time, counted
/// by [`Conditioned::failed_sends`]. Such a packet is dropped, like on an unreliable link: the
/// session's redundancy resends it.
pub struct Conditioned<T: Transport> {
    inner: T,
    conditions: LinkConditions,
    links: BTreeMap<PeerId, Rng>,
    /// Keyed by due time, then send order, so equal due times keep their send order.
    pending: BTreeMap<(u64, u64), (PeerId, Vec<u8>)>,
    sequence: u64,
    failed_sends: u64,
    last_send_error: Option<TransportError>,
}

impl<T: Transport> Conditioned<T> {
    pub fn new(inner: T, conditions: LinkConditions) -> Result<Self, ConditionsError> {
        for (field, value) in [
            ("loss_per_mille", conditions.loss_per_mille),
            ("duplicate_per_mille", conditions.duplicate_per_mille),
            ("reorder_per_mille", conditions.reorder_per_mille),
        ] {
            if value > 1000 {
                return Err(ConditionsError::PerMilleTooLarge { field, value });
            }
        }
        if conditions.jitter_micros == u32::MAX {
            return Err(ConditionsError::JitterTooLarge);
        }
        Ok(Self {
            inner,
            conditions,
            links: BTreeMap::new(),
            pending: BTreeMap::new(),
            sequence: 0,
            failed_sends: 0,
            last_send_error: None,
        })
    }

    /// How many due packets the inner transport refused (and so were dropped).
    pub fn failed_sends(&self) -> u64 {
        self.failed_sends
    }

    /// The error of the latest such refusal.
    pub fn last_send_error(&self) -> Option<&TransportError> {
        self.last_send_error.as_ref()
    }

    fn due(conditions: &LinkConditions, rng: &mut Rng, now_micros: u64) -> u64 {
        let jitter = rng.range_u32(0, conditions.jitter_micros + 1);
        let reordered = rng.range_u32(0, 1000) < u32::from(conditions.reorder_per_mille);
        let extra = if reordered {
            u64::from(conditions.reorder_extra_micros)
        } else {
            0
        };
        now_micros
            .saturating_add(u64::from(conditions.latency_micros))
            .saturating_add(u64::from(jitter))
            .saturating_add(extra)
    }
}

impl<T: Transport> Transport for Conditioned<T> {
    fn send(&mut self, to: PeerId, packet: Vec<u8>, now_micros: u64) -> Result<(), TransportError> {
        check_size(packet.len())?;
        let conditions = self.conditions;
        let rng = self
            .links
            .entry(to)
            .or_insert_with(|| Rng::new(conditions.seed, u64::from(to.0)));
        if rng.range_u32(0, 1000) < u32::from(conditions.loss_per_mille) {
            return Ok(());
        }
        let copies = if rng.range_u32(0, 1000) < u32::from(conditions.duplicate_per_mille) {
            2
        } else {
            1
        };
        for _ in 0..copies {
            let due = Self::due(&conditions, rng, now_micros);
            self.pending
                .insert((due, self.sequence), (to, packet.clone()));
            self.sequence += 1;
        }
        Ok(())
    }

    fn receive(&mut self, now_micros: u64) -> Option<(PeerId, Vec<u8>)> {
        while let Some(entry) = self.pending.first_entry() {
            if entry.key().0 > now_micros {
                break;
            }
            let (to, packet) = entry.remove();
            if let Err(error) = self.inner.send(to, packet, now_micros) {
                self.failed_sends += 1;
                self.last_send_error = Some(error);
            }
        }
        self.inner.receive(now_micros)
    }
}
