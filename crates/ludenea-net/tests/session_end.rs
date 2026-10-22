mod support;

use ludenea_net::{
    EndReason, LinkConditions, MatchRules, Message, Packet, PeerId, Request, Session, SessionError,
    SessionEvent, SessionState, Timeouts, ViolationRule,
};
use support::{Harness, handshake, identity, offline_hashes, sessions};

const ONE_SECOND: u64 = 1_000_000;
const NOW: u64 = 1_000;

/// Hands `session` a packet of `messages` from `from`.
fn deliver(session: &mut Session, from: u8, messages: Vec<Message>) {
    let packet = Packet {
        match_id: 1,
        messages,
    };
    session.receive(PeerId(from), &packet.encode(), NOW);
}

/// The inputs of `player` from `first_tick`, one byte each.
fn inputs(player: u8, first_tick: u64, bytes: &[u8]) -> Message {
    Message::Inputs {
        player,
        first_tick,
        inputs: bytes.iter().map(|byte| vec![*byte]).collect(),
    }
}

fn ack(player: u8, next: u64) -> Message {
    Message::Ack {
        players: vec![(player, next)],
        hashes_through: 0,
    }
}

/// Adds `input` for player 0, advances, and sends, the packets taken and dropped, as if lost,
/// so the session counts its inputs as sent. Returns the advance's requests.
fn step(session: &mut Session, input: u8) -> Vec<Request> {
    session.add_local_input(0, vec![input]).unwrap();
    let requests = session.advance(NOW).unwrap();
    session.outgoing(NOW);
    requests
}

fn ended(reason: EndReason) -> SessionState {
    SessionState::Ended(reason)
}

fn left(peer: u8, last_tick: u64) -> SessionState {
    ended(EndReason::Left {
        peer: PeerId(peer),
        last_tick,
    })
}

/// The `Confirmed` events of `peer`, encoded: each tick, then each input's length and bytes.
fn encoded_confirmed(harness: &Harness, peer: PeerId) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (tick, inputs) in harness.confirmed(peer) {
        bytes.extend(tick.to_le_bytes());
        for input in inputs {
            bytes.push(u8::try_from(input.len()).unwrap());
            bytes.extend(input);
        }
    }
    bytes
}

/// The session of `peer` ended, and its simulation stands at the state its confirmed inputs
/// give: every tick up to the end tick confirmed in order, the model at the end tick, with the
/// hash of folding those inputs offline. Returns the end tick.
fn assert_landed(harness: &Harness, peer: PeerId) -> u64 {
    assert!(
        matches!(harness.state(peer), SessionState::Ended(_)),
        "{:?}",
        harness.state(peer)
    );
    let confirmed = harness.confirmed(peer);
    let end = u64::try_from(confirmed.len()).unwrap();
    assert!(end > 0, "peer {peer} confirmed nothing");
    for (expected, (tick, _)) in (0..).zip(&confirmed) {
        assert_eq!(*tick, expected, "peer {peer}");
    }
    let model = harness.model(peer);
    assert_eq!(model.tick(), end, "peer {peer} stands at another tick");
    assert_eq!(
        model.hash(),
        *offline_hashes(&confirmed).last().unwrap(),
        "peer {peer} stands at a state its confirmed inputs do not give"
    );
    end
}

/// The times `peer` emitted each event `wanted` picks, with its value.
fn timed<T>(
    harness: &mut Harness,
    max_micros: u64,
    peer: PeerId,
    wanted: impl Fn(&SessionEvent) -> Option<T>,
) -> Option<(u64, T)> {
    let end = harness.now_micros() + max_micros;
    let seen = harness.events(peer).len();
    while harness.now_micros() < end {
        harness.step();
        if let Some(found) = harness.events(peer)[seen..].iter().find_map(&wanted) {
            return Some((harness.now_micros(), found));
        }
    }
    None
}

fn lossy(seed: u64) -> LinkConditions {
    LinkConditions {
        latency_micros: 100_000,
        jitter_micros: 0,
        loss_per_mille: 100,
        duplicate_per_mille: 20,
        reorder_per_mille: 0,
        reorder_extra_micros: 0,
        seed,
    }
}

