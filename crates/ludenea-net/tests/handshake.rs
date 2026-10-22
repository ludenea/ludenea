mod support;

use ludenea_app::{PlayerInputSpec, SimError};
use ludenea_net::{
    DropReason, EndReason, HANDSHAKE_RESEND_MICROS, LinkConditions, MAX_INPUT_BYTES, Message,
    NET_PROTOCOL_VERSION, Packet, PeerId, Session, SessionEvent, SessionState, SettingsError,
};
use support::{Harness, identity, model_input, sessions, settings};

const ONE_SECOND: u64 = 1_000_000;

fn started(harness: &Harness, peer: PeerId) -> usize {
    harness
        .events(peer)
        .iter()
        .filter(|event| **event == SessionEvent::Started)
        .count()
}

fn drops(harness: &Harness, peer: PeerId) -> Vec<SessionEvent> {
    harness
        .events(peer)
        .iter()
        .filter(|event| matches!(event, SessionEvent::Dropped { .. }))
        .cloned()
        .collect()
}

fn dropped(session: &Session, peer: PeerId, reason: DropReason) -> u64 {
    session
        .stats()
        .peers
        .get(&peer)
        .and_then(|stats| stats.dropped.get(&reason))
        .copied()
        .unwrap_or(0)
}

fn bytes(match_id: u64, messages: Vec<Message>) -> Vec<u8> {
    Packet { match_id, messages }.encode()
}

/// A running two-peer match over a lossless network.
fn running_pair() -> Harness {
    let mut harness = Harness::new(sessions(2));
    assert!(harness.run_until_running(ONE_SECOND));
    harness
}

#[test]
fn handshake_starts_two_three_and_four_peers() {
    for peers in 2..=4 {
        let mut harness = Harness::new(sessions(peers));
        assert!(
            harness.run_until_running(ONE_SECOND),
            "{peers} peers did not start"
        );
        for peer in harness.peers() {
            assert_eq!(started(&harness, peer), 1, "{peer} of {peers}");
            assert_eq!(drops(&harness, peer), [], "{peer} of {peers}");
        }
        // Running peers stay running and never start twice.
        harness.run_until(ONE_SECOND, |_| false);
        for peer in harness.peers() {
            assert_eq!(harness.state(peer), SessionState::Running);
            assert_eq!(started(&harness, peer), 1);
        }
    }
}

#[test]
fn handshake_survives_loss() {
    for seed in 0..8 {
        let conditions = LinkConditions {
            latency_micros: 20_000,
            jitter_micros: 10_000,
            loss_per_mille: 300,
            duplicate_per_mille: 50,
            reorder_per_mille: 50,
            reorder_extra_micros: 30_000,
            seed,
        };
        let mut harness = Harness::conditioned(sessions(4), conditions);
        assert!(
            harness.run_until_running(3 * ONE_SECOND),
            "seed {seed} did not start"
        );
        harness.run_until(ONE_SECOND, |_| false);
        for peer in harness.peers() {
            assert_eq!(harness.state(peer), SessionState::Running, "seed {seed}");
            assert_eq!(started(&harness, peer), 1, "seed {seed}");
        }
    }
}

#[test]
fn hello_is_resent_at_most_every_resend_interval() {
    let mut session = sessions(2).remove(0);
    let mut hellos = Vec::new();
    for now in (0..=3 * HANDSHAKE_RESEND_MICROS).step_by(10_000) {
        for (to, packet) in session.outgoing(now) {
            assert_eq!(to, PeerId(1));
            let packet = Packet::decode(&packet).unwrap();
            assert!(matches!(packet.messages[..], [Message::Hello { .. }]));
            hellos.push(now);
        }
    }
    assert_eq!(
        hellos,
        [
            0,
            HANDSHAKE_RESEND_MICROS,
            2 * HANDSHAKE_RESEND_MICROS,
            3 * HANDSHAKE_RESEND_MICROS
        ]
    );
}

