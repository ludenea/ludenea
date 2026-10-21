use std::collections::BTreeSet;

use ludenea_app::players::{
    Claim, ClaimError, KeyboardPart, LiveDevices, Lobby, LobbyEvent, LobbyPress, MAX_LOCAL_PLAYERS,
    automatic_claims, check_claims, split,
};
use ludenea_app::{App, RawGamepad, RawInput};
use ludenea_ecs::schema::Schema;
use serde::{Deserialize, Serialize};

fn keys(codes: &[&str]) -> BTreeSet<String> {
    codes.iter().map(|code| code.to_string()).collect()
}

fn pad(first: f32) -> RawGamepad {
    RawGamepad {
        buttons: vec![first],
        axes: Vec::new(),
    }
}

#[test]
fn left_half_holds_wasd_and_space() {
    for code in [
        "KeyW",
        "KeyA",
        "KeyS",
        "KeyD",
        "Space",
        "ShiftLeft",
        "Digit1",
    ] {
        assert_eq!(KeyboardPart::of(code), KeyboardPart::Left, "{code}");
        assert!(KeyboardPart::Left.contains(code), "{code}");
        assert!(!KeyboardPart::Right.contains(code), "{code}");
    }
}

#[test]
fn right_half_holds_arrows_and_enter() {
    for code in [
        "ArrowUp",
        "ArrowLeft",
        "Enter",
        "KeyP",
        "Digit6",
        "Numpad1",
        "ShiftRight",
    ] {
        assert_eq!(KeyboardPart::of(code), KeyboardPart::Right, "{code}");
        assert!(KeyboardPart::Right.contains(code), "{code}");
        assert!(!KeyboardPart::Left.contains(code), "{code}");
    }
}

#[test]
fn whole_keyboard_gets_every_key() {
    assert!(KeyboardPart::Whole.contains("KeyW"));
    assert!(KeyboardPart::Whole.contains("ArrowUp"));
    let devices = LiveDevices {
        keys: keys(&["KeyW", "ArrowUp", "Space"]),
        gamepads: Vec::new(),
    };
    let claims = [Claim {
        keyboard: Some(KeyboardPart::Whole),
        gamepad: None,
    }];
    let inputs = split(&devices, &claims);
    assert_eq!(inputs[0].as_ref().unwrap().keys, devices.keys);
}

#[test]
fn automatic_claims_for_one_two_and_four_players() {
    assert_eq!(MAX_LOCAL_PLAYERS, 4);
    let claim = |keyboard, gamepad| Claim { keyboard, gamepad };
    assert_eq!(
        automatic_claims(1),
        vec![claim(Some(KeyboardPart::Whole), Some(0))]
    );
    assert_eq!(
        automatic_claims(2),
        vec![
            claim(Some(KeyboardPart::Left), Some(0)),
            claim(Some(KeyboardPart::Right), Some(1))
        ]
    );
    assert_eq!(
        automatic_claims(4),
        vec![
            claim(Some(KeyboardPart::Left), Some(0)),
            claim(Some(KeyboardPart::Right), Some(1)),
            claim(None, Some(2)),
            claim(None, Some(3))
        ]
    );
    for players in 1..=MAX_LOCAL_PLAYERS {
        assert_eq!(check_claims(&automatic_claims(players)), Ok(()));
    }
}

#[test]
fn split_gives_each_player_its_half_and_its_gamepad() {
    let devices = LiveDevices {
        keys: keys(&["KeyW", "ArrowUp"]),
        gamepads: vec![Some(pad(1.0)), Some(pad(0.25))],
    };
    let inputs = split(&devices, &automatic_claims(2));
    assert_eq!(
        inputs[0],
        Some(RawInput {
            keys: keys(&["KeyW"]),
            gamepads: vec![pad(1.0)]
        })
    );
    assert_eq!(
        inputs[1],
        Some(RawInput {
            keys: keys(&["ArrowUp"]),
            gamepads: vec![pad(0.25)]
        })
    );
}

