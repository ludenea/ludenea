mod support;

use ludenea_app::PlayerInputSpec;
use ludenea_net::{
    Advanced, DropReason, LinkConditions, MAX_INPUT_BYTES, MAX_PACKET_BYTES, MatchRules, Message,
    Packet, PeerId, Request, Session, SessionError, SessionEvent, SessionState,
};
use support::{
    Harness, handshake, identity, model_byte, offline_hashes, sessions, sessions_of, settings,
};

const ONE_SECOND: u64 = 1_000_000;
const NOW: u64 = 1_000;

/// Peer 0 of a running two-peer match at tick 0, with `rules`.
fn local_session(rules: MatchRules) -> Session {
    let mut identity = identity(&[0, 1]);
    identity.rules = rules;
    handshake(sessions_of(&identity)).remove(0)
}

/// Peer 0 of a running two-peer match with the default rules.
fn peer_zero() -> Session {
    local_session(MatchRules::default())
}

/// Hands `session` the inputs of player 1 from `first_tick`, as peer 1 sends them.
fn remote_inputs(session: &mut Session, first_tick: u64, inputs: &[u8]) {
    let packet = Packet {
        match_id: 1,
        messages: vec![Message::Inputs {
            player: 1,
            first_tick,
            inputs: inputs.iter().map(|byte| vec![*byte]).collect(),
        }],
    };
    session.receive(PeerId(1), &packet.encode(), NOW);
}

/// Hands `session` a packet of `messages` from peer 1.
fn from_peer_one(session: &mut Session, messages: Vec<Message>) {
    let packet = Packet {
        match_id: 1,
        messages,
    };
    session.receive(PeerId(1), &packet.encode(), NOW);
}

fn dropped(session: &Session, reason: DropReason) -> u64 {
    peer_one_stats(session)
        .dropped
        .get(&reason)
        .copied()
        .unwrap_or(0)
}

/// Adds `input` for player 0, then advances.
fn step(session: &mut Session, input: u8) -> Vec<Request> {
    session.add_local_input(0, vec![input]).unwrap();
    session.advance(NOW).unwrap()
}

fn save(tick: u64) -> Request {
    Request::Save { tick, hash: false }
}

fn advance(tick: u64, inputs: [u8; 2]) -> Request {
    Request::Advance {
        tick,
        inputs: inputs.iter().map(|byte| vec![*byte]).collect(),
    }
}

fn peer_one_stats(session: &Session) -> ludenea_net::PeerStats {
    session.stats().peers[&PeerId(1)].clone()
}

/// The messages of the packet `session` sends peer 1 now.
fn sent_to_peer_one(session: &mut Session) -> Vec<Message> {
    let outgoing = session.outgoing(NOW);
    let (_, packet) = outgoing
        .iter()
        .find(|(to, _)| *to == PeerId(1))
        .expect("a packet for peer 1");
    Packet::decode(packet).unwrap().messages
}

/// Every peer confirmed at least `ticks` ticks, all the same inputs from tick 0, and every
/// model holds, at every confirmed tick it ran past, the state those inputs give offline.
fn assert_agree(harness: &Harness, ticks: usize) {
    let reference = harness.confirmed(PeerId(0));
    for peer in harness.peers() {
        let confirmed = harness.confirmed(peer);
        assert!(
            confirmed.len() >= ticks,
            "{peer} confirmed {} ticks",
            confirmed.len()
        );
        for (index, (tick, _)) in confirmed.iter().enumerate() {
            assert_eq!(*tick, u64::try_from(index).unwrap(), "{peer} out of order");
        }
        let common = confirmed.len().min(reference.len());
        assert_eq!(confirmed[..common], reference[..common], "{peer}");
        let model = harness.model(peer);
        for (tick, hash) in (1..).zip(offline_hashes(&confirmed)) {
            if tick <= model.tick() {
                assert_eq!(model.hash_at(tick), Some(hash), "{peer} at tick {tick}");
            }
        }
    }
}

fn confirmed_at_least(harness: &Harness, ticks: usize) -> bool {
    harness
        .peers()
        .all(|peer| harness.confirmed(peer).len() >= ticks)
}