#[test]
fn hello_after_start_is_welcomed_again() {
    let mut harness = running_pair();
    let now = harness.now_micros();
    let hello = Message::Hello {
        identity: identity(&[0, 1]),
    };
    let session = harness.session(PeerId(0));
    session.receive(PeerId(1), &bytes(1, vec![hello]), now);
    let outgoing = session.outgoing(now);
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].0, PeerId(1));
    assert_eq!(
        Packet::decode(&outgoing[0].1).unwrap().messages,
        [Message::Welcome]
    );
    // Answered once per received Hello.
    assert_eq!(session.outgoing(now), []);
    assert_eq!(session.state(), SessionState::Running);
}

/// Two peers whose settings differ by `change`, run until both end.
fn mismatch(change: impl Fn(&mut ludenea_net::SessionSettings)) -> [SessionState; 2] {
    let identity = identity(&[0, 1]);
    let first = settings(0, &identity);
    let mut second = settings(1, &identity);
    change(&mut second);
    let mut harness = Harness::new(vec![
        Session::new(first, model_input()).unwrap(),
        Session::new(second, model_input()).unwrap(),
    ]);
    let ended = harness.run_until(ONE_SECOND, |harness| {
        harness
            .peers()
            .all(|peer| matches!(harness.state(peer), SessionState::Ended(_)))
    });
    assert!(ended, "both peers must end");
    for peer in harness.peers() {
        assert_eq!(started(&harness, peer), 0);
    }
    [harness.state(PeerId(0)), harness.state(PeerId(1))]
}

fn ended_with(peer: u8, field: &'static str, local: &str, remote: &str) -> SessionState {
    SessionState::Ended(EndReason::Mismatch {
        peer: PeerId(peer),
        field,
        local: local.to_string(),
        remote: remote.to_string(),
    })
}

#[test]
fn handshake_refuses_mismatch() {
    type Change = fn(&mut ludenea_net::SessionSettings);
    let cases: [(&str, Change, &str, &str); 9] = [
        (
            "engine",
            |s| s.identity.engine = "ludenea-other".to_string(),
            "ludenea-test",
            "ludenea-other",
        ),
        (
            "fingerprint",
            |s| s.identity.fingerprint = 0xabc,
            "0000000000000007",
            "0000000000000abc",
        ),
        (
            "scene_and_assets",
            |s| s.identity.scene_and_assets = 0xff,
            "0000000000000009",
            "00000000000000ff",
        ),
        ("tick_rate", |s| s.identity.tick_rate = 30, "60", "30"),
        (
            "players",
            |s| s.identity.players = vec![PeerId(1), PeerId(0)],
            "peer 0, peer 1",
            "peer 1, peer 0",
        ),
        (
            "input_delay",
            |s| s.identity.rules.input_delay = 3,
            "2",
            "3",
        ),
        (
            "rollback_window",
            |s| s.identity.rules.rollback_window = 7,
            "8",
            "7",
        ),
        (
            "hash_interval",
            |s| s.identity.rules.hash_interval = 60,
            "30",
            "60",
        ),
        (
            "match_id",
            |s| s.match_id = 2,
            "0000000000000001",
            "0000000000000002",
        ),
    ];
    for (field, change, first, second) in cases {
        let [zero, one] = mismatch(change);
        assert_eq!(zero, ended_with(1, field, first, second), "{field}");
        assert_eq!(one, ended_with(0, field, second, first), "{field}");
    }
}

#[test]
fn first_differing_field_is_named() {
    let [zero, _] = mismatch(|s| {
        s.identity.tick_rate = 30;
        s.identity.rules.hash_interval = 60;
        s.match_id = 2;
    });
    assert_eq!(zero, ended_with(1, "tick_rate", "60", "30"));
}

/// A packet of `version`, otherwise a well-formed one.
fn of_version(version: u16) -> Vec<u8> {
    let mut packet = bytes(1, vec![Message::Welcome]);
    packet[..2].copy_from_slice(&version.to_le_bytes());
    packet
}

