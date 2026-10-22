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

#[derive(ludenea_ecs::Asset, serde::Serialize, serde::Deserialize)]
struct Stats {
    damage: u32,
}

#[derive(Clone, ludenea_ecs::Component, serde::Serialize, serde::Deserialize)]
struct Turret {
    stats: ludenea_ecs::Handle<Stats>,
}

/// The resolved text keeps component values as written, so only the layout around them
/// changes here.
#[test]
fn scene_hash_with_assets_ignores_scene_formatting() {
    let id = ludenea_ecs::AssetId(5);
    let assets: ludenea_app::AssetTexts = [(
        id,
        "AssetFile(type: \"trace::Stats\", value: (damage: 3))".to_string(),
    )]
    .into();
    let hash = |scene: &str| {
        let mut app = ludenea_app::App::new();
        app.component::<Turret>().asset::<Stats>();
        let mut sim = app.build().unwrap();
        sim.load_scene_with(scene, &assets).unwrap();
        assert_eq!(sim.world().asset_ids(), [id]);
        scene_hash(scene.as_bytes(), &sim)
    };
    let compact = format!(
        "Scene(version: 2, next_id: 1, resources: {{}}, entities: [(id: 0, components: {{\"trace::Turret\": (stats: \"{id}\")}})])"
    );
    let spread = format!(
        "Scene(\n  version: 2,\n  next_id: 1,\n  resources: {{}},\n  entities: [\n    (id: 0, components: {{\"trace::Turret\": (stats: \"{id}\")}}),\n  ],\n)\n"
    );
    assert_eq!(hash(&compact), hash(&spread));
    assert_ne!(
        hash(&compact),
        xxhash_rust::xxh3::xxh3_64(compact.as_bytes())
    );
}