#[test]
fn silent_peer_raises_a_notice_then_ends() {
    let timeouts = Timeouts::default();
    let mut harness = Harness::new(sessions(2));
    assert!(harness.run_until_running(ONE_SECOND));
    harness.run_until(ONE_SECOND, |_| false);
    harness.mute(PeerId(1));
    let muted = harness.now_micros();
    let silent = timed(
        &mut harness,
        2 * ONE_SECOND,
        PeerId(0),
        |event| match event {
            SessionEvent::PeerSilent { peer, for_micros } => Some((*peer, *for_micros)),
            _ => None,
        },
    );
    let (noticed, (peer, for_micros)) = silent.expect("no PeerSilent");
    assert_eq!(peer, PeerId(1));
    // Peer 1's last packet left one step before the mute at most, and the session checks
    // every step.
    assert!(
        (timeouts.notice_after_micros..timeouts.notice_after_micros + 20_000).contains(&for_micros),
        "{for_micros}"
    );
    assert!(noticed - muted <= timeouts.notice_after_micros + 1_000);
    assert_eq!(harness.state(PeerId(0)), SessionState::Running);
    // One notice per silence.
    harness.run_until(ONE_SECOND / 2, |_| false);
    let notices = |harness: &Harness| {
        harness
            .events(PeerId(0))
            .iter()
            .filter(|event| matches!(event, SessionEvent::PeerSilent { .. }))
            .count()
    };
    assert_eq!(notices(&harness), 1);
    harness.unmute(PeerId(1));
    let back = timed(&mut harness, ONE_SECOND, PeerId(0), |event| match event {
        SessionEvent::PeerBack { peer } => Some(*peer),
        _ => None,
    });
    assert_eq!(back.map(|(_, peer)| peer), Some(PeerId(1)));
    assert_eq!(harness.state(PeerId(0)), SessionState::Running);
    // Silent again: a second notice, then the end after `disconnect_after_micros`.
    harness.run_until(ONE_SECOND, |_| false);
    harness.mute(PeerId(1));
    let muted = harness.now_micros();
    assert!(harness.run_until(4 * ONE_SECOND, |harness| {
        harness.state(PeerId(0)) != SessionState::Running
    }));
    let silence = harness.now_micros() - muted;
    assert!(
        (timeouts.disconnect_after_micros - 20_000..=timeouts.disconnect_after_micros + 2_000)
            .contains(&silence),
        "{silence}"
    );
    assert_eq!(notices(&harness), 2);
    assert_eq!(
        harness.state(PeerId(0)),
        ended(EndReason::Lost { peer: PeerId(1) })
    );
}

#[test]
fn lost_peer_ends_the_match_for_everyone() {
    let mut harness = Harness::new(sessions(3));
    assert!(harness.run_until_running(ONE_SECOND));
    harness.run_until(ONE_SECOND, |_| false);
    harness.mute(PeerId(2));
    assert!(harness.run_until(4 * ONE_SECOND, |harness| {
        [0, 1]
            .into_iter()
            .all(|peer| harness.state(PeerId(peer)) != SessionState::Running)
    }));
    for peer in [0, 1] {
        assert_eq!(
            harness.state(PeerId(peer)),
            ended(EndReason::Lost { peer: PeerId(2) }),
            "peer {peer}"
        );
        assert_landed(&harness, PeerId(peer));
    }
}

#[test]
fn lost_notice_naming_its_sender_is_implausible() {
    let mut session = handshake(sessions(3)).remove(0);
    deliver(
        &mut session,
        1,
        vec![Message::End {
            reason: ludenea_net::EndNotice::Lost { peer: PeerId(1) },
        }],
    );
    assert_eq!(
        session.state(),
        ended(EndReason::Violation {
            peer: PeerId(1),
            player: None,
            tick: None,
            rule: ViolationRule::ImplausibleEnd,
        })
    );
    // Naming a third peer, or the receiver, an honest peer can.
    for named in [0, 2] {
        let mut session = handshake(sessions(3)).remove(0);
        deliver(
            &mut session,
            1,
            vec![Message::End {
                reason: ludenea_net::EndNotice::Lost {
                    peer: PeerId(named),
                },
            }],
        );
        assert_eq!(
            session.state(),
            ended(EndReason::Lost {
                peer: PeerId(named)
            })
        );
    }
}