#[test]
fn two_peers_without_loss_never_roll_back() {
    let mut harness = Harness::new(sessions(2));
    assert!(harness.run_until(15 * ONE_SECOND, |h| confirmed_at_least(h, 600)));
    assert_agree(&harness, 600);
    for peer in harness.peers() {
        assert_eq!(harness.model(peer).loads(), 0, "{peer}");
        let stats = &harness.session(peer).stats().peers;
        for (other, stats) in stats {
            assert_eq!(stats.rollbacks, 0, "{peer} from {other}");
            assert_eq!(stats.stalls, 0, "{peer} from {other}");
        }
    }
    // The model hashes of both peers agree tick by tick.
    for tick in 1..=600 {
        assert_eq!(
            harness.model(PeerId(0)).hash_at(tick),
            harness.model(PeerId(1)).hash_at(tick),
            "tick {tick}"
        );
    }
}

#[test]
fn local_input_applies_after_the_input_delay() {
    for delay in [0, 2] {
        let mut session = local_session(MatchRules {
            input_delay: delay,
            ..MatchRules::default()
        });
        let mut advanced = Vec::new();
        for tick in 0..6 {
            let requests = step(&mut session, 10 + u8::try_from(tick).unwrap());
            assert_eq!(requests.len(), 2, "delay {delay}: {requests:?}");
            assert_eq!(requests[0], save(tick), "delay {delay}");
            advanced.push(requests[1].clone());
        }
        for (tick, request) in (0u64..).zip(advanced) {
            let expected = if tick < u64::from(delay) {
                0
            } else {
                10 + u8::try_from(tick - u64::from(delay)).unwrap()
            };
            // Player 1 is predicted with the default input: nothing came from peer 1.
            assert_eq!(request, advance(tick, [expected, 0]), "delay {delay}");
        }
    }
}

#[test]
fn default_inputs_of_the_delay_ticks_are_sent() {
    let mut session = peer_zero();
    step(&mut session, 7);
    let messages = sent_to_peer_one(&mut session);
    assert!(
        messages.contains(&Message::Inputs {
            player: 0,
            first_tick: 0,
            inputs: vec![vec![0], vec![0], vec![7]],
        }),
        "{messages:?}"
    );
    assert!(
        messages.contains(&Message::Ack {
            players: vec![(1, 0)],
            hashes_through: 0,
        }),
        "{messages:?}"
    );
}

#[test]
fn late_input_rolls_back_to_the_first_misprediction() {
    let mut session = peer_zero();
    remote_inputs(&mut session, 0, &[0; 10]);
    for _ in 0..13 {
        step(&mut session, 0);
    }
    remote_inputs(&mut session, 10, &[5]);
    let requests = step(&mut session, 0);
    assert_eq!(
        requests,
        [
            Request::Load { tick: 10 },
            save(10),
            advance(10, [0, 5]),
            save(11),
            advance(11, [0, 5]),
            save(12),
            advance(12, [0, 5]),
            save(13),
            advance(13, [0, 5]),
        ]
    );
    let stats = peer_one_stats(&session);
    assert_eq!(stats.rollbacks, 1);
    assert_eq!(stats.deepest_rollback, 3);
    assert_eq!(stats.prediction_misses, 1);
}

#[test]
fn correct_prediction_does_not_roll_back() {
    let mut session = peer_zero();
    // Tick 2 is known ahead: ticks 3 and after are predicted to repeat it.
    remote_inputs(&mut session, 0, &[0, 0, 5]);
    for _ in 0..8 {
        step(&mut session, 0);
    }
    remote_inputs(&mut session, 3, &[5, 5, 5, 5, 5]);
    assert_eq!(step(&mut session, 0), [save(8), advance(8, [0, 5])]);
    let stats = peer_one_stats(&session);
    assert_eq!(stats.rollbacks, 0);
    assert_eq!(stats.prediction_misses, 0);
}

#[test]
fn future_input_is_not_a_misprediction() {
    let mut session = peer_zero();
    for _ in 0..5 {
        step(&mut session, 0);
    }
    // Ticks 0 to 4 as predicted; 5 to 9 not simulated yet, and unlike the prediction.
    remote_inputs(&mut session, 0, &[0, 0, 0, 0, 0, 7, 7, 7, 7, 7]);
    assert_eq!(step(&mut session, 0), [save(5), advance(5, [0, 7])]);
    let stats = peer_one_stats(&session);
    assert_eq!(stats.rollbacks, 0);
    assert_eq!(stats.prediction_misses, 0);
}

