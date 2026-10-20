//! The wgpu renderer, drawn offscreen and read back pixel by pixel. These tests need a GPU
//! adapter (hardware or software); without one they fail naming what was tried.
#![cfg(feature = "gpu")]

use ludenea_app::meta::{Filter, Grid, TextureMeta};
use ludenea_draw::Color;
use ludenea_draw::frame::{CameraView, LayerInfo, RenderFrame, SpriteItem};
use ludenea_ecs::{AssetId, Entity, World};
use ludenea_math::{Rot, Vec2};
use ludenea_render::atlas::{Atlas, AtlasPage, TextureSource, pack};
use ludenea_render::gpu::{
    BACKGROUND, DrawReport, Frames, Overlay, RenderError, Renderer, Size, headless,
};

const SIZE: Size = Size {
    width: 64,
    height: 64,
};
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];

fn device() -> (wgpu::Device, wgpu::Queue) {
    pollster::block_on(headless()).unwrap_or_else(|error| panic!("{error}"))
}

fn renderer() -> Renderer {
    let (device, queue) = device();
    Renderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm)
}

fn background() -> [u8; 4] {
    BACKGROUND.0.to_be_bytes()
}

/// A `width` by `height` RGBA PNG whose pixel at `(x, y)` is `paint(x, y)`.
fn png(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let paint = &paint;
    let pixels: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| paint(x, y)))
        .collect();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    bytes
}

/// Packs one texture as `AssetId(1)` and uploads it.
fn upload(renderer: &mut Renderer, png: &[u8], meta: &TextureMeta) -> Atlas {
    let sources = [TextureSource {
        id: AssetId(1),
        path: Some("art/test.png"),
        png,
        meta,
    }];
    let atlas = pack(&sources, renderer.max_page_side()).unwrap();
    assert!(atlas.errors.is_empty(), "{:?}", atlas.errors);
    renderer.set_atlas(&atlas).unwrap();
    atlas
}

fn entity() -> Entity {
    World::new().spawn()
}

fn sprite(entity: Entity, position: Vec2) -> SpriteItem {
    SpriteItem {
        entity,
        layer: 0,
        order: 0,
        texture: AssetId(1),
        frame: 0,
        position,
        rotation: Rot::IDENTITY,
        flip_x: false,
        flip_y: false,
        tint: Color(0xFFFFFFFF),
    }
}

/// A frame whose camera shows 4 by 4 world units around the origin: 16 pixels per unit on a
/// 64 by 64 target, the origin at pixel (32, 32), +y up.
fn frame(sprites: Vec<SpriteItem>) -> RenderFrame {
    RenderFrame {
        tick: 0,
        camera: Some(CameraView {
            center: Vec2::ZERO,
            view: Vec2::new(4.0, 4.0),
        }),
        layers: vec![LayerInfo {
            name: "world".into(),
            parallax: 1.0,
        }],
        sprites,
        tilemaps: Vec::new(),
        teleported: Vec::new(),
        debug: Default::default(),
        warnings: Vec::new(),
    }
}

fn render(renderer: &mut Renderer, size: Size, frame: &RenderFrame) -> Vec<u8> {
    let frames = Frames {
        previous: None,
        current: frame,
        alpha: 1.0,
    };
    let pixels = renderer
        .render_rgba(size, frames, &Overlay::default())
        .unwrap();
    assert_eq!(pixels.len(), (size.width * size.height * 4) as usize);
    pixels
}

fn pixel(pixels: &[u8], size: Size, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * size.width + x) * 4) as usize;
    pixels[start..start + 4].try_into().unwrap()
}

fn solid(color: [u8; 4]) -> Vec<u8> {
    png(16, 16, |_, _| color)
}

#[test]
fn clears_to_the_background() {
    let mut renderer = renderer();
    let pixels = render(&mut renderer, SIZE, &frame(Vec::new()));
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            assert_eq!(pixel(&pixels, SIZE, x, y), background(), "at ({x}, {y})");
        }
    }
}