#[test]
fn other_version_names_net_version() {
    let mut session = sessions(2).remove(0);
    session.receive(PeerId(1), &of_version(2), 0);
    assert_eq!(
        session.state(),
        ended_with(1, "net_version", &NET_PROTOCOL_VERSION.to_string(), "2")
    );

    // Once running, the packet is dropped and counted, and the match goes on.
    let mut harness = running_pair();
    let now = harness.now_micros();
    let session = harness.session(PeerId(0));
    session.receive(PeerId(1), &of_version(2), now);
    assert_eq!(session.state(), SessionState::Running);
    let reason = DropReason::Wire("Version");
    assert_eq!(dropped(session, PeerId(1), reason), 1);
    assert_eq!(
        session.events(),
        [SessionEvent::Dropped {
            peer: PeerId(1),
            reason
        }]
    );
}

/// Steps until just before `at`, asserting `peers` are still connecting, then once more.
fn step_to(harness: &mut Harness, peers: &[PeerId], at: u64) {
    harness.run_until(at - harness.now_micros(), |_| false);
    assert_eq!(harness.now_micros(), at);
    for &peer in peers {
        assert_eq!(
            harness.state(peer),
            SessionState::Connecting,
            "{peer} before {at}"
        );
    }
    harness.step();
}

#[test]
fn unreachable_peer_ends_the_handshake() {
    // Peer 2 never answers. Peer 1 comes up one second late, so its clock and its silence
    // of peer 2 start then, and peers 0 and 1 hear each other from then on: each limit below
    // is a second away from any other silence.
    let mut harness = Harness::new(sessions(3));
    harness.mute(PeerId(1));
    harness.mute(PeerId(2));
    harness.run_until(ONE_SECOND, |_| false);
    harness.unmute(PeerId(1));
    let timeout = ludenea_net::Timeouts::default().disconnect_after_micros;
    let unreachable = SessionState::Ended(EndReason::Unreachable { peer: PeerId(2) });
    step_to(&mut harness, &[PeerId(0)], timeout);
    assert_eq!(harness.state(PeerId(0)), unreachable);
    step_to(&mut harness, &[PeerId(1)], ONE_SECOND + timeout);
    assert_eq!(harness.state(PeerId(1)), unreachable);
    // An ended session stays ended and sends nothing more.
    harness.run_until(ONE_SECOND, |_| false);
    let now = harness.now_micros();
    assert_eq!(harness.session(PeerId(0)).outgoing(now), []);
    assert_eq!(harness.state(PeerId(0)), unreachable);
}

#[test]
fn only_the_peer_at_fault_is_unreachable() {
    // Peers 0 and 1 finish their handshake at once and have nothing more to tell each other;
    // peer 2 sends a single Hello at 2.9 s, then goes dark. Only peer 2 is at fault, from
    // its last packet.
    let mut harness = Harness::new(sessions(3));
    harness.mute(PeerId(2));
    harness.run_until(2_900_000, |_| false);
    let now = harness.now_micros();
    let hello = bytes(
        1,
        vec![Message::Hello {
            identity: identity(&[0, 1, 2]),
        }],
    );
    for peer in [PeerId(0), PeerId(1)] {
        harness.session(peer).receive(PeerId(2), &hello, now);
    }
    let timeout = ludenea_net::Timeouts::default().disconnect_after_micros;
    step_to(&mut harness, &[PeerId(0), PeerId(1)], now + timeout);
    for peer in [PeerId(0), PeerId(1)] {
        assert_eq!(
            harness.state(peer),
            SessionState::Ended(EndReason::Unreachable { peer: PeerId(2) }),
            "{peer}"
        );
    }
}

#[test]
fn time_never_runs_backwards() {
    // A lower time is read as the last one: the silence of peer 1 still counts from 0.
    let mut session = sessions(2).remove(0);
    session.outgoing(0);
    session.outgoing(2_000_000);
    session.outgoing(1_000);
    session.receive(PeerId(1), &[], 5);
    session.outgoing(2_999_999);
    assert_eq!(session.state(), SessionState::Connecting);
    session.outgoing(3_000_000);
    assert_eq!(
        session.state(),
        SessionState::Ended(EndReason::Unreachable { peer: PeerId(1) })
    );
}