#[test]
fn repredicted_tick_is_compared_with_its_new_prediction() {
    let mut session = peer_zero();
    remote_inputs(&mut session, 0, &[0; 10]);
    for _ in 0..13 {
        step(&mut session, 0);
    }
    remote_inputs(&mut session, 10, &[5]);
    step(&mut session, 0);
    // Ticks 11 to 13 now ran with 5. Tick 11 confirms it; tick 12 differs.
    remote_inputs(&mut session, 11, &[5, 6]);
    assert_eq!(
        step(&mut session, 0),
        [
            Request::Load { tick: 12 },
            save(12),
            advance(12, [0, 6]),
            save(13),
            advance(13, [0, 6]),
            save(14),
            advance(14, [0, 6]),
        ]
    );
    let stats = peer_one_stats(&session);
    assert_eq!(stats.rollbacks, 2);
    assert_eq!(stats.deepest_rollback, 3);
    assert_eq!(stats.prediction_misses, 2);
}

#[test]
fn last_advance_reports_ran_and_stalled() {
    let mut session = peer_zero();
    assert_eq!(session.last_advance(), None);
    let window = u64::from(MatchRules::default().rollback_window);
    for tick in 0..window {
        step(&mut session, 1);
        assert_eq!(session.last_advance(), Some(Advanced::Ran { tick }));
    }
    // The remote is silent: the window is full.
    session.advance(NOW).unwrap();
    assert_eq!(session.last_advance(), Some(Advanced::Stalled));
    // Another call at the window is another stall.
    assert_eq!(session.advance(NOW), Ok(vec![]));
    assert_eq!(session.last_advance(), Some(Advanced::Stalled));
    remote_inputs(&mut session, 0, &[0; 11]);
    step(&mut session, 1);
    assert_eq!(session.last_advance(), Some(Advanced::Ran { tick: window }));
}

#[test]
fn confirmed_and_next_ticks_follow_the_session() {
    let mut session = peer_zero();
    assert_eq!((session.confirmed_tick(), session.next_tick()), (0, 0));
    step(&mut session, 1);
    step(&mut session, 1);
    assert_eq!((session.confirmed_tick(), session.next_tick()), (0, 2));
    remote_inputs(&mut session, 0, &[0, 0, 0]);
    assert_eq!(session.confirmed_tick(), 3);
    step(&mut session, 1);
    assert_eq!((session.confirmed_tick(), session.next_tick()), (3, 3));
}

#[test]
fn stalls_at_the_window() {
    let mut session = peer_zero();
    let window = u64::from(MatchRules::default().rollback_window);
    for tick in 0..window {
        // Ticks 0 and 1, within the input delay, hold the default input.
        let local = if tick < 2 { 0 } else { 1 };
        assert_eq!(
            step(&mut session, 1),
            [save(tick), advance(tick, [local, 0])]
        );
    }
    // A stalled advance needs no input: it advances no tick.
    assert_eq!(session.advance(NOW), Ok(vec![]));
    assert_eq!(peer_one_stats(&session).stalls, 1);
    for stalls in 2..=4 {
        assert_eq!(step(&mut session, 1), []);
        assert_eq!(peer_one_stats(&session).stalls, stalls);
        // Still sending every local input from the unacknowledged tick 0 on.
        let messages = sent_to_peer_one(&mut session);
        let mut inputs = vec![vec![0], vec![0]];
        inputs.extend(vec![vec![1]; 8]);
        assert!(
            messages.contains(&Message::Inputs {
                player: 0,
                first_tick: 0,
                inputs,
            }),
            "{messages:?}"
        );
    }
    // During the stall, a new input replaces the pending one.
    session.add_local_input(0, vec![7]).unwrap();
    session.add_local_input(0, vec![9]).unwrap();
    remote_inputs(&mut session, 0, &[0; 11]);
    assert_eq!(session.advance(NOW).unwrap(), [save(8), advance(8, [1, 0])]);
    let messages = sent_to_peer_one(&mut session);
    let Some(Message::Inputs { inputs, .. }) = messages
        .iter()
        .find(|message| matches!(message, Message::Inputs { .. }))
    else {
        panic!("no inputs in {messages:?}");
    };
    assert_eq!(inputs.len(), 11);
    assert_eq!(inputs[10], [9]);
}

