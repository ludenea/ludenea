//! The simulated link of the multiplayer rounds spike. Throwaway, never
//! product code.
//!
//! The lockstep spike's link (a latency in frames, a jitter and a loss drawn
//! from a seeded stream) with what a test fixture driven by rounds needs:
//!
//! - **delivery at its advance**: a message sent in a round is handed to the
//!   link, which draws for it and moves it only when the fixture advances the
//!   link, after every peer has run. A message is received in the round after
//!   it was sent at the earliest, so a latency of zero means one frame;
//! - **state per direction**: each direction has its own random stream, its
//!   own queue and its own sequence, so the link never sees the order in
//!   which its two ends sent;
//! - **a log**: what each message became, in the order the link fixes (by
//!   direction, then by sequence), kept as a bounded ring and as one hash over
//!   every entry;
//! - **a schedule**: a loss rate that replaces a direction's own for a span
//!   of rounds, a burst or a total loss.
//!
//! The other form, [`Sharing::OneRandomState`], is the lockstep spike's link
//! as it was: one stream for both directions, drawn from when a message is
//! sent. It is here to show what the second condition is for.

use std::collections::VecDeque;

use reference_workload::{Fnv1a, SplitMix64};
use workload_lockstep::lockstep::LinkSettings;

/// From a peer to the relay.
pub const UP: usize = 0;
/// From the relay to a peer.
pub const DOWN: usize = 1;
/// The entries a link's ring keeps.
pub const LOG_RING: usize = 256;
/// What one entry of the log holds: the direction, the fate, the sequence,
/// the frame sent, the frame of delivery and the size.
pub const LOG_ENTRY_BYTES: usize = 1 + 1 + 8 + 8 + 8 + 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sharing {
    /// Nothing is shared by the two directions, and every draw is made at the
    /// link's advance.
    PerDirection,
    /// One random stream for both directions, drawn from at each send.
    OneRandomState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    /// With the frame the message is received in.
    Delivered(u64),
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub direction: u8,
    pub sequence: u64,
    pub sent: u64,
    pub size: u32,
    pub fate: Fate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkStats {
    pub sent: u64,
    pub delivered: u64,
    pub lost: u64,
    pub bytes: u64,
}

struct InFlight {
    delivery: u64,
    sequence: u64,
    bytes: Vec<u8>,
}

struct Direction {
    rng: SplitMix64,
    sequence: u64,
    /// Sent in the current round, waiting for the link's advance.
    outbox: Vec<Vec<u8>>,
    in_flight: Vec<InFlight>,
    /// What the receiving end takes when it next runs.
    inbox: Vec<Vec<u8>>,
    /// A loss rate that replaces the settings' until a round, excluded.
    loss_override: Option<(f32, u64)>,
}

pub struct Link {
    settings: LinkSettings,
    sharing: Sharing,
    directions: [Direction; 2],
    /// The one stream of [`Sharing::OneRandomState`].
    shared: SplitMix64,
    keep_log: bool,
    ring: VecDeque<LogEntry>,
    log_hash: Fnv1a,
    pub logged: u64,
    pub stats: LinkStats,
}

impl Link {
    pub fn new(settings: LinkSettings, seed: u64, sharing: Sharing, keep_log: bool) -> Link {
        let direction = |salt: u64| Direction { rng: SplitMix64::new(seed ^ salt), sequence: 0, outbox: Vec::new(), in_flight: Vec::new(), inbox: Vec::new(), loss_override: None };
        Link {
            settings,
            sharing,
            directions: [direction(0x5550_5550_5550_5550), direction(0x444f_574e_444f_574e)],
            shared: SplitMix64::new(seed),
            keep_log,
            ring: VecDeque::new(),
            log_hash: Fnv1a::new(),
            logged: 0,
            stats: LinkStats::default(),
        }
    }

    /// Replaces a direction's loss rate from now until `until`, excluded.
    pub fn override_loss(&mut self, direction: usize, loss: f32, until: u64) {
        self.directions[direction].loss_override = Some((loss, until));
    }

    fn loss(&self, direction: usize, frame: u64) -> f32 {
        match self.directions[direction].loss_override {
            Some((loss, until)) if frame < until => loss,
            _ => self.settings.loss,
        }
    }

    /// The frames a message takes: at least one, whatever the latency.
    fn transit(&self, jitter_draw: u64) -> u64 {
        let jitter = if self.settings.jitter_max == 0 { 0 } else { jitter_draw % (u64::from(self.settings.jitter_max) + 1) };
        (u64::from(self.settings.latency) + jitter).max(1)
    }

    fn log(&mut self, entry: LogEntry) {
        if !self.keep_log {
            return;
        }
        let (fate, delivery) = match entry.fate {
            Fate::Delivered(frame) => (1_u8, frame),
            Fate::Lost => (0, 0),
        };
        self.log_hash.write(&[entry.direction, fate]);
        self.log_hash.write(&entry.sequence.to_le_bytes());
        self.log_hash.write(&entry.sent.to_le_bytes());
        self.log_hash.write(&delivery.to_le_bytes());
        self.log_hash.write(&entry.size.to_le_bytes());
        self.logged += 1;
        if self.ring.len() == LOG_RING {
            self.ring.pop_front();
        }
        self.ring.push_back(entry);
    }

    /// Draws for one message and puts it in flight or loses it.
    fn draw(&mut self, direction: usize, frame: u64, bytes: Vec<u8>, loss_draw: f32, jitter_draw: u64) {
        let sequence = self.directions[direction].sequence;
        self.directions[direction].sequence += 1;
        self.stats.sent += 1;
        self.stats.bytes += bytes.len() as u64;
        let size = bytes.len() as u32;
        let fate = if loss_draw < self.loss(direction, frame) {
            self.stats.lost += 1;
            Fate::Lost
        } else {
            let delivery = frame + self.transit(jitter_draw);
            self.directions[direction].in_flight.push(InFlight { delivery, sequence, bytes });
            Fate::Delivered(delivery)
        };
        self.log(LogEntry { direction: direction as u8, sequence, sent: frame, size, fate });
    }

    /// Hands a message to the link in the round `frame`.
    pub fn send(&mut self, direction: usize, frame: u64, bytes: Vec<u8>) {
        match self.sharing {
            Sharing::PerDirection => self.directions[direction].outbox.push(bytes),
            Sharing::OneRandomState => {
                // The draws are made now, in the order the two ends happen to
                // send in: what the first form exists to avoid.
                let (loss_draw, jitter_draw) = (self.shared.u01(), self.shared.next());
                self.draw(direction, frame, bytes, loss_draw, jitter_draw);
            }
        }
    }

    /// What the receiving end of a direction takes when it runs.
    pub fn take(&mut self, direction: usize) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.directions[direction].inbox)
    }

    /// The end of the round `frame`: draws for what was sent in it, each
    /// direction from its own stream in sequence order, then moves what is
    /// due in the next round to the inboxes, ordered by delivery frame and
    /// sequence.
    pub fn advance(&mut self, frame: u64) {
        for direction in [UP, DOWN] {
            for bytes in std::mem::take(&mut self.directions[direction].outbox) {
                let state = &mut self.directions[direction];
                let (loss_draw, jitter_draw) = (state.rng.u01(), state.rng.next());
                self.draw(direction, frame, bytes, loss_draw, jitter_draw);
            }
            let state = &mut self.directions[direction];
            let (mut due, rest): (Vec<InFlight>, Vec<InFlight>) = std::mem::take(&mut state.in_flight).into_iter().partition(|message| message.delivery <= frame + 1);
            state.in_flight = rest;
            due.sort_by_key(|message| (message.delivery, message.sequence));
            self.stats.delivered += due.len() as u64;
            state.inbox.extend(due.into_iter().map(|message| message.bytes));
        }
    }

    /// One hash over every entry the log ever held.
    pub fn log_hash(&self) -> u64 {
        self.log_hash.finish()
    }

    /// The last entries, as the artifact a failed test would leave.
    pub fn ring(&self) -> impl Iterator<Item = &LogEntry> {
        self.ring.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(latency: u32, jitter_max: u32, loss: f32, sharing: Sharing) -> Link {
        Link::new(LinkSettings { latency, jitter_max, loss }, 42, sharing, true)
    }

    #[test]
    fn a_message_is_received_in_the_next_round_at_the_earliest() {
        for latency in [0, 1] {
            let mut link = link(latency, 0, 0.0, Sharing::PerDirection);
            link.send(UP, 5, vec![1]);
            assert!(link.take(UP).is_empty(), "nothing is delivered before the link advances");
            link.advance(5);
            assert_eq!(link.take(UP), vec![vec![1]], "latency {latency}: received in round 6");
        }
        let mut link = link(3, 0, 0.0, Sharing::PerDirection);
        link.send(DOWN, 5, vec![2]);
        for frame in 5..7 {
            link.advance(frame);
            assert!(link.take(DOWN).is_empty());
        }
        link.advance(7);
        assert_eq!(link.take(DOWN), vec![vec![2]], "a latency of three frames is received in round 8");
    }

    #[test]
    fn the_order_of_the_two_ends_changes_nothing_per_direction_and_something_with_one_stream() {
        let run = |sharing: Sharing, up_first: bool| {
            let mut link = link(1, 2, 0.3, sharing);
            for frame in 0..200 {
                let order = if up_first { [UP, DOWN] } else { [DOWN, UP] };
                for direction in order {
                    link.send(direction, frame, vec![direction as u8; 8]);
                }
                link.advance(frame);
                link.take(UP);
                link.take(DOWN);
            }
            (link.log_hash(), link.stats)
        };
        assert_eq!(run(Sharing::PerDirection, true), run(Sharing::PerDirection, false));
        assert_ne!(run(Sharing::OneRandomState, true).0, run(Sharing::OneRandomState, false).0);
    }

    #[test]
    fn a_total_loss_spans_its_rounds_and_ends() {
        let mut link = link(0, 0, 0.0, Sharing::PerDirection);
        link.override_loss(UP, 1.0, 12);
        let mut received = Vec::new();
        for frame in 10..14 {
            link.send(UP, frame, vec![frame as u8]);
            link.advance(frame);
            received.extend(link.take(UP));
        }
        assert_eq!(received, vec![vec![12], vec![13]]);
        assert_eq!((link.stats.lost, link.stats.delivered, link.logged), (2, 2, 4));
        assert_eq!(link.ring().filter(|entry| entry.fate == Fate::Lost).count(), 2);
    }

    #[test]
    fn the_ring_is_bounded_and_the_hash_covers_every_entry() {
        let mut kept = link(0, 0, 0.0, Sharing::PerDirection);
        let mut longer = link(0, 0, 0.0, Sharing::PerDirection);
        for frame in 0..LOG_RING as u64 + 10 {
            kept.send(UP, frame, vec![0]);
            kept.advance(frame);
            longer.send(UP, frame, vec![0]);
            longer.advance(frame);
        }
        assert_eq!(kept.ring().count(), LOG_RING);
        assert_eq!(kept.log_hash(), longer.log_hash());
        longer.send(UP, 999, vec![0]);
        longer.advance(999);
        assert_ne!(kept.log_hash(), longer.log_hash());
    }
}
