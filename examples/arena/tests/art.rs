use std::fs;
use std::path::{Path, PathBuf};

use ludenea::app::meta::{Filter, Meta};
use ludenea::app::project::project_textures;
use ludenea::render::SpriteSheet;
use ludenea::render::atlas::{self, TextureSource, frames_of};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// The largest atlas page, in pixels a side, the test packs into.
const MAX_PAGE: u32 = 4096;
const SHEETS: [&str; 8] = [
    "warrior-idle",
    "warrior-run",
    "warrior-jump",
    "warrior-fall",
    "hero-idle",
    "hero-run",
    "hero-jump",
    "hero-fall",
];

fn root() -> PathBuf {
    PathBuf::from(ROOT)
}

fn files_under(folder: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(folder).unwrap_or_else(|e| panic!("{}: {e}", folder.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn every_texture_packs_without_errors() {
    let loaded = project_textures(&root().join("assets"), Some(&root())).unwrap();
    assert!(loaded.failures.is_empty(), "{:?}", loaded.failures);
    assert_eq!(loaded.textures.len(), 11, "8 animations and 3 town images");
    let sources: Vec<_> = loaded
        .textures
        .iter()
        .map(|(id, path, png, meta)| TextureSource {
            id: *id,
            path: Some(path),
            png,
            meta,
        })
        .collect();
    let atlas = atlas::pack(&sources, MAX_PAGE);
    assert!(atlas.errors.is_empty(), "{:?}", atlas.errors);
}

#[test]
fn every_clip_fits_its_texture() {
    let loaded = project_textures(&root().join("assets"), Some(&root())).unwrap();
    assert!(loaded.failures.is_empty(), "{:?}", loaded.failures);
    for name in SHEETS {
        let text = fs::read_to_string(root().join(format!("assets/{name}.sheet.ron")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let sheet: SpriteSheet = ron::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (_, path, png, meta) = loaded
            .textures
            .iter()
            .find(|(id, ..)| *id == sheet.texture)
            .unwrap_or_else(|| panic!("{name} names a texture that does not exist"));
        let frames = frames_of(png, meta).unwrap_or_else(|e| panic!("{path}: {e}"));
        let clip_name = name.split('-').nth(1).unwrap();
        assert_eq!(
            sheet.clips.keys().collect::<Vec<_>>(),
            [clip_name],
            "{name}"
        );
        let clip = sheet.clips[clip_name];
        let frame_count = u32::try_from(frames.frames.len()).unwrap();
        assert!(
            frame_count >= clip.first + clip.count,
            "{name}: clip of {} frames from {} on a texture of {frame_count}",
            clip.count,
            clip.first
        );
        // One clip per sheet: it plays the whole strip, so a wrong frame width shows up.
        assert_eq!(
            (clip.first, clip.count),
            (0, frame_count),
            "{name} plays {} frames of a texture of {frame_count}",
            clip.count
        );
        let expected = if matches!(clip_name, "idle" | "run") {
            5
        } else {
            4
        };
        assert_eq!(clip.ticks_per_frame, expected, "{name}");
    }
}

#[test]
fn identifier_constants_match_the_metas() {
    for (name, id) in [
        ("warrior-idle", arena::WARRIOR_IDLE_SHEET),
        ("warrior-run", arena::WARRIOR_RUN_SHEET),
        ("warrior-jump", arena::WARRIOR_JUMP_SHEET),
        ("warrior-fall", arena::WARRIOR_FALL_SHEET),
        ("hero-idle", arena::HERO_IDLE_SHEET),
        ("hero-run", arena::HERO_RUN_SHEET),
        ("hero-jump", arena::HERO_JUMP_SHEET),
        ("hero-fall", arena::HERO_FALL_SHEET),
    ] {
        let meta = Meta::read(&root().join(format!("assets/{name}.sheet.ron.meta"))).unwrap();
        assert_eq!(format!("{}", meta.id), id, "{name}");
    }
    for (path, id) in [
        ("warrior/idle", arena::WARRIOR_IDLE_TEXTURE),
        ("warrior/run", arena::WARRIOR_RUN_TEXTURE),
        ("warrior/jump", arena::WARRIOR_JUMP_TEXTURE),
        ("warrior/fall", arena::WARRIOR_FALL_TEXTURE),
        ("hero/idle", arena::HERO_IDLE_TEXTURE),
        ("hero/run", arena::HERO_RUN_TEXTURE),
        ("hero/jump", arena::HERO_JUMP_TEXTURE),
        ("hero/fall", arena::HERO_FALL_TEXTURE),
        ("town/tileset", arena::TOWN_TILESET_TEXTURE),
        ("town/background", arena::TOWN_BACKGROUND_TEXTURE),
        ("town/middleground", arena::TOWN_MIDDLEGROUND_TEXTURE),
    ] {
        let meta = Meta::read(&root().join(format!("assets/art/{path}.png.meta"))).unwrap();
        assert_eq!(format!("{}", meta.id), id, "{path}");
    }
}

#[test]
fn credits_list_every_committed_art_file() {
    let credits = fs::read_to_string(root().join("CREDITS.md")).expect("CREDITS.md exists");
    let mut files = Vec::new();
    files_under(&root().join("assets/art"), &mut files);
    let pngs: Vec<_> = files
        .iter()
        .filter(|file| file.extension().is_some_and(|extension| extension == "png"))
        .collect();
    assert_eq!(pngs.len(), 11);
    for png in pngs {
        let relative = png.strip_prefix(root().join("assets")).unwrap();
        let relative = relative.to_string_lossy();
        assert!(credits.contains(&*relative), "CREDITS.md omits {relative}");
    }
    let packs: Vec<_> = credits.split("\n## ").skip(1).collect();
    assert_eq!(packs.len(), 3, "one entry per pack");
    for pack in packs {
        assert!(
            pack.contains("CC0"),
            "an entry does not state CC0: {}",
            pack.lines().next().unwrap_or_default()
        );
    }
}

/// Over the frames of an idle image, `frame` pixels wide: the lowest bottom edge of the opaque
/// pixels and the tallest figure, in pixels.
fn idle_extent(png: &[u8], frame: u32) -> (u32, u32) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba, "RGBA expected");
    let (width, height) = (info.width, info.height);
    let (mut bottom, mut tallest) = (0, 0);
    for first_column in (0..width).step_by(frame as usize) {
        let opaque_rows: Vec<u32> = (0..height)
            .filter(|&y| {
                (first_column..first_column + frame)
                    .any(|x| pixels[((y * width + x) * 4 + 3) as usize] > 0)
            })
            .collect();
        let (top, low) = (opaque_rows[0], opaque_rows[opaque_rows.len() - 1] + 1);
        bottom = bottom.max(low);
        tallest = tallest.max(low - top);
    }
    (bottom, tallest)
}

#[test]
fn metas_give_the_scale_the_pivot_and_the_sampling_the_game_needs() {
    let loaded = project_textures(&root().join("assets"), Some(&root())).unwrap();
    assert!(loaded.failures.is_empty(), "{:?}", loaded.failures);
    for (_, path, png, meta) in &loaded.textures {
        assert_eq!(meta.filter, Filter::Nearest, "{path}");
        if path.ends_with("town/tileset.png") {
            let grid = meta.grid.expect("the tileset has a grid");
            assert_eq!((grid.frame_width, grid.frame_height), (16, 16), "{path}");
            assert_eq!(meta.pixels_per_unit, 16, "a tile is one unit");
        }
        if path.ends_with("/idle.png") {
            let frame = meta.grid.expect("a fighter has a grid").frame_height;
            let (bottom, tallest) = idle_extent(png, frame);
            let pivot = meta.pivot.expect("a fighter has a pivot");
            assert_eq!(
                pivot.1, bottom,
                "{path}: the pivot is on the lowest opaque row"
            );
            let height = tallest as f32 / meta.pixels_per_unit as f32;
            assert!(
                (1.8..=2.2).contains(&height),
                "{path}: stands {height} units tall"
            );
        }
    }
}
