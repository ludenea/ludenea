//! The multiplayer fixture: the worlds, the relay, the links, the rounds, the
//! schedule and the oracle. Throwaway, never product code.
//!
//! The fixture is every peer's host. A round is one frame of the test's time:
//! each participant (the peers and the relay) runs its host's part once, it
//! receives, steps and sends, then every link advances by one frame. The
//! participants of a round run in a fixed order, in an order shuffled afresh
//! at every round from a seed, or at once on a pool of threads. No wall clock
//! is read by anything a hash depends on.
//!
//! After each round the oracle compares, for each tick, the hashes of the
//! peers that hold a confirmed hash for it, as soon as two do, and the run
//! stops at the first tick on which two differ.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use reference_workload::{Fnv1a, SplitMix64};
use workload_lockstep::lockstep::LinkSettings;
use workload_lockstep::world::WorldConfig;

use crate::link::{DOWN, Fate, Link, LinkStats, Sharing, UP};
use crate::protocol::{Mode, Peer, Relay, RollbackStats};

// ---------------------------------------------------------------------------
// The schedule
// ---------------------------------------------------------------------------

/// One event of a schedule: test data, placed at a round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The peer runs no round for a number of rounds; its links go on
    /// receiving and its clock goes on.
    Stall { peer: usize, rounds: u64 },
    /// The same with its clock stopped.
    Suspension { peer: usize, rounds: u64 },
    /// One direction of a peer's link loses at this rate for a number of rounds.
    LossBurst { link: usize, direction: usize, rounds: u64, per_mille: u32 },
    /// Both directions of a peer's link lose everything for a number of
    /// rounds: all a disconnection with its reconnection is to a protocol
    /// that holds no connection.
    TotalLoss { link: usize, rounds: u64 },
    /// The peer counts this many thousandths of a frame for a round from now on.
    ClockRate { peer: usize, per_mille: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scheduled {
    pub round: u64,
    pub event: Event,
}

/// How often each kind of event is drawn, in events per thousand rounds and
/// per peer or link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rates {
    pub stall: u32,
    pub suspension: u32,
    pub loss_burst: u32,
    pub total_loss: u32,
    pub clock_rate: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schedule(pub Vec<Scheduled>);

impl Schedule {
    /// A schedule drawn from a seed under stated rates, for the rounds before
    /// `quiet_from`: the span the test asks to be left without an event.
    pub fn drawn(seed: u64, rates: &Rates, peers: usize, quiet_from: u64) -> Schedule {
        let mut rng = SplitMix64::new(seed);
        let mut events = Vec::new();
        for round in 2..quiet_from {
            for target in 0..peers {
                let mut happens = |rate: u32| rng.next() % 1_000 < u64::from(rate);
                let (stall, suspension, burst, total, clock) = (happens(rates.stall), happens(rates.suspension), happens(rates.loss_burst), happens(rates.total_loss), happens(rates.clock_rate));
                let rounds = 2 + rng.next() % 8;
                let pick = rng.next();
                let event = if stall {
                    Event::Stall { peer: target, rounds }
                } else if suspension {
                    Event::Suspension { peer: target, rounds }
                } else if burst {
                    Event::LossBurst { link: target, direction: (pick % 2) as usize, rounds, per_mille: 600 }
                } else if total {
                    Event::TotalLoss { link: target, rounds }
                } else if clock {
                    Event::ClockRate { peer: target, per_mille: [900, 950, 1_050, 1_100][(pick % 4) as usize] }
                } else {
                    continue;
                };
                events.push(Scheduled { round, event });
            }
        }
        Schedule(events)
    }

    /// The events as a record keeps them: one per line.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for scheduled in &self.0 {
            let event = match scheduled.event {
                Event::Stall { peer, rounds } => format!("stall peer {peer} rounds {rounds}"),
                Event::Suspension { peer, rounds } => format!("suspension peer {peer} rounds {rounds}"),
                Event::LossBurst { link, direction, rounds, per_mille } => format!("loss_burst link {link} direction {direction} rounds {rounds} per_mille {per_mille}"),
                Event::TotalLoss { link, rounds } => format!("total_loss link {link} rounds {rounds}"),
                Event::ClockRate { peer, per_mille } => format!("clock_rate peer {peer} per_mille {per_mille}"),
            };
            text.push_str(&format!("round {} {event}\n", scheduled.round));
        }
        text
    }

    /// A schedule from its recorded events.
    pub fn parse(text: &str) -> Result<Schedule, String> {
        let mut events = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let words: Vec<&str> = line.split_whitespace().collect();
            let number = |name: &str| -> Result<u64, String> {
                let position = words.iter().position(|word| *word == name).ok_or_else(|| format!("no {name} in {line:?}"))?;
                words.get(position + 1).and_then(|value| value.parse().ok()).ok_or_else(|| format!("no value for {name} in {line:?}"))
            };
            let event = match words.get(2).copied() {
                Some("stall") => Event::Stall { peer: number("peer")? as usize, rounds: number("rounds")? },
                Some("suspension") => Event::Suspension { peer: number("peer")? as usize, rounds: number("rounds")? },
                Some("loss_burst") => Event::LossBurst { link: number("link")? as usize, direction: number("direction")? as usize, rounds: number("rounds")?, per_mille: number("per_mille")? as u32 },
                Some("total_loss") => Event::TotalLoss { link: number("link")? as usize, rounds: number("rounds")? },
                Some("clock_rate") => Event::ClockRate { peer: number("peer")? as usize, per_mille: number("per_mille")? },
                _ => return Err(format!("an event of no known kind: {line:?}")),
            };
            events.push(Scheduled { round: number("round")?, event });
        }
        Ok(Schedule(events))
    }
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// The peers by identity, then the relay.
    Fixed,
    /// The participants shuffled afresh at every round, from this seed.
    Shuffled(u64),
    /// The peers of a round run at once on a pool of threads, the relay beside them.
    AtOnce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Until {
    /// Every peer has confirmed every tick.
    Finished,
    /// Every peer has confirmed the first tick: the fixture's starting point.
    FirstAgreedTick,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub peers: usize,
    pub config: WorldConfig,
    pub ticks: u32,
    pub delay: u32,
    pub link: LinkSettings,
    pub link_seed: u64,
    pub sharing: Sharing,
    pub keep_log: bool,
    /// The peer that rolls back, with its window in ticks.
    pub rollback: Option<(usize, u32)>,
    /// The peer that applies, at a tick, a kick it never sends.
    pub injection: Option<(usize, u32)>,
    pub schedule: Schedule,
    pub order: Order,
    pub until: Until,
}

