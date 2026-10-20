//! The wgpu renderer, drawn offscreen and read back pixel by pixel. These tests need a GPU
//! adapter (hardware or software); without one they fail naming what was tried.
#![cfg(feature = "gpu")]

use ludenea_app::meta::{Filter, Grid, TextureMeta};
use std::collections::BTreeSet;

use ludenea_draw::frame::{CameraView, LayerInfo, RenderFrame, SpriteItem, TilemapItem};
use ludenea_draw::mesh::HIGHLIGHT;
use ludenea_draw::{Color, Primitive};
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

#[test]
fn render_rgba_refuses_a_target_whose_read_back_exceeds_the_buffer_limit() {
    let (device, queue) = device();
    let limits = device.limits();
    // The largest square target the texture limit allows; its read-back must still pass the
    // buffer limit for this test to mean anything.
    let side = limits.max_texture_dimension_2d;
    let padded = u64::from((side * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT))
        * u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let requested = padded * u64::from(side);
    assert!(
        requested > limits.max_buffer_size,
        "a {side}x{side} read-back ({requested} bytes) fits the {} byte buffer limit: this \
         device cannot exercise the check",
        limits.max_buffer_size
    );
    let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm);
    let scene = frame(Vec::new());
    let frames = Frames {
        previous: None,
        current: &scene,
        alpha: 1.0,
    };
    let size = Size {
        width: side,
        height: side,
    };
    assert_eq!(
        renderer.render_rgba(size, frames, &Overlay::default()),
        Err(RenderError::Limit {
            operation: "render_rgba",
            limit: "max_buffer_size",
            requested,
            allowed: limits.max_buffer_size,
        })
    );
}

/// Packs a tileset as `AssetId(1)`: three 16 by 16 frames, red, green, then blue.
fn tileset(renderer: &mut Renderer) {
    let meta = TextureMeta {
        grid: Some(Grid {
            frame_width: 16,
            frame_height: 16,
            margin: 0,
            spacing: 0,
        }),
        ..TextureMeta::default()
    };
    let texture = png(48, 16, |x, _| [RED, GREEN, BLUE][(x / 16) as usize]);
    upload(renderer, &texture, &meta);
}

/// A tilemap of one-unit cells on layer 0 with order 0, its bottom-left corner at `position`.
fn tilemap(
    entity: Entity,
    position: Vec2,
    width: u32,
    height: u32,
    tiles: Vec<Option<u32>>,
) -> TilemapItem {
    TilemapItem {
        entity,
        layer: 0,
        order: 0,
        tileset: AssetId(1),
        position,
        cell_size: Vec2::new(1.0, 1.0),
        width,
        height,
        tiles,
    }
}

fn with_tilemaps(sprites: Vec<SpriteItem>, tilemaps: Vec<TilemapItem>) -> RenderFrame {
    RenderFrame {
        tilemaps,
        ..frame(sprites)
    }
}

/// A sprite of the tileset's frame `frame`: one unit centred on `position`.
fn tile_sprite(entity: Entity, position: Vec2, frame: u32) -> SpriteItem {
    SpriteItem {
        frame,
        ..sprite(entity, position)
    }
}

fn layer(name: &str, parallax: f32) -> LayerInfo {
    LayerInfo {
        name: name.into(),
        parallax,
    }
}

fn render_with(renderer: &mut Renderer, frame: &RenderFrame, overlay: &Overlay) -> Vec<u8> {
    let frames = Frames {
        previous: None,
        current: frame,
        alpha: 1.0,
    };
    renderer.render_rgba(SIZE, frames, overlay).unwrap()
}

fn report(device: &wgpu::Device, renderer: &mut Renderer, frame: &RenderFrame) -> DrawReport {
    draw(
        device,
        renderer,
        SIZE,
        frame,
        wgpu::TextureFormat::Rgba8Unorm,
    )
    .unwrap()
}

/// Draws `frame` with `renderer` into a new `SIZE` target of `format`, told it is `size`.
fn draw(
    device: &wgpu::Device,
    renderer: &mut Renderer,
    size: Size,
    frame: &RenderFrame,
    format: wgpu::TextureFormat,
) -> Result<DrawReport, RenderError> {
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
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let frames = Frames {
        previous: None,
        current: frame,
        alpha: 1.0,
    };
    renderer.draw(
        &target.create_view(&Default::default()),
        size,
        frames,
        &Overlay::default(),
    )
}

