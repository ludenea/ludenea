use boxes::BoxesInput;
use ludenea::app::RawInput;

fn keys(codes: &[&str]) -> RawInput {
    RawInput {
        keys: codes.iter().map(|code| code.to_string()).collect(),
        gamepads: Vec::new(),
    }
}

fn map(raw: RawInput) -> BoxesInput {
    let sim = boxes::app().build().unwrap();
    let bytes = sim.map_raw_input(&[raw]).unwrap();
    postcard::from_bytes::<Vec<BoxesInput>>(&bytes)
        .unwrap()
        .remove(0)
}

#[test]
fn boxes_mapping_turns_arrow_keys_into_velocity_change() {
    let input = map(keys(&["ArrowRight", "ArrowUp", "KeyS", "Delete"]));
    assert_eq!(input.velocity_change.map(|v| (v.x, v.y)), Some((1.0, 1.0)));
    assert_eq!(input.spawn, Some(0.0));
    assert!(input.despawn);

    let input = map(keys(&["ArrowLeft", "ArrowRight"]));
    assert_eq!(input.velocity_change, None);
    assert_eq!(input.spawn, None);
    assert!(!input.despawn);
}