#[test]
fn disconnected_gamepad_only_player_gets_none() {
    let claims = [
        Claim {
            keyboard: None,
            gamepad: Some(0),
        },
        Claim {
            keyboard: None,
            gamepad: Some(5),
        },
    ];
    let devices = LiveDevices {
        keys: keys(&["KeyW"]),
        gamepads: vec![None],
    };
    assert_eq!(split(&devices, &claims), vec![None, None]);
}

#[test]
fn disconnected_gamepad_with_a_keyboard_half_keeps_the_keys() {
    let devices = LiveDevices {
        keys: keys(&["KeyW", "ArrowUp"]),
        gamepads: vec![None],
    };
    let inputs = split(&devices, &automatic_claims(2));
    assert_eq!(
        inputs[0],
        Some(RawInput {
            keys: keys(&["KeyW"]),
            gamepads: Vec::new()
        })
    );
}

#[test]
fn gamepad_claimed_twice_is_refused_naming_both_players() {
    let claims = [
        Claim {
            keyboard: None,
            gamepad: Some(1),
        },
        Claim {
            keyboard: Some(KeyboardPart::Left),
            gamepad: None,
        },
        Claim {
            keyboard: None,
            gamepad: Some(1),
        },
    ];
    assert_eq!(
        check_claims(&claims),
        Err(ClaimError::GamepadTwice {
            gamepad: 1,
            players: (0, 2)
        })
    );
}

#[test]
fn overlapping_keyboard_parts_are_refused() {
    let part = |keyboard| Claim {
        keyboard: Some(keyboard),
        gamepad: None,
    };
    for (a, b) in [
        (KeyboardPart::Whole, KeyboardPart::Right),
        (KeyboardPart::Left, KeyboardPart::Whole),
        (KeyboardPart::Left, KeyboardPart::Left),
    ] {
        assert_eq!(
            check_claims(&[part(a), part(b)]),
            Err(ClaimError::KeyboardOverlap { players: (0, 1) })
        );
    }
}

#[derive(Serialize, Deserialize, Schema, Default, Clone, Debug, PartialEq)]
struct Pad {
    jump: bool,
}

fn map_pad(raw: &RawInput, _player: usize) -> Pad {
    Pad {
        jump: raw.keys.contains("Space"),
    }
}

#[test]
fn map_players_encodes_default_for_none() {
    let mut app = App::new();
    app.input::<Pad>().input_mapping(map_pad);
    let mut sim = app.build().unwrap();
    sim.set_players(2);
    let bytes = sim.map_players(&[None, None]).unwrap();
    assert_eq!(Some(bytes), sim.encoded_default_inputs());
    let some = sim
        .map_players(&[
            Some(RawInput {
                keys: keys(&["Space"]),
                gamepads: Vec::new(),
            }),
            None,
        ])
        .unwrap();
    let pads: Vec<Pad> = postcard::from_bytes(&some).unwrap();
    assert_eq!(pads, vec![Pad { jump: true }, Pad { jump: false }]);
    assert!(sim.map_players(&[None]).is_err());
}

#[test]
fn check_claims_reports_the_first_clashing_pair_gamepad_before_keyboard() {
    let claim = |keyboard, gamepad| Claim { keyboard, gamepad };
    // One pair clashes on both: the gamepad is reported.
    assert_eq!(
        check_claims(&[
            claim(Some(KeyboardPart::Left), Some(0)),
            claim(Some(KeyboardPart::Left), Some(0)),
        ]),
        Err(ClaimError::GamepadTwice {
            gamepad: 0,
            players: (0, 1)
        })
    );
    // Pairs are scanned by the later player, then the earlier one: the keyboard overlap of
    // players 0 and 1 comes before the gamepad clash of players 0 and 2.
    assert_eq!(
        check_claims(&[
            claim(Some(KeyboardPart::Whole), Some(0)),
            claim(Some(KeyboardPart::Left), None),
            claim(None, Some(0)),
        ]),
        Err(ClaimError::KeyboardOverlap { players: (0, 1) })
    );
}

fn key_claim(part: KeyboardPart) -> Claim {
    Claim {
        keyboard: Some(part),
        gamepad: None,
    }
}