#[test]
fn tilemap_draws_tiles_at_their_cells() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    // Cells of world [0, 1] and [1, 2] by [0, 1]: pixels 32..48 and 48..64 across, 16..32 down.
    let scene = with_tilemaps(
        Vec::new(),
        vec![tilemap(entity(), Vec2::ZERO, 2, 1, vec![Some(0), Some(1)])],
    );
    let pixels = render(&mut renderer, SIZE, &scene);
    for (x, y) in [(32, 16), (47, 31), (40, 24)] {
        assert_eq!(pixel(&pixels, SIZE, x, y), RED, "first cell at ({x}, {y})");
    }
    for (x, y) in [(48, 16), (63, 31), (56, 24)] {
        assert_eq!(
            pixel(&pixels, SIZE, x, y),
            GREEN,
            "second cell at ({x}, {y})"
        );
    }
    for (x, y) in [(31, 24), (40, 15), (40, 32), (56, 32)] {
        assert_eq!(
            pixel(&pixels, SIZE, x, y),
            background(),
            "outside at ({x}, {y})"
        );
    }
}

#[test]
fn empty_cells_stay_transparent() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    // A blue sprite under the second cell, which is empty: it shows through.
    let scene = with_tilemaps(
        vec![tile_sprite(entity(), Vec2::new(1.5, 0.5), 2)],
        vec![tilemap(entity(), Vec2::ZERO, 2, 1, vec![Some(0), None])],
    );
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 40, 24), RED);
    assert_eq!(pixel(&pixels, SIZE, 56, 24), BLUE);
}

#[test]
fn tilemap_row_zero_is_at_the_bottom() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    // Row 0 covers world y [0, 1] (pixels 16..32 down), row 1 covers [1, 2] (pixels 0..16).
    let scene = with_tilemaps(
        Vec::new(),
        vec![tilemap(entity(), Vec2::ZERO, 1, 2, vec![Some(0), Some(1)])],
    );
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 40, 24), RED);
    assert_eq!(pixel(&pixels, SIZE, 40, 8), GREEN);
}

#[test]
fn later_layers_draw_over_earlier_ones() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    let mut scene = with_tilemaps(Vec::new(), Vec::new());
    scene.layers = vec![layer("back", 1.0), layer("front", 1.0)];
    // A tilemap on the front layer with the lowest order covers a sprite on the back layer
    // with the highest order, whatever their order in the frame's lists.
    let mut front = tilemap(entity(), Vec2::ZERO, 1, 1, vec![Some(0)]);
    front.layer = 1;
    front.order = i32::MIN;
    let mut back = tile_sprite(entity(), Vec2::new(0.5, 0.5), 1);
    back.order = i32::MAX;
    // Two sprites, the front one listed first: the front one still covers the back one.
    let mut front_sprite = tile_sprite(entity(), Vec2::new(-0.5, -0.5), 2);
    front_sprite.layer = 1;
    let mut back_sprite = tile_sprite(entity(), Vec2::new(-0.5, -0.5), 1);
    back_sprite.order = 1;
    scene.sprites = vec![front_sprite, back, back_sprite];
    scene.tilemaps = vec![front];
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 40, 24), RED);
    assert_eq!(pixel(&pixels, SIZE, 24, 40), BLUE);
}

#[test]
fn order_sorts_within_a_layer() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    // On one layer: a green sprite (order 0) under a red tilemap (order 1) under a blue
    // sprite (order 2) that covers only the cell's left half. The blue sprite is listed first.
    let mut cells = tilemap(entity(), Vec2::ZERO, 1, 1, vec![Some(0)]);
    cells.order = 1;
    let under = tile_sprite(entity(), Vec2::new(0.5, 0.5), 1);
    let mut over = tile_sprite(entity(), Vec2::new(0.0, 0.5), 2);
    over.order = 2;
    let scene = with_tilemaps(vec![over, under], vec![cells]);
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 44, 24), RED);
    assert_eq!(pixel(&pixels, SIZE, 36, 24), BLUE);
}

