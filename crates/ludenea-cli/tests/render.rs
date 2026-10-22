//! `render`: a tick of a scene or replay drawn offscreen to a PNG. These tests need a GPU
//! adapter (hardware or software); without one they fail naming what was tried.
#![cfg(not(target_family = "wasm"))]

use std::fs;
use std::path::{Path, PathBuf};

use ludenea_app::pose::Pose;
use ludenea_app::{App, InputLog, Inputs, Sim};
use ludenea_cli::run;
use ludenea_ecs::{Component, Query, Runtime, With};
use ludenea_render::RenderPlugin;
use serde::{Deserialize, Serialize};

/// Moves right by a sixteenth of a unit per unit of input.
#[derive(Clone, Component, Serialize, Deserialize)]
struct Walker;

fn walk(inputs: Runtime<Inputs<u8>>, mut walkers: Query<&mut Pose, With<Walker>>) {
    let step = f32::from(*inputs.player(0)) / 16.0;
    for pose in walkers.iter_mut() {
        pose.position.x += step;
    }
}

fn app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .plugin(RenderPlugin)
        .render_layer("world", 1.0)
        .component::<Walker>()
        .system(Sim, walk);
    app
}

const RED: [u8; 4] = [255, 0, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const BACKGROUND: [u8; 4] = [0x12, 0x12, 0x14, 0xFF];

/// The fixture project: a camera showing 4 by 4 units around the origin (16 pixels per unit
/// on 64 by 64 pixels, the origin at pixel (32, 32), +y up), a walker animated by a sprite
/// sheet (frame 0 red, frame 1 yellow, showing frame 1 as loaded) centred on (0, 1), and a
/// tilemap of a green and a blue tile whose bottom-left corner is (-2, -2).
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render")
}

/// A fresh directory for outputs, removed when the returned guard drops, holding
/// `inputs.bin`: 16 at tick 0, which moves the walker one unit right, and 0 for the next 9
/// ticks.
fn dir() -> tempfile::TempDir {
    let guard = tempfile::tempdir().unwrap();
    let dir = guard.path();
    let mut log = InputLog::new(1);
    log.record(&[16u8]);
    for _ in 1..10 {
        log.record(&[0u8]);
    }
    fs::write(dir.join("inputs.bin"), log.encode()).unwrap();
    guard
}

/// Runs the command line with `args`, `{}` standing for `dir` and `{fixture}` for the
/// fixture project.
fn cli(dir: &Path, args: &[&str]) -> (i32, String) {
    let mut full = vec!["test".to_string()];
    full.extend(args.iter().map(|arg| {
        arg.replace("{fixture}", fixture().to_str().unwrap())
            .replace("{}", dir.to_str().unwrap())
    }));
    let mut out = Vec::new();
    let code = run(app, &full, &mut out);
    (code, String::from_utf8(out).unwrap())
}

/// Renders `scene` at `tick` to `{dir}/<name>.png` on 64 by 64 pixels, with `extra`
/// arguments, and asserts it succeeded.
fn render(dir: &Path, scene: &str, tick: &str, name: &str, extra: &[&str]) -> String {
    let out = format!("{{}}/{name}.png");
    let mut args = vec![
        "render", scene, "--tick", tick, "--out", &out, "--width", "64", "--height", "64",
    ];
    args.extend(extra);
    let (code, output) = cli(dir, &args);
    assert_eq!(code, 0, "{output}");
    output
}

struct Image {
    width: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * self.width + x) * 4) as usize;
        self.rgba[start..start + 4].try_into().unwrap()
    }
}

