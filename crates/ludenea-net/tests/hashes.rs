mod support;

use ludenea_app::PlayerInputSpec;
use ludenea_net::{
    DropReason, END_LINGER_MICROS, EndNotice, EndReason, LinkConditions, MAX_MESSAGES_PER_PACKET,
    MAX_PACKET_BYTES, MatchIdentity, MatchRules, Message, Packet, PeerId, Request, Session,
    SessionError, SessionEvent, SessionState, ViolationRule,
};
use support::{
    FRAME_MICROS, Harness, Model, handshake, identity, offline_hashes, sessions, settings,
};

const ONE_SECOND: u64 = 1_000_000;
const NOW: u64 = 1_000;

/// The default `hash_interval`.
const INTERVAL: u64 = 30;

/// The ticks of the `HashMatched` events of `peer`, in order.
fn matched(harness: &Harness, peer: PeerId) -> Vec<u64> {
    harness
        .events(peer)
        .iter()
        .filter_map(|event| match event {
            SessionEvent::HashMatched { tick } => Some(*tick),
            _ => None,
        })
        .collect()
}

/// Whether every peer emitted `HashMatched` for every reported tick up to `tick`.
fn all_matched_through(harness: &Harness, tick: u64) -> bool {
    let expected: Vec<u64> = (1..=tick / INTERVAL).map(|n| n * INTERVAL).collect();
    harness
        .peers()
        .all(|peer| matched(harness, peer).starts_with(&expected))
}

/// Every hash report in `messages`, as `(tick, hash)`.
fn hash_reports(messages: &[Message]) -> Vec<(u64, u64)> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Hash { tick, hash } => Some((*tick, *hash)),
            _ => None,
        })
        .collect()
}

fn dropped(session: &Session, reason: DropReason) -> u64 {
    session.stats().peers[&PeerId(1)]
        .dropped
        .get(&reason)
        .copied()
        .unwrap_or(0)
}

fn violation(tick: u64, rule: ViolationRule) -> SessionState {
    SessionState::Ended(EndReason::Violation {
        peer: PeerId(1),
        player: None,
        tick: Some(tick),
        rule,
    })
}

/// Peer 0 of a running two-peer match, driven frame by frame, with the packets of peer 1
/// written by hand.
struct Driven {
    session: Session,
    model: Model,
    /// The ticks peer 1 confirmed for player 1 so far.
    remote_ticks: u64,
}

impl Driven {
    fn new() -> Self {
        Self {
            session: handshake(sessions(2)).remove(0),
            model: Model::default(),
            remote_ticks: 0,
        }
    }

    /// A session that ran `ticks` ticks with every input of both players `0`, peer 1's input
    /// of each tick arriving before the tick runs. Its model answers every hash asked.
    fn at(ticks: u64) -> Self {
        let mut driven = Self::new();
        for _ in 0..ticks {
            driven.remote(&[0]);
            driven.frame(0);
        }
        driven
    }

    /// Hands the session a packet of `messages` from peer 1.
    fn receive(&mut self, messages: Vec<Message>) {
        let packet = Packet {
            match_id: 1,
            messages,
        };
        self.session.receive(PeerId(1), &packet.encode(), NOW);
    }

    /// Peer 1's next inputs, and an acknowledgment of player 0 up to the current tick, which
    /// peer 0 sent already.
    fn remote(&mut self, bytes: &[u8]) {
        let first_tick = self.remote_ticks;
        self.remote_ticks += u64::try_from(bytes.len()).unwrap();
        let messages = vec![
            Message::Inputs {
                player: 1,
                first_tick,
                inputs: bytes.iter().map(|byte| vec![*byte]).collect(),
            },
            Message::Ack {
                players: vec![(0, self.model.tick())],
                hashes_through: 0,
            },
        ];
        self.receive(messages);
    }

    /// Adds `input` for player 0 and advances; the model executes the requests and answers
    /// the hashes asked. Returns the messages then sent to peer 1.
    fn frame(&mut self, input: u8) -> Vec<Message> {
        let requests = self.advance(input);
        self.model.execute_for(&mut self.session, &requests);
        self.sent()
    }