/// Runs a match of `peers` behind [`lossy`] conditions seeded with `seed`, has `leaver` leave
/// after two seconds, and checks every peer ends with the same `Left`, holding the same
/// confirmed inputs through its last tick, at the state they give, well before any timeout.
fn goodbye_run(peers: u8, seed: u64, leaver: PeerId) {
    let mut harness = Harness::conditioned(sessions(peers), lossy(seed));
    assert!(harness.run_until_running(2 * ONE_SECOND), "seed {seed}");
    harness.run_until(2 * ONE_SECOND, |_| false);
    harness.leave(leaver);
    assert_eq!(harness.state(leaver), SessionState::Leaving);
    let all_ended = |harness: &Harness| {
        harness
            .peers()
            .all(|peer| matches!(harness.state(peer), SessionState::Ended(_)))
    };
    // Well within `disconnect_after_micros`: the peers ended on the goodbye, not by silence.
    assert!(
        harness.run_until(2 * ONE_SECOND, all_ended),
        "seed {seed}: {:?}",
        harness
            .peers()
            .map(|peer| harness.state(peer))
            .collect::<Vec<_>>()
    );
    // Lingering ends quietly: every peer stays as it ended.
    harness.run_until(ONE_SECOND, |_| false);
    let SessionState::Ended(EndReason::Left { peer, last_tick }) = harness.state(leaver) else {
        panic!("seed {seed}: the leaver ended {:?}", harness.state(leaver));
    };
    assert_eq!(peer, leaver, "seed {seed}");
    let reference = encoded_confirmed(&harness, leaver);
    for peer in harness.peers() {
        assert_eq!(
            harness.state(peer),
            left(leaver.0, last_tick),
            "seed {seed}"
        );
        assert_eq!(assert_landed(&harness, peer), last_tick + 1, "seed {seed}");
        assert_eq!(
            encoded_confirmed(&harness, peer),
            reference,
            "seed {seed}: peer {peer} confirmed other inputs"
        );
    }
}

#[test]
fn goodbye_ends_everyone_at_the_same_tick() {
    for peers in [2, 3] {
        for seed in 0..8 {
            goodbye_run(
                peers,
                seed,
                PeerId(u8::try_from(seed % u64::from(peers)).unwrap()),
            );
        }
    }
}

#[test]
fn leaver_that_ends_at_once_still_ends_everyone() {
    // Without latency, peer 1 holds every input of peer 0 and acknowledged them: peer 0 ends
    // on the next packet it reads, before it sends any goodbye. Peer 1 hears it from the
    // farewell peer 0 sends while it lingers, not by a timeout.
    let mut harness = Harness::new(sessions(2));
    assert!(harness.run_until_running(ONE_SECOND));
    harness.run_until(ONE_SECOND, |_| false);
    harness.leave(PeerId(0));
    harness.step();
    let SessionState::Ended(EndReason::Left { peer, last_tick }) = harness.state(PeerId(0)) else {
        panic!("{:?}", harness.state(PeerId(0)));
    };
    assert_eq!(peer, PeerId(0));
    assert!(harness.run_until(ONE_SECOND / 10, |harness| {
        harness.state(PeerId(1)) != SessionState::Running
    }));
    assert_eq!(harness.state(PeerId(1)), left(0, last_tick));
    for peer in harness.peers() {
        assert_eq!(assert_landed(&harness, peer), last_tick + 1);
    }
    assert_eq!(
        encoded_confirmed(&harness, PeerId(0)),
        encoded_confirmed(&harness, PeerId(1))
    );
}

#[test]
fn leaving_peer_stops_adding_inputs_and_ends_after_the_timeout() {
    let timeouts = Timeouts::default();
    let mut harness = Harness::new(sessions(2));
    assert!(harness.run_until_running(ONE_SECOND));
    harness.run_until(ONE_SECOND, |_| false);
    // Peer 0 runs a few frames past what peer 1 heard, so it waits for acknowledgments.
    harness.mute(PeerId(1));
    harness.run_until(ONE_SECOND / 10, |_| false);
    harness.leave(PeerId(0));
    let session = harness.session(PeerId(0));
    assert_eq!(session.leave(), Err(SessionError::NotRunning));
    assert_eq!(
        session.add_local_input(0, vec![1]),
        Err(SessionError::NotRunning)
    );
    let leaving = harness.now_micros();
    assert!(harness.run_until(4 * ONE_SECOND, |harness| {
        harness.state(PeerId(0)) != SessionState::Leaving
    }));
    let waited = harness.now_micros() - leaving;
    assert!(
        (timeouts.disconnect_after_micros..=timeouts.disconnect_after_micros + 2_000)
            .contains(&waited),
        "{waited}"
    );
    let SessionState::Ended(EndReason::Left { peer, .. }) = harness.state(PeerId(0)) else {
        panic!("{:?}", harness.state(PeerId(0)));
    };
    assert_eq!(peer, PeerId(0));
    assert_landed(&harness, PeerId(0));
}

