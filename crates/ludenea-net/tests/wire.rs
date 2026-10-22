use ludenea_app::Rng;
use ludenea_net::{
    EndNotice, MAX_ENGINE_BYTES, MAX_INPUT_BYTES, MAX_MESSAGES_PER_PACKET, MAX_PACKET_BYTES,
    MAX_PLAYERS, MAX_TICKS_PER_MESSAGE, MatchIdentity, MatchRules, Message, NET_PROTOCOL_VERSION,
    Packet, PeerId, SessionSettings, SettingsError, Timeouts, WireError,
};

fn identity(owners: &[u8]) -> MatchIdentity {
    MatchIdentity {
        engine: "ludenea-test".to_string(),
        fingerprint: 7,
        scene_and_assets: 9,
        tick_rate: 60,
        players: owners.iter().copied().map(PeerId).collect(),
        rules: MatchRules::default(),
    }
}

fn settings(owners: &[u8]) -> SessionSettings {
    SessionSettings {
        local: PeerId(0),
        match_id: 1,
        identity: identity(owners),
        timeouts: Timeouts::default(),
    }
}

fn packet(messages: Vec<Message>) -> Packet {
    Packet {
        match_id: u64::MAX,
        messages,
    }
}

fn every_message() -> Vec<Message> {
    vec![
        Message::Hello {
            identity: identity(&[0, 1, 1]),
        },
        Message::Welcome,
        Message::Inputs {
            player: 3,
            first_tick: u64::MAX - 1,
            inputs: vec![vec![], vec![0xFF; MAX_INPUT_BYTES]],
        },
        Message::Ack {
            players: vec![(0, 5), (2, u64::MAX)],
            hashes_through: 90,
        },
        Message::Quality {
            tick: 12,
            sent_micros: u64::MAX,
            advantage: i32::MIN,
        },
        Message::QualityReply {
            echo_micros: 1_000_000,
            tick: 13,
        },
        Message::Hash {
            tick: 30,
            hash: u64::MAX,
        },
        Message::Goodbye { last_tick: 600 },
        Message::End {
            reason: EndNotice::Desync {
                tick: 90,
                peer: PeerId(2),
            },
        },
        Message::End {
            reason: EndNotice::Lost { peer: PeerId(3) },
        },
        Message::End {
            reason: EndNotice::LocalFailure { tick: u64::MAX },
        },
    ]
}

fn refusal(bytes: &[u8]) -> WireError {
    Packet::decode(bytes).expect_err("the packet must be refused")
}

#[test]
fn every_message_round_trips() {
    for message in every_message() {
        let sent = packet(vec![message]);
        let bytes = sent.encode();
        assert_eq!(&bytes[..2], &NET_PROTOCOL_VERSION.to_le_bytes());
        assert_eq!(Packet::decode(&bytes), Ok(sent));
    }
    let all = packet(every_message());
    assert_eq!(Packet::decode(&all.encode()), Ok(all));
}