    /// Adds `input` for player 0 and advances, leaving the requests to the caller.
    fn advance(&mut self, input: u8) -> Vec<Request> {
        self.session.add_local_input(0, vec![input]).unwrap();
        self.session.advance(NOW).unwrap()
    }

    /// The messages the session sends peer 1 now, none when it sends nothing.
    fn sent(&mut self) -> Vec<Message> {
        sent_at(&mut self.session, NOW)
    }
}

/// The messages `session` sends peer 1 at `now`, none when it sends nothing.
fn sent_at(session: &mut Session, now: u64) -> Vec<Message> {
    session
        .outgoing(now)
        .into_iter()
        .filter(|(to, _)| *to == PeerId(1))
        .flat_map(|(_, bytes)| Packet::decode(&bytes).unwrap().messages)
        .collect()
}

#[test]
fn saves_of_reported_ticks_ask_for_the_hash() {
    let mut driven = Driven::new();
    let mut saves = Vec::new();
    for _ in 0..=2 * INTERVAL {
        driven.remote(&[0]);
        let requests = driven.advance(0);
        driven.model.execute_for(&mut driven.session, &requests);
        driven.sent();
        saves.extend(requests.into_iter().filter_map(|request| match request {
            Request::Save { tick, hash } => Some((tick, hash)),
            _ => None,
        }));
    }
    let expected: Vec<(u64, bool)> = (0..=2 * INTERVAL)
        .map(|tick| (tick, tick > 0 && tick % INTERVAL == 0))
        .collect();
    assert_eq!(saves, expected);
}

#[test]
fn hashes_agree_without_desync() {
    for peers in 2..=4 {
        let mut harness = Harness::new(sessions(peers));
        assert!(
            harness.run_until(10 * ONE_SECOND, |h| all_matched_through(h, 5 * INTERVAL)),
            "{peers} peers"
        );
        for peer in harness.peers() {
            assert_eq!(harness.state(peer), SessionState::Running, "{peer}");
            let matched = matched(&harness, peer);
            assert!(
                matched.windows(2).all(|pair| pair[1] == pair[0] + INTERVAL),
                "{peer}: {matched:?}"
            );
        }
    }
}

#[test]
fn hash_is_sent_under_latency_without_a_rollback() {
    let conditions = LinkConditions {
        latency_micros: 100_000,
        jitter_micros: 0,
        loss_per_mille: 0,
        duplicate_per_mille: 0,
        reorder_per_mille: 0,
        reorder_extra_micros: 0,
        seed: 3,
    };
    let mut harness = Harness::conditioned(sessions(2), conditions);
    harness.default_inputs();
    assert!(harness.run_until(10 * ONE_SECOND, |h| all_matched_through(h, INTERVAL)));
    for peer in harness.peers() {
        // Every prediction was right: nothing ran again, so the state of tick 30 was saved
        // once, from predicted inputs, and its hash is the one of the confirmed inputs.
        assert_eq!(harness.model(peer).loads(), 0, "{peer}");
        let offline = offline_hashes(&harness.confirmed(peer));
        let index = usize::try_from(INTERVAL - 1).unwrap();
        assert_eq!(
            harness.model(peer).hash_at(INTERVAL),
            Some(offline[index]),
            "{peer}"
        );
    }
}