#[test]
fn draws_a_sprite_at_its_pose() {
    let mut renderer = renderer();
    upload(&mut renderer, &solid(RED), &TextureMeta::default());
    // One world unit (16 pixels at 16 pixels per unit) centred on (1, 1): pixels 40..56 across
    // and 8..24 down.
    let pixels = render(
        &mut renderer,
        SIZE,
        &frame(vec![sprite(entity(), Vec2::new(1.0, 1.0))]),
    );
    for (x, y) in [(40, 8), (55, 8), (40, 23), (55, 23), (48, 16)] {
        assert_eq!(pixel(&pixels, SIZE, x, y), RED, "inside at ({x}, {y})");
    }
    for (x, y) in [(39, 16), (56, 16), (48, 7), (48, 24), (16, 48)] {
        assert_eq!(
            pixel(&pixels, SIZE, x, y),
            background(),
            "outside at ({x}, {y})"
        );
    }
}

#[test]
fn rotation_turns_the_sprite_around_its_pivot() {
    let mut renderer = renderer();
    // Two units wide, one unit tall, centred on the origin; a quarter turn makes it one unit
    // wide and two units tall.
    let meta = TextureMeta::default();
    let texture = png(32, 16, |_, _| RED);
    upload(&mut renderer, &texture, &meta);
    let mut turned = sprite(entity(), Vec2::ZERO);
    turned.rotation = Rot { cos: 0.0, sin: 1.0 };
    let pixels = render(&mut renderer, SIZE, &frame(vec![turned]));
    assert_eq!(pixel(&pixels, SIZE, 32, 17), RED);
    assert_eq!(pixel(&pixels, SIZE, 32, 46), RED);
    assert_eq!(pixel(&pixels, SIZE, 18, 32), background());
    assert_eq!(pixel(&pixels, SIZE, 45, 32), background());
}

#[test]
fn pivot_sits_on_the_position() {
    let mut renderer = renderer();
    // The pivot at the frame's bottom-left corner: the sprite covers world [0, 1] x [0, 1],
    // pixels 32..48 across and 16..32 down.
    let meta = TextureMeta {
        pivot: Some((0, 16)),
        ..TextureMeta::default()
    };
    upload(&mut renderer, &solid(RED), &meta);
    let pixels = render(
        &mut renderer,
        SIZE,
        &frame(vec![sprite(entity(), Vec2::ZERO)]),
    );
    for (x, y) in [(32, 16), (47, 16), (32, 31), (47, 31)] {
        assert_eq!(pixel(&pixels, SIZE, x, y), RED, "inside at ({x}, {y})");
    }
    for (x, y) in [(31, 24), (48, 24), (40, 15), (40, 32)] {
        assert_eq!(
            pixel(&pixels, SIZE, x, y),
            background(),
            "outside at ({x}, {y})"
        );
    }
}

#[test]
fn flip_x_mirrors_the_texels() {
    let mut renderer = renderer();
    // Left half red, right half blue, the pivot on the left edge: unflipped, the sprite covers
    // world x [0, 1]; flipped, it mirrors around the pivot to [-1, 0], blue on the left.
    let meta = TextureMeta {
        pivot: Some((0, 8)),
        ..TextureMeta::default()
    };
    let texture = png(16, 16, |x, _| if x < 8 { RED } else { BLUE });
    upload(&mut renderer, &texture, &meta);

    let unflipped = render(
        &mut renderer,
        SIZE,
        &frame(vec![sprite(entity(), Vec2::ZERO)]),
    );
    assert_eq!(pixel(&unflipped, SIZE, 36, 32), RED);
    assert_eq!(pixel(&unflipped, SIZE, 44, 32), BLUE);
    assert_eq!(pixel(&unflipped, SIZE, 28, 32), background());

    let mut flipped = sprite(entity(), Vec2::ZERO);
    flipped.flip_x = true;
    let pixels = render(&mut renderer, SIZE, &frame(vec![flipped]));
    assert_eq!(pixel(&pixels, SIZE, 20, 32), BLUE);
    assert_eq!(pixel(&pixels, SIZE, 28, 32), RED);
    assert_eq!(pixel(&pixels, SIZE, 36, 32), background());
}

