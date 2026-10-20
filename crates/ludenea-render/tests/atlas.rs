use ludenea_app::meta::{Filter, Grid, TextureMeta};
use ludenea_ecs::AssetId;
use ludenea_render::atlas::{Atlas, AtlasPage, Region, TextureError, TextureSource, pack};

const PAGE: u32 = 4096;

/// A `width` by `height` RGBA PNG whose pixel at `(x, y)` is `[x, y, tag, 255]`, so a test can
/// tell where every atlas pixel came from.
fn png(width: u32, height: u32, tag: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let pixels: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, tag, 255]))
        .collect();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    bytes
}

fn grid(frame_width: u32, frame_height: u32, margin: u32, spacing: u32) -> TextureMeta {
    TextureMeta {
        grid: Some(Grid {
            frame_width,
            frame_height,
            margin,
            spacing,
        }),
        ..TextureMeta::default()
    }
}

fn source<'a>(id: u128, png: &'a [u8], meta: &'a TextureMeta) -> TextureSource<'a> {
    TextureSource {
        id: AssetId(id),
        path: Some("art/hero.png"),
        png,
        meta,
    }
}

fn pixel(page: &AtlasPage, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * page.width + x) * 4) as usize;
    page.rgba[start..start + 4].try_into().unwrap()
}

fn region(atlas: &Atlas, id: u128, frame: u32) -> Region {
    atlas.regions[&(AssetId(id), frame)]
}

/// Asserts the region shows the source pixels from `(left, top)` in the image tagged `tag`.
fn assert_frame(atlas: &Atlas, region: Region, left: u32, top: u32, tag: u8) {
    let page = &atlas.pages[region.page as usize];
    for y in 0..region.height {
        for x in 0..region.width {
            assert_eq!(
                pixel(page, region.x + x, region.y + y),
                [(left + x) as u8, (top + y) as u8, tag, 255],
                "pixel ({x}, {y}) of the region"
            );
        }
    }
}

#[test]
fn single_image_is_one_frame() {
    let image = png(3, 5, 7);
    let meta = TextureMeta::default();
    let atlas = pack(&[source(1, &image, &meta)], PAGE).unwrap();

    assert_eq!(atlas.regions.len(), 1);
    assert!(atlas.errors.is_empty());
    let region = region(&atlas, 1, 0);
    assert_eq!((region.width, region.height), (3, 5));
    assert_eq!(
        region.pivot,
        (1, 2),
        "the default pivot is the frame centre"
    );
    assert_eq!(region.pixels_per_unit, 16);
    assert_eq!(region.filter, Filter::Nearest);
    assert_frame(&atlas, region, 0, 0, 7);
}

#[test]
fn grid_cuts_frames_row_major() {
    let image = png(4, 2, 3);
    let meta = TextureMeta {
        pivot: Some((1, 0)),
        pixels_per_unit: 32,
        filter: Filter::Linear,
        ..grid(2, 1, 0, 0)
    };
    let atlas = pack(&[source(1, &image, &meta)], PAGE).unwrap();

    assert_eq!(atlas.regions.len(), 4);
    for (frame, (left, top)) in [(0, 0), (2, 0), (0, 1), (2, 1)].into_iter().enumerate() {
        let region = region(&atlas, 1, frame as u32);
        assert_eq!((region.width, region.height), (2, 1));
        assert_eq!(region.pivot, (1, 0));
        assert_eq!(region.pixels_per_unit, 32);
        assert_eq!(region.filter, Filter::Linear);
        assert_frame(&atlas, region, left, top, 3);
    }
}

#[test]
fn margins_and_spacing_are_honoured() {
    // margin 1, two 2x2 frames 1 pixel apart: 1 + 2 + 1 + 2 + 1 = 7 wide, 1 + 2 + 1 = 4 high.
    let image = png(7, 4, 9);
    let meta = grid(2, 2, 1, 1);
    let atlas = pack(&[source(1, &image, &meta)], PAGE).unwrap();

    assert_eq!(atlas.regions.len(), 2);
    assert_frame(&atlas, region(&atlas, 1, 0), 1, 1, 9);
    assert_frame(&atlas, region(&atlas, 1, 1), 4, 1, 9);
}

