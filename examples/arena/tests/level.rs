//! The level `build-level` saved in the committed scene: the fighters stand on its ground,
//! every run of painted tiles has its collider, and `render` draws the tiles and a fighter.
#![cfg(not(target_family = "wasm"))]

use std::fs;
use std::path::Path;

use arena::{
    Fighter, HERO_FALL_SHEET, HERO_IDLE_SHEET, HERO_JUMP_SHEET, HERO_RUN_SHEET, WARRIOR_FALL_SHEET,
    WARRIOR_IDLE_SHEET, WARRIOR_JUMP_SHEET, WARRIOR_RUN_SHEET,
};
use ludenea::app::project::project_assets;
use ludenea::prelude::*;

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/arena.scene.ron");
const PROJECT: &str = env!("CARGO_MANIFEST_DIR");
/// How far two edges computed along different float paths may be apart.
const TOLERANCE: f32 = 0.01;
/// The color the renderer clears to (`BACKGROUND` in ludenea-render's `gpu` module).
const BACKGROUND: [u8; 4] = [0x12, 0x12, 0x14, 0xFF];

/// The committed scene loaded for `players` players.
fn level(players: usize) -> Simulation {
    let scene = fs::read_to_string(SCENE).expect("the scene is committed");
    let assets = project_assets(Path::new(SCENE), None).expect("the project's assets read");
    let mut simulation = arena::app().build().expect("the arena app builds");
    simulation
        .load_scene_with(&scene, &assets)
        .unwrap_or_else(|error| panic!("{error}"));
    simulation.set_players(players);
    simulation
}

/// The scene's one tilemap and the position of its bottom-left corner.
fn tilemap(world: &World) -> (Tilemap, Vec2) {
    let mut found = world.entities().filter_map(|entity| {
        Some((
            world.get::<Tilemap>(entity)?.clone(),
            world.get::<Pose>(entity)?.position,
        ))
    });
    let tilemap = found.next().expect("the scene has a tilemap");
    assert!(found.next().is_none(), "the scene has one tilemap");
    tilemap
}

/// A horizontal run of painted cells, in world units.
#[derive(Debug)]
struct Run {
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
}

/// Each maximal horizontal run of painted cells.
fn runs(map: &Tilemap, corner: Vec2) -> Vec<Run> {
    let painted = |column: u32, row: u32| map.tiles[(row * map.width + column) as usize].is_some();
    let mut runs = Vec::new();
    for row in 0..map.height {
        let mut column = 0;
        while column < map.width {
            if !painted(column, row) {
                column += 1;
                continue;
            }
            let first = column;
            while column < map.width && painted(column, row) {
                column += 1;
            }
            let size = map.cell_size;
            runs.push(Run {
                left: corner.x + first as f32 * size.x,
                right: corner.x + column as f32 * size.x,
                bottom: corner.y + row as f32 * size.y,
                top: corner.y + (row + 1) as f32 * size.y,
            });
        }
    }
    runs
}

/// The bounds of each fixed box collider, as (left, right, bottom, top).
fn fixed_boxes(world: &World) -> Vec<(f32, f32, f32, f32)> {
    world
        .entities()
        .filter_map(|entity| {
            let body = world.get::<Body>(entity)?;
            let collider = world.get::<Collider>(entity)?;
            let pose = world.get::<Pose>(entity)?;
            let Shape::Polygon(polygon) = &collider.shape else {
                return None;
            };
            if body.kind != BodyKind::Static || pose.rotation != Rot::IDENTITY {
                return None;
            }
            let xs = polygon.vertices().iter().map(|v| pose.position.x + v.x);
            let ys = polygon.vertices().iter().map(|v| pose.position.y + v.y);
            let fold = |values: &mut dyn Iterator<Item = f32>, pick: fn(f32, f32) -> f32| {
                values.reduce(pick).expect("a polygon has vertices")
            };
            Some((
                fold(&mut xs.clone(), ludenea::math::min),
                fold(&mut xs.clone(), ludenea::math::max),
                fold(&mut ys.clone(), ludenea::math::min),
                fold(&mut ys.clone(), ludenea::math::max),
            ))
        })
        .collect()
}