#[test]
fn flip_y_mirrors_the_texels() {
    let mut renderer = renderer();
    // Top half red, bottom half blue: flipped, blue is on top.
    let texture = png(16, 16, |_, y| if y < 8 { RED } else { BLUE });
    upload(&mut renderer, &texture, &TextureMeta::default());
    let mut flipped = sprite(entity(), Vec2::ZERO);
    flipped.flip_y = true;
    let pixels = render(&mut renderer, SIZE, &frame(vec![flipped]));
    assert_eq!(pixel(&pixels, SIZE, 32, 28), BLUE);
    assert_eq!(pixel(&pixels, SIZE, 32, 36), RED);
}

#[test]
fn tint_multiplies() {
    let mut renderer = renderer();
    upload(
        &mut renderer,
        &solid([200, 100, 50, 255]),
        &TextureMeta::default(),
    );
    let mut tinted = sprite(entity(), Vec2::ZERO);
    tinted.tint = Color(0x80FF00FF);
    let pixels = render(&mut renderer, SIZE, &frame(vec![tinted]));
    let [r, g, b, a] = pixel(&pixels, SIZE, 32, 32);
    assert!(r.abs_diff(100) <= 1, "red {r}");
    assert_eq!((g, b, a), (100, 0, 255));
}

#[test]
fn translucent_tint_blends_over_the_background() {
    let mut renderer = renderer();
    upload(
        &mut renderer,
        &solid([255, 255, 255, 255]),
        &TextureMeta::default(),
    );
    let mut tinted = sprite(entity(), Vec2::ZERO);
    tinted.tint = Color(0xFFFFFF80);
    let pixels = render(&mut renderer, SIZE, &frame(vec![tinted]));
    let alpha = 128.0 / 255.0;
    let [r, g, b, _] = pixel(&pixels, SIZE, 32, 32);
    for (channel, back) in [
        (r, background()[0]),
        (g, background()[1]),
        (b, background()[2]),
    ] {
        let expected = 255.0 * alpha + f32::from(back) * (1.0 - alpha);
        assert!(
            (f32::from(channel) - expected).abs() <= 1.5,
            "{channel} != {expected}"
        );
    }
}

#[test]
fn nearest_filter_keeps_hard_edges() {
    let mut renderer = renderer();
    // A 2 by 2 checker at 1 pixel per unit: 2 world units, 32 screen pixels, each texel a
    // 16 by 16 block with no blend at its edges.
    let meta = TextureMeta {
        pixels_per_unit: 1,
        filter: Filter::Nearest,
        ..TextureMeta::default()
    };
    let texture = png(2, 2, |x, y| if (x + y) % 2 == 0 { RED } else { BLUE });
    upload(&mut renderer, &texture, &meta);
    let pixels = render(
        &mut renderer,
        SIZE,
        &frame(vec![sprite(entity(), Vec2::ZERO)]),
    );
    for y in 16..48 {
        for x in 16..48 {
            let expected = if (x < 32) == (y < 32) { RED } else { BLUE };
            assert_eq!(pixel(&pixels, SIZE, x, y), expected, "at ({x}, {y})");
        }
    }
}

#[test]
fn animation_frame_selects_its_region() {
    let mut renderer = renderer();
    let meta = TextureMeta {
        grid: Some(Grid {
            frame_width: 16,
            frame_height: 16,
            margin: 0,
            spacing: 0,
        }),
        ..TextureMeta::default()
    };
    let texture = png(32, 16, |x, _| if x < 16 { RED } else { GREEN });
    upload(&mut renderer, &texture, &meta);
    let mut second = sprite(entity(), Vec2::ZERO);
    second.frame = 1;
    let pixels = render(&mut renderer, SIZE, &frame(vec![second]));
    for (x, y) in [(24, 24), (39, 24), (24, 39), (39, 39)] {
        assert_eq!(pixel(&pixels, SIZE, x, y), GREEN, "at ({x}, {y})");
    }
}