#[test]
fn hash_report_uses_the_state_resimulated_after_a_correction() {
    let mut driven = Driven::new();
    for _ in 0..24 {
        driven.remote(&[0]);
        driven.frame(0);
    }
    // Peer 1's inputs from tick 24 are late: ticks 24 to 31 run on the prediction, `0`, and
    // the state of tick 30 is saved from it and its hash reported.
    for _ in 24..32 {
        let sent = driven.frame(0);
        assert_eq!(hash_reports(&sent), [], "nothing below 30 is confirmed");
    }
    let predicted = driven.model.hash_at(INTERVAL).unwrap();
    // Peer 1 actually played `1`: every input below 30 is confirmed now, but the state of 30
    // was built from a wrong prediction.
    driven.remote(&[1; 8]);
    assert_eq!(hash_reports(&driven.sent()), [], "before the rollback runs");
    let requests = driven.advance(0);
    assert!(
        requests.contains(&Request::Load { tick: 24 }),
        "{requests:?}"
    );
    assert!(
        requests.contains(&Request::Save {
            tick: INTERVAL,
            hash: true
        }),
        "{requests:?}"
    );
    let reports = driven.model.execute(&requests);
    assert_eq!(
        hash_reports(&driven.sent()),
        [],
        "before the hash is reported"
    );
    for (tick, hash) in reports {
        driven.session.report_hash(tick, hash).unwrap();
    }
    let resimulated = driven.model.hash_at(INTERVAL).unwrap();
    assert_ne!(resimulated, predicted);
    let confirmed: Vec<(u64, Vec<Vec<u8>>)> = driven
        .session
        .events()
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::Confirmed { tick, inputs } => Some((tick, inputs)),
            _ => None,
        })
        .collect();
    let offline = offline_hashes(&confirmed);
    assert_eq!(offline[usize::try_from(INTERVAL - 1).unwrap()], resimulated);
    // The report rides on the next packet.
    driven.remote(&[0]);
    assert_eq!(hash_reports(&driven.frame(0)), [(INTERVAL, resimulated)]);
}

#[test]
fn hash_reports_survive_loss() {
    for peers in [2, 3] {
        let conditions = LinkConditions {
            latency_micros: 20_000,
            jitter_micros: 10_000,
            loss_per_mille: 300,
            duplicate_per_mille: 0,
            reorder_per_mille: 0,
            reorder_extra_micros: 0,
            seed: 17,
        };
        let mut harness = Harness::conditioned(sessions(peers), conditions);
        assert!(
            harness.run_until(30 * ONE_SECOND, |h| all_matched_through(h, 5 * INTERVAL)),
            "{peers} peers"
        );
        for peer in harness.peers() {
            assert_eq!(harness.state(peer), SessionState::Running, "{peer}");
        }
    }
}

#[test]
fn diverging_hash_ends_every_peer_with_desync() {
    let tick = 3 * INTERVAL;
    for peers in [2, 3] {
        let mut harness = Harness::new(sessions(peers));
        // Peer 0's game is broken at tick 90, and no peer ever receives its report of it: the
        // others learn the desync only from its end notice.
        harness.flip_hash(PeerId(0), tick);
        harness.lose_hash_reports(PeerId(0), tick);
        let ended = |h: &Harness| {
            h.peers()
                .all(|peer| matches!(h.state(peer), SessionState::Ended(_)))
        };
        assert!(harness.run_until(10 * ONE_SECOND, ended), "{peers} peers");
        let flipped = harness.model(PeerId(0)).hash_at(tick).unwrap() ^ 1;
        let honest = harness.model(PeerId(1)).hash_at(tick).unwrap();
        // Peer 0 found it against the first honest report to arrive, and every other peer
        // ends with peer 0's view of it, both hashes included.
        let found = harness.state(PeerId(0));
        match &found {
            SessionState::Ended(EndReason::Desync {
                tick: at,
                peer,
                local,
                remote,
            }) => {
                assert_eq!(*at, tick);
                assert_ne!(*peer, PeerId(0));
                assert_eq!((*local, *remote), (flipped, honest));
            }
            state => panic!("peer 0 of {peers}: {state:?}"),
        }
        for peer in harness.peers().skip(1) {
            assert_eq!(harness.state(peer), found, "{peer} of {peers}");
        }
    }
}

#[test]
fn reported_state_is_kept_until_every_peer_matches() {
    let conditions = LinkConditions {
        latency_micros: 50_000,
        jitter_micros: 20_000,
        loss_per_mille: 100,
        duplicate_per_mille: 0,
        reorder_per_mille: 0,
        reorder_extra_micros: 0,
        seed: 5,
    };
    let mut harness = Harness::conditioned(sessions(3), conditions);
    let last = 6 * INTERVAL;
    while !all_matched_through(&harness, last) {
        assert!(
            harness.now_micros() < 30 * ONE_SECOND,
            "no match through {last}"
        );
        harness.step();
        for peer in harness.peers() {
            let matched = matched(&harness, peer);
            let latest = matched.last().copied().unwrap_or(0);
            // A reported state is released only once a later report matched on every peer,
            // in tick order, at the next advance; the latest matching one stays kept.
            let below_latest: Vec<u64> = (1..latest / INTERVAL).map(|n| n * INTERVAL).collect();
            let released = harness.model(peer).released();
            assert!(below_latest.starts_with(released), "{peer}: {released:?}");
            if latest > 0 {
                assert!(harness.model(peer).kept().contains(&latest), "{peer}");
            }
        }
    }
    for peer in harness.peers() {
        let released = harness.model(peer).released().len();
        assert!(released >= 4, "{peer}: {released} released");
    }
}