#[test]
fn advance_without_local_input_is_an_error() {
    let mut session = peer_zero();
    assert_eq!(
        session.advance(NOW),
        Err(SessionError::MissingLocalInput { player: 0 })
    );
    // The input is consumed by the tick it advanced.
    step(&mut session, 0);
    assert_eq!(
        session.advance(NOW),
        Err(SessionError::MissingLocalInput { player: 0 })
    );
}

#[test]
fn local_inputs_are_refused_with_the_reason() {
    let mut connecting = sessions(2).remove(0);
    assert_eq!(
        connecting.add_local_input(0, vec![0]),
        Err(SessionError::NotRunning)
    );
    assert_eq!(connecting.advance(NOW), Err(SessionError::NotRunning));
    let mut session = peer_zero();
    for player in [1, 2, u8::MAX] {
        assert_eq!(
            session.add_local_input(player, vec![0]),
            Err(SessionError::NotLocal { player })
        );
    }
    assert_eq!(
        session.add_local_input(0, vec![0; MAX_INPUT_BYTES + 1]),
        Err(SessionError::InputTooLarge {
            player: 0,
            len: MAX_INPUT_BYTES + 1
        })
    );
    assert!(matches!(
        session.add_local_input(0, vec![0, 0]),
        Err(SessionError::InvalidInput { player: 0, .. })
    ));
    // Nothing refused became the pending input.
    assert_eq!(
        session.advance(NOW),
        Err(SessionError::MissingLocalInput { player: 0 })
    );
}

#[test]
fn confirmed_events_follow_every_player_in_tick_order() {
    let mut session = peer_zero();
    for _ in 0..4 {
        step(&mut session, 3);
    }
    assert!(
        !session
            .events()
            .iter()
            .any(|event| matches!(event, SessionEvent::Confirmed { .. }))
    );
    remote_inputs(&mut session, 0, &[4, 4, 4]);
    step(&mut session, 3);
    let confirmed: Vec<SessionEvent> = session.events();
    assert_eq!(
        confirmed,
        [0, 1, 2].map(|tick| SessionEvent::Confirmed {
            tick,
            inputs: vec![vec![if tick < 2 { 0 } else { 3 }], vec![4]],
        })
    );
}

#[test]
fn redundant_inputs_survive_loss() {
    let mut harness = Harness::new(sessions(2));
    harness.drop_every_other_packet();
    assert!(harness.run_until(15 * ONE_SECOND, |h| confirmed_at_least(h, 300)));
    assert_agree(&harness, 300);
}

#[test]
fn two_local_players_and_one_remote() {
    let conditions = LinkConditions {
        latency_micros: 30_000,
        jitter_micros: 10_000,
        loss_per_mille: 200,
        duplicate_per_mille: 20,
        reorder_per_mille: 50,
        reorder_extra_micros: 20_000,
        seed: 11,
    };
    let mut harness = Harness::conditioned(sessions_of(&identity(&[0, 0, 1])), conditions);
    assert!(harness.run_until(20 * ONE_SECOND, |h| confirmed_at_least(h, 300)));
    assert_agree(&harness, 300);
    for (tick, inputs) in harness.confirmed(PeerId(1)) {
        assert_eq!(inputs.len(), 3, "tick {tick}");
    }
    // Both local players of peer 0 played: their inputs vary, so a lost one would show.
    let confirmed = harness.confirmed(PeerId(0));
    for player in 0..2 {
        let played = [model_byte(u8::try_from(player).unwrap(), 5)];
        assert!(
            confirmed.iter().any(|(_, inputs)| inputs[player] == played),
            "player {player}"
        );
    }
}

#[test]
fn save_precedes_every_load() {
    let mut loads = 0;
    for seed in 0..4 {
        let conditions = LinkConditions {
            latency_micros: 50_000,
            jitter_micros: 20_000,
            loss_per_mille: 100,
            duplicate_per_mille: 20,
            reorder_per_mille: 50,
            reorder_extra_micros: 30_000,
            seed,
        };
        let mut harness = Harness::conditioned(sessions(3), conditions);
        // The model panics on a Load that breaks the invariant.
        assert!(harness.run_until(20 * ONE_SECOND, |h| confirmed_at_least(h, 300)));
        assert_agree(&harness, 300);
        for peer in harness.peers() {
            loads += harness.model(peer).loads();
            assert_eq!(harness.state(peer), SessionState::Running);
        }
    }
    assert!(loads > 0, "no rollback happened: the test proves nothing");
}