#[test]
fn missing_texture_draws_a_placeholder_and_reports_it() {
    let (device, queue) = device();
    let mut renderer = Renderer::new(device.clone(), queue, wgpu::TextureFormat::Rgba8Unorm);
    upload(&mut renderer, &solid(RED), &TextureMeta::default());
    let (unloaded, missing_frame, drawn) = {
        let mut world = World::new();
        (world.spawn(), world.spawn(), world.spawn())
    };
    let mut no_texture = sprite(unloaded, Vec2::new(-1.0, -1.0));
    no_texture.texture = AssetId(99);
    no_texture.frame = 3;
    let mut no_frame = sprite(missing_frame, Vec2::new(1.0, -1.0));
    no_frame.frame = 5;
    let scene = frame(vec![
        no_texture,
        no_frame,
        sprite(drawn, Vec2::new(1.0, 1.0)),
    ]);

    let pixels = render(&mut renderer, SIZE, &scene);
    // One magenta world unit on each missing sprite, and the rest of the frame draws.
    assert_eq!(pixel(&pixels, SIZE, 16, 48), MAGENTA);
    assert_eq!(pixel(&pixels, SIZE, 48, 48), MAGENTA);
    assert_eq!(pixel(&pixels, SIZE, 39, 48), background());
    assert_eq!(pixel(&pixels, SIZE, 48, 16), RED);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: SIZE.width,
            height: SIZE.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let frames = Frames {
        previous: None,
        current: &scene,
        alpha: 1.0,
    };
    let report = renderer
        .draw(&view, SIZE, frames, &Overlay::default())
        .unwrap();
    assert_eq!(
        report,
        DrawReport {
            missing: vec![(unloaded, AssetId(99), 3), (missing_frame, AssetId(1), 5)],
        }
    );
}

#[test]
fn sprites_draw_without_an_atlas_as_placeholders() {
    let mut renderer = renderer();
    let pixels = render(
        &mut renderer,
        SIZE,
        &frame(vec![sprite(entity(), Vec2::ZERO)]),
    );
    assert_eq!(pixel(&pixels, SIZE, 32, 32), MAGENTA);
}

#[test]
fn wide_and_tall_targets_show_the_same_minimum_view() {
    let mut renderer = renderer();
    upload(&mut renderer, &solid(RED), &TextureMeta::default());
    // Inside the 4 by 4 minimum view, and outside it to the right.
    let scene = frame(vec![
        sprite(entity(), Vec2::new(1.5, 1.5)),
        sprite(entity(), Vec2::new(3.0, 0.0)),
    ]);

    // 128 by 64 shows 8 by 4 units: 16 pixels per unit, the origin at (64, 32).
    let wide = Size {
        width: 128,
        height: 64,
    };
    let pixels = render(&mut renderer, wide, &scene);
    assert_eq!(pixel(&pixels, wide, 81, 1), RED);
    assert_eq!(pixel(&pixels, wide, 95, 15), RED);
    assert_eq!(pixel(&pixels, wide, 80 - 1, 8), background());
    assert_eq!(pixel(&pixels, wide, 112, 32), RED);

    // 64 by 128 shows 4 by 8 units: still 16 pixels per unit, the origin at (32, 64).
    let tall = Size {
        width: 64,
        height: 128,
    };
    let pixels = render(&mut renderer, tall, &scene);
    assert_eq!(pixel(&pixels, tall, 49, 33), RED);
    assert_eq!(pixel(&pixels, tall, 63, 47), RED);
    assert_eq!(pixel(&pixels, tall, 48, 31), background());
    assert_eq!(pixel(&pixels, tall, 63, 64), background());
}

