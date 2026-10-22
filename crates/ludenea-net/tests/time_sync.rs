mod support;

use ludenea_net::{
    DropReason, LinkConditions, MatchRules, Message, Packet, PeerId, QUALITY_INTERVAL_MICROS,
    Session, SessionEvent, SessionState,
};
use support::{FRAME_MICROS, Harness, handshake, identity, model_input, sessions, settings};

const ONE_SECOND: u64 = 1_000_000;

fn latency(latency_micros: u32, jitter_micros: u32, seed: u64) -> LinkConditions {
    LinkConditions {
        latency_micros,
        jitter_micros,
        loss_per_mille: 0,
        duplicate_per_mille: 0,
        reorder_per_mille: 0,
        reorder_extra_micros: 0,
        seed,
    }
}

/// The packet `session` sends `to` now, decoded.
fn packet_to(session: &mut Session, to: PeerId, now: u64) -> Packet {
    let outgoing = session.outgoing(now);
    let (_, bytes) = outgoing
        .iter()
        .find(|(peer, _)| *peer == to)
        .unwrap_or_else(|| panic!("no packet for {to} at {now}"));
    Packet::decode(bytes).unwrap()
}

fn dropped(session: &Session, peer: PeerId, reason: DropReason) -> u64 {
    session.stats().peers[&peer]
        .dropped
        .get(&reason)
        .copied()
        .unwrap_or(0)
}

/// Peer 0 sends peer 1 its quality request at `sent`, peer 1 reads it at `sent` and answers
/// at once, and peer 0 reads the answer at `sent + round_trip`. Returns the answer.
fn round_trip(pair: &mut [Session], sent: u64, round_trip: u64) -> Packet {
    let request = packet_to(&mut pair[0], PeerId(1), sent);
    assert!(
        request.messages.iter().any(|message| matches!(
            message,
            Message::Quality { sent_micros, .. } if *sent_micros == sent
        )),
        "{request:?}"
    );
    pair[1].receive(PeerId(0), &request.encode(), sent);
    let reply = packet_to(&mut pair[1], PeerId(0), sent);
    assert!(
        reply.messages.contains(&Message::QualityReply {
            echo_micros: sent,
            tick: 0,
        }),
        "{reply:?}"
    );
    pair[0].receive(PeerId(1), &reply.encode(), sent + round_trip);
    reply
}

#[test]
fn ping_converges_to_the_conditioned_latency() {
    let mut harness = Harness::conditioned(sessions(2), latency(100_000, 0, 7));
    harness.run_until(5 * ONE_SECOND, |_| false);
    for peer in harness.peers() {
        assert_eq!(harness.state(peer), SessionState::Running);
        for (other, stats) in &harness.session(peer).stats().peers {
            assert!(
                (195_000..=205_000).contains(&stats.ping_micros),
                "{peer} to {other}: ping {}",
                stats.ping_micros
            );
            assert!(stats.jitter_micros <= 5_000, "{peer} to {other}: {stats:?}");
        }
    }
}

#[test]
fn ping_and_jitter_are_the_mean_and_mean_deviation_of_recent_round_trips() {
    let mut pair = handshake(sessions(2));
    // Four slow round trips, then sixteen alternating between 100 and 140 ms: only the
    // sixteen latest count.
    for request in 0..20 {
        let rtt = match request {
            0..4 => 900_000,
            _ if request % 2 == 0 => 100_000,
            _ => 140_000,
        };
        round_trip(&mut pair, (request + 1) * ONE_SECOND, rtt);
    }
    let stats = &pair[0].stats().peers[&PeerId(1)];
    assert_eq!(stats.ping_micros, 120_000);
    assert_eq!(stats.jitter_micros, 20_000);
}