#[test]
fn hash_beyond_horizon_is_a_violation() {
    // At tick 27, the highest tick a local player has an input for is 28: no honest peer has
    // confirmed tick 30.
    let mut driven = Driven::at(INTERVAL - 3);
    driven.receive(vec![Message::Hash {
        tick: INTERVAL,
        hash: 1,
    }]);
    assert_eq!(
        driven.session.state(),
        violation(INTERVAL, ViolationRule::HashBeyondHorizon)
    );
    // At tick 28 it has, at the horizon plus one; tick 60 is beyond it.
    let mut driven = Driven::at(INTERVAL - 2);
    let hash = Message::Hash {
        tick: INTERVAL,
        hash: 1,
    };
    driven.receive(vec![hash]);
    assert_eq!(driven.session.state(), SessionState::Running);
    driven.receive(vec![Message::Hash {
        tick: u64::MAX,
        hash: 1,
    }]);
    assert_eq!(
        driven.session.state(),
        violation(u64::MAX, ViolationRule::HashBeyondHorizon)
    );
}

#[test]
fn hashes_off_the_interval_or_received_again_are_dropped_alone() {
    let mut driven = Driven::at(2 * INTERVAL);
    let honest = driven.model.hash_at(INTERVAL).unwrap();
    let report = Message::Hash {
        tick: INTERVAL,
        hash: honest,
    };
    driven.receive(vec![
        Message::Hash { tick: 0, hash: 1 },
        Message::Hash {
            tick: INTERVAL + 1,
            hash: 1,
        },
        report.clone(),
        report,
        Message::Inputs {
            player: 1,
            first_tick: 2 * INTERVAL,
            inputs: vec![vec![0]],
        },
    ]);
    assert_eq!(driven.session.state(), SessionState::Running);
    assert_eq!(dropped(&driven.session, DropReason::HashOffInterval), 2);
    assert_eq!(dropped(&driven.session, DropReason::HashAlreadyReceived), 1);
    let events = driven.session.events();
    assert!(
        events.contains(&SessionEvent::HashMatched { tick: INTERVAL }),
        "{events:?}"
    );
    // The inputs after the dropped reports were read.
    driven.frame(0);
    assert!(driven.session.events().contains(&SessionEvent::Confirmed {
        tick: 2 * INTERVAL,
        inputs: vec![vec![0], vec![0]],
    }));
}

#[test]
fn received_hashes_wait_in_bounded_storage() {
    // The model never answers: no local hash is known, and peer 1's reports wait for one.
    let mut driven = Driven::new();
    let mut unanswered = Vec::new();
    for _ in 0..3 * INTERVAL - 2 {
        driven.remote(&[0]);
        let requests = driven.advance(0);
        unanswered.extend(driven.model.execute(&requests));
        driven.sent();
    }
    let report = |driven: &Driven, n: u64| Message::Hash {
        tick: n * INTERVAL,
        hash: driven.model.hash_at(n * INTERVAL).unwrap(),
    };
    // Tick 90 has not run yet, but it is at the horizon plus one: a peer may have confirmed
    // it. `(rollback_window + input_delay) / hash_interval + 2` = 2 reports wait at most.
    let ninety = Message::Hash {
        tick: 3 * INTERVAL,
        hash: 9,
    };
    let reports = vec![report(&driven, 1), report(&driven, 2), ninety.clone()];
    driven.receive(reports);
    assert_eq!(driven.session.state(), SessionState::Running);
    assert_eq!(dropped(&driven.session, DropReason::HashBacklog), 1);
    // The report left out is not acknowledged, so peer 1 sends it again.
    driven.remote(&[0]);
    let acked = driven
        .frame(0)
        .into_iter()
        .find_map(|message| match message {
            Message::Ack { hashes_through, .. } => Some(hashes_through),
            _ => None,
        });
    assert_eq!(acked, Some(2 * INTERVAL));
    for (tick, hash) in unanswered {
        driven.session.report_hash(tick, hash).unwrap();
    }
    driven.receive(vec![ninety]);
    assert_eq!(driven.session.state(), SessionState::Running);
    assert_eq!(dropped(&driven.session, DropReason::HashBacklog), 1);
    let events = driven.session.events();
    for n in 1..=2 {
        assert!(
            events.contains(&SessionEvent::HashMatched { tick: n * INTERVAL }),
            "{n}: {events:?}"
        );
    }
}

