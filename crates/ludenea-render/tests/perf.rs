//! The renderer's frame budget: 10,000 sprites drawn 144 times per pair of ticks, as a
//! 144 Hz display shows a 60 Hz simulation with room to spare. Run in release:
//! `cargo test --release --locked -p ludenea-render --features gpu --test perf -- --ignored
//! --nocapture`. `PERF_STRICT=1` turns the report into an assertion, which holds on the
//! reference machine (an Apple M-series).
#![cfg(feature = "gpu")]

use std::time::Duration;

use ludenea_app::meta::TextureMeta;
use ludenea_draw::Color;
use ludenea_draw::frame::{CameraItem, LayerInfo, RenderFrame, SpriteItem};
use ludenea_ecs::{AssetId, Entity, World};
use ludenea_math::{Rot, Vec2};
use ludenea_render::atlas::{TextureSource, pack};
use ludenea_render::gpu::{Overlay, Renderer, Size, headless};

const SPRITES: usize = 10_000;
const DRAWS_PER_TICK: usize = 144;
const TICKS: usize = 10;
const SIZE: Size = Size {
    width: 1280,
    height: 720,
};
/// The 95th percentile CPU time of one `draw`, on the reference machine.
const BUDGET: Duration = Duration::from_millis(1);

/// A 16 by 16 white PNG.
fn white_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 16, 16);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[255; 16 * 16 * 4]).unwrap();
    writer.finish().unwrap();
    bytes
}

/// The frame of `tick`: every sprite a little further right and turned a little more.
fn frame(tick: usize, entities: &[Entity], camera: Entity) -> RenderFrame {
    let sprites = entities
        .iter()
        .enumerate()
        .map(|(index, &entity)| {
            let (column, row) = ((index % 100) as f32, (index / 100) as f32);
            SpriteItem {
                entity,
                layer: 0,
                order: 0,
                texture: AssetId(1),
                frame: 0,
                position: Vec2::new(column * 0.2 - 10.0 + tick as f32 * 0.01, row * 0.11 - 5.5),
                rotation: Rot::from_angle(tick as f32 * 0.05 + index as f32),
                flip_x: false,
                flip_y: false,
                tint: Color(0xFFFFFFFF),
            }
        })
        .collect();
    RenderFrame {
        tick: tick as u64,
        camera: Some(CameraItem {
            entity: camera,
            center: Vec2::new(tick as f32 * 0.01, 0.0),
            view: Vec2::new(20.0, 11.25),
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

#[test]
#[ignore = "timing, run in release"]
#[expect(
    clippy::disallowed_methods,
    reason = "the test measures wall-clock time, which is outside the simulation"
)]
fn ten_thousand_sprites_draw_144_frames_per_tick_pair() {
    let (device, queue) = pollster::block_on(headless()).unwrap_or_else(|error| panic!("{error}"));
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(device.clone(), queue, format);
    let png = white_png();
    let meta = TextureMeta::default();
    let atlas = pack(
        &[TextureSource {
            id: AssetId(1),
            path: None,
            png: &png,
            meta: &meta,
        }],
        renderer.max_page_side(),
    );
    renderer.set_atlas(&atlas).unwrap();
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
    let view = target.create_view(&Default::default());

    let mut world = World::new();
    let entities: Vec<Entity> = (0..SPRITES).map(|_| world.spawn()).collect();
    let camera = world.spawn();
    let mut previous = frame(0, &entities, camera);
    let mut times = Vec::with_capacity(TICKS * DRAWS_PER_TICK);
    for tick in 1..=TICKS {
        let current = frame(tick, &entities, camera);
        renderer
            .set_frames(Some(&previous), &current, &Overlay::default())
            .unwrap();
        for draw in 0..DRAWS_PER_TICK {
            let alpha = draw as f32 / DRAWS_PER_TICK as f32;
            let started = std::time::Instant::now();
            renderer.draw(&view, SIZE, alpha, None).unwrap();
            times.push(started.elapsed());
        }
        // The GPU catches up between ticks, outside the measure, as a display's vsync paces it.
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        previous = current;
    }
    times.sort();
    let percentile =
        |fraction: f64| times[((times.len() as f64 * fraction) as usize).min(times.len() - 1)];
    let (median, p95, worst) = (percentile(0.5), percentile(0.95), times[times.len() - 1]);
    println!(
        "{SPRITES} sprites, {} draws: median {median:?}, 95th percentile {p95:?}, worst {worst:?}",
        times.len()
    );
    if std::env::var("PERF_STRICT").as_deref() == Ok("1") {
        assert!(
            p95 < BUDGET,
            "95th percentile {p95:?} is past the {BUDGET:?} budget"
        );
    }
}