#[test]
fn background_layer_scrolls_slower() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    // The camera at x = 1: the background (parallax 0.5) sees it at 0.5, the world at 1.
    let mut cell = tilemap(entity(), Vec2::new(-0.5, -0.5), 1, 1, vec![Some(0)]);
    cell.layer = 0;
    let mut marker = tile_sprite(entity(), Vec2::new(1.0, 1.5), 1);
    marker.layer = 1;
    let mut scene = with_tilemaps(vec![marker], vec![cell]);
    scene.layers = vec![layer("background", 0.5), layer("world", 1.0)];
    scene.camera = Some(CameraView {
        center: Vec2::new(1.0, 0.0),
        view: Vec2::new(4.0, 4.0),
    });
    // The cell, world x [-0.5, 0.5], moves by 0.5 to pixels 16..32; the sprite by 1, its
    // centre to pixel 32.
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 20, 32), RED);
    assert_eq!(pixel(&pixels, SIZE, 36, 32), background());
    assert_eq!(pixel(&pixels, SIZE, 32, 8), GREEN);

    // The camera at x = 2, the tilemap unchanged: the cell moves by 1, to pixels 8..24.
    scene.camera = Some(CameraView {
        center: Vec2::new(2.0, 0.0),
        view: Vec2::new(4.0, 4.0),
    });
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 12, 32), RED);
    assert_eq!(pixel(&pixels, SIZE, 28, 32), background());
}

/// A filled circle of radius 0.5 on debug layer `layer`.
fn dot(frame: &mut RenderFrame, layer: &str, entity: Option<Entity>, center: Vec2, color: Color) {
    frame.debug.push(
        layer,
        entity,
        Primitive::Circle {
            center,
            radius: 0.5,
            color,
            fill: true,
        },
    );
}

#[test]
fn debug_draw_is_on_top() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    let (plain, picked) = {
        let mut world = World::new();
        (world.spawn(), world.spawn())
    };
    // Over a sprite and a tilemap on a front layer with the highest order, even with the
    // sprite's layer fixed to the screen: the debug draw moves with the world.
    let mut red = tile_sprite(plain, Vec2::ZERO, 0);
    red.order = i32::MAX;
    let mut cells = tilemap(picked, Vec2::new(0.5, 0.5), 1, 1, vec![Some(0)]);
    cells.order = i32::MAX;
    let mut scene = with_tilemaps(vec![red], vec![cells]);
    scene.layers = vec![layer("hud", 0.0)];
    dot(
        &mut scene,
        "physics",
        Some(plain),
        Vec2::ZERO,
        Color(0x0000FFFF),
    );
    dot(
        &mut scene,
        "physics",
        Some(picked),
        Vec2::new(1.0, 1.0),
        Color(0x0000FFFF),
    );
    let overlay = Overlay {
        debug_layers: BTreeSet::from(["physics".to_string()]),
        selected: BTreeSet::from([picked]),
        ..Overlay::default()
    };
    let pixels = render_with(&mut renderer, &scene, &overlay);
    assert_eq!(pixel(&pixels, SIZE, 32, 32), BLUE);
    assert_eq!(pixel(&pixels, SIZE, 48, 16), HIGHLIGHT.0.to_be_bytes());
    // Outside the circle, the sprite shows.
    assert_eq!(pixel(&pixels, SIZE, 25, 25), RED);
}

#[test]
fn debug_layers_not_requested_are_hidden() {
    let mut renderer = renderer();
    let mut scene = frame(Vec::new());
    dot(
        &mut scene,
        "physics",
        None,
        Vec2::new(-1.0, 0.0),
        Color(0x0000FFFF),
    );
    dot(
        &mut scene,
        "navigation",
        None,
        Vec2::new(1.0, 0.0),
        Color(0x00FF00FF),
    );
    let overlay = Overlay {
        debug_layers: BTreeSet::from(["physics".to_string()]),
        ..Overlay::default()
    };
    let pixels = render_with(&mut renderer, &scene, &overlay);
    assert_eq!(pixel(&pixels, SIZE, 16, 32), BLUE);
    assert_eq!(pixel(&pixels, SIZE, 48, 32), background());
    // Without layers, nothing of the debug draw shows.
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 16, 32), background());
}