#[test]
fn report_hash_refuses_a_hash_never_asked() {
    let mut driven = Driven::at(INTERVAL + 1);
    assert_eq!(
        driven.session.report_hash(INTERVAL, 1),
        Err(SessionError::HashNotAsked { tick: INTERVAL })
    );
    assert_eq!(
        driven.session.report_hash(7, 1),
        Err(SessionError::HashNotAsked { tick: 7 })
    );
    assert_eq!(driven.session.state(), SessionState::Running);
}

#[test]
fn acknowledged_hash_reports_are_no_longer_sent() {
    let mut driven = Driven::at(INTERVAL + 2);
    let hash = driven.model.hash_at(INTERVAL).unwrap();
    // Resent in every packet until acknowledged.
    for _ in 0..2 {
        driven.remote(&[0]);
        assert_eq!(hash_reports(&driven.frame(0)), [(INTERVAL, hash)]);
    }
    let ack = |hashes_through| Message::Ack {
        players: vec![],
        hashes_through,
    };
    driven.receive(vec![ack(INTERVAL)]);
    driven.remote(&[0]);
    assert_eq!(hash_reports(&driven.frame(0)), []);
    // Only the ticks of reports sent can be acknowledged.
    for never_sent in [INTERVAL + 1, 2 * INTERVAL] {
        let mut driven = Driven::at(INTERVAL + 2);
        driven.remote(&[0]);
        driven.frame(0);
        driven.receive(vec![ack(never_sent)]);
        assert_eq!(
            driven.session.state(),
            violation(never_sent, ViolationRule::AckNeverSent)
        );
    }
}

#[test]
fn unacknowledged_hash_reports_are_packed_within_the_packet_limits() {
    // Three local players with the largest inputs, a report every tick, and a peer that
    // never acknowledges a report: they pile up beyond what one packet holds.
    let mut identity: MatchIdentity = identity(&[0, 0, 0, 1]);
    identity.rules = MatchRules {
        hash_interval: 1,
        ..MatchRules::default()
    };
    let any_input = PlayerInputSpec {
        default: vec![0],
        check: |_| Ok(()),
    };
    let mut pair: Vec<Session> = (0..2)
        .map(|local| Session::new(settings(local, &identity), any_input.clone()).unwrap())
        .collect();
    pair = handshake(pair);
    let mut session = pair.remove(0);
    let large = vec![0xff; ludenea_net::MAX_INPUT_BYTES];
    let mut first_unacked = 1;
    for tick in 0..60u64 {
        let messages = vec![
            Message::Inputs {
                player: 3,
                first_tick: tick,
                inputs: vec![vec![0]],
            },
            Message::Ack {
                players: (0..3).map(|player| (player, tick)).collect(),
                hashes_through: if tick == 40 { first_unacked - 1 } else { 0 },
            },
        ];
        let packet = Packet {
            match_id: 1,
            messages,
        };
        session.receive(PeerId(1), &packet.encode(), NOW);
        assert_eq!(session.state(), SessionState::Running, "tick {tick}");
        for player in 0..3 {
            session.add_local_input(player, large.clone()).unwrap();
        }
        for request in session.advance(NOW).unwrap() {
            if let Request::Save { tick, hash: true } = request {
                session.report_hash(tick, tick.wrapping_mul(7)).unwrap();
            }
        }
        for (_, bytes) in session.outgoing(NOW) {
            assert!(bytes.len() <= MAX_PACKET_BYTES, "{} bytes", bytes.len());
            let messages = Packet::decode(&bytes).unwrap().messages;
            assert!(messages.len() <= MAX_MESSAGES_PER_PACKET, "{messages:?}");
            let ticks: Vec<u64> = hash_reports(&messages).iter().map(|(t, _)| *t).collect();
            // The oldest unacknowledged reports, in order.
            if let Some(first) = ticks.first() {
                assert_eq!(*first, first_unacked, "tick {tick}");
                let count = u64::try_from(ticks.len()).unwrap();
                let expected: Vec<u64> = (*first..*first + count).collect();
                assert_eq!(ticks, expected);
                if tick == 39 {
                    first_unacked = ticks.last().unwrap() + 1;
                }
            }
            if tick == 59 {
                assert!(ticks.len() > 1 && ticks.len() < 50, "{ticks:?}");
            }
        }
    }
    assert!(first_unacked > 1);
}