#[test]
fn a_frame_without_a_camera_shows_twenty_by_eleven_and_a_quarter_units() {
    let mut renderer = renderer();
    upload(&mut renderer, &solid(RED), &TextureMeta::default());
    let mut scene = frame(vec![sprite(entity(), Vec2::new(9.5, 0.0))]);
    scene.camera = None;
    // 160 by 90 pixels over 20 by 11.25 units: 8 pixels per unit, the origin at (80, 45); the
    // sprite covers x [9, 10], pixels 152..160.
    let size = Size {
        width: 160,
        height: 90,
    };
    let pixels = render(&mut renderer, size, &scene);
    assert_eq!(pixel(&pixels, size, 152, 42), RED);
    assert_eq!(pixel(&pixels, size, 159, 48), RED);
    assert_eq!(pixel(&pixels, size, 151, 45), background());
}

#[test]
fn overlay_camera_overrides_the_frame_camera() {
    let mut renderer = renderer();
    upload(&mut renderer, &solid(RED), &TextureMeta::default());
    let scene = frame(vec![sprite(entity(), Vec2::new(10.0, 10.0))]);
    let overlay = Overlay {
        camera: Some(CameraView {
            center: Vec2::new(10.0, 10.0),
            view: Vec2::new(4.0, 4.0),
        }),
        ..Overlay::default()
    };
    let frames = Frames {
        previous: None,
        current: &scene,
        alpha: 1.0,
    };
    let pixels = renderer.render_rgba(SIZE, frames, &overlay).unwrap();
    assert_eq!(pixel(&pixels, SIZE, 32, 32), RED);
}

#[test]
fn parallax_layers_follow_the_camera_by_their_factor() {
    let mut renderer = renderer();
    upload(&mut renderer, &solid(RED), &TextureMeta::default());
    let mut scene = frame(vec![sprite(entity(), Vec2::ZERO)]);
    scene.camera = Some(CameraView {
        center: Vec2::new(1.0, 0.0),
        view: Vec2::new(4.0, 4.0),
    });
    // A layer fixed to the screen ignores the camera: the sprite stays at the centre.
    scene.layers[0].parallax = 0.0;
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 32, 32), RED);
    assert_eq!(pixel(&pixels, SIZE, 20, 32), background());
}

#[test]
fn max_page_side_is_capped_at_4096() {
    let renderer = renderer();
    assert!(renderer.max_page_side() <= 4096);
    assert!(renderer.max_page_side() > 0);
}

#[test]
fn set_atlas_refuses_a_page_larger_than_the_device_allows() {
    let mut renderer = renderer();
    let side = renderer.max_page_side() + 1;
    let atlas = Atlas {
        pages: vec![AtlasPage {
            width: side,
            height: 1,
            rgba: vec![0; side as usize * 4],
        }],
        regions: Default::default(),
        errors: Vec::new(),
    };
    assert_eq!(
        renderer.set_atlas(&atlas),
        Err(RenderError::Limit {
            operation: "set_atlas",
            limit: "max_texture_dimension_2d",
            requested: u64::from(side),
            allowed: u64::from(side - 1),
        })
    );
}

#[test]
fn render_rgba_of_an_empty_target_is_empty() {
    let mut renderer = renderer();
    let empty = Size {
        width: 0,
        height: 64,
    };
    assert!(render(&mut renderer, empty, &frame(Vec::new())).is_empty());
}

#[test]
fn render_rgba_refuses_a_target_larger_than_the_device_allows() {
    let mut renderer = renderer();
    let scene = frame(Vec::new());
    let frames = Frames {
        previous: None,
        current: &scene,
        alpha: 1.0,
    };
    let huge = Size {
        width: u32::MAX,
        height: 1,
    };
    let error = renderer
        .render_rgba(huge, frames, &Overlay::default())
        .unwrap_err();
    assert!(
        matches!(
            error,
            RenderError::Limit {
                operation: "render_rgba",
                limit: "max_texture_dimension_2d",
                requested,
                ..
            } if requested == u64::from(u32::MAX)
        ),
        "{error}"
    );
}
