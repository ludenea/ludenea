use std::collections::BTreeSet;

use ludenea_app::players::{
    Claim, ClaimError, KeyboardPart, LiveDevices, MAX_LOCAL_PLAYERS, automatic_claims,
    check_claims, split,
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