/// Two confirmed hashes of one tick that differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Desync {
    pub tick: u32,
    pub round: u64,
    pub peers: (usize, usize),
    pub hashes: (u64, u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerSummary {
    pub tick: u32,
    pub confirmed: u32,
    pub run_hash: u64,
    pub stalled_rounds: u64,
    pub rollback: RollbackStats,
}

/// What a run leaves, and the three hashes that make its history: the hashes
/// of every peer at every confirmed tick, the round table with the hash each
/// world held at each round, and the link logs.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub rounds: u64,
    pub elapsed: Duration,
    pub finished: bool,
    pub peers: Vec<PeerSummary>,
    pub tick_hashes: u64,
    /// By round, by peer: the tick the world had reached and its confirmed tick.
    pub round_table: Vec<Vec<(u32, u32)>>,
    pub round_table_hash: u64,
    pub link_log_hash: u64,
    pub link: LinkStats,
    pub logged: u64,
    pub desync: Option<Desync>,
    pub threads: usize,
}

impl Outcome {
    pub fn history(&self) -> (u64, u64, u64) {
        (self.tick_hashes, self.round_table_hash, self.link_log_hash)
    }
}

/// What a round asks of one peer and what the peer answers, for the pool.
#[derive(Default)]
struct Work {
    incoming: Vec<Vec<u8>>,
    outgoing: Option<Vec<u8>>,
    runs: bool,
}