fn pad_claim(gamepad: u32) -> Claim {
    Claim {
        keyboard: None,
        gamepad: Some(gamepad),
    }
}

fn button(gamepad: u32, button: usize) -> LobbyPress<'static> {
    LobbyPress::Button { gamepad, button }
}

#[test]
fn key_claims_its_half() {
    let mut lobby = Lobby::new();
    assert_eq!(
        lobby.press(LobbyPress::Key("KeyA")),
        LobbyEvent::Claimed { player: 0 }
    );
    assert_eq!(
        lobby.press(LobbyPress::Key("ArrowUp")),
        LobbyEvent::Claimed { player: 1 }
    );
    assert_eq!(
        lobby.claims(),
        [
            key_claim(KeyboardPart::Left),
            key_claim(KeyboardPart::Right)
        ]
    );
}

#[test]
fn same_half_twice_claims_once() {
    let mut lobby = Lobby::new();
    lobby.press(LobbyPress::Key("KeyA"));
    assert_eq!(lobby.press(LobbyPress::Key("KeyS")), LobbyEvent::Ignored);
    assert_eq!(lobby.claims().len(), 1);
}

#[test]
fn gamepad_button_claims_its_slot() {
    let mut lobby = Lobby::new();
    assert_eq!(lobby.press(button(2, 0)), LobbyEvent::Claimed { player: 0 });
    assert_eq!(lobby.press(button(2, 1)), LobbyEvent::Ignored);
    assert_eq!(lobby.claims(), [pad_claim(2)]);
}

#[test]
fn start_without_claims_is_ignored() {
    let mut lobby = Lobby::new();
    assert_eq!(lobby.press(LobbyPress::Key("Enter")), LobbyEvent::Ignored);
    assert_eq!(lobby.press(button(0, 9)), LobbyEvent::Ignored);
    assert!(!lobby.started());
}

#[test]
fn enter_starts_with_the_claims_in_order() {
    let mut lobby = Lobby::new();
    lobby.press(button(1, 0));
    lobby.press(button(0, 0));
    assert_eq!(
        lobby.press(LobbyPress::Key("Enter")),
        LobbyEvent::Started {
            claims: vec![pad_claim(1), pad_claim(0)]
        }
    );
    assert!(lobby.started());
    assert_eq!(lobby.claims(), [pad_claim(1), pad_claim(0)]);

    let mut by_start = Lobby::new();
    by_start.press(button(0, 0));
    assert_eq!(
        by_start.press(button(0, 9)),
        LobbyEvent::Started {
            claims: vec![pad_claim(0)]
        }
    );
}

#[test]
fn lone_keyboard_half_becomes_whole_at_start() {
    let mut lobby = Lobby::new();
    lobby.press(LobbyPress::Key("KeyD"));
    lobby.press(button(0, 0));
    let event = lobby.press(LobbyPress::Key("Enter"));
    let expected = vec![key_claim(KeyboardPart::Whole), pad_claim(0)];
    assert_eq!(
        event,
        LobbyEvent::Started {
            claims: expected.clone()
        }
    );
    assert_eq!(lobby.claims(), expected);

    let mut two = Lobby::new();
    two.press(LobbyPress::Key("KeyD"));
    two.press(LobbyPress::Key("KeyL"));
    two.press(LobbyPress::Key("Enter"));
    assert_eq!(
        two.claims(),
        [
            key_claim(KeyboardPart::Left),
            key_claim(KeyboardPart::Right)
        ]
    );
}

#[test]
fn fifth_claim_is_full() {
    let mut lobby = Lobby::new();
    for slot in 0..MAX_LOCAL_PLAYERS as u32 {
        assert!(matches!(
            lobby.press(button(slot, 0)),
            LobbyEvent::Claimed { .. }
        ));
    }
    assert_eq!(lobby.press(button(9, 0)), LobbyEvent::Full);
    assert_eq!(lobby.press(LobbyPress::Key("KeyA")), LobbyEvent::Full);
    assert_eq!(lobby.press(button(0, 1)), LobbyEvent::Ignored);
    assert_eq!(lobby.claims().len(), MAX_LOCAL_PLAYERS);
}