#[test]
fn version_1_bytes_are_pinned() {
    // A reordered variant or field, or another encoding, changes these bytes: that is a new
    // version of the format.
    let cases: Vec<(Message, Vec<u8>)> = vec![
        (
            Message::Hello {
                identity: MatchIdentity {
                    engine: "ab".to_string(),
                    fingerprint: 300,
                    scene_and_assets: 2,
                    tick_rate: 60,
                    players: vec![PeerId(0), PeerId(1)],
                    rules: MatchRules::default(),
                },
            },
            vec![0, 2, b'a', b'b', 0xAC, 0x02, 2, 60, 2, 0, 1, 2, 8, 30],
        ),
        (Message::Welcome, vec![1]),
        (
            Message::Inputs {
                player: 3,
                first_tick: 300,
                inputs: vec![vec![7], vec![]],
            },
            vec![2, 3, 0xAC, 0x02, 2, 1, 7, 0],
        ),
        (
            Message::Ack {
                players: vec![(1, 300)],
                hashes_through: 4,
            },
            vec![3, 1, 1, 0xAC, 0x02, 4],
        ),
        (
            Message::Quality {
                tick: 5,
                sent_micros: 300,
                advantage: -2,
            },
            vec![4, 5, 0xAC, 0x02, 3],
        ),
        (
            Message::QualityReply {
                echo_micros: 300,
                tick: 6,
            },
            vec![5, 0xAC, 0x02, 6],
        ),
        (Message::Hash { tick: 7, hash: 300 }, vec![6, 7, 0xAC, 0x02]),
        (Message::Goodbye { last_tick: 300 }, vec![7, 0xAC, 0x02]),
        (
            Message::End {
                reason: EndNotice::Desync {
                    tick: 300,
                    peer: PeerId(2),
                },
            },
            vec![8, 0, 0xAC, 0x02, 2],
        ),
        (
            Message::End {
                reason: EndNotice::Lost { peer: PeerId(3) },
            },
            vec![8, 1, 3],
        ),
        (
            Message::End {
                reason: EndNotice::LocalFailure { tick: 300 },
            },
            vec![8, 2, 0xAC, 0x02],
        ),
    ];
    for (message, body) in cases {
        let sent = Packet {
            match_id: 300,
            messages: vec![message],
        };
        // The version, the match identifier, one message.
        let mut expected = vec![1, 0, 0xAC, 0x02, 1];
        expected.extend(&body);
        assert_eq!(sent.encode(), expected, "{sent:?}");
        assert_eq!(Packet::decode(&expected), Ok(sent));
    }
}

#[test]
fn decode_refuses_a_packet_too_large() {
    let mut bytes = vec![0; MAX_PACKET_BYTES + 1];
    bytes[..2].copy_from_slice(&NET_PROTOCOL_VERSION.to_le_bytes());
    assert_eq!(refusal(&bytes), WireError::TooLarge { len: 1201 });
    assert!(refusal(&bytes).to_string().contains("1201"));
}

#[test]
fn an_oversized_packet_of_another_version_names_its_version() {
    let mut bytes = vec![0; MAX_PACKET_BYTES + 1];
    bytes[..2].copy_from_slice(&7u16.to_le_bytes());
    assert_eq!(
        refusal(&bytes),
        WireError::Version {
            found: 7,
            expected: 1
        }
    );
}

#[test]
fn decode_refuses_another_version() {
    let mut bytes = packet(vec![Message::Welcome]).encode();
    bytes[..2].copy_from_slice(&7u16.to_le_bytes());
    let error = refusal(&bytes);
    assert_eq!(
        error,
        WireError::Version {
            found: 7,
            expected: 1
        }
    );
    assert_eq!(error.to_string(), "network protocol version 7, expected 1");
}

#[test]
fn version_of_reads_any_version() {
    let mut bytes = packet(vec![Message::Welcome]).encode();
    assert_eq!(Packet::version_of(&bytes), Some(NET_PROTOCOL_VERSION));
    bytes[..2].copy_from_slice(&7u16.to_le_bytes());
    assert_eq!(Packet::version_of(&bytes), Some(7));
    // The prefix alone names the version, whatever follows.
    assert_eq!(Packet::version_of(&[7, 0]), Some(7));
    assert_eq!(Packet::version_of(&[0xFF, 0xFF, 1, 2, 3]), Some(u16::MAX));
    assert_eq!(Packet::version_of(&[7]), None);
    assert_eq!(Packet::version_of(&[]), None);
}

#[test]
fn decode_refuses_malformed_bytes() {
    let mut bytes = packet(vec![Message::Welcome]).encode();
    bytes.push(0);
    let error = refusal(&bytes);
    assert!(
        matches!(&error, WireError::Malformed(reason) if reason.contains("trailing")),
        "{error}"
    );
    for short in [&[][..], &[1][..]] {
        assert!(matches!(refusal(short), WireError::Malformed(_)));
    }
    // The version and nothing else, and a truncated message.
    assert!(matches!(refusal(&[1, 0]), WireError::Malformed(_)));
    let whole = packet(vec![Message::Goodbye { last_tick: 600 }]).encode();
    assert!(matches!(
        refusal(&whole[..whole.len() - 1]),
        WireError::Malformed(_)
    ));
    // 9, the first message tag version 1 does not define.
    let mut unknown = packet(vec![Message::Welcome]).encode();
    *unknown.last_mut().unwrap() = 9;
    assert!(matches!(refusal(&unknown), WireError::Malformed(_)));
}

