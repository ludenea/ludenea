mod support;

use ludenea_net::{
    EndReason, LinkConditions, MatchRules, Message, Packet, PeerId, Request, Session, SessionError,
    SessionEvent, SessionState, Timeouts, ViolationRule,
};
use support::{Harness, handshake, identity, offline_hashes, sessions, sessions_of};

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

/// [`lossy`], with 30 ms of jitter and 150 per mille of packets reordered by 60 ms more.
fn reordered(seed: u64) -> LinkConditions {
    LinkConditions {
        jitter_micros: 30_000,
        reorder_per_mille: 150,
        reorder_extra_micros: 60_000,
        ..lossy(seed)
    }
}

/// Steps until every peer ended, within `max_micros`; whether they did.
fn run_until_all_ended(harness: &mut Harness, max_micros: u64) -> bool {
    harness.run_until(max_micros, |harness| {
        harness
            .peers()
            .all(|peer| matches!(harness.state(peer), SessionState::Ended(_)))
    })
}

/// Every peer ended with the same `Left`, at the state of the tick after its last tick, with
/// the same confirmed inputs, encoded. Returns the `Left`'s peer and last tick.
fn assert_same_left(harness: &Harness, case: &str) -> (PeerId, u64) {
    let SessionState::Ended(EndReason::Left { peer, last_tick }) = harness.state(PeerId(0)) else {
        panic!("{case}: peer 0 ended {:?}", harness.state(PeerId(0)));
    };
    let reference = encoded_confirmed(harness, PeerId(0));
    for other in harness.peers() {
        assert_eq!(harness.state(other), left(peer.0, last_tick), "{case}");
        assert_eq!(assert_landed(harness, other), last_tick + 1, "{case}");
        assert_eq!(
            encoded_confirmed(harness, other),
            reference,
            "{case}: peer {other} confirmed other inputs"
        );
    }
    (peer, last_tick)
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
fn goodbye_run(peers: u8, conditions: LinkConditions, leaver: PeerId) {
    let case = format!("{peers} peers, {conditions:?}");
    let mut harness = Harness::conditioned(sessions(peers), conditions);
    assert!(harness.run_until_running(2 * ONE_SECOND), "{case}");
    harness.run_until(2 * ONE_SECOND, |_| false);
    harness.leave(leaver);
    assert_eq!(harness.state(leaver), SessionState::Leaving);
    // Well within `disconnect_after_micros`: the peers ended on the goodbye, not by silence.
    assert!(
        run_until_all_ended(&mut harness, 2 * ONE_SECOND),
        "{case}: {:?}",
        harness
            .peers()
            .map(|peer| harness.state(peer))
            .collect::<Vec<_>>()
    );
    // Lingering ends quietly: every peer stays as it ended.
    harness.run_until(ONE_SECOND, |_| false);
    assert_eq!(assert_same_left(&harness, &case).0, leaver, "{case}");
}

#[test]
fn goodbye_ends_everyone_at_the_same_tick() {
    for peers in [2, 3] {
        for seed in 0..8 {
            let leaver = PeerId(u8::try_from(seed % u64::from(peers)).unwrap());
            goodbye_run(peers, lossy(seed), leaver);
            goodbye_run(peers, reordered(seed), leaver);
        }
    }
}

#[test]
fn two_leavers_end_everyone_at_the_lowest_last_tick() {
    // Peers 1 and 2 leave 0, 20 or 150 ms apart, under loss, duplication and reordering:
    // every peer ends at the lowest last tick, naming the same leaver, with the same inputs.
    for peers in [3, 4] {
        for offset_micros in [0, 20_000, 150_000] {
            for seed in 0..4 {
                let case = format!("{peers} peers, {offset_micros} us apart, seed {seed}");
                let mut harness = Harness::conditioned(sessions(peers), reordered(seed));
                assert!(harness.run_until_running(2 * ONE_SECOND), "{case}");
                harness.run_until(2 * ONE_SECOND, |_| false);
                harness.leave(PeerId(1));
                harness.run_until(offset_micros, |_| false);
                assert_eq!(harness.state(PeerId(2)), SessionState::Running, "{case}");
                harness.leave(PeerId(2));
                assert!(run_until_all_ended(&mut harness, 2 * ONE_SECOND), "{case}");
                harness.run_until(ONE_SECOND, |_| false);
                let (peer, _) = assert_same_left(&harness, &case);
                assert!([PeerId(1), PeerId(2)].contains(&peer), "{case}: {peer}");
            }
        }
    }
}

#[test]
fn leave_at_tick_zero_stops_after_the_input_delay() {
    // With no input delay, the leaver's last input is the default one of tick 0; with the
    // default delay of 2, those of ticks 0 and 1. It runs those ticks, then waits.
    for delay in [0u32, 2] {
        let mut identity = identity(&[0, 1]);
        identity.rules.input_delay = delay;
        let mut session = handshake(sessions_of(&identity)).remove(0);
        session.leave().unwrap();
        assert_eq!(session.state(), SessionState::Leaving, "delay {delay}");
        let last_tick = u64::from(delay.max(1)) - 1;
        let mut ran = Vec::new();
        for _ in 0..4 {
            let requests = session.advance(NOW).unwrap();
            ran.extend(requests.into_iter().filter_map(|request| match request {
                Request::Advance { tick, .. } => Some(tick),
                _ => None,
            }));
        }
        assert_eq!(ran, (0..=last_tick).collect::<Vec<_>>(), "delay {delay}");
        let sent = session.outgoing(NOW);
        let goodbye = Packet::decode(&sent[0].1).unwrap();
        assert!(
            goodbye.messages.contains(&Message::Goodbye { last_tick }),
            "delay {delay}: {goodbye:?}"
        );
        let remote = vec![0; usize::try_from(last_tick + 1).unwrap()];
        deliver(
            &mut session,
            1,
            vec![inputs(1, 0, &remote), ack(0, last_tick + 1)],
        );
        assert_eq!(session.state(), left(0, last_tick), "delay {delay}");
        assert_eq!(session.advance(NOW), Ok(Vec::new()), "delay {delay}");
        assert_eq!(session.advance(NOW), Err(SessionError::NotRunning));
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
fn leaver_heard_out_is_not_silent() {
    // Three peers: peer 2 left at tick 5 and peer 0 holds its inputs through it, so peer 2
    // has nothing more to say once it ended. Peer 0 still waits for peer 1's inputs, which
    // keeps talking: the silence of peer 2 raises no notice and ends nothing.
    let timeouts = Timeouts::default();
    let mut session = handshake(sessions(3)).remove(0);
    for _ in 0..5 {
        step(&mut session, 0);
    }
    deliver(
        &mut session,
        2,
        vec![inputs(2, 0, &[0; 6]), Message::Goodbye { last_tick: 5 }],
    );
    session.events();
    let peer_one = Packet {
        match_id: 1,
        messages: vec![inputs(1, 0, &[0; 3])],
    };
    let mut now = NOW;
    while now <= NOW + 2 * timeouts.disconnect_after_micros {
        session.receive(PeerId(1), &peer_one.encode(), now);
        session.outgoing(now);
        assert_eq!(session.state(), SessionState::Running, "at {now}");
        now += 100_000;
    }
    assert!(
        !session
            .events()
            .iter()
            .any(|event| matches!(event, SessionEvent::PeerSilent { .. })),
    );
    // A leaver whose inputs through its last tick are still missing stays watched.
    let mut session = handshake(sessions(3)).remove(0);
    for _ in 0..5 {
        step(&mut session, 0);
    }
    deliver(
        &mut session,
        2,
        vec![inputs(2, 0, &[0; 5]), Message::Goodbye { last_tick: 5 }],
    );
    let mut now = NOW;
    while session.state() == SessionState::Running {
        session.receive(PeerId(1), &peer_one.encode(), now);
        session.outgoing(now);
        now += 100_000;
    }
    assert_eq!(session.state(), ended(EndReason::Lost { peer: PeerId(2) }));
}

#[test]
fn silent_leaver_waives_only_its_own_acknowledgments() {
    // Three peers: peer 2 leaves and ends. Peer 0 then holds every input through the last
    // tick, but peer 1, which still hears peer 0, gets none of peer 0's inputs, so it cannot
    // acknowledge them. Peer 2's silence must not end peer 0 while peer 1, still talking,
    // needs its inputs: once the link heals, every peer ends at the same tick.
    let timeouts = Timeouts::default();
    let mut harness = Harness::new(sessions(3));
    assert!(harness.run_until_running(ONE_SECOND));
    harness.run_until(ONE_SECOND, |_| false);
    // A few frames before the leave, so peer 1 lacks peer 0's inputs up to the last tick.
    harness.strip_inputs(PeerId(0), Some(PeerId(1)));
    harness.run_until(ONE_SECOND / 10, |_| false);
    harness.leave(PeerId(2));
    assert!(harness.run_until(ONE_SECOND, |harness| {
        matches!(harness.state(PeerId(2)), SessionState::Ended(_))
    }));
    harness.run_until(timeouts.disconnect_after_micros + ONE_SECOND, |_| false);
    for peer in [0, 1] {
        assert_eq!(
            harness.state(PeerId(peer)),
            SessionState::Running,
            "peer {peer}"
        );
    }
    harness.strip_inputs(PeerId(0), None);
    assert!(run_until_all_ended(&mut harness, ONE_SECOND));
    let (peer, _) = assert_same_left(&harness, "one-way loss");
    assert_eq!(peer, PeerId(2));
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
fn tied_goodbyes_name_the_lowest_peer() {
    // Whatever order the goodbyes of the same last tick arrive in, `Left` names the lowest
    // peer, so it is the same on every peer.
    for order in [[2, 1], [1, 2]] {
        let mut session = handshake(sessions(3)).remove(0);
        for _ in 0..12 {
            step(&mut session, 1);
        }
        for from in order {
            deliver(&mut session, from, vec![Message::Goodbye { last_tick: 8 }]);
        }
        deliver(&mut session, 1, vec![inputs(1, 0, &[2; 9]), ack(0, 9)]);
        deliver(&mut session, 2, vec![inputs(2, 0, &[3; 9]), ack(0, 9)]);
        assert_eq!(session.state(), left(1, 8), "{order:?}");
    }
}

#[test]
fn landing_asks_no_hash() {
    // Peer 0 at tick 33 with peer 1's default inputs through 27, then inputs of 28 to 34
    // that differ, in a packet that breaks a rule. The landing runs tick 30 again, a reported
    // tick, without asking its hash: an ended session compares nothing and releases nothing.
    let mut session = handshake(sessions(2)).remove(0);
    for tick in 0..33 {
        step(&mut session, 0);
        if tick < 28 {
            deliver(
                &mut session,
                1,
                vec![inputs(1, tick, &[0]), ack(0, tick + 1)],
            );
        }
    }
    deliver(
        &mut session,
        1,
        vec![inputs(1, 28, &[1; 7]), inputs(0, 0, &[0])],
    );
    assert!(matches!(
        session.state(),
        SessionState::Ended(EndReason::Violation { .. })
    ));
    let mut expected = vec![Request::Load { tick: 28 }];
    for tick in 28..35 {
        expected.push(Request::Save { tick, hash: false });
        expected.push(Request::Advance {
            tick,
            inputs: vec![vec![0], vec![1]],
        });
    }
    assert_eq!(session.advance(NOW), Ok(expected));
    assert_eq!(session.report_hash(30, 7), Ok(()));
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