#[test]
fn wrong_match_id_after_start_is_dropped_and_counted() {
    let mut harness = running_pair();
    let now = harness.now_micros();
    let hello = Message::Hello {
        identity: identity(&[0, 1]),
    };
    let session = harness.session(PeerId(0));
    session.receive(PeerId(1), &bytes(2, vec![hello]), now);
    session.receive(PeerId(1), &bytes(2, vec![Message::Welcome]), now);
    assert_eq!(session.state(), SessionState::Running);
    assert_eq!(dropped(session, PeerId(1), DropReason::WrongMatch), 2);
    let drop = SessionEvent::Dropped {
        peer: PeerId(1),
        reason: DropReason::WrongMatch,
    };
    assert_eq!(session.events(), [drop.clone(), drop]);
    // Nothing is answered for a packet of another match.
    assert_eq!(session.outgoing(now), []);
}

#[test]
fn wrong_match_id_without_hello_is_dropped_while_connecting() {
    let mut session = sessions(2).remove(0);
    session.receive(PeerId(1), &bytes(2, vec![Message::Welcome]), 0);
    assert_eq!(session.state(), SessionState::Connecting);
    assert_eq!(dropped(&session, PeerId(1), DropReason::WrongMatch), 1);
}

#[test]
fn inputs_before_running_are_dropped() {
    let mut session = sessions(2).remove(0);
    let inputs = Message::Inputs {
        player: 1,
        first_tick: 0,
        inputs: vec![vec![0]],
    };
    session.receive(PeerId(1), &bytes(1, vec![inputs, Message::Welcome]), 0);
    assert_eq!(session.state(), SessionState::Connecting);
    assert_eq!(dropped(&session, PeerId(1), DropReason::BeforeRunning), 1);
    assert_eq!(
        session.events(),
        [SessionEvent::Dropped {
            peer: PeerId(1),
            reason: DropReason::BeforeRunning
        }]
    );
    // The Welcome of the same packet was read: the matching Hello is all that is missing.
    let hello = Message::Hello {
        identity: identity(&[0, 1]),
    };
    session.receive(PeerId(1), &bytes(1, vec![hello]), 0);
    assert_eq!(session.state(), SessionState::Running);
    assert_eq!(session.events(), [SessionEvent::Started]);
}

#[test]
fn packets_from_unknown_peers_and_broken_packets_are_dropped() {
    let mut session = sessions(2).remove(0);
    for unknown in [PeerId(0), PeerId(2), PeerId(u8::MAX)] {
        session.receive(unknown, &of_version(2), 0);
        assert_eq!(dropped(&session, unknown, DropReason::UnknownPeer), 1);
    }
    session.receive(PeerId(1), &[1], 0);
    session.receive(PeerId(1), &[1, 0, 0xff], 0);
    let mut oversized = vec![0; ludenea_net::MAX_PACKET_BYTES + 1];
    oversized[..2].copy_from_slice(&NET_PROTOCOL_VERSION.to_le_bytes());
    session.receive(PeerId(1), &oversized, 0);
    assert_eq!(session.state(), SessionState::Connecting);
    assert_eq!(
        dropped(&session, PeerId(1), DropReason::Wire("Malformed")),
        2
    );
    assert_eq!(
        dropped(&session, PeerId(1), DropReason::Wire("TooLarge")),
        1
    );
    assert_eq!(session.events().len(), 6);
}

#[test]
fn session_refuses_an_invalid_default_input() {
    let identity = identity(&[0, 1]);
    let too_large = PlayerInputSpec {
        default: vec![0; MAX_INPUT_BYTES + 1],
        check: |_| Ok(()),
    };
    assert_eq!(
        Session::new(settings(0, &identity), too_large).err(),
        Some(SettingsError::DefaultInputTooLarge {
            len: MAX_INPUT_BYTES + 1
        })
    );
    let refused = PlayerInputSpec {
        default: vec![0, 0],
        ..model_input()
    };
    assert_eq!(
        Session::new(settings(0, &identity), refused).err(),
        Some(SettingsError::DefaultInputRefused {
            reason: SimError::InputDecode("a model input takes 1 byte, got 2".to_string())
                .to_string()
        })
    );
    let mut invalid = settings(0, &identity);
    invalid.identity.tick_rate = 0;
    assert_eq!(
        Session::new(invalid, model_input()).err(),
        Some(SettingsError::TickRate)
    );
}
