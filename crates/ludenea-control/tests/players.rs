//! Local players in the host: the player count, the claims, and the split of live devices.

use ludenea_app::players::{Claim, KeyboardPart, LiveDevices};
use ludenea_app::{App, InputLog, Inputs, RawGamepad, Sim};
use ludenea_control::*;
use ludenea_ecs::{Component, Query, Runtime};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize)]
struct Score(u32);

fn grow(mut scores: Query<&mut Score>, inputs: Runtime<Inputs<u8>>) {
    for score in scores.iter_mut() {
        score.0 += u32::from(*inputs.player(0));
    }
}

/// 100 for any raw input, plus 1 for `KeyA` and 2 for `KeyL`: an idle mapping (100) differs
/// from the default input (0).
fn app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .input_mapping(|raw, _| {
            100 + u8::from(raw.keys.contains("KeyA")) + 2 * u8::from(raw.keys.contains("KeyL"))
        })
        .component::<Score>()
        .system(Sim, grow);
    app
}

const SCENE: &str = r#"Scene(version: 1, resources: {}, entities: [{ "players::Score": (5) }])"#;

fn host() -> Host {
    let mut host = Host::new(app).unwrap();
    host.handle(Command::LoadScene {
        scene: SCENE.into(),
        assets: Vec::new(),
        document: Document::Scene,
    })
    .unwrap();
    host
}

fn error(host: &mut Host, command: Command) -> ProtocolError {
    host.handle(command).unwrap_err()
}

fn keys(codes: &[&str]) -> LiveDevices {
    LiveDevices {
        keys: codes.iter().map(|code| (*code).to_string()).collect(),
        gamepads: Vec::new(),
    }
}

/// Plays one tick with `devices` held and returns the recorded inputs of the players.
fn first_recorded_tick(host: &mut Host, devices: LiveDevices) -> Vec<u8> {
    host.handle(Command::SetLiveInput { devices }).unwrap();
    host.handle(Command::Play).unwrap();
    host.advance(1.0 / 60.0);
    let Reply::Session { inputs, .. } = host.handle(Command::ExportSession).unwrap() else {
        panic!("a session");
    };
    let log = InputLog::decode(&inputs.0).unwrap();
    postcard::from_bytes(&log.ticks[0]).unwrap()
}

#[test]
fn set_players_records_that_many_players() {
    let mut host = host();
    let reply = host
        .handle(Command::SetPlayers {
            players: 3,
            claims: None,
        })
        .unwrap();
    assert_eq!(
        reply,
        Reply::Players {
            claims: ludenea_app::players::automatic_claims(3),
            devices: vec![
                "left keyboard half and gamepad 1".to_string(),
                "right keyboard half and gamepad 2".to_string(),
                "gamepad 3".to_string(),
            ],
        }
    );
    let Reply::Session { inputs, .. } = host.handle(Command::ExportSession).unwrap() else {
        panic!("a session");
    };
    let log = InputLog::decode(&inputs.0).unwrap();
    assert_eq!((log.players, log.ticks.len()), (3, 0));
}

#[test]
fn set_players_zero_is_refused() {
    let mut host = host();
    let refused = error(
        &mut host,
        Command::SetPlayers {
            players: 0,
            claims: None,
        },
    );
    assert_eq!(refused.code, ErrorCode::InvalidInputs);
}

#[test]
fn claims_of_the_wrong_length_are_refused_naming_both_counts() {
    let mut host = host();
    let refused = error(
        &mut host,
        Command::SetPlayers {
            players: 3,
            claims: Some(vec![Claim {
                keyboard: Some(KeyboardPart::Whole),
                gamepad: None,
            }]),
        },
    );
    assert_eq!(refused.code, ErrorCode::InvalidInputs);
    assert!(
        refused.message.contains('1') && refused.message.contains('3'),
        "{}",
        refused.message
    );
}

#[test]
fn conflicting_claims_are_refused() {
    let mut host = host();
    let both_whole = Claim {
        keyboard: Some(KeyboardPart::Whole),
        gamepad: None,
    };
    let refused = error(
        &mut host,
        Command::SetPlayers {
            players: 2,
            claims: Some(vec![both_whole.clone(), both_whole]),
        },
    );
    assert_eq!(refused.code, ErrorCode::InvalidInputs);
}

#[test]
fn two_players_on_keyboard_halves_record_their_own_input() {
    let mut host = host();
    host.handle(Command::SetPlayers {
        players: 2,
        claims: None,
    })
    .unwrap();
    let recorded = first_recorded_tick(&mut host, keys(&["KeyA", "KeyL"]));
    assert_eq!(recorded, vec![101, 102]);
}