#[test]
fn frames_are_extruded_by_one_pixel() {
    let image = png(2, 2, 5);
    let meta = TextureMeta::default();
    let atlas = pack(&[source(1, &image, &meta)], PAGE).unwrap();
    let region = region(&atlas, 1, 0);
    let page = &atlas.pages[region.page as usize];
    assert!(region.x >= 1 && region.y >= 1, "room for the extrusion");

    // Every pixel of the 4x4 extruded rectangle repeats the nearest frame pixel.
    for y in 0..4 {
        for x in 0..4 {
            let nearest = (x.clamp(1, 2) - 1, y.clamp(1, 2) - 1);
            assert_eq!(
                pixel(page, region.x + x - 1, region.y + y - 1),
                [nearest.0 as u8, nearest.1 as u8, 5, 255],
                "extruded pixel ({x}, {y})"
            );
        }
    }
}

#[test]
fn packing_is_reproducible_byte_for_byte() {
    let tall = png(3, 6, 1);
    let sheet = png(8, 4, 2);
    let small = png(2, 2, 3);
    let single = TextureMeta::default();
    let sheet_meta = grid(2, 2, 0, 0);
    let sources = [
        source(10, &tall, &single),
        source(20, &sheet, &sheet_meta),
        source(30, &small, &single),
    ];
    let reordered = [sources[2], sources[0], sources[1]];

    let first = pack(&sources, 12).unwrap();
    let second = pack(&reordered, 12).unwrap();

    assert_eq!(first.regions, second.regions);
    assert_eq!(first.pages, second.pages);
    assert!(first.pages.len() > 1, "the small page splits the frames");
}

#[test]
fn full_page_opens_a_second_page() {
    // Six 4x4 frames, 6x6 once extruded: a 12x12 page holds four of them.
    let image = png(12, 8, 4);
    let meta = grid(4, 4, 0, 0);
    let atlas = pack(&[source(1, &image, &meta)], 12).unwrap();

    assert_eq!(atlas.pages.len(), 2);
    for page in &atlas.pages {
        assert!(page.width <= 12 && page.height <= 12);
        assert_eq!(page.rgba.len(), (page.width * page.height * 4) as usize);
    }
    let pages: Vec<u32> = (0..6).map(|frame| region(&atlas, 1, frame).page).collect();
    assert_eq!(pages, [0, 0, 0, 0, 1, 1]);
    for frame in 0..6 {
        let left = frame % 3 * 4;
        let top = frame / 3 * 4;
        assert_frame(&atlas, region(&atlas, 1, frame), left, top, 4);
    }
}

#[test]
fn texture_larger_than_a_page_is_too_large() {
    let large = png(11, 4, 1);
    let fits = png(10, 10, 2);
    let meta = TextureMeta::default();
    let atlas = pack(&[source(1, &large, &meta), source(2, &fits, &meta)], 12).unwrap();

    assert_eq!(
        atlas.errors,
        [TextureError::TooLarge {
            id: AssetId(1),
            path: Some("art/hero.png".to_string()),
            size: (11, 4),
            max: 12,
        }]
    );
    let message = atlas.errors[0].to_string();
    for part in [&AssetId(1).to_string(), "art/hero.png", "11x4", "12"] {
        assert!(message.contains(part), "{message} names {part}");
    }
    assert_frame(&atlas, region(&atlas, 2, 0), 0, 0, 2);
}