fn decode(path: &Path) -> Image {
    let decoder = png::Decoder::new(std::io::Cursor::new(fs::read(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut rgba).unwrap();
    assert_eq!(
        (info.width, info.height, info.color_type, info.bit_depth),
        (64, 64, png::ColorType::Rgba, png::BitDepth::Eight)
    );
    Image {
        width: info.width,
        rgba,
    }
}

/// The scene's 10 ticks with the inputs, recorded as `{dir}/run.replay`.
fn record_replay(dir: &Path) {
    let (code, output) = cli(
        dir,
        &[
            "run",
            "{fixture}/scenes/main.scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--ticks",
            "10",
            "--replay",
            "{}/run.replay",
        ],
    );
    assert_eq!(code, 0, "{output}");
}

#[test]
fn renders_a_tick_to_a_png() {
    let guard = dir();
    let dir = guard.path();
    let output = render(dir, "{fixture}/scenes/main.scene.ron", "3", "frame", &[]);
    assert!(output.contains("frame.png"), "{output}");
    let image = decode(&dir.join("frame.png"));
    // Without inputs the walker stays on (0, 1): pixels 24..40 across and 8..24 down. The
    // animation started at tick 0 and alternates frames every tick: the third step shows
    // frame 0.
    assert_eq!(image.pixel(32, 16), RED);
    assert_eq!(image.pixel(23, 16), BACKGROUND);
    assert_eq!(image.pixel(40, 16), BACKGROUND);
    // The tiles: world [-2, -1] and [-1, 0] across, [-2, -1] up.
    assert_eq!(image.pixel(8, 56), GREEN);
    assert_eq!(image.pixel(24, 56), BLUE);
    assert_eq!(image.pixel(40, 56), BACKGROUND);
}

#[test]
fn tick_0_renders_the_scene_as_loaded() {
    let guard = dir();
    let dir = guard.path();
    render(dir, "{fixture}/scenes/main.scene.ron", "0", "frame", &[]);
    // The scene's frame 1: no step has animated the sprite yet.
    assert_eq!(decode(&dir.join("frame.png")).pixel(32, 16), YELLOW);
}

#[test]
fn render_of_a_replay_matches_render_of_its_scene_and_inputs() {
    let guard = dir();
    let dir = guard.path();
    record_replay(dir);
    render(
        dir,
        "{}/run.replay",
        "4",
        "replay",
        &["--project", "{fixture}"],
    );
    render(
        dir,
        "{fixture}/scenes/main.scene.ron",
        "4",
        "scene",
        &["--inputs", "{}/inputs.bin"],
    );
    let replay = fs::read(dir.join("replay.png")).unwrap();
    assert_eq!(replay, fs::read(dir.join("scene.png")).unwrap());
    // The input of tick 0 moved the walker to (1, 1): pixels 40..56 across.
    let image = decode(&dir.join("replay.png"));
    assert_eq!(image.pixel(48, 16), YELLOW);
    assert_eq!(image.pixel(32, 16), BACKGROUND);
}

#[test]
fn replay_whose_recorded_hash_differs_warns_and_still_renders() {
    let guard = dir();
    let dir = guard.path();
    record_replay(dir);
    let path = dir.join("run.replay");
    let mut replay = ludenea_app::replay::Replay::decode(&fs::read(&path).unwrap()).unwrap();
    let played = replay.hashes[4];
    replay.hashes[4] ^= 1;
    let recorded = replay.hashes[4];
    fs::write(&path, replay.encode()).unwrap();
    let output = render(
        dir,
        "{}/run.replay",
        "4",
        "frame",
        &["--project", "{fixture}"],
    );
    let expected = format!(
        "warning: tick 4 hash {played:016x} differs from the hash {recorded:016x} the replay recorded; run `verify` on the replay to find the first divergent tick"
    );
    assert!(output.lines().any(|line| line == expected), "{output}");
    assert!(dir.join("frame.png").is_file());
}

#[test]
fn broken_texture_warns_and_still_renders() {
    let guard = dir();
    let dir = guard.path();
    let project = dir.join("project");
    copy(&fixture(), &project);
    fs::write(project.join("assets/tiles.png"), b"not a png").unwrap();
    let output = render(dir, "{}/project/scenes/main.scene.ron", "3", "frame", &[]);
    let warning = output
        .lines()
        .find(|line| line.starts_with("warning: "))
        .unwrap_or_else(|| panic!("no warning in {output}"));
    assert!(warning.contains("tiles.png"), "{warning}");
    assert!(warning.contains("cannot decode the PNG"), "{warning}");
    let image = decode(&dir.join("frame.png"));
    // The tiles are drawn as placeholders, the walker as before.
    assert_eq!(image.pixel(8, 56), MAGENTA);
    assert_eq!(image.pixel(24, 56), MAGENTA);
    assert_eq!(image.pixel(32, 16), RED);
}

#[test]
fn invalid_texture_meta_warns_and_still_renders() {
    for meta in [
        "Meta(".to_string(),
        fs::read_to_string(fixture().join("assets/tiles.png.meta"))
            .unwrap()
            .replace("pixels_per_unit: 16", "pixels_per_unit: 0"),
    ] {
        let guard = dir();
        let dir = guard.path();
        let project = dir.join("project");
        copy(&fixture(), &project);
        fs::write(project.join("assets/tiles.png.meta"), meta).unwrap();
        let output = render(dir, "{}/project/scenes/main.scene.ron", "3", "frame", &[]);
        let warning = output
            .lines()
            .find(|line| line.starts_with("warning: "))
            .unwrap_or_else(|| panic!("no warning in {output}"));
        assert!(
            warning.contains("tiles.png: cannot read its meta: "),
            "{warning}"
        );
        assert!(warning.contains("the texture is left out"), "{warning}");
        let image = decode(&dir.join("frame.png"));
        // The tiles are drawn as placeholders, the walker as before.
        assert_eq!(image.pixel(8, 56), MAGENTA);
        assert_eq!(image.pixel(32, 16), RED);
    }
}

#[test]
fn textures_sharing_an_identifier_are_refused_before_the_run() {
    let guard = dir();
    let dir = guard.path();
    let project = dir.join("project");
    copy(&fixture(), &project);
    let hero = fs::read_to_string(project.join("assets/hero.png.meta")).unwrap();
    fs::write(project.join("assets/tiles.png.meta"), hero).unwrap();
    let (code, output) = cli(
        dir,
        &[
            "render",
            "{}/project/scenes/main.scene.ron",
            "--tick",
            "3",
            "--out",
            "{}/frame.png",
        ],
    );
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("have the same identifier"), "{output}");
    // Refused before the scene runs: no tick is played, so no hash is printed.
    assert!(!output.contains("hash"), "{output}");
    assert!(!dir.join("frame.png").exists());
}

#[test]
fn width_past_the_device_limit_is_refused_naming_it() {
    let guard = dir();
    let dir = guard.path();
    let (code, output) = cli(
        dir,
        &[
            "render",
            "{fixture}/scenes/main.scene.ron",
            "--tick",
            "0",
            "--out",
            "{}/frame.png",
            "--width",
            "4000000000",
        ],
    );
    assert_eq!(code, 2, "{output}");
    assert!(output.contains("max_texture_dimension_2d"), "{output}");
    // Refused before the scene runs: no tick is played, so no hash is printed.
    assert!(!output.contains("hash"), "{output}");
    assert!(!dir.join("frame.png").exists());
}

#[test]
fn corrupt_replay_is_refused_with_its_decode_error() {
    let guard = dir();
    let dir = guard.path();
    fs::write(dir.join("bad.replay"), b"not a replay").unwrap();
    let (code, output) = cli(
        dir,
        &[
            "render",
            "{}/bad.replay",
            "--tick",
            "0",
            "--out",
            "{}/frame.png",
        ],
    );
    assert_eq!(code, 2, "{output}");
    assert!(!output.contains("hash"), "{output}");
    assert!(!dir.join("frame.png").exists());
}

#[test]
fn sprite_with_an_unknown_texture_warns_naming_it() {
    let guard = dir();
    let dir = guard.path();
    let project = dir.join("project");
    copy(&fixture(), &project);
    let scene = project.join("scenes/main.scene.ron");
    let text = fs::read_to_string(&scene).unwrap().replace(
        "\"ludenea::Sprite\": (texture: \"000000000000000000000000000000a1\"",
        "\"ludenea::Sprite\": (texture: \"000000000000000000000000000000ff\"",
    );
    fs::write(&scene, text).unwrap();
    let output = render(dir, "{}/project/scenes/main.scene.ron", "0", "frame", &[]);
    assert!(
        output.lines().any(|line| line
            == "warning: entity 1v0: frame 1 of texture 000000000000000000000000000000ff is not loaded, so a placeholder is drawn"),
        "{output}"
    );
    assert_eq!(decode(&dir.join("frame.png")).pixel(32, 16), MAGENTA);
}

#[test]
fn tick_past_a_replay_end_is_refused() {
    let guard = dir();
    let dir = guard.path();
    record_replay(dir);
    let (code, output) = cli(
        dir,
        &[
            "render",
            "{}/run.replay",
            "--tick",
            "11",
            "--out",
            "{}/frame.png",
        ],
    );
    assert_eq!(code, 2, "{output}");
    assert!(
        output.contains("cannot render tick 11: the replay ends at tick 10"),
        "{output}"
    );
    assert!(!dir.join("frame.png").exists());
}

#[test]
fn unknown_debug_layer_is_refused_naming_the_known_ones() {
    let guard = dir();
    let dir = guard.path();
    let (code, output) = cli(
        dir,
        &[
            "render",
            "{fixture}/scenes/main.scene.ron",
            "--tick",
            "1",
            "--out",
            "{}/frame.png",
            "--debug-layer",
            "colliders",
        ],
    );
    assert_eq!(code, 2, "{output}");
    assert!(
        output.contains("no debug draw layer `colliders`; this game has none"),
        "{output}"
    );
}

#[test]
fn render_does_not_change_the_run_hashes() {
    let guard = dir();
    let dir = guard.path();
    let (code, run) = cli(
        dir,
        &[
            "run",
            "{fixture}/scenes/main.scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--ticks",
            "5",
            "--hash",
        ],
    );
    assert_eq!(code, 0, "{run}");
    let rendered = render(
        dir,
        "{fixture}/scenes/main.scene.ron",
        "5",
        "frame",
        &["--inputs", "{}/inputs.bin"],
    );
    let hash = run.lines().next().unwrap();
    assert!(hash.starts_with("tick 5 hash "), "{run}");
    assert!(rendered.lines().any(|line| line == hash), "{rendered}");
}

/// Copies the folder `from` into `to`, recursively.
fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

#[test]
fn rendering_a_replay_refuses_inputs_and_edits() {
    let guard = dir();
    let dir = guard.path();
    record_replay(dir);
    for option in ["--inputs", "--edits"] {
        let (code, output) = cli(
            dir,
            &[
                "render",
                "{}/run.replay",
                "--tick",
                "1",
                "--out",
                "{}/frame.png",
                option,
                "{}/inputs.bin",
            ],
        );
        assert_eq!(code, 2, "{output}");
        assert!(
            output.contains("a replay holds its own inputs and edits; drop --inputs and --edits"),
            "{output}"
        );
    }
}