#[test]
fn duplicate_or_unknown_replies_do_not_skew_the_ping() {
    // Sixteen round trips of 100 ms: the handshake's own one at time 0 is forgotten.
    let mut pair = handshake(sessions(2));
    let mut reply = None;
    for request in 1..=16 {
        reply = Some(round_trip(&mut pair, request * ONE_SECOND, 100_000));
    }
    assert_eq!(pair[0].stats().peers[&PeerId(1)].ping_micros, 100_000);
    pair[0].events();
    let echo = |echo_micros| Packet {
        match_id: 1,
        messages: vec![Message::QualityReply {
            echo_micros,
            tick: u64::MAX,
        }],
    };
    let now = 16 * ONE_SECOND + 300_000;
    // The same reply again, a reply to a request never sent, and one from the future.
    let reply = reply.unwrap();
    for packet in [reply, echo(ONE_SECOND - 1), echo(u64::MAX), echo(now + 1)] {
        pair[0].receive(PeerId(1), &packet.encode(), now);
    }
    assert_eq!(pair[0].state(), SessionState::Running);
    let stats = &pair[0].stats().peers[&PeerId(1)];
    assert_eq!(stats.ping_micros, 100_000);
    assert_eq!(stats.jitter_micros, 0);
    assert_eq!(dropped(&pair[0], PeerId(1), DropReason::UnknownEcho), 4);
    let drop = SessionEvent::Dropped {
        peer: PeerId(1),
        reason: DropReason::UnknownEcho,
    };
    assert_eq!(pair[0].events(), vec![drop; 4]);
    // A request still pending after sixteen later ones is forgotten: its reply is unknown.
    // They span 3.4 s of silence from peer 1, which the default timeouts disconnect.
    let identity = identity(&[0, 1]);
    let patient = (0..2)
        .map(|local| {
            let mut settings = settings(local, &identity);
            settings.timeouts.disconnect_after_micros = 10 * ONE_SECOND;
            Session::new(settings, model_input()).unwrap()
        })
        .collect();
    let mut pair = handshake(patient);
    let ping = pair[0].stats().peers[&PeerId(1)].ping_micros;
    let first = packet_to(&mut pair[0], PeerId(1), ONE_SECOND);
    pair[1].receive(PeerId(0), &first.encode(), ONE_SECOND);
    let late_reply = packet_to(&mut pair[1], PeerId(0), ONE_SECOND);
    for request in 1..=16 {
        pair[0].outgoing(ONE_SECOND + request * QUALITY_INTERVAL_MICROS);
    }
    let now = ONE_SECOND + 17 * QUALITY_INTERVAL_MICROS;
    pair[0].receive(PeerId(1), &late_reply.encode(), now);
    assert_eq!(dropped(&pair[0], PeerId(1), DropReason::UnknownEcho), 1);
    assert_eq!(pair[0].stats().peers[&PeerId(1)].ping_micros, ping);
}

#[test]
fn hostile_reports_skip_at_most_half_the_window_per_correction() {
    let cap = u64::from(MatchRules::default().rollback_window / 2);
    let mut pair = handshake(sessions(2));
    round_trip(&mut pair, ONE_SECOND, 20_000);
    let mut session = pair.remove(0);
    let mut now = ONE_SECOND + 100_000;
    // Reports claiming peer 1 is far behind, with fields at their extremes.
    let report = |session: &mut Session, now: u64| {
        let packet = Packet {
            match_id: 1,
            messages: vec![Message::Quality {
                tick: 0,
                sent_micros: now,
                advantage: i32::MIN,
            }],
        };
        session.receive(PeerId(1), &packet.encode(), now);
    };
    // Reports closer together than half the quality interval count once: not enough for a
    // correction.
    for _ in 0..8 {
        report(&mut session, now);
        now += 10_000;
    }
    session.add_local_input(0, vec![0]).unwrap();
    assert_ne!(session.advance(now).unwrap(), []);
    assert_eq!(session.stats().skips, 0);
    for _ in 0..3 {
        now += QUALITY_INTERVAL_MICROS;
        report(&mut session, now);
    }
    let skips = |session: &mut Session, now: u64, frames: u64| {
        let before = session.stats().skips;
        for _ in 0..frames {
            session.add_local_input(0, vec![0]).unwrap();
            assert_eq!(session.advance(now).unwrap(), []);
        }
        session.stats().skips - before
    };
    assert_eq!(skips(&mut session, now, cap), cap);
    // The skips spent the evidence: the next frame runs a tick.
    session.add_local_input(0, vec![0]).unwrap();
    assert_ne!(session.advance(now).unwrap(), []);
    // A report of a tick near `u64::MAX` claims peer 1 is far ahead: never a skip.
    for round in 0..4 {
        now += QUALITY_INTERVAL_MICROS;
        let packet = Packet {
            match_id: 1,
            messages: vec![Message::Quality {
                tick: u64::MAX,
                sent_micros: u64::MAX - 3 + round,
                advantage: i32::MAX,
            }],
        };
        session.receive(PeerId(1), &packet.encode(), now);
    }
    assert_eq!(session.state(), SessionState::Running);
    session.add_local_input(0, vec![0]).unwrap();
    assert_ne!(session.advance(now).unwrap(), []);
    assert_eq!(session.stats().skips, cap);
    assert_eq!(
        session.stats().peers[&PeerId(1)].frame_advantage,
        i32::MIN,
        "the local tick minus a tick near u64::MAX"
    );
}