#[test]
fn remaining_peer_missing_only_acknowledgments_ends_left_on_silence() {
    // Peer 0 at tick 5 holds its inputs through 6, and peer 1's through 5 with its goodbye:
    // every input through 5 is confirmed, and only peer 1's acknowledgments are missing.
    let timeouts = Timeouts::default();
    let run = |last_tick| {
        let mut session = handshake(sessions(2)).remove(0);
        for _ in 0..5 {
            step(&mut session, 0);
        }
        deliver(
            &mut session,
            1,
            vec![inputs(1, 0, &[0; 6]), Message::Goodbye { last_tick }],
        );
        assert_eq!(session.state(), SessionState::Running);
        session.outgoing(NOW + timeouts.disconnect_after_micros - 1);
        assert_eq!(session.state(), SessionState::Running);
        session.outgoing(NOW + timeouts.disconnect_after_micros);
        session.state()
    };
    assert_eq!(run(5), left(1, 5));
    // With an input of tick 6 still missing, the silent leaver is lost.
    assert_eq!(run(6), ended(EndReason::Lost { peer: PeerId(1) }));
}

#[test]
fn goodbye_behind_is_a_violation() {
    // Peer 0 at tick 5 holds its inputs through 6; peer 1's through 5: confirmed tick 6.
    let setup = || {
        let mut session = handshake(sessions(2)).remove(0);
        for _ in 0..5 {
            step(&mut session, 0);
        }
        deliver(&mut session, 1, vec![inputs(1, 0, &[0; 6])]);
        session
    };
    let mut session = setup();
    deliver(&mut session, 1, vec![Message::Goodbye { last_tick: 4 }]);
    assert_eq!(
        session.state(),
        ended(EndReason::Violation {
            peer: PeerId(1),
            player: None,
            tick: Some(4),
            rule: ViolationRule::GoodbyeBehind,
        })
    );
    let mut session = setup();
    deliver(&mut session, 1, vec![Message::Goodbye { last_tick: 5 }]);
    assert_eq!(session.state(), SessionState::Running);
    // Past the furthest input an honest peer can have: tick 5 + delay 2 + window 8 + delay 2.
    let mut session = setup();
    deliver(&mut session, 1, vec![Message::Goodbye { last_tick: 17 }]);
    assert_eq!(session.state(), SessionState::Running);
    let mut session = setup();
    deliver(&mut session, 1, vec![Message::Goodbye { last_tick: 18 }]);
    assert_eq!(
        session.state(),
        ended(EndReason::Violation {
            peer: PeerId(1),
            player: None,
            tick: Some(18),
            rule: ViolationRule::BeyondHorizon,
        })
    );
}

#[test]
fn lowest_goodbye_wins_and_nothing_is_confirmed_beyond_it() {
    let mut session = handshake(sessions(3)).remove(0);
    for _ in 0..12 {
        step(&mut session, 1);
    }
    deliver(&mut session, 1, vec![Message::Goodbye { last_tick: 10 }]);
    deliver(&mut session, 2, vec![Message::Goodbye { last_tick: 8 }]);
    deliver(&mut session, 1, vec![Message::Goodbye { last_tick: 9 }]);
    assert_eq!(session.state(), SessionState::Running);
    deliver(&mut session, 1, vec![inputs(1, 0, &[2; 13])]);
    deliver(&mut session, 2, vec![inputs(2, 0, &[3; 13])]);
    // Peer 0's inputs through 8 are not acknowledged yet.
    assert_eq!(session.state(), SessionState::Running);
    let requests = step(&mut session, 1);
    let confirmed: Vec<u64> = session
        .events()
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::Confirmed { tick, .. } => Some(tick),
            _ => None,
        })
        .collect();
    assert_eq!(confirmed, (0..=8).collect::<Vec<_>>());
    // At the tick after the last one, the session runs no tick and needs no input.
    assert!(
        !requests
            .iter()
            .any(|request| matches!(request, Request::Advance { tick, .. } if *tick > 8)),
        "{requests:?}"
    );
    assert_eq!(session.advance(NOW).map(|_| ()), Ok(()));
    deliver(&mut session, 1, vec![ack(0, 9)]);
    assert_eq!(session.state(), SessionState::Running);
    deliver(&mut session, 2, vec![ack(0, 9)]);
    assert_eq!(session.state(), left(2, 8));
}

