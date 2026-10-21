//! The committed assets, generated: the scene and the bug sprite sheet (flat shapes, four
//! wing positions). Everything is a pure function of the constants here, so running the
//! `generate` binary twice writes identical bytes.

use std::fmt::Write;

use ludenea::math::sin_cos;
use ludenea::prelude::*;

use crate::PLAYERS;

/// The identifier in `bug.png.meta`.
pub const BUG_TEXTURE: &str = "00000000000000000000000000005b01";
/// The identifier in `bug.sheet.ron.meta`.
pub const BUG_SHEET: &str = "00000000000000000000000000005b02";

const WANDERERS: usize = 10_000;
const COLUMNS: usize = 125;
const SPACING: Vec2 = Vec2::new(0.48, 0.4);
const ORIGIN: Vec2 = Vec2::new(-29.76, -15.8);
const SEED: u64 = 7;
const FRAME: u32 = 16;
const FRAMES: u32 = 4;
/// Ticks each wing position shows.
const TICKS_PER_FRAME: u32 = 8;

/// Every generated asset as (file name in `assets/`, bytes).
pub fn generated_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("swarm.scene.ron", generate_scene().into_bytes()),
        ("bug.png", generate_sheet_png()),
        ("bug.png.meta", texture_meta_text().into_bytes()),
        ("bug.sheet.ron", sheet_text().into_bytes()),
        ("bug.sheet.ron.meta", sheet_meta_text().into_bytes()),
    ]
}

