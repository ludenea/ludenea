use ludenea_app::{App, SimError};
use ludenea_ecs::schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Schema, Default, Clone, Debug, PartialEq)]
struct Pad {
    run: u32,
    jump: bool,
}

fn spec() -> ludenea_app::PlayerInputSpec {
    let mut app = App::new();
    app.input::<Pad>();
    app.build()
        .unwrap()
        .player_input_spec()
        .expect("the app has an input type")
}

#[test]
fn app_without_input_type_has_no_spec() {
    assert!(App::new().build().unwrap().player_input_spec().is_none());
}

#[test]
fn default_is_the_encoding_of_one_default_input() {
    let spec = spec();
    assert_eq!(
        spec.default,
        postcard::to_allocvec(&Pad::default()).unwrap()
    );
    (spec.check)(&spec.default).expect("the default input is canonical");
}

#[test]
fn player_input_spec_accepts_canonical_bytes() {
    let bytes = postcard::to_allocvec(&Pad {
        run: 300,
        jump: true,
    })
    .unwrap();
    (spec().check)(&bytes).expect("the canonical encoding of one input");
}

#[test]
fn player_input_spec_refuses_non_canonical_bytes() {
    // `run: 1` as an overlong varint (0x81 0x00 instead of 0x01), then `jump: false`.
    let overlong = [0x81, 0x00, 0x00];
    let decoded: Pad = postcard::from_bytes(&overlong).expect("postcard reads overlong varints");
    assert_eq!(
        decoded,
        Pad {
            run: 1,
            jump: false
        }
    );
    let error = (spec().check)(&overlong).expect_err("not the canonical encoding");
    assert!(
        matches!(&error, SimError::InputDecode(message)
            if message.contains("3 bytes") && message.contains("canonical encoding takes 2")),
        "{error}"
    );
}

#[test]
fn player_input_spec_refuses_leftover_and_undecodable_bytes() {
    let check = spec().check;
    let mut leftover = postcard::to_allocvec(&Pad::default()).unwrap();
    leftover.push(0);
    assert!(matches!(check(&leftover), Err(SimError::InputDecode(_))));
    assert!(matches!(check(&[]), Err(SimError::InputDecode(_))));
    assert!(matches!(
        check(&[0x00, 0x02]),
        Err(SimError::InputDecode(_))
    ));
}