/// What one peer counted at one time.
#[derive(Debug, Clone, Copy)]
struct Counts {
    skips: u64,
    stalls: u64,
}

fn counts(harness: &mut Harness, peer: PeerId) -> Counts {
    let stats = harness.session(peer).stats();
    Counts {
        skips: stats.skips,
        stalls: stats.peers.values().map(|stats| stats.stalls).sum(),
    }
}

/// Runs the peers to 10 s, then to 60 s, and returns what each counted over those 50 s and
/// the widest tick gap between two peers from 10 s on, both ends included.
fn run_drifting(harness: &mut Harness) -> (Vec<Counts>, u64) {
    harness.run_until(10 * ONE_SECOND, |_| false);
    let peers: Vec<PeerId> = harness.peers().collect();
    let start: Vec<Counts> = peers.iter().map(|peer| counts(harness, *peer)).collect();
    let gap = |harness: &Harness| {
        let ticks: Vec<u64> = peers
            .iter()
            .map(|peer| harness.model(*peer).tick())
            .collect();
        ticks.iter().max().unwrap() - ticks.iter().min().unwrap()
    };
    // From the step the counts start at, so the gap bounds the ticks at both ends.
    let mut widest = gap(harness);
    while harness.now_micros() < 60 * ONE_SECOND {
        harness.step();
        widest = widest.max(gap(harness));
    }

    for peer in &peers {
        assert_eq!(harness.state(*peer), SessionState::Running, "{peer}");
    }
    let spent = peers
        .iter()
        .zip(start)
        .map(|(peer, start)| {
            let end = counts(harness, *peer);
            Counts {
                skips: end.skips - start.skips,
                stalls: end.stalls - start.stalls,
            }
        })
        .collect();
    (spent, widest)
}

#[test]
fn faster_peer_slows_down() {
    let mut harness = Harness::conditioned(sessions(2), latency(50_000, 10_000, 3));
    harness.set_frame_micros(PeerId(0), FRAME_MICROS * 98 / 100);
    let (counts, widest) = run_drifting(&mut harness);
    assert!(widest <= 2, "the ticks drifted {widest} apart: {counts:?}");
    for (peer, counts) in counts.iter().enumerate() {
        assert_eq!(counts.stalls, 0, "peer {peer} stalled: {counts:?}");
    }
    // 1.2 skips per second over 50 s, within 0.4 per second.
    assert!(
        (40..=80).contains(&counts[0].skips),
        "the faster peer skipped {} ticks in 50 s",
        counts[0].skips
    );
    assert_eq!(counts[1].skips, 0, "the slower peer skipped: {counts:?}");
}

#[test]
fn three_peers_converge() {
    let mut harness = Harness::conditioned(sessions(3), latency(30_000, 10_000, 5));
    harness.set_frame_micros(PeerId(0), FRAME_MICROS * 98 / 100);
    harness.set_frame_micros(PeerId(2), FRAME_MICROS * 102 / 100);
    let (counts, widest) = run_drifting(&mut harness);
    // The fastest peer drifts 2.4 ticks a second from the slowest, and a correction waits for
    // four fresh reports, at least 0.6 s: it skips about two ticks at a time, so the fastest
    // and the slowest peers stay up to four ticks apart, as the frames of each fall.
    assert!(widest <= 4, "the ticks drifted {widest} apart: {counts:?}");
    for (peer, counts) in counts.iter().enumerate() {
        assert_eq!(counts.stalls, 0, "peer {peer} stalled: {counts:?}");
    }
    // Every peer keeps pace with the slowest one: about 2.4 skips per second for the fastest,
    // 1.2 for the middle one, none for the slowest.
    assert!(
        (100..=140).contains(&counts[0].skips),
        "the fastest peer: {counts:?}"
    );
    assert!(
        (40..=80).contains(&counts[1].skips),
        "the middle peer: {counts:?}"
    );
    assert_eq!(counts[2].skips, 0, "the slowest peer: {counts:?}");
}