#[test]
fn desync_notice_lingers_then_stops() {
    let mut driven = Driven::at(INTERVAL + 2);
    let local = driven.model.hash_at(INTERVAL).unwrap();
    driven.receive(vec![Message::Hash {
        tick: INTERVAL,
        hash: !local,
    }]);
    assert_eq!(
        driven.session.state(),
        SessionState::Ended(EndReason::Desync {
            tick: INTERVAL,
            peer: PeerId(1),
            local,
            remote: !local,
        })
    );
    let notice = vec![Message::End {
        reason: EndNotice::Desync {
            tick: INTERVAL,
            peer: PeerId(1),
            local,
            remote: !local,
        },
    }];
    // At most once a tick, for END_LINGER_MICROS, with nothing else.
    assert_eq!(sent_at(&mut driven.session, NOW), notice);
    assert_eq!(sent_at(&mut driven.session, NOW + FRAME_MICROS - 1), []);
    assert_eq!(sent_at(&mut driven.session, NOW + FRAME_MICROS), notice);
    let last = NOW + END_LINGER_MICROS - 1;
    assert_eq!(sent_at(&mut driven.session, last), notice);
    assert_eq!(sent_at(&mut driven.session, last + FRAME_MICROS), []);
}

#[test]
fn end_notice_ends_the_receiver_with_the_same_reason() {
    let cases = [
        (
            EndNotice::Lost { peer: PeerId(1) },
            EndReason::Lost { peer: PeerId(1) },
        ),
        (
            EndNotice::LocalFailure { tick: 5 },
            EndReason::LocalFailure { tick: 5 },
        ),
    ];
    for (notice, reason) in cases {
        let mut driven = Driven::at(INTERVAL + 2);
        driven.receive(vec![Message::End { reason: notice }]);
        assert_eq!(driven.session.state(), SessionState::Ended(reason));
        // A relayed ending is not relayed again.
        assert_eq!(driven.sent(), []);
    }
    // A desync ends the receiver with the sender's view of it: the peer whose hash differed
    // from the sender's, here this one, the sender's hash and that peer's.
    let mut driven = Driven::at(INTERVAL + 2);
    driven.receive(vec![Message::End {
        reason: EndNotice::Desync {
            tick: INTERVAL,
            peer: PeerId(0),
            local: 7,
            remote: 8,
        },
    }]);
    assert_eq!(
        driven.session.state(),
        SessionState::Ended(EndReason::Desync {
            tick: INTERVAL,
            peer: PeerId(0),
            local: 7,
            remote: 8,
        })
    );
    assert_eq!(driven.sent(), []);
}

#[test]
fn identity_change_while_running_is_not_relayed() {
    // No end notice tells of a mismatch: the peer that changed its identity is the one at
    // fault, and the other peers find it themselves or stop hearing from this one.
    let mut driven = Driven::at(3);
    let mut other = identity(&[0, 1]);
    other.fingerprint ^= 1;
    driven.receive(vec![Message::Hello { identity: other }]);
    assert!(matches!(
        driven.session.state(),
        SessionState::Ended(EndReason::Mismatch {
            peer: PeerId(1),
            field: "fingerprint",
            ..
        })
    ));
    assert_eq!(driven.sent(), []);
    assert_eq!(
        sent_at(&mut driven.session, NOW + END_LINGER_MICROS / 2),
        []
    );
}
