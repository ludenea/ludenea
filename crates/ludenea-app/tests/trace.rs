use ludenea_app::trace::{Trace, scene_hash};

/// An integration test: `boxes` links this crate, so its simulation type is this one only here.
#[test]
fn scene_hash_of_assetless_scene_is_raw_bytes() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let scene = std::fs::read(format!("{root}/examples/boxes/assets/boxes.scene.ron")).unwrap();
    let golden =
        Trace::decode(&std::fs::read(format!("{root}/golden/boxes.trace")).unwrap()).unwrap();
    let mut sim = boxes::app().build().unwrap();
    sim.load_scene(std::str::from_utf8(&scene).unwrap())
        .unwrap();
    assert_eq!(scene_hash(&scene, &sim), golden.scene_hash);
}
