use ludenea_app::{App, AppError, RawGamepad, RawInput, SimError};
use ludenea_ecs::schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Schema, Default, Clone, Debug, PartialEq)]
struct Pad {
    jump: bool,
    player: u8,
}

#[derive(Serialize, Deserialize, Schema, Default, Clone)]
struct Other;

fn map_pad(raw: &RawInput, player: usize) -> Pad {
    Pad {
        jump: raw.keys.contains("Space"),
        player: player as u8,
    }
}

fn keys(codes: &[&str]) -> RawInput {
    RawInput {
        keys: codes.iter().map(|code| code.to_string()).collect(),
        gamepads: Vec::new(),
    }
}

#[test]
fn idle_raw_input_maps_to_default() {
    let mut app = App::new();
    app.input::<Pad>().input_mapping(map_pad);
    let sim = app.build().unwrap();
    let bytes = sim.map_raw_input(&[RawInput::default()]).unwrap();
    assert_eq!(Some(bytes), sim.encoded_default_inputs());
}

#[test]
fn mapping_receives_the_player_index() {
    let mut app = App::new();
    app.input::<Pad>().input_mapping(map_pad);
    let mut sim = app.build().unwrap();
    sim.set_players(2);
    let bytes = sim.map_raw_input(&[keys(&["Space"]), keys(&[])]).unwrap();
    let pads: Vec<Pad> = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(
        pads,
        vec![
            Pad {
                jump: true,
                player: 0
            },
            Pad {
                jump: false,
                player: 1
            }
        ]
    );
    assert!(sim.map_raw_input(&[keys(&[])]).is_err());
}

#[test]
fn map_without_mapping_is_no_input_mapping() {
    let mut app = App::new();
    app.input::<Pad>();
    let sim = app.build().unwrap();
    assert!(matches!(
        sim.map_raw_input(&[RawInput::default()]),
        Err(SimError::NoInputMapping)
    ));
}

#[test]
fn mapping_for_another_type_fails_the_build() {
    let mut app = App::new();
    app.input::<Other>().input_mapping(map_pad);
    assert!(matches!(
        app.build(),
        Err(AppError::InputMappingMismatch { .. })
    ));
}

#[test]
fn is_idle_ignores_small_axes() {
    let pad = |buttons: Vec<f32>, axes: Vec<f32>| RawInput {
        keys: Default::default(),
        gamepads: vec![RawGamepad { buttons, axes }],
    };
    assert!(RawInput::default().is_idle());
    assert!(pad(vec![0.5, 0.0], vec![0.2, -0.2]).is_idle());
    assert!(!pad(vec![0.6], vec![]).is_idle());
    assert!(!pad(vec![], vec![0.0, -0.3]).is_idle());
    assert!(!keys(&["KeyA"]).is_idle());
}