#[test]
fn ended_session_lands_on_the_confirmed_state() {
    // Exactly: peer 0 ran ticks 0 to 4 predicting peer 1's default input; peer 1's inputs of
    // 0 to 6 differ, and the packet breaks a rule. The landing loads tick 0, the latest state
    // built only from confirmed inputs, then runs every confirmed tick, 0 to 6, past the
    // current tick 5.
    let mut session = handshake(sessions(2)).remove(0);
    for _ in 0..5 {
        step(&mut session, 0);
    }
    deliver(
        &mut session,
        1,
        vec![inputs(1, 0, &[1; 7]), inputs(0, 0, &[0])],
    );
    assert!(matches!(
        session.state(),
        SessionState::Ended(EndReason::Violation { .. })
    ));
    let mut expected = vec![Request::Load { tick: 0 }];
    for tick in 0..7 {
        expected.push(Request::Save { tick, hash: false });
        expected.push(Request::Advance {
            tick,
            inputs: vec![vec![0], vec![1]],
        });
    }
    assert_eq!(session.advance(NOW), Ok(expected));
    assert_eq!(session.advance(NOW), Err(SessionError::NotRunning));

    // Every ending of a running match, through the model game under latency, with
    // mispredictions to correct. Left: the end tick is the tick after the last one.
    let latency = LinkConditions {
        loss_per_mille: 0,
        duplicate_per_mille: 0,
        latency_micros: 60_000,
        ..lossy(5)
    };
    let mut harness = Harness::conditioned(sessions(2), latency);
    assert!(harness.run_until_running(ONE_SECOND));
    harness.run_until(ONE_SECOND, |_| false);
    harness.leave(PeerId(1));
    assert!(harness.run_until(2 * ONE_SECOND, |harness| {
        harness
            .peers()
            .all(|peer| matches!(harness.state(peer), SessionState::Ended(_)))
    }));
    for peer in harness.peers() {
        assert_landed(&harness, peer);
    }

    // Lost, Desync, a violation and a mismatch: the end tick is the confirmed tick.
    let mut harness = Harness::conditioned(sessions(2), latency);
    assert!(harness.run_until_running(ONE_SECOND));
    harness.run_until(ONE_SECOND, |_| false);
    harness.mute(PeerId(1));
    assert!(harness.run_until(4 * ONE_SECOND, |harness| {
        harness.state(PeerId(0)) != SessionState::Running
    }));
    assert_eq!(
        harness.state(PeerId(0)),
        ended(EndReason::Lost { peer: PeerId(1) })
    );
    assert_landed(&harness, PeerId(0));

    let mut harness = Harness::conditioned(sessions(2), latency);
    harness.flip_hash(PeerId(1), 60);
    assert!(harness.run_until(4 * ONE_SECOND, |harness| {
        harness
            .peers()
            .all(|peer| matches!(harness.state(peer), SessionState::Ended(_)))
    }));
    for peer in harness.peers() {
        assert!(
            matches!(
                harness.state(peer),
                SessionState::Ended(EndReason::Desync { tick: 60, .. })
            ),
            "{:?}",
            harness.state(peer)
        );
        assert_landed(&harness, peer);
    }

    let hostile = [
        // An input of a player peer 1 does not own.
        vec![inputs(0, 0, &[0])],
        // Another view of the match.
        vec![Message::Hello {
            identity: {
                let mut other = identity(&[0, 1]);
                other.rules = MatchRules {
                    hash_interval: 31,
                    ..other.rules
                };
                other
            },
        }],
    ];
    for messages in hostile {
        let mut harness = Harness::conditioned(sessions(2), latency);
        assert!(harness.run_until_running(ONE_SECOND));
        harness.run_until(ONE_SECOND, |_| false);
        let now = harness.now_micros();
        let packet = Packet {
            match_id: 1,
            messages,
        };
        harness
            .session(PeerId(0))
            .receive(PeerId(1), &packet.encode(), now);
        assert!(matches!(
            harness.state(PeerId(0)),
            SessionState::Ended(EndReason::Violation { .. } | EndReason::Mismatch { .. })
        ));
        harness.step();
        assert_landed(&harness, PeerId(0));
    }
}