#[test]
fn missing_tile_draws_a_placeholder_and_reports_it() {
    let (device, queue) = device();
    let mut renderer = Renderer::new(device.clone(), queue, wgpu::TextureFormat::Rgba8Unorm);
    tileset(&mut renderer);
    let map = entity();
    // Frame 7 is not in the tileset: its cell is magenta, the others draw.
    let scene = with_tilemaps(
        Vec::new(),
        vec![tilemap(map, Vec2::ZERO, 2, 1, vec![Some(7), Some(1)])],
    );
    let pixels = render(&mut renderer, SIZE, &scene);
    assert_eq!(pixel(&pixels, SIZE, 40, 24), MAGENTA);
    assert_eq!(pixel(&pixels, SIZE, 56, 24), GREEN);
    assert_eq!(
        report(&device, &mut renderer, &scene),
        DrawReport {
            missing: vec![(map, AssetId(1), 7)],
        }
    );
}

#[test]
fn a_changed_tilemap_or_atlas_redraws_its_tiles() {
    let mut renderer = renderer();
    let mut scene = with_tilemaps(
        Vec::new(),
        vec![tilemap(entity(), Vec2::ZERO, 1, 1, vec![Some(0)])],
    );
    // Without an atlas the cell is a placeholder; with one, the tile.
    assert_eq!(
        pixel(&render(&mut renderer, SIZE, &scene), SIZE, 40, 24),
        MAGENTA
    );
    tileset(&mut renderer);
    assert_eq!(
        pixel(&render(&mut renderer, SIZE, &scene), SIZE, 40, 24),
        RED
    );
    scene.tilemaps[0].tiles = vec![Some(1)];
    assert_eq!(
        pixel(&render(&mut renderer, SIZE, &scene), SIZE, 40, 24),
        GREEN
    );
    scene.tilemaps.clear();
    assert_eq!(
        pixel(&render(&mut renderer, SIZE, &scene), SIZE, 40, 24),
        background()
    );
}

#[test]
fn a_tilemap_larger_than_a_chunk_draws_every_cell() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    // 70 by 70 cells of a quarter unit: three chunks across and up, all inside the view.
    let (width, height) = (70, 70);
    let mut map = tilemap(
        entity(),
        Vec2::new(-2.0, -2.0),
        width,
        height,
        (0..width * height)
            .map(|cell| Some(cell % width / 32))
            .collect(),
    );
    map.cell_size = Vec2::new(4.0 / 70.0, 4.0 / 70.0);
    let pixels = render(&mut renderer, SIZE, &with_tilemaps(Vec::new(), vec![map]));
    // Columns 0..32 red, 32..64 green, 64..70 blue; every row drawn, the top one included.
    for y in [0, 32, 63] {
        assert_eq!(pixel(&pixels, SIZE, 2, y), RED, "at (2, {y})");
        assert_eq!(pixel(&pixels, SIZE, 40, y), GREEN, "at (40, {y})");
        assert_eq!(pixel(&pixels, SIZE, 62, y), BLUE, "at (62, {y})");
    }
}

#[test]
fn a_tilemap_whose_tiles_do_not_match_its_size_draws_nothing_at_once() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    // Walking 10^10 cells would hang the draw (the test would never finish): the item is
    // skipped instead, and so is one with more tiles than cells.
    let huge = tilemap(entity(), Vec2::ZERO, 100_000, 100_000, Vec::new());
    let short = tilemap(
        entity(),
        Vec2::new(-2.0, -2.0),
        1,
        1,
        vec![Some(0), Some(1)],
    );
    let scene = with_tilemaps(Vec::new(), vec![huge, short]);
    let pixels = render(&mut renderer, SIZE, &scene);
    assert!(
        pixels.chunks_exact(4).all(|texel| texel == background()),
        "something was drawn"
    );
}