#[test]
fn escape_releases_both_halves_and_compacts() {
    let mut lobby = Lobby::new();
    lobby.press(button(0, 0));
    lobby.press(LobbyPress::Key("KeyA"));
    lobby.press(button(1, 0));
    lobby.press(LobbyPress::Key("ArrowLeft"));
    assert_eq!(
        lobby.press(LobbyPress::Key("Escape")),
        LobbyEvent::Released {
            players: vec![1, 3]
        }
    );
    assert_eq!(lobby.claims(), [pad_claim(0), pad_claim(1)]);
    assert_eq!(lobby.press(LobbyPress::Key("Escape")), LobbyEvent::Ignored);
}

#[test]
fn select_releases_its_gamepad() {
    let mut lobby = Lobby::new();
    lobby.press(button(0, 0));
    lobby.press(button(1, 0));
    assert_eq!(lobby.press(button(3, 8)), LobbyEvent::Ignored);
    assert_eq!(
        lobby.press(button(0, 8)),
        LobbyEvent::Released { players: vec![0] }
    );
    assert_eq!(lobby.claims(), [pad_claim(1)]);
}

#[test]
fn presses_after_start_are_ignored() {
    let mut lobby = Lobby::new();
    lobby.press(button(0, 0));
    lobby.press(LobbyPress::Key("Enter"));
    for press in [
        LobbyPress::Key("KeyA"),
        LobbyPress::Key("Escape"),
        button(1, 0),
        button(0, 8),
        button(0, 9),
    ] {
        assert_eq!(lobby.press(press), LobbyEvent::Ignored);
    }
    assert_eq!(lobby.claims(), [pad_claim(0)]);
}

#[test]
fn at_capacity_a_claimed_half_is_ignored_and_a_new_device_is_full() {
    let mut lobby = Lobby::new();
    lobby.press(LobbyPress::Key("KeyA"));
    for slot in 0..(MAX_LOCAL_PLAYERS - 1) as u32 {
        lobby.press(button(slot, 0));
    }
    assert_eq!(lobby.claims().len(), MAX_LOCAL_PLAYERS);
    assert_eq!(lobby.press(LobbyPress::Key("KeyS")), LobbyEvent::Ignored);
    assert_eq!(lobby.press(LobbyPress::Key("ArrowUp")), LobbyEvent::Full);
}

#[test]
fn claim_after_release_takes_the_freed_index() {
    let mut lobby = Lobby::new();
    lobby.press(button(0, 0));
    lobby.press(button(1, 0));
    lobby.press(button(2, 0));
    lobby.press(button(0, 8));
    assert_eq!(lobby.claims(), [pad_claim(1), pad_claim(2)]);
    assert_eq!(lobby.press(button(5, 0)), LobbyEvent::Claimed { player: 2 });
    assert_eq!(lobby.claims(), [pad_claim(1), pad_claim(2), pad_claim(5)]);
}

#[test]
fn lobby_events_serialize_tagged_for_the_web_page() {
    let json = |event: LobbyEvent| serde_json::to_value(event).unwrap();
    assert_eq!(
        json(LobbyEvent::Claimed { player: 1 }),
        serde_json::json!({ "type": "claimed", "player": 1 })
    );
    assert_eq!(
        json(LobbyEvent::Released {
            players: vec![0, 2]
        }),
        serde_json::json!({ "type": "released", "players": [0, 2] })
    );
    assert_eq!(
        json(LobbyEvent::Full),
        serde_json::json!({ "type": "full" })
    );
    assert_eq!(
        json(LobbyEvent::Ignored),
        serde_json::json!({ "type": "ignored" })
    );
    assert_eq!(
        json(LobbyEvent::Started {
            claims: automatic_claims(1)
        }),
        serde_json::json!({
            "type": "started",
            "claims": [{ "keyboard": "Whole", "gamepad": 0 }],
        })
    );
}