#[test]
fn unknown_echo_drops_only_its_reply() {
    let mut session = handshake(sessions(2)).remove(0);
    session.events();
    let packet = Packet {
        match_id: 1,
        messages: vec![
            Message::QualityReply {
                echo_micros: u64::MAX,
                tick: 0,
            },
            Message::Inputs {
                player: 1,
                first_tick: 0,
                inputs: vec![vec![3]],
            },
        ],
    };
    session.receive(PeerId(1), &packet.encode(), ONE_SECOND);
    assert_eq!(dropped(&session, PeerId(1), DropReason::UnknownEcho), 1);
    session.add_local_input(0, vec![0]).unwrap();
    session.advance(ONE_SECOND).unwrap();
    // The input after the reply was read: tick 0 is confirmed with it.
    assert_eq!(
        session.events(),
        [
            SessionEvent::Dropped {
                peer: PeerId(1),
                reason: DropReason::UnknownEcho,
            },
            SessionEvent::Confirmed {
                tick: 0,
                inputs: vec![vec![0], vec![3]],
            },
        ]
    );
}

#[test]
fn peer_behind_never_skips_when_the_round_trip_exceeds_the_window() {
    // From 135 ms each way, the round trip passes `rollback_window + input_delay` ticks: both
    // peers run at the edge of their window, and each stall of the faster one leaves the
    // slower one briefly ahead. Were the slower one to skip then, the faster one would stall
    // the more for it.
    let fast_frame = FRAME_MICROS * 98 / 100;
    // The frames the faster peer runs beyond the slower one's in 50 s, one more for each
    // count's rounding.
    let extra_frames = 50 * ONE_SECOND / fast_frame - 50 * ONE_SECOND / FRAME_MICROS + 2;
    for latency_micros in [135_000, 150_000] {
        let mut harness = Harness::conditioned(sessions(2), latency(latency_micros, 10_000, 0));
        harness.set_frame_micros(PeerId(0), fast_frame);
        let (counts, widest) = run_drifting(&mut harness);
        let [fast, slow] = [counts[0], counts[1]];
        assert_eq!(slow.skips, 0, "{latency_micros} us: {counts:?}");
        // A peer's ticks grow by its frames less its skips and stalls. Over the 50 s, the
        // faster peer's ticks grow by at most `2 * widest` fewer than the slower one's: its
        // lead moves from at most `widest` ahead to at most `widest` behind. So it loses at
        // most its extra frames, plus the slower one's skips (none) and stalls, plus
        // `2 * widest`.
        assert!(
            fast.skips + fast.stalls <= extra_frames + slow.stalls + 2 * widest,
            "{latency_micros} us: {counts:?}, widest {widest}"
        );
    }
}

#[test]
fn committed_skips_stop_once_the_session_is_no_longer_ahead() {
    let mut pair = handshake(sessions(2));
    // A ping of 10 ms, the mean of the handshake's instant round trip and this one: half of
    // it rounds to no tick.
    round_trip(&mut pair, ONE_SECOND, 20_000);
    let mut session = pair.remove(0);
    let advance = |session: &mut Session, now: u64| {
        session.add_local_input(0, vec![0]).unwrap();
        session.advance(now).unwrap()
    };
    for _ in 0..5 {
        assert_ne!(advance(&mut session, ONE_SECOND + 30_000), []);
    }
    // Peer 1 reports tick 4, one tick behind the local tick 5, and claims to be four ticks
    // behind: a mean correction of 2.5, three skips.
    let mut now = ONE_SECOND;
    for _ in 0..4 {
        now += QUALITY_INTERVAL_MICROS;
        let packet = Packet {
            match_id: 1,
            messages: vec![Message::Quality {
                tick: 4,
                sent_micros: now,
                advantage: -4,
            }],
        };
        session.receive(PeerId(1), &packet.encode(), now);
    }
    assert_eq!(session.stats().peers[&PeerId(1)].frame_advantage, 1);
    assert_eq!(advance(&mut session, now), []);
    assert_eq!(session.stats().skips, 1);
    // A frame later peer 1 is estimated at tick 5: the session is no longer ahead, so the
    // remaining skips are cancelled and the tick runs.
    assert_ne!(advance(&mut session, now + FRAME_MICROS), []);
    assert_eq!(session.stats().skips, 1);
    // The reports before the cancelled skips count no more.
    assert_ne!(advance(&mut session, now + 2 * FRAME_MICROS), []);
    assert_eq!(session.stats().skips, 1);
}