#[test]
fn truncated_png_is_refused_and_others_still_pack() {
    let valid = png(4, 4, 1);
    let truncated = valid[..valid.len() / 2].to_vec();
    let mut wrong_signature = valid.clone();
    wrong_signature[1] = b'X';
    let meta = TextureMeta::default();
    let unnamed = TextureSource {
        path: None,
        ..source(3, &wrong_signature, &meta)
    };
    let atlas = pack(
        &[
            source(1, &valid, &meta),
            source(2, &truncated, &meta),
            unnamed,
        ],
        PAGE,
    )
    .unwrap();

    assert_eq!(atlas.regions.len(), 1);
    assert_frame(&atlas, region(&atlas, 1, 0), 0, 0, 1);
    assert_eq!(atlas.errors.len(), 2);
    match &atlas.errors[0] {
        TextureError::Decode { id, path, .. } => {
            assert_eq!((*id, path.as_deref()), (AssetId(2), Some("art/hero.png")));
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
    let message = atlas.errors[0].to_string();
    assert!(message.contains(&AssetId(2).to_string()), "{message}");
    assert!(message.contains("art/hero.png"), "{message}");
    match &atlas.errors[1] {
        TextureError::Decode { id, path, .. } => assert_eq!((*id, path), (AssetId(3), &None)),
        other => panic!("expected a decode error, got {other:?}"),
    }
    assert!(
        atlas.errors[1]
            .to_string()
            .contains(&AssetId(3).to_string())
    );
}

#[test]
fn grid_larger_than_image_is_refused_naming_sizes() {
    let image = png(4, 4, 1);
    let meta = grid(8, 8, 0, 0);
    let other = png(2, 2, 2);
    let single = TextureMeta::default();
    let atlas = pack(
        &[source(1, &image, &meta), source(2, &other, &single)],
        PAGE,
    )
    .unwrap();

    assert_eq!(
        atlas.errors,
        [TextureError::Grid {
            id: AssetId(1),
            path: Some("art/hero.png".to_string()),
            image: (4, 4),
            grid: meta.grid.unwrap(),
        }]
    );
    let message = atlas.errors[0].to_string();
    for part in [&AssetId(1).to_string(), "art/hero.png", "4x4", "8x8"] {
        assert!(message.contains(part), "{message} names {part}");
    }
    assert_eq!(atlas.regions.len(), 1);
}

#[test]
fn partial_last_frame_is_refused() {
    // 2x2 frames 1 pixel apart cut 5 pixels into two whole frames; 6 leaves a partial third.
    let image = png(6, 2, 1);
    let meta = grid(2, 2, 0, 1);
    let error = pack(&[source(1, &image, &meta)], PAGE).unwrap_err();

    assert_eq!(
        error,
        [TextureError::Grid {
            id: AssetId(1),
            path: Some("art/hero.png".to_string()),
            image: (6, 2),
            grid: meta.grid.unwrap(),
        }]
    );
}

#[test]
fn invalid_meta_is_refused_naming_the_field() {
    let image = png(2, 2, 1);
    let meta = TextureMeta {
        pixels_per_unit: 0,
        ..TextureMeta::default()
    };
    let error = pack(&[source(1, &image, &meta)], PAGE).unwrap_err();

    assert_eq!(
        error,
        [TextureError::Meta {
            id: AssetId(1),
            path: Some("art/hero.png".to_string()),
            reason: "pixels_per_unit must be at least 1".to_string(),
        }]
    );
    assert!(error[0].to_string().contains("pixels_per_unit"));
}

#[test]
fn no_sources_pack_into_an_empty_atlas() {
    let atlas = pack(&[], PAGE).unwrap();
    assert!(atlas.pages.is_empty() && atlas.regions.is_empty() && atlas.errors.is_empty());
}

#[test]
fn grayscale_and_palette_pngs_decode_to_rgba() {
    let encode = |color, data: &[u8], palette: Option<Vec<u8>>| {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        if let Some(palette) = palette {
            encoder.set_palette(palette);
        }
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        bytes
    };
    let gray = encode(png::ColorType::Grayscale, &[10, 200], None);
    let indexed = encode(
        png::ColorType::Indexed,
        &[1, 0],
        Some(vec![1, 2, 3, 4, 5, 6]),
    );
    let meta = TextureMeta::default();
    let atlas = pack(&[source(1, &gray, &meta), source(2, &indexed, &meta)], PAGE).unwrap();

    let colors = |id| {
        let region = region(&atlas, id, 0);
        let page = &atlas.pages[region.page as usize];
        [
            pixel(page, region.x, region.y),
            pixel(page, region.x + 1, region.y),
        ]
    };
    assert_eq!(colors(1), [[10, 10, 10, 255], [200, 200, 200, 255]]);
    assert_eq!(colors(2), [[4, 5, 6, 255], [1, 2, 3, 255]]);
}