/// The threads that run the peers of a round at once.
struct Pool {
    shared: Arc<PoolShared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

struct PoolShared {
    peers: Arc<Vec<Mutex<Peer>>>,
    work: Vec<Mutex<Work>>,
    next: AtomicUsize,
    stop: AtomicBool,
    start: Barrier,
    done: Barrier,
}

impl PoolShared {
    /// Runs the peers no other thread has claimed yet.
    fn drain(&self) {
        loop {
            let index = self.next.fetch_add(1, Ordering::AcqRel);
            if index >= self.work.len() {
                return;
            }
            let mut work = self.work[index].lock().expect("a round's work");
            if work.runs {
                let incoming = std::mem::take(&mut work.incoming);
                work.outgoing = Some(self.peers[index].lock().expect("a peer").round(incoming));
            }
        }
    }
}

impl Pool {
    fn new(peers: &Arc<Vec<Mutex<Peer>>>) -> Pool {
        let count = std::thread::available_parallelism().map_or(1, |n| n.get()).min(peers.len()).max(1);
        let shared = Arc::new(PoolShared {
            peers: Arc::clone(peers),
            work: (0..peers.len()).map(|_| Mutex::new(Work::default())).collect(),
            next: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            start: Barrier::new(count + 1),
            done: Barrier::new(count + 1),
        });
        let threads = (0..count)
            .map(|_| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    loop {
                        shared.start.wait();
                        if shared.stop.load(Ordering::Acquire) {
                            return;
                        }
                        shared.drain();
                        shared.done.wait();
                    }
                })
            })
            .collect();
        Pool { shared, threads }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.start.wait();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

pub struct Fixture {
    settings: Settings,
    peers: Arc<Vec<Mutex<Peer>>>,
    relay: Relay,
    links: Vec<Link>,
    stalled_until: Vec<u64>,
    suspended_until: Vec<u64>,
    /// By peer, the highest tick the oracle has compared.
    compared: Vec<u32>,
    /// By tick, the first confirmed hash seen and the peer that held it.
    seen: Vec<Option<(u64, usize)>>,
    shuffle: SplitMix64,
    round: u64,
}

impl Fixture {
    /// The creations: one world per peer, the relay, one link per peer.
    pub fn new(settings: Settings) -> Fixture {
        let peers: Vec<Mutex<Peer>> = (0..settings.peers)
            .map(|id| {
                let mode = match settings.rollback {
                    Some((peer, window)) if peer == id => Mode::Rollback(window),
                    _ => Mode::Lockstep,
                };
                let injection = settings.injection.filter(|(peer, _)| *peer == id).map(|(_, tick)| tick);
                Mutex::new(Peer::new(id, settings.peers, mode, settings.config, settings.delay, settings.ticks, injection))
            })
            .collect();
        let links = (0..settings.peers).map(|id| Link::new(settings.link, settings.link_seed ^ (id as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15), settings.sharing, settings.keep_log)).collect();
        let shuffle = SplitMix64::new(match settings.order {
            Order::Shuffled(seed) => seed,
            _ => 0,
        });
        Fixture {
            relay: Relay::new(settings.peers, settings.ticks, settings.delay),
            links,
            stalled_until: vec![0; settings.peers],
            suspended_until: vec![0; settings.peers],
            compared: vec![0; settings.peers],
            seen: vec![None; settings.ticks as usize + 1],
            shuffle,
            round: 0,
            peers: Arc::new(peers),
            settings,
        }
    }

    pub fn peer<T>(&self, id: usize, read: impl FnOnce(&Peer) -> T) -> T {
        read(&self.peers[id].lock().expect("a peer"))
    }