#[test]
fn previous_frame_and_alpha_draw_in_between() {
    let mut renderer = renderer();
    tileset(&mut renderer);
    let mut world = World::new();
    let (moving, map) = (world.spawn(), world.spawn());
    let previous = with_tilemaps(
        vec![sprite(moving, Vec2::new(-1.0, 1.0))],
        vec![tilemap(map, Vec2::new(-2.0, -2.0), 1, 1, vec![Some(1)])],
    );
    let current = with_tilemaps(
        vec![sprite(moving, Vec2::new(1.0, 1.0))],
        vec![tilemap(map, Vec2::new(0.0, -2.0), 1, 1, vec![Some(1)])],
    );
    let between = |renderer: &mut Renderer, alpha: f32| {
        let frames = Frames {
            previous: Some(&previous),
            current: &current,
            alpha,
        };
        renderer
            .render_rgba(SIZE, frames, &Overlay::default())
            .unwrap()
    };
    // Halfway: the sprite centred on (0, 1), pixels 24..40 across and 8..24 down; the cell at
    // world [-1, 0] by [-2, -1], pixels 16..32 across and 48..64 down.
    let pixels = between(&mut renderer, 0.5);
    assert_eq!(pixel(&pixels, SIZE, 32, 16), RED);
    assert_eq!(pixel(&pixels, SIZE, 20, 16), background());
    assert_eq!(pixel(&pixels, SIZE, 44, 16), background());
    assert_eq!(pixel(&pixels, SIZE, 24, 56), GREEN);
    assert_eq!(pixel(&pixels, SIZE, 40, 56), background());
    // At alpha 1 both are where `current` puts them.
    let pixels = between(&mut renderer, 1.0);
    assert_eq!(pixel(&pixels, SIZE, 48, 16), RED);
    assert_eq!(pixel(&pixels, SIZE, 32, 16), background());
    assert_eq!(pixel(&pixels, SIZE, 40, 56), GREEN);
    assert_eq!(pixel(&pixels, SIZE, 24, 56), background());
}

/// A device that allows buffers of at most `max_buffer_size` bytes, so a test can reach the
/// limit with an ordinary frame.
fn device_with_buffer_limit(max_buffer_size: u64) -> (wgpu::Device, wgpu::Queue) {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = match instance.request_adapter(&Default::default()).await {
            Ok(adapter) => adapter,
            Err(_) => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    force_fallback_adapter: true,
                    ..Default::default()
                })
                .await
                .unwrap_or_else(|error| panic!("no GPU adapter: {error}")),
        };
        let required_limits = wgpu::Limits {
            max_buffer_size,
            ..wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits())
        };
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits,
                ..Default::default()
            })
            .await
            .unwrap()
    })
}

#[test]
fn sixty_thousand_sprites_then_one_both_draw() {
    // 60,000 instances of 72 bytes are 4.32 MB: past a 1 MiB buffer limit.
    let (device, queue) = device_with_buffer_limit(1 << 20);
    let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm);
    upload(&mut renderer, &solid(RED), &TextureMeta::default());
    let mut world = World::new();
    let mut sprites: Vec<SpriteItem> = (0..60_000)
        .map(|_| sprite(world.spawn(), Vec2::new(100.0, 100.0)))
        .collect();
    sprites[0].position = Vec2::new(-1.0, 1.0);
    sprites[30_000].position = Vec2::new(1.0, 1.0);
    sprites[59_999].position = Vec2::new(1.0, -1.0);
    let pixels = render(&mut renderer, SIZE, &frame(sprites));
    assert_eq!(pixel(&pixels, SIZE, 16, 16), RED, "the first sprite");
    assert_eq!(pixel(&pixels, SIZE, 48, 16), RED, "the middle sprite");
    assert_eq!(pixel(&pixels, SIZE, 48, 48), RED, "the last sprite");
    assert_eq!(pixel(&pixels, SIZE, 16, 48), background());

    let pixels = render(
        &mut renderer,
        SIZE,
        &frame(vec![sprite(world.spawn(), Vec2::new(-1.0, -1.0))]),
    );
    assert_eq!(pixel(&pixels, SIZE, 16, 48), RED);
    assert_eq!(pixel(&pixels, SIZE, 16, 16), background());
}