#[test]
fn decode_refuses_too_many_messages() {
    let full = packet(vec![Message::Welcome; MAX_MESSAGES_PER_PACKET]).encode();
    Packet::decode(&full).expect("exactly the limit");
    let bytes = packet(vec![Message::Welcome; MAX_MESSAGES_PER_PACKET + 1]).encode();
    assert_eq!(refusal(&bytes), WireError::TooManyMessages { count: 17 });
}

fn inputs(ticks: usize, bytes: usize) -> Vec<u8> {
    packet(vec![Message::Inputs {
        player: 2,
        first_tick: 1,
        inputs: vec![vec![1; bytes]; ticks],
    }])
    .encode()
}

#[test]
fn decode_refuses_too_many_ticks() {
    let limit = MAX_TICKS_PER_MESSAGE as usize;
    Packet::decode(&inputs(limit, 0)).expect("exactly the limit");
    assert_eq!(
        refusal(&inputs(limit + 1, 0)),
        WireError::TooManyTicks {
            player: 2,
            count: 33
        }
    );
    assert_eq!(
        refusal(&inputs(limit + 1, 0)).to_string(),
        "input message of player 2 holds 33 ticks, at most 32 allowed"
    );
}

#[test]
fn decode_refuses_inputs_without_ticks() {
    let error = refusal(&inputs(0, 0));
    assert_eq!(error, WireError::NoTicks { player: 2 });
    assert_eq!(error.to_string(), "input message of player 2 holds no tick");
}

#[test]
fn decode_refuses_an_input_too_large() {
    Packet::decode(&inputs(1, MAX_INPUT_BYTES)).expect("exactly the limit");
    assert_eq!(
        refusal(&inputs(1, MAX_INPUT_BYTES + 1)),
        WireError::InputTooLarge { player: 2, len: 25 }
    );
    assert_eq!(
        refusal(&inputs(1, MAX_INPUT_BYTES + 1)).to_string(),
        "input of player 2 takes 25 bytes, at most 24 allowed"
    );
}

#[test]
fn decode_refuses_too_many_players() {
    let ack = packet(vec![Message::Ack {
        players: vec![(0, 1); MAX_PLAYERS + 1],
        hashes_through: 0,
    }]);
    let error = refusal(&ack.encode());
    assert_eq!(
        error,
        WireError::TooManyPlayers {
            message: "Ack",
            count: 5
        }
    );
    assert_eq!(
        error.to_string(),
        "Ack message lists 5 players, at most 4 allowed"
    );
    let hello = packet(vec![Message::Hello {
        identity: identity(&[0, 1, 0, 1, 0]),
    }]);
    assert_eq!(
        refusal(&hello.encode()),
        WireError::TooManyPlayers {
            message: "Hello",
            count: 5
        }
    );
}

#[test]
fn decode_refuses_an_engine_too_long() {
    let hello = |engine_bytes: usize| {
        let mut identity = identity(&[0, 1]);
        identity.engine = "e".repeat(engine_bytes);
        packet(vec![Message::Hello { identity }]).encode()
    };
    Packet::decode(&hello(MAX_ENGINE_BYTES)).expect("exactly the limit");
    let error = refusal(&hello(MAX_ENGINE_BYTES + 1));
    assert_eq!(error, WireError::EngineTooLong { len: 65 });
    assert!(error.to_string().contains("65") && error.to_string().contains("64"));
}

#[test]
fn the_largest_hello_fits() {
    let hello = packet(vec![Message::Hello {
        identity: MatchIdentity {
            engine: "\u{10FFFF}".repeat(MAX_ENGINE_BYTES / 4),
            fingerprint: u64::MAX,
            scene_and_assets: u64::MAX,
            tick_rate: u32::MAX,
            players: vec![PeerId(u8::MAX); MAX_PLAYERS],
            rules: MatchRules {
                input_delay: u32::MAX,
                rollback_window: u32::MAX,
                hash_interval: u32::MAX,
            },
        },
    }]);
    assert!(hello.encode().len() <= MAX_PACKET_BYTES);
}

