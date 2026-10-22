mod support;

use std::collections::BTreeSet;

use ludenea_app::{PlayerInputSpec, Rng};
use ludenea_net::{
    DropReason, EndNotice, EndReason, LinkConditions, MAX_INPUT_BYTES, MAX_TICKS_PER_MESSAGE,
    MatchIdentity, MatchRules, Message, Packet, PeerId, Session, SessionError, SessionState,
    ViolationRule,
};
use support::{Harness, Model, handshake, identity, sessions, settings};

const ONE_SECOND: u64 = 1_000_000;
const NOW: u64 = 1_000;
/// The default `hash_interval`.
const HASH_INTERVAL: u64 = 30;

/// Peer 0 of a running two-peer match at tick 0, with the default rules.
fn peer_zero() -> Session {
    handshake(sessions(2)).remove(0)
}

/// Hands `session` a packet of `messages` from peer 1.
fn from_peer_one(session: &mut Session, messages: Vec<Message>) {
    let packet = Packet {
        match_id: 1,
        messages,
    };
    session.receive(PeerId(1), &packet.encode(), NOW);
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

/// Adds `input` for player 0, advances, and sends: the packet to peer 1 is taken and dropped,
/// as if lost, so the session counts its inputs as sent.
fn step(session: &mut Session, input: u8) {
    session.add_local_input(0, vec![input]).unwrap();
    session.advance(NOW).unwrap();
    session.outgoing(NOW);
}

fn violation(peer: u8, player: Option<u8>, tick: Option<u64>, rule: ViolationRule) -> SessionState {
    SessionState::Ended(EndReason::Violation {
        peer: PeerId(peer),
        player,
        tick,
        rule,
    })
}

fn rules() -> (u64, u64) {
    let rules = MatchRules::default();
    (
        u64::from(rules.input_delay),
        u64::from(rules.rollback_window),
    )
}

/// The local horizon of a session at `tick`: the highest tick it has a local input for.
fn horizon(tick: u64) -> u64 {
    let (delay, _) = rules();
    tick + delay - 1
}

/// The highest remote input tick a session at `tick` accepts: its horizon, plus one, plus
/// `rollback_window + input_delay`.
fn horizon_bound(tick: u64) -> u64 {
    let (delay, window) = rules();
    horizon(tick) + 1 + window + delay
}

/// How far the newest acknowledgment of a peer may trail the local horizon.
fn lag_bound() -> u64 {
    let (delay, window) = rules();
    2 * (window + delay) + 1
}

/// An ended session ignores what it receives, and refuses to run.
fn assert_stays_ended(session: &mut Session) {
    let state = session.state();
    session.events();
    from_peer_one(session, vec![inputs(1, 0, &[0])]);
    assert_eq!(session.state(), state);
    assert_eq!(session.events(), []);
    assert_eq!(session.advance(NOW), Err(SessionError::NotRunning));
    assert_eq!(session.outgoing(NOW), []);
}

#[test]
fn conflicting_input_disconnects_the_peer() {
    let mut session = peer_zero();
    from_peer_one(&mut session, vec![inputs(1, 0, &[3, 4])]);
    assert_eq!(session.state(), SessionState::Running);
    // Tick 0 is the same and tick 1 differs: the confirmed input of tick 1 is kept, as the
    // queue refuses to overwrite it (the queue's own test
    // `refuses_a_conflict_and_a_gap_without_changing_anything` checks it), and the peer that
    // sent another one is disconnected.
    from_peer_one(&mut session, vec![inputs(1, 0, &[3, 5, 6])]);
    assert_eq!(
        session.state(),
        violation(1, Some(1), Some(1), ViolationRule::ConflictingInput)
    );
    assert_stays_ended(&mut session);
}

#[test]
fn undecodable_input_disconnects_the_peer() {
    let mut session = peer_zero();
    let message = Message::Inputs {
        player: 1,
        first_tick: 4,
        inputs: vec![vec![0], vec![0, 0]],
    };
    // Ticks 0 to 3 first, so the refused tick would otherwise be accepted.
    from_peer_one(&mut session, vec![inputs(1, 0, &[0; 4]), message]);
    assert_eq!(
        session.state(),
        violation(1, Some(1), Some(5), ViolationRule::InvalidInput)
    );
    assert_stays_ended(&mut session);
}

#[test]
fn input_for_a_player_the_peer_does_not_own_is_a_violation() {
    let cases = [
        // Player 0 is peer 0's own, and player 9 does not exist.
        (inputs(0, 0, &[0]), Some(0), Some(0)),
        (inputs(9, 3, &[0]), Some(9), Some(3)),
        // An acknowledgment names a player of the receiver: player 1 is peer 1's.
        (ack(1, 0), Some(1), Some(0)),
    ];
    for (message, player, tick) in cases {
        let mut session = peer_zero();
        from_peer_one(&mut session, vec![message.clone()]);
        assert_eq!(
            session.state(),
            violation(1, player, tick, ViolationRule::NotOwner),
            "{message:?}"
        );
        assert_stays_ended(&mut session);
    }
    // A connecting session reads the inputs of a peer whose matching Hello it has, with the
    // same checks: peer 1 waits for peer 2, and peer 0 sends an input of player 2.
    let identity = identity(&[0, 1, 2]);
    let mut session = Session::new(settings(1, &identity), support::model_input()).unwrap();
    let packet = Packet {
        match_id: 1,
        messages: vec![Message::Hello { identity }, inputs(2, 0, &[0])],
    };
    session.receive(PeerId(0), &packet.encode(), NOW);
    assert_eq!(
        session.state(),
        violation(0, Some(2), Some(0), ViolationRule::NotOwner)
    );
}

#[test]
fn input_beyond_the_horizon() {
    let start = || {
        let mut session = peer_zero();
        for _ in 0..3 {
            step(&mut session, 0);
        }
        session
    };
    let bound = horizon_bound(3);
    let ticks = |count: u64| vec![0; usize::try_from(count).unwrap()];
    // Every tick up to the bound, in two messages.
    let mut session = start();
    from_peer_one(
        &mut session,
        vec![inputs(1, 0, &ticks(10)), inputs(1, 10, &ticks(bound - 9))],
    );
    assert_eq!(session.state(), SessionState::Running);
    // One tick above it, alone or at the end of a message.
    from_peer_one(&mut session, vec![inputs(1, bound + 1, &[0])]);
    assert_eq!(
        session.state(),
        violation(1, Some(1), Some(bound + 1), ViolationRule::BeyondHorizon)
    );
    let mut session = start();
    from_peer_one(
        &mut session,
        vec![inputs(1, 0, &ticks(10)), inputs(1, 10, &ticks(bound - 8))],
    );
    assert_eq!(
        session.state(),
        violation(1, Some(1), Some(bound + 1), ViolationRule::BeyondHorizon)
    );
    assert_stays_ended(&mut session);
}

#[test]
fn honest_peer_at_the_horizon_under_jitter_is_not_disconnected() {
    // 150 ms each way is more than the 8-tick window (133 ms): each peer keeps stalling at
    // the edge of its window, waiting for the other's inputs. Peer 1 also freezes for a
    // second, still receiving and sending: peer 0 gets every input peer 1 has and runs to the
    // edge of its window, so its inputs reach the highest tick an honest peer can send peer 1.
    // Then peer 1 catches up one frame per step, at the edge of its own window.
    for seed in 0..3 {
        let conditions = LinkConditions {
            latency_micros: 150_000,
            jitter_micros: 30_000,
            loss_per_mille: 0,
            duplicate_per_mille: 0,
            reorder_per_mille: 0,
            reorder_extra_micros: 0,
            seed,
        };
        let confirmed =
            |ticks| move |h: &Harness| h.peers().all(|peer| h.confirmed(peer).len() >= ticks);
        let mut harness = Harness::conditioned(sessions(2), conditions);
        assert!(harness.run_until(30 * ONE_SECOND, confirmed(60)));
        harness.freeze(PeerId(1));
        harness.run_until(ONE_SECOND, |_| false);
        harness.thaw(PeerId(1));
        assert!(harness.run_until(30 * ONE_SECOND, confirmed(300)));
        for peer in harness.peers() {
            assert_eq!(harness.state(peer), SessionState::Running, "{peer}");
            let stats = &harness.session(peer).stats().peers;
            let stalls: u64 = stats.values().map(|stats| stats.stalls).sum();
            assert!(stalls > 0, "{peer} never ran at the edge of its window");
        }
        let reference = harness.confirmed(PeerId(0));
        let other = harness.confirmed(PeerId(1));
        let common = reference.len().min(other.len());
        assert_eq!(reference[..common], other[..common]);
    }
}

#[test]
fn ack_never_sent_is_a_violation() {
    // Peer 0 sent ticks 0 to 2 of player 0: the input delay and one tick.
    let sent = || {
        let mut session = peer_zero();
        step(&mut session, 0);
        session
    };
    let mut session = sent();
    from_peer_one(&mut session, vec![ack(0, 3)]);
    assert_eq!(session.state(), SessionState::Running);
    from_peer_one(&mut session, vec![ack(0, 4)]);
    assert_eq!(
        session.state(),
        violation(1, Some(0), Some(4), ViolationRule::AckNeverSent)
    );
    // Held but not sent yet is never sent.
    let mut session = sent();
    session.add_local_input(0, vec![0]).unwrap();
    session.advance(NOW).unwrap();
    from_peer_one(&mut session, vec![ack(0, 4)]);
    assert_eq!(
        session.state(),
        violation(1, Some(0), Some(4), ViolationRule::AckNeverSent)
    );
    // No hash report was sent: tick 30 is not confirmed yet, and 7 is not a reported tick.
    let mut session = sent();
    from_peer_one(
        &mut session,
        vec![Message::Ack {
            players: vec![(0, 0)],
            hashes_through: 7,
        }],
    );
    assert_eq!(
        session.state(),
        violation(1, None, Some(7), ViolationRule::AckNeverSent)
    );
    assert_stays_ended(&mut session);
}

#[test]
fn ack_of_a_tick_left_out_of_a_full_packet_is_never_sent() {
    // Three local players with inputs at the largest size: 18 unacknowledged ticks each do
    // not fit in one packet, so the newest ticks wait for a later one.
    let identity = identity(&[0, 0, 0, 1]);
    let any_input = PlayerInputSpec {
        default: vec![0],
        check: |_| Ok(()),
    };
    let sessions = (0..2)
        .map(|local| Session::new(settings(local, &identity), any_input.clone()).unwrap())
        .collect();
    let mut session = handshake(sessions).remove(0);
    for tick in 0..16 {
        for player in 0..3 {
            session
                .add_local_input(player, vec![0; MAX_INPUT_BYTES])
                .unwrap();
        }
        let packet = Packet {
            match_id: 1,
            messages: vec![inputs(3, tick, &[0])],
        };
        session.receive(PeerId(1), &packet.encode(), NOW);
        session.advance(NOW).unwrap();
    }
    let outgoing = session.outgoing(NOW);
    let messages = Packet::decode(&outgoing[0].1).unwrap().messages;
    // The player the packet cut the most ticks of, and how many of its ticks it holds.
    let (sent, player) = messages
        .iter()
        .filter_map(|message| match message {
            Message::Inputs { player, inputs, .. } => {
                Some((u64::try_from(inputs.len()).unwrap(), *player))
            }
            _ => None,
        })
        .min()
        .expect("inputs of the local players");
    // Ticks 0 to 17 are held; the packet holds only the first `sent`.
    assert!(sent < 18, "{sent} ticks sent: the packet was not full");
    let ack = |next| Packet {
        match_id: 1,
        messages: vec![ack(player, next)],
    };
    session.receive(PeerId(1), &ack(sent).encode(), NOW);
    assert_eq!(session.state(), SessionState::Running);
    session.receive(PeerId(1), &ack(sent + 1).encode(), NOW);
    assert_eq!(
        session.state(),
        violation(1, Some(player), Some(sent + 1), ViolationRule::AckNeverSent)
    );
}

/// Peer 0 running 30 ticks while peer 1 sends its inputs, each with an acknowledgment that
/// trails peer 0's horizon by `lag` ticks, or with none.
fn lagging_peer(lag: Option<u64>) -> Session {
    let mut session = peer_zero();
    for tick in 0..30 {
        let horizon = horizon(tick);
        let mut messages = Vec::new();
        if let Some(lag) = lag {
            messages.push(ack(0, horizon.saturating_sub(lag)));
        }
        messages.push(inputs(1, tick, &[0]));
        from_peer_one(&mut session, messages);
        if session.state() != SessionState::Running {
            break;
        }
        step(&mut session, 0);
    }
    session
}

#[test]
fn ack_lagging_is_a_violation() {
    let bound = lag_bound();
    assert_eq!(lagging_peer(Some(bound)).state(), SessionState::Running);
    // The first packet read at a horizon one above the bound ends it: tick 0 is still the
    // newest one acknowledged, at horizon `bound + 1`.
    assert_eq!(
        lagging_peer(Some(bound + 1)).state(),
        violation(1, Some(0), Some(0), ViolationRule::AckLagging),
    );
    // A peer that never acknowledges is lagging just the same, so the local inputs kept for
    // it stay bounded.
    let mut session = lagging_peer(None);
    assert_eq!(
        session.state(),
        violation(1, Some(0), Some(0), ViolationRule::AckLagging)
    );
    assert_stays_ended(&mut session);
}

#[test]
fn ticks_near_u64_max_never_panic() {
    let full = vec![vec![0]; usize::try_from(MAX_TICKS_PER_MESSAGE).unwrap()];
    let cases = [
        (
            inputs(1, u64::MAX, &[0]),
            violation(1, Some(1), Some(u64::MAX), ViolationRule::BeyondHorizon),
        ),
        (
            inputs(1, u64::MAX - 1, &[0, 0, 0]),
            violation(1, Some(1), Some(u64::MAX - 1), ViolationRule::BeyondHorizon),
        ),
        (
            Message::Inputs {
                player: 1,
                first_tick: u64::MAX - 5,
                inputs: full,
            },
            violation(1, Some(1), Some(u64::MAX - 5), ViolationRule::BeyondHorizon),
        ),
        (
            ack(0, u64::MAX),
            violation(1, Some(0), Some(u64::MAX), ViolationRule::AckNeverSent),
        ),
        (
            Message::Ack {
                players: vec![],
                hashes_through: u64::MAX,
            },
            violation(1, None, Some(u64::MAX), ViolationRule::AckNeverSent),
        ),
        // A report of a peer at the last tick, read and answered; a reply to no request,
        // dropped. Never a panic.
        (
            Message::Quality {
                tick: u64::MAX,
                sent_micros: u64::MAX,
                advantage: i32::MIN,
            },
            SessionState::Running,
        ),
        (
            Message::QualityReply {
                echo_micros: u64::MAX,
                tick: u64::MAX,
            },
            SessionState::Running,
        ),
        (
            Message::Hash {
                tick: u64::MAX,
                hash: u64::MAX,
            },
            violation(1, None, Some(u64::MAX), ViolationRule::HashBeyondHorizon),
        ),
        // Not read yet, and never a panic.
        (
            Message::Goodbye {
                last_tick: u64::MAX,
            },
            SessionState::Running,
        ),
        (
            Message::End {
                reason: EndNotice::LocalFailure { tick: u64::MAX },
            },
            SessionState::Ended(EndReason::LocalFailure { tick: u64::MAX }),
        ),
        (
            Message::End {
                reason: EndNotice::Desync {
                    tick: u64::MAX,
                    peer: PeerId(u8::MAX),
                },
            },
            SessionState::Ended(EndReason::Desync {
                tick: u64::MAX,
                peer: PeerId(1),
                local: None,
                remote: None,
            }),
        ),
    ];
    for (message, state) in cases {
        let mut session = peer_zero();
        step(&mut session, 0);
        from_peer_one(&mut session, vec![message.clone()]);
        assert_eq!(session.state(), state, "{message:?}");
    }
}

// The fuzz of `Session::receive`.

/// How many frames one seed of the fuzz runs: more than the ticks it takes an acknowledgment
/// to lag beyond its bound.
const ROUNDS: u32 = 40;

/// A tick near `tick`, near `u64::MAX`, or anywhere.
fn fuzz_tick(rng: &mut Rng, tick: u64) -> u64 {
    match rng.range_u32(0, 4) {
        0 | 1 => tick.saturating_add(u64::from(rng.range_u32(0, 40))),
        2 => u64::MAX - u64::from(rng.range_u32(0, 40)),
        _ => (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32()),
    }
}

/// Any `u64`, often an extreme one.
fn fuzz_u64(rng: &mut Rng) -> u64 {
    match rng.range_u32(0, 3) {
        0 => u64::from(rng.range_u32(0, 3)),
        1 => u64::MAX - u64::from(rng.range_u32(0, 3)),
        _ => (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32()),
    }
}

/// A player of the match, or one that does not exist.
fn fuzz_player(rng: &mut Rng) -> u8 {
    u8::try_from(rng.range_u32(0, 4)).unwrap()
}

/// One input of the model game, or bytes it refuses.
fn fuzz_input(rng: &mut Rng) -> Vec<u8> {
    let len = if rng.range_u32(0, 8) == 0 {
        rng.range_u32(0, 4)
    } else {
        1
    };
    (0..len)
        .map(|_| u8::try_from(rng.range_u32(0, 4)).unwrap())
        .collect()
}

/// A well-formed message with random fields around the receiver's `tick`. A `Hello` carries
/// the match's identity or another one.
fn fuzz_message(rng: &mut Rng, tick: u64, identity: &MatchIdentity) -> Message {
    match rng.range_u32(0, 10) {
        0..=2 => Message::Inputs {
            player: fuzz_player(rng),
            first_tick: fuzz_tick(rng, tick),
            inputs: (0..rng.range_u32(1, 9)).map(|_| fuzz_input(rng)).collect(),
        },
        3 | 4 => Message::Ack {
            players: (0..rng.range_u32(0, 3))
                .map(|_| (fuzz_player(rng), fuzz_tick(rng, tick)))
                .collect(),
            hashes_through: if rng.range_u32(0, 4) == 0 {
                fuzz_u64(rng)
            } else {
                0
            },
        },
        5 => Message::Quality {
            tick: fuzz_tick(rng, tick),
            sent_micros: fuzz_u64(rng),
            advantage: i32::from_ne_bytes(rng.next_u32().to_ne_bytes()),
        },
        6 => Message::QualityReply {
            // An echo of a time in the future: no request is pending.
            echo_micros: NOW.saturating_add(fuzz_u64(rng)),
            tick: fuzz_tick(rng, tick),
        },
        7 => match rng.range_u32(0, 3) {
            // Often a reported tick near the receiver's, which a forged hash makes a desync.
            0 => Message::Hash {
                tick: if rng.range_u32(0, 2) == 0 {
                    (tick / HASH_INTERVAL + u64::from(rng.range_u32(0, 2))) * HASH_INTERVAL
                } else {
                    fuzz_tick(rng, tick)
                },
                hash: fuzz_u64(rng),
            },
            1 => Message::Goodbye {
                last_tick: fuzz_tick(rng, tick),
            },
            _ => Message::End {
                reason: match rng.range_u32(0, 3) {
                    0 => EndNotice::Desync {
                        tick: fuzz_tick(rng, tick),
                        peer: PeerId(fuzz_player(rng)),
                    },
                    1 => EndNotice::Lost {
                        peer: PeerId(fuzz_player(rng)),
                    },
                    _ => EndNotice::LocalFailure {
                        tick: fuzz_tick(rng, tick),
                    },
                },
            },
        },
        8 => Message::Welcome,
        _ => {
            let mut identity = identity.clone();
            if rng.range_u32(0, 2) == 0 {
                identity.fingerprint = fuzz_u64(rng);
            }
            Message::Hello { identity }
        }
    }
}

/// What a fuzzed session may become after reading `packet`, given what it was before: a
/// session stays as it is or ends with a violation of the sender, or, for a `Hello` of another
/// view of the match in a packet of this match, with a mismatch, which is how this build ends
/// on a peer that changes its identity. Quality messages end nothing: a reply that answers no
/// request is dropped, and a report moves at most the skips. An `End` of this match ends it
/// with the reason it tells, a desync naming the sender; and a hash report of the sender, at
/// its tick or one received earlier, with a desync against the sender, when its state hash of
/// that tick is known. Goodbye messages are not read yet.
fn assert_allowed(before: &SessionState, after: &SessionState, from: PeerId, packet: &Packet) {
    if after == before {
        return;
    }
    let ours = packet.match_id == 1;
    let other_hello = ours
        && packet.messages.iter().any(|message| match message {
            Message::Hello { identity: other } => *other != identity(&[0, 1]),
            _ => false,
        });
    let relayed = |reason: &EndReason| {
        ours && packet
            .messages
            .iter()
            .any(|message| match (message, reason) {
                (
                    Message::End {
                        reason: EndNotice::Desync { tick, .. },
                    },
                    EndReason::Desync {
                        tick: ended, peer, ..
                    },
                ) => tick == ended && *peer == from,
                (
                    Message::End {
                        reason: EndNotice::Lost { peer },
                    },
                    EndReason::Lost { peer: ended },
                ) => peer == ended,
                (
                    Message::End {
                        reason: EndNotice::LocalFailure { tick },
                    },
                    EndReason::LocalFailure { tick: ended },
                ) => tick == ended,
                _ => false,
            })
    };
    match (before, after) {
        (SessionState::Running, SessionState::Ended(reason)) if relayed(reason) => {}
        // A forged report waits for the local hash, so the desync may come from a hash report
        // an earlier packet carried: only its sender and a known pair of hashes are checked.
        (
            SessionState::Running,
            SessionState::Ended(EndReason::Desync {
                peer,
                local: Some(local),
                remote: Some(remote),
                ..
            }),
        ) if ours => {
            assert_eq!(*peer, from, "{packet:?}");
            assert_ne!(local, remote, "{packet:?}");
        }
        (SessionState::Running, SessionState::Ended(EndReason::Violation { peer, .. })) => {
            assert_eq!(*peer, from, "{packet:?}");
        }
        (SessionState::Running, SessionState::Ended(EndReason::Mismatch { peer, .. }))
            if other_hello =>
        {
            assert_eq!(*peer, from, "{packet:?}");
        }
        _ => panic!("{before:?} became {after:?} reading {packet:?}"),
    }
}

/// Reads `bytes` from `from` into `session`, checking how its state may change.
fn deliver(session: &mut Session, from: PeerId, packet: &Packet, now: u64) {
    let before = session.state();
    session.receive(from, &packet.encode(), now);
    assert_allowed(&before, &session.state(), from, packet);
    session.events();
}

/// One frame of `session` at `frame`: an input for its player, an advance executed by `model`,
/// which answers the hashes asked, the packet to the other peer.
fn honest_frame(
    session: &mut Session,
    model: &mut Model,
    player: u8,
    frame: u64,
    now: u64,
) -> Vec<Packet> {
    if session.state() == SessionState::Running {
        let input = vec![u8::try_from(frame % 3).unwrap()];
        session.add_local_input(player, input).unwrap();
        let requests = session.advance(now).unwrap();
        model.execute_for(session, &requests);
    } else {
        assert_eq!(session.advance(now), Err(SessionError::NotRunning));
    }
    session
        .outgoing(now)
        .into_iter()
        .map(|(_, bytes)| Packet::decode(&bytes).unwrap())
        .collect()
}

#[test]
fn fuzz_receive_never_panics() {
    let identity = identity(&[0, 1]);
    // How the fuzzed session ended, by seed, so the fuzz is shown to reach every rule.
    let mut outcomes = BTreeSet::new();
    let mut unknown_echoes = 0;
    for seed in 0..10_000 {
        let mut rng = Rng::new(seed, 0);
        let mut pair = handshake(sessions(2));
        let mut frames = [0u64; 2];
        let mut models = [Model::default(), Model::default()];
        // In sixteenths, how often a frame also brings a hostile packet: some seeds never do.
        let hostility = [0, 1, 4, 8][usize::try_from(rng.range_u32(0, 4)).unwrap()];
        // Some seeds have peer 1 never acknowledge, its packets otherwise valid.
        let never_acknowledges = rng.range_u32(0, 8) == 0;
        for round in 0..ROUNDS {
            let now = NOW + u64::from(round) * 16_667;
            // Valid traffic both ways, which the hostile packets interleave with.
            for (local, other) in [(0, 1), (1, 0)] {
                let player = u8::try_from(local).unwrap();
                let model = &mut models[local];
                for mut packet in honest_frame(&mut pair[local], model, player, frames[local], now)
                {
                    if local == 1 && never_acknowledges {
                        packet
                            .messages
                            .retain(|message| !matches!(message, Message::Ack { .. }));
                    }
                    deliver(&mut pair[other], PeerId(player), &packet, now);
                }
                frames[local] += 1;
            }
            if rng.range_u32(0, 16) < hostility {
                let messages = (0..rng.range_u32(1, 5))
                    .map(|_| fuzz_message(&mut rng, frames[0], &identity))
                    .collect();
                let match_id = if rng.range_u32(0, 16) == 0 { 2 } else { 1 };
                let packet = Packet { match_id, messages };
                deliver(&mut pair[0], PeerId(1), &packet, now);
            }
        }
        // Without hostile packets, an honest pair never disconnects. Only `pair[0]` is
        // checked: hostile packets forge inputs of peer 1 that `pair[0]` acknowledges, so
        // `pair[1]` can end with `AckNeverSent` on a hostile seed (seed 179, for one), which
        // is right, as `pair[0]` then acknowledges inputs peer 1 never sent.
        if hostility == 0 && !never_acknowledges {
            assert_eq!(pair[0].state(), SessionState::Running, "seed {seed}");
        }
        unknown_echoes += pair[0].stats().peers[&PeerId(1)]
            .dropped
            .get(&DropReason::UnknownEcho)
            .copied()
            .unwrap_or(0);
        outcomes.insert(match pair[0].state() {
            SessionState::Ended(EndReason::Violation { rule, .. }) => format!("{rule:?}"),
            SessionState::Ended(EndReason::Mismatch { .. }) => "Mismatch".to_string(),
            SessionState::Ended(EndReason::Desync {
                local: Some(local),
                remote: Some(remote),
                ..
            }) if local != remote => "Desync".to_string(),
            SessionState::Ended(EndReason::Desync { .. }) => "RelayedDesync".to_string(),
            SessionState::Ended(EndReason::Lost { .. }) => "Lost".to_string(),
            SessionState::Ended(EndReason::LocalFailure { .. }) => "LocalFailure".to_string(),
            state => format!("{state:?}"),
        });
    }
    let expected = [
        "AckLagging",
        "AckNeverSent",
        "BeyondHorizon",
        "ConflictingInput",
        "Desync",
        "HashBeyondHorizon",
        "InvalidInput",
        "LocalFailure",
        "Lost",
        "Mismatch",
        "NotOwner",
        "RelayedDesync",
        "Running",
    ];
    assert_eq!(outcomes, expected.map(String::from).into(), "{outcomes:?}");
    // Replies echoing a time in the future answer no request.
    assert!(unknown_echoes > 0);
}
