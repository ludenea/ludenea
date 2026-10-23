//! The host's online match: the rollback session of `ludenea-net`, the executor that applies
//! its requests to the host's simulation, the local players and their mapped inputs, the
//! confirmed inputs the session's input log takes at the end, and the clock the session reads
//! its time from.

use ludenea_app::players::Claim;
use ludenea_app::{InputLog, join_player_inputs};
use ludenea_net::{EndReason, Executor, PeerId, Session, SessionEvent, SessionState};

use crate::protocol::{MatchEnd, MatchEndKind, OnlineState};

/// An online match the host plays, from `join_match` until another match or session loads.
pub(crate) struct OnlineMatch {
    pub session: Session,
    pub executor: Executor,
    /// The global index of each local player, in player order.
    pub locals: Vec<u8>,
    /// The devices of each local player, in the order of `locals`.
    pub claims: Vec<Claim>,
    /// The mapped input of each local player, added before each tick the match runs.
    pub inputs: Vec<Vec<u8>>,
    /// The confirmed inputs, the players' inputs of each tick joined, from tick 0.
    pub confirmed: InputLog,
    pub clock: Clock,
    /// Why the match ended here alone, without its session: it was dropped while it was
    /// starting. The session is no longer driven then: it would go on greeting the others.
    pub dropped: Option<MatchEnd>,
    /// Whether the host finished the match: the session's input log took the confirmed
    /// inputs. The session lingers, as long as its own linger lasts.
    pub finished: bool,
}

impl OnlineMatch {
    /// The state `Event::Online` reports.
    pub fn state(&self) -> OnlineState {
        if let Some(end) = &self.dropped {
            return OnlineState::Ended { end: end.clone() };
        }
        match self.session.state() {
            SessionState::Connecting => OnlineState::Connecting,
            SessionState::Running => OnlineState::Running,
            SessionState::Leaving => OnlineState::Leaving,
            SessionState::Ended(reason) => OnlineState::Ended {
                end: match_end(&reason),
            },
        }
    }

    /// Whether the match is over for this peer, finished by the host or not yet.
    pub fn ended(&self) -> bool {
        self.dropped.is_some() || matches!(self.session.state(), SessionState::Ended(_))
    }

    /// Ends the match here alone, as `kind` with `message`, naming this peer: the other
    /// peers learn nothing from this one any more.
    pub fn drop_locally(&mut self, kind: MatchEndKind, message: String) {
        self.dropped = Some(MatchEnd {
            kind,
            message,
            peer: Some(self.session.settings().local.0),
            tick: None,
        });
    }

    /// The local peer, as messages name it.
    pub fn local(&self) -> PeerId {
        self.session.settings().local
    }

    /// Records the confirmed inputs of `events`, the session's events in order; returns
    /// whether the match started among them.
    pub fn record(&mut self, events: Vec<SessionEvent>) -> bool {
        let mut started = false;
        for event in events {
            match event {
                SessionEvent::Started => started = true,
                SessionEvent::Confirmed { inputs, .. } => {
                    self.confirmed.ticks.push(join_player_inputs(&inputs));
                }
                // The drops and the silences are counted in the session's statistics.
                SessionEvent::Dropped { .. }
                | SessionEvent::HashMatched { .. }
                | SessionEvent::PeerSilent { .. }
                | SessionEvent::PeerBack { .. } => {}
            }
        }
        started
    }
}

/// How an ending reads in the protocol: `message` is the reason's own words, and the peer and
/// tick are those it names.
pub(crate) fn match_end(reason: &EndReason) -> MatchEnd {
    let (kind, peer, tick) = match *reason {
        EndReason::Mismatch { peer, .. } => (MatchEndKind::Mismatch, peer, None),
        EndReason::Unreachable { peer } => (MatchEndKind::Unreachable, peer, None),
        EndReason::Lost { peer } => (MatchEndKind::Lost, peer, None),
        EndReason::Violation { peer, tick, .. } => (MatchEndKind::Violation, peer, tick),
        EndReason::Desync { tick, peer, .. } => (MatchEndKind::Desync, peer, Some(tick)),
        EndReason::Left { peer, last_tick } => (MatchEndKind::Left, peer, Some(last_tick)),
        EndReason::LocalFailure { peer, tick } => (MatchEndKind::LocalFailure, peer, Some(tick)),
    };
    MatchEnd {
        kind,
        message: reason.to_string(),
        peer: Some(peer.0),
        tick,
    }
}

/// The time of the match, in whole microseconds since it was joined: the sum of the elapsed
/// times `Host::advance` passed, the fraction of a microsecond carried to the next one.
#[derive(Default)]
pub(crate) struct Clock {
    micros: u64,
    fraction: f64,
}

impl Clock {
    pub fn now(&self) -> u64 {
        self.micros
    }

    /// Adds `seconds` and returns the new time. A negative or non-finite time adds nothing.
    pub fn advance(&mut self, seconds: f64) -> u64 {
        if seconds.is_finite() && seconds > 0.0 {
            let total = seconds * 1_000_000.0 + self.fraction;
            let whole = total.floor();
            self.fraction = total - whole;
            // A float to integer cast saturates: a time past `u64::MAX` microseconds stops there.
            self.micros = self.micros.saturating_add(whole as u64);
        }
        self.micros
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_carries_the_fraction_of_a_microsecond() {
        let mut clock = Clock::default();
        for _ in 0..120 {
            clock.advance(1.0 / 120.0);
        }
        assert_eq!(clock.now(), 1_000_000);
        clock.advance(0.000_000_4);
        assert_eq!(clock.now(), 1_000_000);
        clock.advance(0.000_000_7);
        assert_eq!(clock.now(), 1_000_001);
    }

    #[test]
    fn the_clock_ignores_negative_and_non_finite_times() {
        let mut clock = Clock::default();
        clock.advance(0.5);
        for seconds in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(clock.advance(seconds), 500_000);
        }
    }

    #[test]
    fn an_ending_names_the_peer_and_the_tick_of_its_reason() {
        let end = match_end(&EndReason::Desync {
            tick: 90,
            found_by: PeerId(0),
            peer: PeerId(2),
            local: 1,
            remote: 2,
        });
        assert_eq!(end.kind, MatchEndKind::Desync);
        assert_eq!((end.peer, end.tick), (Some(2), Some(90)));
        assert!(
            end.message
                .starts_with("the game states diverged at tick 90")
        );
        let end = match_end(&EndReason::Lost { peer: PeerId(1) });
        assert_eq!(
            (end.kind, end.peer, end.tick),
            (MatchEndKind::Lost, Some(1), None)
        );
        let end = match_end(&EndReason::Violation {
            peer: PeerId(1),
            player: Some(1),
            tick: Some(7),
            rule: ludenea_net::ViolationRule::NotOwner,
        });
        assert_eq!(
            (end.kind, end.peer, end.tick),
            (MatchEndKind::Violation, Some(1), Some(7))
        );
    }
}