#[test]
fn worst_case_packet_fits() {
    for owners in [
        &[0, 1][..],
        &[0, 0, 1],
        &[0, 0, 0, 1],
        &[0, 1, 2, 3],
        &[1, 0, 0, 0],
    ] {
        settings(owners)
            .validate()
            .unwrap_or_else(|error| panic!("{owners:?}: {error}"));
    }
}

#[test]
fn validate_refuses_a_layout_whose_worst_packet_does_not_fit() {
    // Three local players: each tick of the window adds three inputs of 25 encoded bytes, on
    // top of 979 bytes at the default window of 8 (11 ticks).
    let mut s = settings(&[0, 0, 0, 1]);
    s.identity.rules.rollback_window = 10;
    s.validate().expect("13 ticks take 1129 bytes");
    for (window, expected) in [(11, 1204), (24, 2179)] {
        s.identity.rules.rollback_window = window;
        let error = s
            .validate()
            .expect_err("the worst packet exceeds 1200 bytes");
        let SettingsError::PacketTooLarge { worst, max } = error else {
            panic!("unexpected refusal: {error}");
        };
        assert_eq!((worst, max), (expected, MAX_PACKET_BYTES));
        let text = error.to_string();
        assert!(
            text.contains(&expected.to_string()) && text.contains("1200"),
            "{text}"
        );
    }
}

#[test]
fn a_peer_alone_sends_no_packet() {
    // Four local players over the default window exceed one packet, but no packet is sent.
    settings(&[0, 0, 0, 0])
        .validate()
        .expect("a single peer has nobody to send to");
}

#[test]
fn validate_refuses_an_engine_too_long() {
    let mut s = settings(&[0, 1]);
    s.identity.engine = "e".repeat(MAX_ENGINE_BYTES);
    s.validate().expect("exactly the limit");
    s.identity.engine.push('e');
    let error = s.validate().expect_err("one byte over");
    assert_eq!(error, SettingsError::EngineTooLong { len: 65 });
    assert!(error.to_string().contains("65") && error.to_string().contains("64"));
}

// ---- Fuzzing ----

fn below(rng: &mut Rng, high: usize) -> usize {
    rng.range_u32(0, u32::try_from(high).unwrap()) as usize
}

fn u64_of(rng: &mut Rng) -> u64 {
    match rng.range_u32(0, 4) {
        0 => u64::from(rng.range_u32(0, 300)),
        1 => u64::MAX - u64::from(rng.range_u32(0, 3)),
        _ => (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32()),
    }
}

fn bytes_of(rng: &mut Rng, len: usize) -> Vec<u8> {
    (0..len).map(|_| rng.next_u32() as u8).collect()
}

/// A length from `low` to `limit`, or one past the limit one time in eight.
fn length(rng: &mut Rng, low: usize, limit: usize) -> usize {
    if rng.range_u32(0, 8) == 0 {
        limit + 1
    } else {
        low + below(rng, limit + 1 - low)
    }
}