#[test]
fn disconnected_gamepad_records_the_default_input() {
    let mut host = host();
    let claims = vec![
        Claim {
            keyboard: Some(KeyboardPart::Whole),
            gamepad: None,
        },
        Claim {
            keyboard: None,
            gamepad: Some(0),
        },
    ];
    host.handle(Command::SetPlayers {
        players: 2,
        claims: Some(claims),
    })
    .unwrap();
    let mut devices = keys(&["KeyA"]);
    devices.gamepads = vec![None];
    assert_eq!(first_recorded_tick(&mut host, devices), vec![101, 0]);
}

#[test]
fn a_connected_idle_gamepad_leaves_live_input_off() {
    let mut host = host();
    let claims = vec![Claim {
        keyboard: None,
        gamepad: Some(0),
    }];
    host.handle(Command::SetPlayers {
        players: 1,
        claims: Some(claims),
    })
    .unwrap();
    let devices = LiveDevices {
        keys: Default::default(),
        gamepads: vec![Some(RawGamepad::default())],
    };
    // Every player idle: the live input is none, the tick records defaults.
    assert_eq!(first_recorded_tick(&mut host, devices), vec![0]);
}

#[test]
fn loading_a_session_resets_automatic_claims() {
    let mut host = host();
    let swapped = vec![
        Claim {
            keyboard: Some(KeyboardPart::Right),
            gamepad: None,
        },
        Claim {
            keyboard: Some(KeyboardPart::Left),
            gamepad: None,
        },
    ];
    host.handle(Command::SetPlayers {
        players: 2,
        claims: Some(swapped),
    })
    .unwrap();
    let Reply::Session {
        scene,
        saved_scene,
        inputs,
        edits,
        undo,
        hashes,
        assets,
        saved_assets,
        branches,
        document,
        keeps_assets,
    } = host.handle(Command::ExportSession).unwrap()
    else {
        panic!("a session");
    };
    host.handle(Command::LoadSession {
        scene,
        saved_scene,
        inputs,
        edits,
        undo,
        expected: hashes,
        assets,
        saved_assets,
        branches,
        document,
        keeps_assets,
    })
    .unwrap();
    assert_eq!(
        first_recorded_tick(&mut host, keys(&["KeyA", "KeyL"])),
        vec![101, 102]
    );
}

#[test]
fn loading_inputs_resets_automatic_claims() {
    let mut host = host();
    let swapped = vec![
        Claim {
            keyboard: Some(KeyboardPart::Right),
            gamepad: None,
        },
        Claim {
            keyboard: Some(KeyboardPart::Left),
            gamepad: None,
        },
    ];
    host.handle(Command::SetPlayers {
        players: 2,
        claims: Some(swapped),
    })
    .unwrap();
    host.handle(Command::LoadInputs {
        log: Bytes(InputLog::new(2).encode()),
    })
    .unwrap();
    assert_eq!(
        first_recorded_tick(&mut host, keys(&["KeyA", "KeyL"])),
        vec![101, 102]
    );
}

#[test]
fn tick_progress_grows_between_ticks() {
    let mut host = host();
    assert_eq!(host.tick_progress(), 0.0);
    host.handle(Command::Play).unwrap();
    host.advance(0.25 / 60.0);
    let quarter = host.tick_progress();
    host.advance(0.25 / 60.0);
    let half = host.tick_progress();
    assert!(
        0.0 < quarter && quarter < half && half < 1.0,
        "{quarter} {half}"
    );
}

#[test]
fn set_players_replies_the_claims_it_applied() {
    let mut host = host();
    let claims = vec![
        Claim {
            keyboard: Some(KeyboardPart::Right),
            gamepad: Some(2),
        },
        Claim {
            keyboard: None,
            gamepad: None,
        },
    ];
    let reply = host
        .handle(Command::SetPlayers {
            players: 2,
            claims: Some(claims.clone()),
        })
        .unwrap();
    assert_eq!(
        reply,
        Reply::Players {
            claims,
            devices: vec![
                "right keyboard half and gamepad 3".to_string(),
                "no device".to_string()
            ],
        }
    );
}

#[test]
fn loading_a_replay_resets_automatic_claims() {
    let mut host = host();
    let swap = |host: &mut Host| {
        let swapped = vec![
            Claim {
                keyboard: Some(KeyboardPart::Right),
                gamepad: None,
            },
            Claim {
                keyboard: Some(KeyboardPart::Left),
                gamepad: None,
            },
        ];
        host.handle(Command::SetPlayers {
            players: 2,
            claims: Some(swapped),
        })
        .unwrap();
    };
    swap(&mut host);
    let Reply::Replay { replay } = host.handle(Command::ExportReplay { branch: None }).unwrap()
    else {
        panic!("a replay");
    };
    swap(&mut host);
    host.handle(Command::LoadReplay { replay }).unwrap();
    assert_eq!(
        first_recorded_tick(&mut host, keys(&["KeyA", "KeyL"])),
        vec![101, 102]
    );
}