#[test]
fn inputs_after_a_gap_are_dropped_and_counted() {
    // An input that breaks a rule of the protocol ends the session instead: see the
    // violation tests.
    let mut session = peer_zero();
    remote_inputs(&mut session, 0, &[3, 4]);
    for _ in 0..2 {
        // Tick 2 is missing; tick 3 waits for a packet that resends from tick 2.
        remote_inputs(&mut session, 3, &[0]);
        assert_eq!(
            session.events(),
            [SessionEvent::Dropped {
                peer: PeerId(1),
                reason: DropReason::Gap
            }]
        );
    }
    assert_eq!(dropped(&session, DropReason::Gap), 2);
    // The confirmed inputs are unchanged.
    step(&mut session, 0);
    let confirmed: Vec<SessionEvent> = session.events();
    assert_eq!(
        confirmed,
        [0, 1].map(|tick| SessionEvent::Confirmed {
            tick,
            inputs: vec![vec![0], vec![3 + u8::try_from(tick).unwrap()]],
        })
    );
    assert_eq!(session.state(), SessionState::Running);
}

#[test]
fn acknowledgments_only_move_forward() {
    let mut session = peer_zero();
    for _ in 0..4 {
        step(&mut session, 1);
    }
    // Ticks 0 to 5 are sent.
    session.outgoing(NOW);
    let ack = |next| Message::Ack {
        players: vec![(0, next)],
        hashes_through: 0,
    };
    from_peer_one(&mut session, vec![ack(5)]);
    // An older acknowledgment, arriving reordered, is ignored.
    from_peer_one(&mut session, vec![ack(3)]);
    assert_eq!(session.events(), []);
    step(&mut session, 1);
    let messages = sent_to_peer_one(&mut session);
    assert!(
        messages.contains(&Message::Inputs {
            player: 0,
            first_tick: 5,
            inputs: vec![vec![1], vec![1]],
        }),
        "{messages:?}"
    );
}

#[test]
fn inputs_of_a_running_peer_are_read_while_connecting() {
    // Peer 1 has peer 0's Hello but not peer 2's: it is still connecting, while peer 0 runs.
    let identity = identity(&[0, 1, 2]);
    let mut session = Session::new(settings(1, &identity), support::model_input()).unwrap();
    let hello = Message::Hello {
        identity: identity.clone(),
    };
    session.receive(
        PeerId(0),
        &Packet {
            match_id: 1,
            messages: vec![hello],
        }
        .encode(),
        NOW,
    );
    let inputs = Message::Inputs {
        player: 0,
        first_tick: 0,
        inputs: vec![vec![0], vec![0], vec![2]],
    };
    // Its inputs say it runs: it received this peer's Hello, even if its Welcome was lost.
    session.receive(
        PeerId(0),
        &Packet {
            match_id: 1,
            messages: vec![inputs],
        }
        .encode(),
        NOW,
    );
    assert_eq!(session.state(), SessionState::Connecting);
    assert_eq!(session.events(), []);
    let hello = Message::Hello {
        identity: identity.clone(),
    };
    let packet = Packet {
        match_id: 1,
        messages: vec![hello, Message::Welcome],
    };
    session.receive(PeerId(2), &packet.encode(), NOW);
    assert_eq!(session.state(), SessionState::Running);
    assert_eq!(session.events(), [SessionEvent::Started]);
    // The inputs read while connecting are held: tick 2 of player 0 is 2, not predicted.
    let ticks: Vec<Request> = (0..3)
        .flat_map(|_| {
            session.add_local_input(1, vec![0]).unwrap();
            session.advance(NOW).unwrap()
        })
        .collect();
    assert_eq!(
        ticks.last(),
        Some(&Request::Advance {
            tick: 2,
            inputs: vec![vec![2], vec![0], vec![0]],
        })
    );
}

