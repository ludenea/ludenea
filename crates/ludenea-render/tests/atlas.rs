use ludenea_app::meta::{Filter, Grid, TextureMeta};
use ludenea_ecs::AssetId;
use ludenea_render::atlas::{
    ATLAS_VERSION, Atlas, AtlasFileError, AtlasPage, Region, TextureError, TextureSource, pack,
};

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
    let atlas = pack(&[source(1, &image, &meta)], PAGE);

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
    let atlas = pack(&[source(1, &image, &meta)], PAGE);

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
    let atlas = pack(&[source(1, &image, &meta)], PAGE);

    assert_eq!(atlas.regions.len(), 2);
    assert_frame(&atlas, region(&atlas, 1, 0), 1, 1, 9);
    assert_frame(&atlas, region(&atlas, 1, 1), 4, 1, 9);
}

#[test]
fn frames_are_extruded_by_one_pixel() {
    let image = png(2, 2, 5);
    let meta = TextureMeta::default();
    let atlas = pack(&[source(1, &image, &meta)], PAGE);
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

    let first = pack(&sources, 12);
    let second = pack(&reordered, 12);

    assert_eq!(first, second);
    assert!(first.pages.len() > 1, "the small page splits the frames");
}

#[test]
fn full_page_opens_a_second_page() {
    // Six 4x4 frames, 6x6 once extruded: a 12x12 page holds four of them.
    let image = png(12, 8, 4);
    let meta = grid(4, 4, 0, 0);
    let atlas = pack(&[source(1, &image, &meta)], 12);

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
    let atlas = pack(&[source(1, &large, &meta), source(2, &fits, &meta)], 12);

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
    );

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
    );

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
    let error = pack(&[source(1, &image, &meta)], PAGE).errors;

    assert_eq!(
        error,
        [TextureError::Grid {
            id: AssetId(1),
            path: Some("art/hero.png".to_string()),
            image: (6, 2),
            grid: meta.grid.unwrap(),
        }]
    );
    let message = error[0].to_string();
    for part in [
        "2x2 frames",
        "margin 0",
        "spacing 1",
        "6x2 image",
        "whole frames",
    ] {
        assert!(message.contains(part), "{message} names {part}");
    }
}

#[test]
fn margin_leaving_a_partial_frame_is_refused() {
    // A margin of 1 on both sides leaves 5 pixels: two 2-pixel frames and a partial third.
    let image = png(7, 2, 1);
    let meta = grid(2, 2, 1, 0);
    let error = pack(&[source(1, &image, &meta)], PAGE).errors;

    assert_eq!(
        error,
        [TextureError::Grid {
            id: AssetId(1),
            path: Some("art/hero.png".to_string()),
            image: (7, 2),
            grid: meta.grid.unwrap(),
        }]
    );
}

