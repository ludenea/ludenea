//! How a texture's meta cuts its PNG into frames, without packing it: what the editor's
//! texture inspector and tile palette show, and the same cut `pack` makes.

use ludenea_app::meta::{Grid, TextureMeta};
use ludenea_ecs::AssetId;
use ludenea_render::atlas::{
    FrameRect, FramesError, TextureFrames, TextureSource, frames_of, pack,
};

/// A `width` by `height` RGBA PNG whose pixel at `(x, y)` is `[x, y, 7, 255]`.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let pixels: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, 7, 255]))
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

#[test]
fn grid_frames_match_what_pack_cuts() {
    // 4 by 4 frames, 1 pixel in from the edges and 1 apart: two columns and two rows.
    let image = png(11, 11);
    let meta = TextureMeta {
        pivot: Some((1, 3)),
        ..grid(4, 4, 1, 1)
    };

    let frames = frames_of(&image, &meta).unwrap();
    let rect = |x, y| FrameRect {
        x,
        y,
        width: 4,
        height: 4,
        pivot: (1, 3),
    };
    assert_eq!(
        frames,
        TextureFrames {
            width: 11,
            height: 11,
            frames: vec![rect(1, 1), rect(6, 1), rect(1, 6), rect(6, 6)],
        }
    );

    // `pack` cuts the same frames, in the same order: each region holds its frame's pixels.
    let atlas = pack(
        &[TextureSource {
            id: AssetId(1),
            path: None,
            png: &image,
            meta: &meta,
        }],
        4096,
    );
    assert_eq!(atlas.regions.len(), frames.frames.len());
    for (index, frame) in (0..).zip(&frames.frames) {
        let region = atlas.regions[&(AssetId(1), index)];
        assert_eq!(
            (region.width, region.height, region.pivot),
            (frame.width, frame.height, frame.pivot)
        );
        let page = &atlas.pages[region.page as usize];
        let start = ((region.y * page.width + region.x) * 4) as usize;
        assert_eq!(
            page.rgba[start..start + 4],
            [frame.x as u8, frame.y as u8, 7, 255],
            "frame {index}"
        );
    }
}

#[test]
fn single_image_is_one_frame_with_the_meta_pivot() {
    let image = png(6, 7);
    let pivoted = TextureMeta {
        pivot: Some((1, 5)),
        ..TextureMeta::default()
    };
    assert_eq!(
        frames_of(&image, &pivoted).unwrap(),
        TextureFrames {
            width: 6,
            height: 7,
            frames: vec![FrameRect {
                x: 0,
                y: 0,
                width: 6,
                height: 7,
                pivot: (1, 5),
            }],
        }
    );
    // Without a pivot, the frame's centre.
    let centred = frames_of(&image, &TextureMeta::default()).unwrap();
    assert_eq!(centred.frames[0].pivot, (3, 3));
}

#[test]
fn frames_of_a_broken_png_name_the_cause() {
    let error = frames_of(&[1, 2, 3], &TextureMeta::default()).unwrap_err();
    match &error {
        FramesError::Decode { reason } => assert!(!reason.is_empty()),
        other => panic!("expected a decode error, got {other:?}"),
    }
    assert!(
        error.to_string().starts_with("cannot decode the PNG: "),
        "{error}"
    );
}

#[test]
fn a_grid_that_does_not_cut_the_image_names_the_sizes() {
    let image = png(6, 2);
    let error = frames_of(&image, &grid(4, 2, 0, 0)).unwrap_err();
    assert!(matches!(error, FramesError::Grid { .. }), "{error:?}");
    let message = error.to_string();
    for part in [
        "4x2 frames",
        "margin 0",
        "spacing 0",
        "6x2 image",
        "whole frames",
    ] {
        assert!(message.contains(part), "{message} names {part}");
    }
    // A frame side of 0 cuts nothing, and is refused rather than divided by.
    for meta in [grid(0, 2, 0, 0), grid(2, 0, 0, 0)] {
        let error = frames_of(&image, &meta).unwrap_err();
        assert!(matches!(error, FramesError::Grid { .. }), "{error:?}");
    }
}