#[test]
fn unacknowledged_inputs_fill_a_packet_oldest_first() {
    // Three local players with inputs at the largest size, and a peer whose acknowledgments
    // trail by as much as they can: the unacknowledged ticks outgrow one packet.
    let identity = identity(&[0, 0, 0, 1]);
    let any_input = PlayerInputSpec {
        default: vec![0],
        check: |_| Ok(()),
    };
    let sessions = (0..2)
        .map(|local| Session::new(settings(local, &identity), any_input.clone()).unwrap())
        .collect();
    let mut session = handshake(sessions).remove(0);
    let rules = MatchRules::default();
    let (delay, window) = (
        u64::from(rules.input_delay),
        u64::from(rules.rollback_window),
    );
    let lag = 2 * (window + delay) + 1;
    let mut acked = 0;
    for tick in 0..40u64 {
        for player in 0..3 {
            session
                .add_local_input(player, vec![u8::try_from(tick).unwrap(); MAX_INPUT_BYTES])
                .unwrap();
        }
        // The horizon of the session at `tick`: its highest local input.
        acked = (tick + delay - 1).saturating_sub(lag);
        let packet = Packet {
            match_id: 1,
            messages: vec![
                Message::Ack {
                    players: (0..3).map(|player| (player, acked)).collect(),
                    hashes_through: 0,
                },
                Message::Inputs {
                    player: 3,
                    first_tick: tick,
                    inputs: vec![vec![0]],
                },
            ],
        };
        session.receive(PeerId(1), &packet.encode(), NOW);
        assert_ne!(session.advance(NOW).unwrap(), []);
        if tick < 39 {
            session.outgoing(NOW);
        }
    }
    assert_eq!(session.state(), SessionState::Running);
    let outgoing = session.outgoing(NOW);
    let (_, bytes) = &outgoing[0];
    assert!(bytes.len() <= MAX_PACKET_BYTES, "{} bytes", bytes.len());
    let messages = Packet::decode(bytes).unwrap().messages;
    let sent: Vec<(u8, u64, usize)> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Inputs {
                player,
                first_tick,
                inputs,
            } => Some((*player, *first_tick, inputs.len())),
            _ => None,
        })
        .collect();
    // The packet is full: one more tick of any player would not fit.
    let mut grown = Packet {
        match_id: 1,
        messages: messages.clone(),
    };
    let Some(Message::Inputs { inputs, .. }) = grown
        .messages
        .iter_mut()
        .find(|message| matches!(message, Message::Inputs { .. }))
    else {
        panic!("no inputs in {messages:?}");
    };
    inputs.push(vec![0; MAX_INPUT_BYTES]);
    assert!(grown.encode().len() > MAX_PACKET_BYTES);
    // Every player from its oldest unacknowledged tick, sharing the room.
    assert_eq!(sent.len(), 3, "{sent:?}");
    for (player, first_tick, ticks) in &sent {
        assert_eq!(*first_tick, acked, "player {player}");
        assert!(*ticks >= 10, "player {player}: {sent:?}");
    }
}

#[test]
fn rollback_starts_at_the_earliest_misprediction_of_any_player() {
    let mut session = handshake(sessions(3)).remove(0);
    let send = |session: &mut Session, peer: u8, first_tick: u64, inputs: Vec<Vec<u8>>| {
        let packet = Packet {
            match_id: 1,
            messages: vec![Message::Inputs {
                player: peer,
                first_tick,
                inputs,
            }],
        };
        session.receive(PeerId(peer), &packet.encode(), NOW);
    };
    send(&mut session, 1, 0, vec![vec![0]; 9]);
    send(&mut session, 2, 0, vec![vec![0]; 11]);
    for _ in 0..13 {
        step(&mut session, 0);
    }
    // Player 2 mispredicted at tick 11, arriving first; player 1 at tick 9.
    send(&mut session, 2, 11, vec![vec![6]]);
    send(&mut session, 1, 9, vec![vec![5]]);
    let three = |tick, one: u8, two: u8| Request::Advance {
        tick,
        inputs: vec![vec![0], vec![one], vec![two]],
    };
    assert_eq!(
        step(&mut session, 0),
        [
            Request::Load { tick: 9 },
            save(9),
            three(9, 5, 0),
            save(10),
            three(10, 5, 0),
            save(11),
            three(11, 5, 6),
            save(12),
            three(12, 5, 6),
            save(13),
            three(13, 5, 6),
        ]
    );
    let stats = &session.stats().peers;
    assert_eq!(stats[&PeerId(1)].deepest_rollback, 4);
    assert_eq!(stats[&PeerId(2)].deepest_rollback, 2);
}