fn random_message(rng: &mut Rng, kind: u32) -> Message {
    match kind {
        0 => {
            let players = length(rng, 1, MAX_PLAYERS);
            let owners: Vec<u8> = (0..players).map(|_| rng.range_u32(0, 4) as u8).collect();
            let mut identity = identity(&owners);
            identity.engine = "x".repeat(length(rng, 0, MAX_ENGINE_BYTES));
            identity.fingerprint = u64_of(rng);
            Message::Hello { identity }
        }
        1 => Message::Welcome,
        2 => {
            let ticks = match rng.range_u32(0, 8) {
                0 => 0,
                1 => MAX_TICKS_PER_MESSAGE as usize + 1,
                _ => 1 + below(rng, 6),
            };
            let inputs = (0..ticks)
                .map(|_| {
                    let len = if rng.range_u32(0, 16) == 0 {
                        MAX_INPUT_BYTES + 1
                    } else {
                        below(rng, MAX_INPUT_BYTES + 1)
                    };
                    bytes_of(rng, len)
                })
                .collect();
            Message::Inputs {
                player: rng.next_u32() as u8,
                first_tick: u64_of(rng),
                inputs,
            }
        }
        3 => Message::Ack {
            players: (0..length(rng, 0, MAX_PLAYERS))
                .map(|_| (rng.next_u32() as u8, u64_of(rng)))
                .collect(),
            hashes_through: u64_of(rng),
        },
        4 => Message::Quality {
            tick: u64_of(rng),
            sent_micros: u64_of(rng),
            advantage: rng.next_u32() as i32,
        },
        5 => Message::QualityReply {
            echo_micros: u64_of(rng),
            tick: u64_of(rng),
        },
        6 => Message::Hash {
            tick: u64_of(rng),
            hash: u64_of(rng),
        },
        7 => Message::Goodbye {
            last_tick: u64_of(rng),
        },
        _ => Message::End {
            reason: match rng.range_u32(0, 3) {
                0 => EndNotice::Desync {
                    tick: u64_of(rng),
                    peer: PeerId(rng.next_u32() as u8),
                },
                1 => EndNotice::Lost {
                    peer: PeerId(rng.next_u32() as u8),
                },
                _ => EndNotice::LocalFailure { tick: u64_of(rng) },
            },
        },
    }
}

/// The postcard varint of `value`.
fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

/// A packet, within the limits of the format or one past one of them, mutated in one of the
/// ways a broken or hostile peer could.
fn mutated_packet(rng: &mut Rng, seed: u64) -> Vec<u8> {
    let count = length(rng, 1, MAX_MESSAGES_PER_PACKET);
    let messages = (0..count)
        .map(|index| {
            // The first message cycles through every kind, so each seed range covers them all.
            let kind = if index == 0 {
                (seed % 9) as u32
            } else {
                rng.range_u32(0, 9)
            };
            random_message(rng, kind)
        })
        .collect();
    let sent = Packet {
        match_id: u64_of(rng),
        messages,
    };
    let mut bytes = sent.encode();
    // Before the mutation, the packet decodes as itself or breaks a limit, never the format.
    match Packet::decode(&bytes) {
        Ok(decoded) => assert_eq!(decoded, sent, "seed {seed}"),
        Err(error @ (WireError::Malformed(_) | WireError::Version { .. })) => {
            panic!("seed {seed}: an encoded packet is refused: {error}")
        }
        Err(_) => {}
    }
    match rng.range_u32(0, 4) {
        0 => {
            for _ in 0..1 + rng.range_u32(0, 4) {
                let at = below(rng, bytes.len());
                bytes[at] ^= 1 << rng.range_u32(0, 8);
            }
        }
        1 => {
            let len = below(rng, bytes.len());
            bytes.truncate(len);
        }
        2 => {
            // A length field, at the message count or at a random place, claims the most.
            let header = 2 + varint(sent.match_id).len();
            let at = if rng.range_u32(0, 2) == 0 {
                header
            } else {
                2 + below(rng, bytes.len() - 2)
            };
            let huge = if rng.range_u32(0, 2) == 0 {
                u64::from(u32::MAX)
            } else {
                u64::MAX
            };
            bytes.splice(at..=at, varint(huge));
        }
        _ => {
            let extra = 1 + below(rng, 16);
            bytes.extend(bytes_of(rng, extra));
        }
    }
    bytes
}

#[test]
fn fuzz_decode_never_panics() {
    for seed in 0..100_000u64 {
        let mut rng = Rng::new(seed, 0);
        let bytes = if seed % 2 == 0 {
            mutated_packet(&mut rng, seed / 2)
        } else {
            let len = below(&mut rng, 1301);
            let mut bytes = bytes_of(&mut rng, len);
            // Half of them carry this version's prefix, so the decoder reads past it.
            if seed % 4 == 1 && len >= 2 {
                bytes[..2].copy_from_slice(&NET_PROTOCOL_VERSION.to_le_bytes());
            }
            bytes
        };
        if let Ok(decoded) = Packet::decode(&bytes) {
            assert!(
                decoded.encode().len() <= MAX_PACKET_BYTES,
                "seed {seed}: a decoded packet encodes beyond the limit"
            );
        }
    }
}