    /// The last entries of every link's log, as the artifact a failed test
    /// leaves: the only trace of what was sent and lost.
    pub fn link_log_text(&self) -> String {
        let mut text = String::new();
        for (peer, link) in self.links.iter().enumerate() {
            for entry in link.ring() {
                let direction = if usize::from(entry.direction) == UP { "to the relay" } else { "from the relay" };
                let fate = match entry.fate {
                    Fate::Delivered(frame) => format!("delivered in round {frame}"),
                    Fate::Lost => "lost".to_string(),
                };
                text.push_str(&format!("link {peer} {direction} message {} sent in round {} of {} bytes {fate}
", entry.sequence, entry.sent, entry.size));
            }
        }
        text
    }

    fn apply_schedule(&mut self) {
        let round = self.round;
        for scheduled in self.settings.schedule.0.iter().filter(|scheduled| scheduled.round == round) {
            match scheduled.event {
                Event::Stall { peer, rounds } => self.stalled_until[peer] = round + rounds,
                Event::Suspension { peer, rounds } => self.suspended_until[peer] = round + rounds,
                Event::LossBurst { link, direction, rounds, per_mille } => self.links[link].override_loss(direction, per_mille as f32 / 1_000.0, round + rounds),
                Event::TotalLoss { link, rounds } => {
                    self.links[link].override_loss(UP, 1.0, round + rounds);
                    self.links[link].override_loss(DOWN, 1.0, round + rounds);
                }
                Event::ClockRate { peer, per_mille } => self.peers[peer].lock().expect("a peer").rate = per_mille,
            }
        }
    }

    /// Whether a peer runs this round; a peer that does not has its clock
    /// moved, or not, here.
    fn runs(&self, peer: usize) -> bool {
        if self.round < self.suspended_until[peer] {
            self.peers[peer].lock().expect("a peer").skip_round(false);
            false
        } else if self.round < self.stalled_until[peer] {
            self.peers[peer].lock().expect("a peer").skip_round(true);
            false
        } else {
            true
        }
    }

    fn run_relay(&mut self) {
        let incoming = self.links.iter_mut().map(|link| link.take(UP)).collect();
        for (peer, bytes) in self.relay.round(incoming).into_iter().enumerate() {
            self.links[peer].send(DOWN, self.round, bytes);
        }
    }

    fn run_in_turn(&mut self) {
        let relay = self.settings.peers;
        let mut order: Vec<usize> = (0..=relay).collect();
        if matches!(self.settings.order, Order::Shuffled(_)) {
            for position in (1..order.len()).rev() {
                order.swap(position, (self.shuffle.next() % (position as u64 + 1)) as usize);
            }
        }
        for participant in order {
            if participant == relay {
                self.run_relay();
            } else if self.runs(participant) {
                let incoming = self.links[participant].take(DOWN);
                let bytes = self.peers[participant].lock().expect("a peer").round(incoming);
                self.links[participant].send(UP, self.round, bytes);
            }
        }
    }

    fn run_at_once(&mut self, pool: &Pool) {
        for peer in 0..self.settings.peers {
            let runs = self.runs(peer);
            let mut work = pool.shared.work[peer].lock().expect("a round's work");
            work.runs = runs;
            work.outgoing = None;
            if runs {
                work.incoming = self.links[peer].take(DOWN);
            }
        }
        // The relay's incoming messages are taken before any peer sends, so
        // that it sees, as in a round run in turn, only earlier rounds.
        let incoming: Vec<Vec<Vec<u8>>> = self.links.iter_mut().map(|link| link.take(UP)).collect();
        pool.shared.next.store(0, Ordering::Release);
        pool.shared.start.wait();
        let answers = self.relay.round(incoming);
        pool.shared.done.wait();
        for peer in 0..self.settings.peers {
            if let Some(bytes) = pool.shared.work[peer].lock().expect("a round's work").outgoing.take() {
                self.links[peer].send(UP, self.round, bytes);
            }
        }
        for (peer, bytes) in answers.into_iter().enumerate() {
            self.links[peer].send(DOWN, self.round, bytes);
        }
    }

    /// The oracle: for each tick, in order, the confirmed hashes of the peers
    /// that hold one, compared as soon as two do.
    fn compare(&mut self) -> Option<Desync> {
        let confirmed: Vec<u32> = (0..self.settings.peers).map(|id| self.peer(id, Peer::confirmed)).collect();
        let from = self.compared.iter().copied().min().unwrap_or(0) + 1;
        let through = confirmed.iter().copied().max().unwrap_or(0);
        for tick in from..=through {
            for peer in 0..self.settings.peers {
                if confirmed[peer] < tick || self.compared[peer] >= tick {
                    continue;
                }
                let hash = self.peer(peer, |held| held.confirmed_hash(tick)).expect("a confirmed tick holds its hash");
                match self.seen[tick as usize] {
                    None => self.seen[tick as usize] = Some((hash, peer)),
                    Some((first, holder)) if first != hash => return Some(Desync { tick, round: self.round, peers: (holder, peer), hashes: (first, hash) }),
                    Some(_) => {}
                }
                self.compared[peer] = tick;
            }
        }
        None
    }

    /// Runs rounds until the settings' end, a desync, or a bound on the
    /// rounds that no healthy run reaches.
    pub fn run(&mut self) -> Outcome {
        let pool = (self.settings.order == Order::AtOnce).then(|| Pool::new(&self.peers));
        let guard = u64::from(self.settings.ticks) * 40 + 1_000;
        let goal = match self.settings.until {
            Until::Finished => self.settings.ticks,
            Until::FirstAgreedTick => 1,
        };
        let mut table = Vec::new();
        let mut table_hash = Fnv1a::new();
        let mut desync = None;
        let mut finished = false;
        let started = Instant::now();
        while self.round < guard {
            self.round += 1;
            self.apply_schedule();
            match &pool {
                Some(pool) => self.run_at_once(pool),
                None => self.run_in_turn(),
            }
            let round = self.round;
            for link in &mut self.links {
                link.advance(round);
            }
            let row: Vec<(u32, u32)> = (0..self.settings.peers)
                .map(|id| {
                    self.peer(id, |peer| {
                        table_hash.write(&peer.tick().to_le_bytes());
                        table_hash.write(&peer.confirmed().to_le_bytes());
                        table_hash.write(&peer.present_hash().to_le_bytes());
                        (peer.tick(), peer.confirmed())
                    })
                })
                .collect();
            finished = row.iter().all(|(_, confirmed)| *confirmed >= goal);
            table.push(row);
            desync = self.compare();
            if desync.is_some() || finished {
                break;
            }
        }
        let elapsed = started.elapsed();
        let threads = pool.as_ref().map_or(1, |pool| pool.threads.len());
        drop(pool);
        let peers: Vec<PeerSummary> = (0..self.settings.peers)
            .map(|id| self.peer(id, |peer| PeerSummary { tick: peer.tick(), confirmed: peer.confirmed(), run_hash: peer.run_hash(), stalled_rounds: peer.stalled_rounds, rollback: peer.rollback }))
            .collect();
        let mut tick_hashes = Fnv1a::new();
        for peer in &peers {
            tick_hashes.write(&peer.run_hash.to_le_bytes());
        }
        let mut link_log_hash = Fnv1a::new();
        let (mut link, mut logged) = (LinkStats::default(), 0);
        for held in &self.links {
            link_log_hash.write(&held.log_hash().to_le_bytes());
            link.sent += held.stats.sent;
            link.delivered += held.stats.delivered;
            link.lost += held.stats.lost;
            link.bytes += held.stats.bytes;
            logged += held.logged;
        }
        Outcome {
            rounds: self.round,
            elapsed,
            finished,
            peers,
            tick_hashes: tick_hashes.finish(),
            round_table: table,
            round_table_hash: table_hash.finish(),
            link_log_hash: link_log_hash.finish(),
            link,
            logged,
            desync,
            threads,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workload_lockstep::world::Size;

    fn settings(peers: usize, link: LinkSettings) -> Settings {
        Settings {
            peers,
            config: WorldConfig::new(Size::Small, 1),
            ticks: 24,
            delay: 2,
            link,
            link_seed: 7,
            sharing: Sharing::PerDirection,
            keep_log: true,
            rollback: None,
            injection: None,
            schedule: Schedule::default(),
            order: Order::Fixed,
            until: Until::Finished,
        }
    }

    const LOSSY: LinkSettings = LinkSettings { latency: 1, jitter_max: 2, loss: 0.1 };

    fn run(settings: Settings) -> Outcome {
        Fixture::new(settings).run()
    }

    #[test]
    fn four_peers_through_the_relay_agree_on_every_tick() {
        let outcome = run(settings(4, LOSSY));
        assert!(outcome.finished && outcome.desync.is_none());
        assert!(outcome.peers.iter().all(|peer| peer.confirmed == 24 && peer.run_hash == outcome.peers[0].run_hash));
        assert!(outcome.link.lost > 0, "the link loses messages and the redundancy carries the inputs");
    }

    #[test]
    fn the_order_of_the_peers_and_the_threads_change_nothing() {
        let fixed = run(settings(4, LOSSY));
        for seed in 1..=3 {
            assert_eq!(run(Settings { order: Order::Shuffled(seed), ..settings(4, LOSSY) }).history(), fixed.history(), "seed {seed}");
        }
        assert_eq!(run(Settings { order: Order::AtOnce, ..settings(4, LOSSY) }).history(), fixed.history());
    }

    #[test]
    fn one_random_state_for_both_directions_lets_the_order_in() {
        let shared = |order: Order| run(Settings { sharing: Sharing::OneRandomState, order, ..settings(4, LOSSY) });
        let fixed = shared(Order::Fixed);
        let shuffled = shared(Order::Shuffled(1));
        assert_ne!(shuffled.link_log_hash, fixed.link_log_hash);
        assert_eq!(shuffled.tick_hashes, fixed.tick_hashes, "the ticks of a lockstep game do not depend on when they are stepped");
    }

    #[test]
    fn a_desync_is_reported_at_its_first_divergent_tick_with_and_without_a_peer_that_rolls_back() {
        for (rollback, injected) in [(None, 2), (Some((1, 6)), 2), (Some((1, 6)), 1)] {
            for order in [Order::Fixed, Order::Shuffled(5), Order::AtOnce] {
                let outcome = run(Settings { rollback, injection: Some((injected, 9)), order, ..settings(3, LOSSY) });
                let desync = outcome.desync.expect("the oracle reports the desync");
                assert_eq!(desync.tick, 9, "rollback {rollback:?}, injected into {injected}, {order:?}");
                assert!(desync.peers.0 == injected || desync.peers.1 == injected);
            }
        }
    }

    #[test]
    fn a_peer_that_rolls_back_confirms_the_hashes_of_the_others() {
        let outcome = run(Settings { rollback: Some((0, 6)), ..settings(3, LOSSY) });
        assert!(outcome.finished && outcome.desync.is_none());
        assert!(outcome.peers[0].rollback.rollbacks > 0);
        assert_eq!(outcome.peers[0].run_hash, outcome.peers[1].run_hash);
    }

    fn written() -> Schedule {
        Schedule(vec![
            Scheduled { round: 4, event: Event::Stall { peer: 0, rounds: 5 } },
            Scheduled { round: 6, event: Event::LossBurst { link: 1, direction: DOWN, rounds: 6, per_mille: 600 } },
            Scheduled { round: 8, event: Event::ClockRate { peer: 2, per_mille: 900 } },
            Scheduled { round: 14, event: Event::Suspension { peer: 1, rounds: 4 } },
            Scheduled { round: 20, event: Event::TotalLoss { link: 2, rounds: 6 } },
        ])
    }

    #[test]
    fn a_schedule_gives_one_history_and_reads_back_from_its_text() {
        let schedule = written();
        assert_eq!(Schedule::parse(&schedule.text()), Ok(schedule.clone()));
        let first = run(Settings { schedule: schedule.clone(), ..settings(3, LOSSY) });
        let second = run(Settings { schedule: Schedule::parse(&schedule.text()).expect("the text is read"), ..settings(3, LOSSY) });
        assert!(first.finished && first.desync.is_none());
        assert_eq!(first.history(), second.history());
        assert_ne!(first.history(), run(settings(3, LOSSY)).history(), "the schedule changes the history");
        assert_eq!(first.tick_hashes, run(settings(3, LOSSY)).tick_hashes);
    }

    #[test]
    fn a_drawn_schedule_is_its_seed_and_leaves_the_quiet_span() {
        let rates = Rates { stall: 30, suspension: 20, loss_burst: 40, total_loss: 20, clock_rate: 20 };
        let drawn = Schedule::drawn(11, &rates, 3, 40);
        assert_eq!(drawn, Schedule::drawn(11, &rates, 3, 40));
        assert_ne!(drawn, Schedule::drawn(12, &rates, 3, 40));
        assert!(!drawn.0.is_empty() && drawn.0.iter().all(|scheduled| scheduled.round < 40));
        let outcome = run(Settings { schedule: drawn, ..settings(3, LOSSY) });
        assert!(outcome.finished && outcome.desync.is_none());
    }

    #[test]
    fn a_stall_is_caught_up_under_the_tick_budget_and_a_suspension_is_not() {
        let perfect = LinkSettings { latency: 0, jitter_max: 0, loss: 0.0 };
        let most_in_a_round = |event: Event| {
            let outcome = run(Settings { schedule: Schedule(vec![Scheduled { round: 8, event }]), ticks: 40, ..settings(2, perfect) });
            assert!(outcome.finished);
            outcome.round_table.windows(2).skip(8).map(|rows| rows[1][0].0 - rows[0][0].0).max().unwrap_or(0)
        };
        assert!(most_in_a_round(Event::Stall { peer: 0, rounds: 6 }) > 1, "a peer whose clock went on steps several ticks in a round");
        assert_eq!(most_in_a_round(Event::Suspension { peer: 0, rounds: 6 }), 1, "a peer whose clock stopped steps one");
    }

    #[test]
    fn the_first_agreed_tick_ends_the_preparation() {
        let outcome = run(Settings { until: Until::FirstAgreedTick, ..settings(4, LinkSettings { latency: 0, jitter_max: 0, loss: 0.0 }) });
        assert!(outcome.finished);
        assert!(outcome.peers.iter().all(|peer| peer.confirmed >= 1));
        assert_eq!(outcome.rounds, 3, "inputs up in the first round, merged records down in the second, the first tick in the third");
    }
}