#[test]
fn every_tile_run_has_a_collider_under_it() {
    let simulation = level(1);
    let world = simulation.world();
    let (map, corner) = tilemap(world);
    let runs = runs(&map, corner);
    let rows_painted: std::collections::BTreeSet<_> =
        runs.iter().map(|run| run.bottom.to_bits()).collect();
    assert!(
        rows_painted.len() >= 2 && runs.len() >= 3,
        "the level has a ground and two platforms: {runs:?}"
    );
    let boxes = fixed_boxes(world);
    for run in &runs {
        let near = |a: f32, b: f32| (a - b).abs() <= TOLERANCE;
        assert!(
            boxes
                .iter()
                .any(|&(left, right, bottom, top)| near(left, run.left)
                    && near(right, run.right)
                    && near(top, run.top)
                    && near(bottom, run.bottom)),
            "no fixed box collider covers the run {run:?}; colliders: {boxes:?}"
        );
    }
    assert_eq!(
        boxes.len(),
        runs.len(),
        "one collider per run of tiles, none elsewhere"
    );
}

#[test]
fn both_fighters_stand_on_the_ground_after_60_ticks() {
    let mut simulation = level(2);
    for _ in 0..60 {
        simulation
            .step_with(&[arena::ArenaInput::default(), arena::ArenaInput::default()])
            .unwrap_or_else(|error| panic!("{error}"));
    }
    let world = simulation.world();
    let (map, corner) = tilemap(world);
    let runs = runs(&map, corner);
    let mut fighters: Vec<(Fighter, Vec2)> = world
        .entities()
        .filter_map(|entity| {
            Some((
                world.get::<Fighter>(entity)?.clone(),
                world.get::<Pose>(entity)?.position,
            ))
        })
        .collect();
    fighters.sort_by_key(|(fighter, _)| fighter.player);
    assert_eq!(fighters.len(), 2, "one fighter per player");
    for (fighter, feet) in fighters {
        assert!(
            fighter.grounded,
            "player {} is not on the ground",
            fighter.player
        );
        assert_eq!(fighter.velocity, Vec2::ZERO, "player {}", fighter.player);
        // The highest painted surface under the feet is the one they stand on.
        let surface = runs
            .iter()
            .filter(|run| {
                run.left <= feet.x && feet.x <= run.right && run.top <= feet.y + TOLERANCE
            })
            .map(|run| run.top)
            .reduce(ludenea::math::max)
            .unwrap_or_else(|| panic!("no tile under player {} at {feet:?}", fighter.player));
        assert!(
            (feet.y - surface).abs() <= TOLERANCE,
            "player {} stands at {feet:?}, not on the tiles' top at y {surface}",
            fighter.player
        );
    }
}

/// Renders tick 300 of `scene` (one player, no input) at 1280 by 720 to RGBA pixels, with
/// the arena's textures.
fn render(scene: &Path, folder: &Path) -> Vec<[u8; 4]> {
    let out = folder.join(format!(
        "{}.png",
        scene.file_stem().unwrap().to_string_lossy()
    ));
    let mut said = Vec::new();
    let arguments = [
        "arena",
        "render",
        scene.to_str().unwrap(),
        "--project",
        PROJECT,
        "--tick",
        "300",
        "--out",
        out.to_str().unwrap(),
    ];
    let code = ludenea::cli::run(arena::app, &arguments, &mut said);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&said));
    let decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(&out).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (1280, 720));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    pixels
        .chunks_exact(4)
        .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
        .collect()
}

/// The committed scene with each `from` replaced by its `to`, written to `folder` as `name`.
fn variant(folder: &Path, name: &str, replacements: &[(&str, &str)]) -> std::path::PathBuf {
    let mut text = fs::read_to_string(SCENE).unwrap();
    for (from, to) in replacements {
        assert_eq!(
            text.matches(from).count(),
            1,
            "the scene holds `{from}` once"
        );
        text = text.replace(from, to);
    }
    let path = folder.join(name);
    fs::write(&path, text).unwrap();
    path
}

/// How many pixels `drawn` shows over the renderer's background where `without` differs.
fn drawn_where_different(drawn: &[[u8; 4]], without: &[[u8; 4]]) -> usize {
    drawn
        .iter()
        .zip(without)
        .filter(|(drawn, without)| drawn != without && **drawn != BACKGROUND)
        .count()
}