#[test]
fn a_debug_draw_past_the_buffer_limit_draws_in_parts() {
    // A filled circle is 96 vertices of 24 bytes: 1,000 of them are 2.3 MB.
    let (device, queue) = device_with_buffer_limit(1 << 20);
    let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm);
    let mut scene = frame(Vec::new());
    let blue = Color(0x0000FFFF);
    dot(&mut scene, "test", None, Vec2::new(-1.0, 1.0), blue);
    for _ in 0..1_000 {
        dot(&mut scene, "test", None, Vec2::new(100.0, 100.0), blue);
    }
    dot(&mut scene, "test", None, Vec2::new(1.0, -1.0), blue);
    let overlay = Overlay {
        debug_layers: BTreeSet::from(["test".to_string()]),
        ..Overlay::default()
    };
    let pixels = render_with(&mut renderer, &scene, &overlay);
    assert_eq!(pixel(&pixels, SIZE, 16, 16), BLUE, "the first circle");
    assert_eq!(pixel(&pixels, SIZE, 48, 48), BLUE, "the last circle");
    assert_eq!(pixel(&pixels, SIZE, 48, 16), background());
}

#[test]
fn oversized_target_is_refused_naming_the_limit() {
    let (device, queue) = device();
    let allowed = device.limits().max_texture_dimension_2d;
    let mut renderer = Renderer::new(device.clone(), queue, wgpu::TextureFormat::Rgba8Unorm);
    let huge = Size {
        width: 1,
        height: allowed + 1,
    };
    assert_eq!(
        draw(
            &device,
            &mut renderer,
            huge,
            &frame(Vec::new()),
            wgpu::TextureFormat::Rgba8Unorm
        ),
        Err(RenderError::Limit {
            operation: "draw",
            limit: "max_texture_dimension_2d",
            requested: u64::from(allowed + 1),
            allowed: u64::from(allowed),
        })
    );
}

#[test]
fn a_validation_error_is_returned_by_the_next_draw() {
    let (device, queue) = device();
    let mut renderer = Renderer::new(device.clone(), queue, wgpu::TextureFormat::Rgba8Unorm);
    // A buffer past the device's limit: a validation error on the renderer's device.
    let _buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("too large"),
        size: device.limits().max_buffer_size + 4,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    });
    let scene = frame(Vec::new());
    let rgba = wgpu::TextureFormat::Rgba8Unorm;
    let error = draw(&device, &mut renderer, SIZE, &scene, rgba).unwrap_err();
    assert!(
        matches!(
            &error,
            RenderError::Gpu { operation: "draw", message } if message.contains("too large")
        ),
        "{error}"
    );
    // Reported once: the next draw succeeds.
    assert_eq!(
        draw(&device, &mut renderer, SIZE, &scene, rgba),
        Ok(DrawReport::default())
    );
}

#[test]
fn a_draw_the_gpu_refuses_returns_its_error() {
    let (device, queue) = device();
    let mut renderer = renderer_on(&device, queue);
    upload(&mut renderer, &solid(RED), &TextureMeta::default());
    // The renderer draws Rgba8Unorm; a Bgra8Unorm target does not match its pipeline.
    let scene = frame(vec![sprite(entity(), Vec2::ZERO)]);
    let error = draw(
        &device,
        &mut renderer,
        SIZE,
        &scene,
        wgpu::TextureFormat::Bgra8Unorm,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            RenderError::Gpu {
                operation: "draw",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_lost_device_is_reported_as_device_lost() {
    let (device, queue) = device();
    let mut renderer = renderer_on(&device, queue);
    device.destroy();
    let scene = frame(Vec::new());
    let rgba = wgpu::TextureFormat::Rgba8Unorm;
    let error = draw(&device, &mut renderer, SIZE, &scene, rgba).unwrap_err();
    assert!(matches!(error, RenderError::DeviceLost(_)), "{error}");
    // Every later call says the same, without touching the lost device.
    let empty = Atlas {
        pages: Vec::new(),
        regions: Default::default(),
        errors: Vec::new(),
    };
    assert!(matches!(
        renderer.set_atlas(&empty),
        Err(RenderError::DeviceLost(_))
    ));
}

fn renderer_on(device: &wgpu::Device, queue: wgpu::Queue) -> Renderer {
    Renderer::new(device.clone(), queue, wgpu::TextureFormat::Rgba8Unorm)
}