/// The RON scene: the camera, a dim background lattice, the four player sprites and the
/// wanderers. Randomness is drawn from `Rng::new(7, 1)`.
fn generate_scene() -> String {
    let mut entities = Vec::new();
    entities.push(entity(&[
        pose(Vec2::ZERO, 0.0),
        r#""ludenea::Camera2d": (view: (x: 64.0, y: 36.0))"#.to_string(),
    ]));
    for row in 0..8 {
        for column in 0..8 {
            let position = Vec2::new(-28.0 + 8.0 * column as f32, -14.0 + 4.0 * row as f32);
            entities.push(entity(&[
                pose(position, 0.0),
                sprite("background", 0, 0xFFFFFF30),
            ]));
        }
    }
    for index in 0..PLAYERS {
        let position = Vec2::new(-9.0 + 6.0 * index as f32, 0.0);
        entities.push(entity(&[
            pose(position, 0.0),
            sprite("world", 1, 0xFFFFFFFF),
            format!(r#""swarm::Player": (index: {index})"#),
        ]));
    }
    let mut rng = Rng::new(SEED, 1);
    for index in 0..WANDERERS {
        let center = ORIGIN
            + Vec2::new(
                (index % COLUMNS) as f32 * SPACING.x,
                (index / COLUMNS) as f32 * SPACING.y,
            );
        let radius = rng.range_f32(0.2, 0.8);
        let phase = rng.range_f32(0.0, std::f32::consts::TAU);
        let speed = rng.range_f32(0.01, 0.05) * if index % 2 == 0 { 1.0 } else { -1.0 };
        let (sin, cos) = sin_cos(phase);
        entities.push(entity(&[
            pose(center + Vec2::new(cos, sin) * radius, phase),
            sprite("world", 0, 0xFFFFFFFF),
            format!(
                r#""ludenea::SpriteAnimation": (sheet: "{BUG_SHEET}", clip: "fly", started: {}, looping: true)"#,
                index % (FRAMES * TICKS_PER_FRAME) as usize
            ),
            format!(
                r#""swarm::Wanderer": (center: (x: {:?}, y: {:?}), radius: {radius:?}, phase: {phase:?}, speed: {speed:?})"#,
                center.x, center.y
            ),
        ]));
    }
    let mut text = format!(
        "Scene(\n    version: 2,\n    next_id: {},\n    resources: {{}},\n    entities: [\n",
        entities.len()
    );
    for (id, components) in entities.iter().enumerate() {
        writeln!(text, "        (id: {id}, components: {{{components}}}),")
            .expect("writing to a string cannot fail");
    }
    text.push_str("    ],\n)\n");
    text
}

/// One line: the components, comma separated.
fn entity(components: &[String]) -> String {
    components.join(", ")
}

fn pose(position: Vec2, angle: f32) -> String {
    let (sin, cos) = sin_cos(angle);
    format!(
        r#""ludenea::Pose": (position: (x: {:?}, y: {:?}), rotation: (cos: {cos:?}, sin: {sin:?}))"#,
        position.x, position.y
    )
}

fn sprite(layer: &str, order: i32, tint: u32) -> String {
    format!(
        r#""ludenea::Sprite": (texture: "{BUG_TEXTURE}", frame: 0, layer: "{layer}", order: {order}, flip_x: false, flip_y: false, tint: ({tint}))"#
    )
}

/// The sheet: four 16 by 16 frames in a row, a bug whose wings beat from frame to frame,
/// as a PNG.
fn generate_sheet_png() -> Vec<u8> {
    const BODY: [u8; 4] = [0x2E, 0x7D, 0x32, 0xFF];
    const HEAD: [u8; 4] = [0x1B, 0x3A, 0x1E, 0xFF];
    const WING: [u8; 4] = [0xF5, 0xF5, 0xDC, 0xFF];
    const WING_LIFT: [i32; FRAMES as usize] = [-3, -1, 2, -1];
    let width = FRAME * FRAMES;
    let mut pixels = vec![0u8; (width * FRAME * 4) as usize];
    let mut paint = |frame: u32, x: u32, y: u32, color: [u8; 4]| {
        let at = ((y * width + frame * FRAME + x) * 4) as usize;
        pixels[at..at + 4].copy_from_slice(&color);
    };
    for frame in 0..FRAMES {
        let lift = WING_LIFT[frame as usize];
        for y in 0..FRAME {
            for x in 0..FRAME {
                let (x, y) = (x as i32, y as i32);
                let color = if inside(x, y, 8, 4, 2, 2) {
                    Some(HEAD)
                } else if inside(x, y, 8, 9, 3, 5) {
                    Some(BODY)
                } else if inside(x, y, 3, 8 + lift, 3, 2) || inside(x, y, 12, 8 + lift, 3, 2) {
                    Some(WING)
                } else {
                    None
                };
                if let Some(color) = color {
                    paint(frame, x as u32, y as u32, color);
                }
            }
        }
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, FRAME);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // Stored blocks and no filter: the bytes do not depend on which compressor the png crate
    // picks for the features a build unifies, so the committed file is reproducible everywhere.
    encoder.set_compression(png::Compression::NoCompression);
    encoder.set_filter(png::Filter::NoFilter);
    let mut writer = encoder.write_header().expect("the PNG header is valid");
    writer
        .write_image_data(&pixels)
        .expect("the pixels match the declared size");
    writer.finish().expect("writing to memory cannot fail");
    bytes
}

/// Whether pixel (x, y) is within the ellipse centred on (cx, cy) with radii rx and ry.
fn inside(x: i32, y: i32, cx: i32, cy: i32, rx: i32, ry: i32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    dx * dx * ry * ry + dy * dy * rx * rx <= rx * rx * ry * ry
}

/// The text of `bug.sheet.ron`.
fn sheet_text() -> String {
    format!(
        r#"(texture: "{BUG_TEXTURE}", clips: {{"fly": (first: 0, count: {FRAMES}, ticks_per_frame: {TICKS_PER_FRAME})}})"#
    ) + "\n"
}

/// The text of `bug.png.meta`.
fn texture_meta_text() -> String {
    format!(
        "Meta(version: 2, id: \"{BUG_TEXTURE}\", kind: Texture((pixels_per_unit: 16, grid: Some((frame_width: {FRAME}, frame_height: {FRAME}, margin: 0, spacing: 0)), pivot: None, filter: Nearest)))\n"
    )
}

/// The text of `bug.sheet.ron.meta`.
fn sheet_meta_text() -> String {
    format!("Meta(version: 2, id: \"{BUG_SHEET}\", kind: Data(type: \"ludenea::SpriteSheet\"))\n")
}