#[test]
fn adjacent_frames_do_not_bleed_into_each_other_extrusion() {
    // Two 2x2 frames side by side in the image: each extruded border repeats its own frame's
    // edge, never the neighbouring frame's pixels.
    let image = png(4, 2, 8);
    let meta = grid(2, 2, 0, 0);
    let atlas = pack(&[source(1, &image, &meta)], PAGE);

    for (frame, left) in [(0, 0), (1, 2)] {
        let region = region(&atlas, 1, frame);
        let page = &atlas.pages[region.page as usize];
        for y in 0..4 {
            for x in 0..4 {
                let nearest = (left + x.clamp(1, 2) - 1, y.clamp(1, 2) - 1);
                assert_eq!(
                    pixel(page, region.x + x - 1, region.y + y - 1),
                    [nearest.0 as u8, nearest.1 as u8, 8, 255],
                    "frame {frame}, extruded pixel ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn invalid_meta_is_refused_naming_the_field() {
    let image = png(2, 2, 1);
    let meta = TextureMeta {
        pixels_per_unit: 0,
        ..TextureMeta::default()
    };
    let error = pack(&[source(1, &image, &meta)], PAGE).errors;

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
fn textures_sharing_an_identifier_are_all_refused() {
    let image = png(2, 2, 1);
    let other = png(2, 2, 2);
    let meta = TextureMeta::default();
    let first = TextureSource {
        path: Some("art/b.png"),
        ..source(1, &image, &meta)
    };
    let second = TextureSource {
        path: Some("art/a.png"),
        ..source(1, &image, &meta)
    };
    let atlas = pack(&[first, source(2, &other, &meta), second], PAGE);

    let duplicate = |path: &str| TextureError::Duplicate {
        id: AssetId(1),
        path: Some(path.to_string()),
    };
    assert_eq!(
        atlas.errors,
        [duplicate("art/a.png"), duplicate("art/b.png")]
    );
    let message = atlas.errors[0].to_string();
    for part in [&AssetId(1).to_string(), "art/a.png", "identifier"] {
        assert!(message.contains(part), "{message} names {part}");
    }
    assert_eq!(
        atlas.regions.keys().copied().collect::<Vec<_>>(),
        [(AssetId(2), 0)]
    );
}

#[test]
fn refused_textures_are_sorted_by_identifier() {
    let image = png(2, 2, 1);
    let broken = b"not a png".to_vec();
    let meta = TextureMeta::default();
    let sources = [
        source(3, &broken, &meta),
        source(1, &image, &meta),
        source(2, &broken, &meta),
    ];
    let reordered = [sources[2], sources[1], sources[0]];

    let atlas = pack(&sources, PAGE);
    let ids: Vec<AssetId> = atlas.errors.iter().map(TextureError::id).collect();
    assert_eq!(ids, [AssetId(2), AssetId(3)]);
    assert_eq!(atlas, pack(&reordered, PAGE));
}

#[test]
fn every_texture_refused_leaves_an_empty_atlas_with_the_errors() {
    let broken = b"not a png".to_vec();
    let meta = TextureMeta::default();
    let atlas = pack(&[source(1, &broken, &meta)], PAGE);
    assert!(atlas.pages.is_empty() && atlas.regions.is_empty());
    assert!(
        matches!(atlas.errors[..], [TextureError::Decode { .. }]),
        "{:?}",
        atlas.errors
    );
}

#[test]
fn no_sources_pack_into_an_empty_atlas() {
    let atlas = pack(&[], PAGE);
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
    let atlas = pack(&[source(1, &gray, &meta), source(2, &indexed, &meta)], PAGE);

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

#[test]
fn huge_spacing_and_margin_with_one_frame_do_not_panic() {
    let image = png(4, 4, 6);
    let spacing = grid(4, 4, 0, u32::MAX);
    let atlas = pack(&[source(1, &image, &spacing)], PAGE);
    assert_frame(&atlas, region(&atlas, 1, 0), 0, 0, 6);

    let margin = grid(4, 4, u32::MAX, 0);
    let error = pack(&[source(1, &image, &margin)], PAGE).errors;
    assert!(
        matches!(error[..], [TextureError::Grid { .. }]),
        "{error:?}"
    );
}

#[test]
fn png_declaring_huge_dimensions_is_refused_without_allocating() {
    // A valid signature and IHDR declaring 60000x60000 RGBA, then an empty IDAT and IEND.
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut crc = crc32(kind);
        crc = data.iter().fold(crc, |crc, &byte| update(crc, byte));
        let mut bytes = (data.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(&(!crc).to_be_bytes());
        bytes
    }
    fn update(crc: u32, byte: u8) -> u32 {
        (0..8).fold(crc ^ u32::from(byte), |crc, _| {
            if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            }
        })
    }
    fn crc32(bytes: &[u8]) -> u32 {
        bytes.iter().fold(u32::MAX, |crc, &byte| update(crc, byte))
    }
    let mut header = 60_000u32.to_be_bytes().to_vec();
    header.extend_from_slice(&60_000u32.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend(chunk(b"IHDR", &header));
    bytes.extend(chunk(b"IDAT", &[]));
    bytes.extend(chunk(b"IEND", &[]));

    let meta = TextureMeta::default();
    let error = pack(&[source(1, &bytes, &meta)], PAGE).errors;
    match &error[..] {
        [TextureError::Decode { id, reason, .. }] => {
            assert_eq!(*id, AssetId(1));
            assert!(reason.contains("60000x60000"), "{reason}");
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
}

fn sheet() -> Atlas {
    let image = png(8, 4, 3);
    let other = png(5, 5, 9);
    let atlas = pack(
        &[
            source(1, &image, &grid(4, 4, 0, 0)),
            source(2, &other, &TextureMeta::default()),
        ],
        PAGE,
    );
    assert!(atlas.errors.is_empty());
    atlas
}

fn manifest_with(edit: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut manifest: serde_json::Value =
        serde_json::from_str(&sheet().to_files().manifest).unwrap();
    edit(&mut manifest);
    manifest.to_string()
}

fn page_refs(pages: &[Vec<u8>]) -> Vec<&[u8]> {
    pages.iter().map(Vec::as_slice).collect()
}

#[test]
fn atlas_round_trips_through_files() {
    let atlas = sheet();
    let files = atlas.to_files();
    assert!(files.manifest.contains("page-0.png"));
    let read = Atlas::from_files(&files.manifest, &page_refs(&files.pages)).unwrap();
    assert_eq!(read, atlas);
}

#[test]
fn to_files_is_reproducible() {
    let first = sheet().to_files();
    let second = sheet().to_files();
    assert_eq!(first, second);
}

#[test]
fn newer_version_is_refused_as_a_version() {
    // The rest of the manifest is not even valid: the version is read first.
    let manifest = format!(r#"{{"version": {}, "pages": "?"}}"#, ATLAS_VERSION + 1);
    assert_eq!(
        Atlas::from_files(&manifest, &[]),
        Err(AtlasFileError::Version {
            found: ATLAS_VERSION + 1,
            supported: ATLAS_VERSION
        })
    );
}

#[test]
fn missing_page_is_refused() {
    let files = sheet().to_files();
    assert_eq!(
        Atlas::from_files(&files.manifest, &[]),
        Err(AtlasFileError::MissingPage { index: 0 })
    );
}

#[test]
fn page_of_another_size_is_refused_naming_both_sizes() {
    let files = sheet().to_files();
    let (width, height) = (sheet().pages[0].width, sheet().pages[0].height);
    let manifest = manifest_with(|manifest| manifest["pages"][0]["width"] = (width + 1).into());
    let error = Atlas::from_files(&manifest, &page_refs(&files.pages)).unwrap_err();
    assert_eq!(
        error,
        AtlasFileError::PageSize {
            index: 0,
            expected: (width + 1, height),
            found: (width, height)
        }
    );
    let message = error.to_string();
    assert!(
        message.contains(&format!("{}x{height}", width + 1))
            && message.contains(&format!("{width}x{height}"))
    );
}

#[test]
fn region_outside_its_page_is_refused() {
    let files = sheet().to_files();
    let width = sheet().pages[0].width;
    for edit in [
        (|m: &mut serde_json::Value, w: u32| m["regions"][0]["region"]["x"] = w.into())
            as fn(&mut _, u32),
        |m, _| m["regions"][0]["region"]["page"] = 7.into(),
        |m, _| m["regions"][0]["region"]["width"] = 0.into(),
    ] {
        let manifest = manifest_with(|manifest| edit(manifest, width));
        assert!(
            matches!(
                Atlas::from_files(&manifest, &page_refs(&files.pages)),
                Err(AtlasFileError::RegionOutside { .. })
            ),
            "{manifest}"
        );
    }
}

#[test]
fn malformed_files_are_refused_without_panicking() {
    let files = sheet().to_files();
    assert!(matches!(
        Atlas::from_files("not json", &[]),
        Err(AtlasFileError::Json(_))
    ));
    assert!(matches!(
        Atlas::from_files(r#"{"version": 0}"#, &[]),
        Err(AtlasFileError::Json(_))
    ));
    assert!(matches!(
        Atlas::from_files(&files.manifest, &[b"not a png".as_slice()]),
        Err(AtlasFileError::Decode { index: 0, .. })
    ));
    let twice = manifest_with(|m| {
        let first = m["regions"][0].clone();
        m["regions"].as_array_mut().unwrap().push(first);
    });
    assert!(matches!(
        Atlas::from_files(&twice, &page_refs(&files.pages)),
        Err(AtlasFileError::Json(_))
    ));
}

/// FNV-1a over the manifest and every page, to pin the exact bytes without a hash dependency.
fn digest(files: &ludenea_render::atlas::AtlasFiles) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in files
        .manifest
        .bytes()
        .chain(files.pages.iter().flatten().copied())
    {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn files_bytes_are_pinned() {
    // A change here means the PNG encoder or the manifest layout changed: the files of every
    // shipped build change with it, so decide deliberately and update the pinned value.
    let files = sheet().to_files();
    assert_eq!(
        digest(&files),
        16_329_721_634_297_886_507,
        "{} manifest bytes, page lengths {:?}",
        files.manifest.len(),
        files.pages.iter().map(Vec::len).collect::<Vec<_>>()
    );
}

#[test]
fn region_with_no_pixels_per_unit_is_refused() {
    let files = sheet().to_files();
    let manifest = manifest_with(|m| m["regions"][0]["region"]["pixels_per_unit"] = 0.into());
    assert!(matches!(
        Atlas::from_files(&manifest, &page_refs(&files.pages)),
        Err(AtlasFileError::Json(message)) if message.contains("pixels_per_unit")
    ));
}

#[test]
fn region_overflowing_by_one_pixel_is_refused_and_a_fitting_one_is_read() {
    let files = sheet().to_files();
    let page = &sheet().pages[0];
    let (width, height) = (page.width, page.height);
    let region = sheet().regions[&(AssetId(1), 0)];
    for (field, value) in [
        ("x", width - region.width + 1),
        ("y", height - region.height + 1),
    ] {
        let manifest = manifest_with(|m| m["regions"][0]["region"][field] = value.into());
        assert!(
            matches!(
                Atlas::from_files(&manifest, &page_refs(&files.pages)),
                Err(AtlasFileError::RegionOutside { .. })
            ),
            "{field}"
        );
        let fitting = manifest_with(|m| m["regions"][0]["region"][field] = (value - 1).into());
        assert!(
            Atlas::from_files(&fitting, &page_refs(&files.pages)).is_ok(),
            "{field}"
        );
    }
}

#[test]
fn second_page_missing_is_refused_naming_it() {
    let big = png(40, 40, 1);
    let atlas = pack(&[source(1, &big, &TextureMeta::default())], 64);
    let mut two = atlas.clone();
    two.pages.push(two.pages[0].clone());
    let files = two.to_files();
    assert_eq!(
        Atlas::from_files(&files.manifest, &page_refs(&files.pages[..1])),
        Err(AtlasFileError::MissingPage { index: 1 })
    );
}

#[test]
fn page_file_name_must_match_its_position() {
    let files = sheet().to_files();
    let manifest = manifest_with(|m| m["pages"][0]["file"] = "other.png".into());
    assert!(matches!(
        Atlas::from_files(&manifest, &page_refs(&files.pages)),
        Err(AtlasFileError::Json(message)) if message.contains("page-0.png")
    ));
}

#[test]
fn refused_textures_are_not_written() {
    let good = png(4, 4, 1);
    let bad = png(3, 3, 2);
    let atlas = pack(
        &[
            source(1, &good, &TextureMeta::default()),
            source(2, &bad, &grid(2, 2, 0, 0)),
        ],
        PAGE,
    );
    assert_eq!(atlas.errors.len(), 1);
    let files = atlas.to_files();
    let read = Atlas::from_files(&files.manifest, &page_refs(&files.pages)).unwrap();
    assert_eq!(read.regions.keys().collect::<Vec<_>>(), [&(AssetId(1), 0)]);
    assert!(read.errors.is_empty());
}