#[test]
fn render_of_tick_300_draws_a_fighter_over_the_tiles() {
    let folder = tempfile::tempdir().unwrap();
    let level = render(Path::new(SCENE), folder.path());
    // The tilemap drawn behind the opaque sky instead: only the tiles disappear.
    let hidden = variant(
        folder.path(),
        "hidden-tiles.scene.ron",
        &[(r#"layer:"tiles",order:0"#, r#"layer:"sky",order:-1"#)],
    );
    let tiles = drawn_where_different(&level, &render(&hidden, folder.path()));
    // Player 0's fighter drawn as the hero: only the fighter's pixels change.
    let swapped = variant(
        folder.path(),
        "hero.scene.ron",
        &[
            (WARRIOR_IDLE_SHEET, HERO_IDLE_SHEET),
            (WARRIOR_RUN_SHEET, HERO_RUN_SHEET),
            (WARRIOR_JUMP_SHEET, HERO_JUMP_SHEET),
            (WARRIOR_FALL_SHEET, HERO_FALL_SHEET),
        ],
    );
    let fighter = drawn_where_different(&level, &render(&swapped, folder.path()));
    // A cell is about 53 pixels a side in the camera's 24 by 13.5 view at 1280 by 720: the
    // visible ground spans dozens of cells.
    assert!(tiles > 10 * 53 * 53, "the tiles cover {tiles} pixels");
    assert!(fighter > 100, "the fighter covers {fighter} pixels");
}

/// A copy of the project in a fresh folder, with `edit` applied to its scene's text; returns
/// the folder's guard and the copied scene's path.
fn project_copy(edit: impl Fn(String) -> String) -> (tempfile::TempDir, std::path::PathBuf) {
    let folder = tempfile::tempdir().unwrap();
    let copy = |from: &Path, to: &Path| {
        fs::create_dir_all(to).unwrap();
        let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
        while let Some((from, to)) = stack.pop() {
            for entry in fs::read_dir(&from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    fs::create_dir_all(&target).unwrap();
                    stack.push((entry.path(), target));
                } else {
                    fs::copy(entry.path(), target).unwrap();
                }
            }
        }
    };
    copy(
        &Path::new(PROJECT).join("assets"),
        &folder.path().join("assets"),
    );
    fs::copy(
        Path::new(PROJECT).join("ludenea.toml"),
        folder.path().join("ludenea.toml"),
    )
    .unwrap();
    let scene = folder.path().join("assets/arena.scene.ron");
    fs::write(&scene, edit(fs::read_to_string(&scene).unwrap())).unwrap();
    (folder, scene)
}

/// Runs `build-level` on `scene`; returns whether it succeeded and what it wrote to its error
/// output.
fn build_level(scene: &Path) -> (bool, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_build-level"))
        .arg(scene)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// The hash after `ticks` ticks of `scene` with one player and no input.
fn hash(scene: &Path, ticks: &str) -> String {
    let mut said = Vec::new();
    let arguments = [
        "arena",
        "run",
        scene.to_str().unwrap(),
        "--ticks",
        ticks,
        "--hash",
    ];
    let code = ludenea::cli::run(arena::app, &arguments, &mut said);
    let said = String::from_utf8(said).unwrap();
    assert_eq!(code, 0, "{said}");
    said
}

#[test]
fn build_level_builds_the_committed_level_again() {
    let (_folder, scene) = project_copy(|text| text);
    let (built, errors) = build_level(&scene);
    assert!(built, "{errors}");
    assert_eq!(hash(&scene, "120"), hash(Path::new(SCENE), "120"));
}

#[test]
fn build_level_stops_at_a_refused_command_and_keeps_the_scene() {
    // A tilemap one cell narrower than its tiles: the game refuses to load the scene.
    let (_folder, scene) = project_copy(|text| text.replacen("width:32", "width:31", 1));
    let before = fs::read_to_string(&scene).unwrap();
    let (built, errors) = build_level(&scene);
    assert!(!built);
    assert!(
        errors.contains("the game refused `load_scene`")
            && errors.contains("tiles has 576 entries"),
        "{errors}"
    );
    assert_eq!(fs::read_to_string(&scene).unwrap(), before);
}
