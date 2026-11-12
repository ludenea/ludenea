//! The measurements, one per hypothesis of the spike. Each writes what it
//! found into the report and judges nothing but identities; the thresholds
//! are read from the report. Throwaway, never product code.

use std::path::PathBuf;
use std::sync::{Barrier, Mutex, OnceLock};
use std::time::{Duration, Instant};

use reference_workload::Json;
use render_device_baseline::frames::{DrawConstants, scale_translate, unit_quad, vertex_bytes};
use render_device_baseline::scene::{CLEAR_COLOR, Image, SAMPLER_NEAREST, Scene, TEXTURE_WHITE};
use skeleton_core::Size;

use crate::graph;
use crate::harness::{Backend, Presenter, RenderedWorld, Rig, percentile, summary, write_summary};
use crate::instance::{Delivered, FRAMES_IN_FLIGHT, FrameTimes, StreamEvent};
use crate::math::{Arithmetic, MathLayer, Platform};
use crate::port::{MemoryClass, PassSpec, Port, PortDraw, Source};
use crate::renderer::{Mode, Request};
use crate::slices::tile_extraction_cost;

/// Where a measurement leaves a picture of what it drew, when a device draws.
pub static PICTURES: OnceLock<(PathBuf, String)> = OnceLock::new();

fn save_picture(name: &str, image: &Image) {
    if let Some((directory, label)) = PICTURES.get() {
        let _ = std::fs::write(directory.join(format!("frame-{label}-{name}.png")), image.png());
    }
}

fn request(tick: u32, factor: f32, reproducible: bool, mode: Mode) -> Request {
    Request { tick, factor, reproducible, content_hash: true, mode }
}

fn hash_of(delivered: &Delivered) -> Option<u64> {
    delivered.image.as_ref().map(Image::hash)
}

fn write_optional_hex(out: &mut Json, key: &str, value: Option<u64>) {
    match value {
        Some(value) => out.hex(key, value),
        None => out.string(key, "none"),
    }
}

/// The content hashes of two frames of one world, prepared with one arithmetic.
fn content_hashes_with<A: Arithmetic>(workers: usize) -> Result<[u64; 2], String> {
    let rig = Rig::new(Backend::Content, Presenter::SteppingThread)?;
    let mut world = RenderedWorld::new(&rig, 0, Size::Default, workers, workers);
    world.step(30);
    let first = world.capture::<A>(&request(30, 0.5, true, Mode::ORDINARY))?;
    world.step(30);
    let second = world.capture::<A>(&request(60, 0.25, true, Mode::ORDINARY))?;
    Ok([first.content_hash.unwrap_or(0), second.content_hash.unwrap_or(0)])
}

/// The content hash of a frame at 1, 4 and 8 workers, prepared with the math
/// layer and with the platform's arithmetic.
pub fn content_hashes(out: &mut Json) -> Result<(), String> {
    out.string("frames", "tick 30 at factor 0.5 and tick 60 at factor 0.25, both reproducible, four views of 10 000 entities and two tile layers");
    let arithmetics: [(&str, fn(usize) -> Result<[u64; 2], String>); 2] = [("math_layer", content_hashes_with::<MathLayer>), ("platform", content_hashes_with::<Platform>)];
    for (name, hashes) in arithmetics {
        out.key(name);
        out.object_start();
        let mut all = Vec::new();
        for workers in [1, 4, 8] {
            let found = hashes(workers)?;
            out.key(&format!("workers_{workers}"));
            out.object_start();
            out.hex("tick_30", found[0]);
            out.hex("tick_60", found[1]);
            out.object_end();
            all.push(found);
        }
        out.bool("one_value_at_every_worker_count", all.windows(2).all(|pair| pair[0] == pair[1]));
        out.hex("frames_hash", reference_workload::run_hash(&all[0]));
        out.object_end();
    }
    Ok(())
}

/// What the math layer costs prepare against the platform's arithmetic.
pub fn prepare_cost(out: &mut Json) -> Result<(), String> {
    const REPETITIONS: usize = 31;
    let rig = Rig::new(Backend::Content, Presenter::SteppingThread)?;
    for budget in [1, 4] {
        let mut world = RenderedWorld::new(&rig, budget as u32, Size::Default, budget, budget);
        world.step(30);
        let frame = request(30, 0.5, true, Mode::ORDINARY);
        world.capture::<MathLayer>(&frame)?;
        let mut series: [[Vec<f64>; 2]; 2] = Default::default();
        let mut items = 0;
        for _ in 0..REPETITIONS {
            let (math, _, count) = world.renderer.measure_prepare::<MathLayer>(&frame);
            let (platform, _, _) = world.renderer.measure_prepare::<Platform>(&frame);
            items = count;
            series[0][0].push(math.wall_us);
            series[0][1].push(math.processor_us);
            series[1][0].push(platform.wall_us);
            series[1][1].push(platform.processor_us);
        }
        out.key(&format!("worker_budget_{budget}"));
        out.object_start();
        out.u64("items_in_four_views", items as u64);
        for (name, series) in ["math_layer", "platform"].into_iter().zip(&series) {
            write_summary(out, &format!("{name}_wall_us"), &series[0]);
            write_summary(out, &format!("{name}_processor_us"), &series[1]);
        }
        out.decimal("ratio_of_medians_wall", summary(&series[0][0])[1] / summary(&series[1][0])[1]);
        out.decimal("ratio_of_medians_processor", summary(&series[0][1])[1] / summary(&series[1][1])[1]);
        out.decimal("ratio_of_fastest_processor", summary(&series[0][1])[0] / summary(&series[1][1])[0]);
        out.object_end();
    }
    Ok(())
}

/// Declaring and compiling a graph of 300 passes and 600 resources, on one thread.
pub fn graph_cost(out: &mut Json) -> Result<(), String> {
    const REPETITIONS: usize = 300;
    let mut times = Vec::with_capacity(REPETITIONS);
    let mut found = graph::synthetic(300);
    for _ in 0..REPETITIONS {
        let started = Instant::now();
        found = std::hint::black_box(graph::synthetic(std::hint::black_box(300)));
        times.push(started.elapsed().as_secs_f64() * 1e6);
    }
    out.u64("passes", found.passes as u64);
    out.u64("resources", found.resources as u64);
    out.u64("passes_kept", found.kept as u64);
    out.u64("passes_removed", found.removed as u64);
    out.u64("transients_alive", found.transients_alive as u64);
    write_summary(out, "declare_and_compile_us", &times);
    out.decimal("percentile_99_us", percentile(&times, 0.99));
    out.bool("median_under_one_millisecond", summary(&times)[1] < 1_000.0);
    out.bool("largest_under_one_millisecond", summary(&times)[2] < 1_000.0);
    Ok(())
}

/// The extraction's bytes per tick for a tile layer's written blocks when
/// one tile changes, at three block sizes and two populations of written blocks.
pub fn tile_extraction(out: &mut Json) -> Result<(), String> {
    out.string("layer", "512 by 512 cells of two bytes; the written blocks are child entities in chunks of 256, with one change flag per chunk; one tile written per tick for 1 000 ticks");
    out.key("cases");
    out.array_start();
    for (population, prior_writes, spread) in [("a corner of the map edited", 64, 96), ("the whole map edited", 20_000, 512)] {
        for block_side in [16, 32, 64] {
            let cost = tile_extraction_cost(block_side, prior_writes, spread, 1_000);
            out.object_start();
            out.string("population", population);
            out.u64("block_side_cells", u64::from(cost.block_side));
            out.u64("tiles_written_before", u64::from(cost.prior_writes));
            out.u64("written_blocks", cost.written_blocks);
            out.u64("chunks_of_written_blocks", cost.chunks);
            out.u64("bytes_of_one_block", cost.block_bytes);
            out.decimal("bytes_gathered_from_the_domain_per_tick", cost.gathered_bytes_per_tick);
            out.decimal("bytes_copied_from_the_previous_slice_per_tick", cost.copied_bytes_per_tick);
            out.object_end();
        }
    }
    out.array_end();
    Ok(())
}

fn write_times(out: &mut Json, frames: &[FrameTimes]) {
    let series = |pick: fn(&FrameTimes) -> f64| frames.iter().map(pick).collect::<Vec<f64>>();
    out.key("stage_medians_us");
    out.object_start();
    out.decimal("refresh_wall", summary(&series(|times| times.refresh.wall_us))[1]);
    out.decimal("prepare_wall", summary(&series(|times| times.prepare.wall_us))[1]);
    out.decimal("prepare_processor", summary(&series(|times| times.prepare.processor_us))[1]);
    out.decimal("sort_wall", summary(&series(|times| times.sort.wall_us))[1]);
    out.decimal("sort_processor", summary(&series(|times| times.sort.processor_us))[1]);
    out.decimal("declare_and_compile", summary(&series(|times| times.declare_us))[1]);
    out.decimal("record_wall", summary(&series(|times| times.record.wall_us))[1]);
    out.decimal("record_processor", summary(&series(|times| times.record.processor_us))[1]);
    out.decimal("in_the_hand_off", summary(&series(|times| times.hand_off_us))[1]);
    out.decimal("submit", summary(&series(|times| times.submit_us))[1]);
    out.decimal("device", summary(&series(|times| times.device_us))[1]);
    out.decimal("read_back", summary(&series(|times| times.read_back_us))[1]);
    out.decimal("start_to_image", summary(&series(|times| times.latency_us))[1]);
    out.object_end();
}

/// The bytes of a frame in the ordinary mode, in the reference mode, and
/// with each switch of the reference mode alone.
pub fn modes(out: &mut Json, backend: Backend) -> Result<(), String> {
    let rig = Rig::new(backend, Presenter::Thread)?;
    let mut world = RenderedWorld::new(&rig, 0, Size::Default, 4, 4);
    let ordinary = Mode::ORDINARY;
    let variants = [
        ("ordinary", ordinary),
        ("reference", Mode::REFERENCE),
        ("recording_serial_alone", Mode { parallel_recording: false, ..ordinary }),
        ("culling_off_alone", Mode { culling: false, ..ordinary }),
        ("unread_passes_kept_alone", Mode { remove_unread_passes: false, ..ordinary }),
        ("transients_marked_alone", Mode { markers: true, ..ordinary }),
    ];
    out.key("frames");
    out.array_start();
    let mut every_frame_equal = true;
    for (tick, factor) in [(30, 0.5), (60, 0.25)] {
        world.step(30);
        let mut first: Option<Delivered> = None;
        for (name, mode) in variants {
            let delivered = world.capture::<MathLayer>(&request(tick, factor, true, mode))?;
            out.object_start();
            out.u64("tick", u64::from(delivered.stamp.tick));
            out.decimal("factor", f64::from(delivered.stamp.factor));
            out.bool("complete", delivered.stamp.complete);
            out.bool("reproducible", delivered.stamp.reproducible);
            out.string("mode", name);
            write_optional_hex(out, "image_hash", hash_of(&delivered));
            write_optional_hex(out, "content_hash", delivered.content_hash);
            out.u64("draws", delivered.stamp.draws);
            out.u64("passes", delivered.stamp.passes);
            if let Some(first) = &first {
                let same = first.image == delivered.image;
                every_frame_equal &= same && first.content_hash == delivered.content_hash;
                out.bool("bytes_of_the_ordinary_mode", same);
                if let (Some(a), Some(b)) = (&first.image, &delivered.image)
                    && let Some(difference) = a.difference(b)
                    && !same
                {
                    out.u64("differing_pixels", difference.differing_pixels);
                    out.u64("largest_difference", u64::from(difference.largest.into_iter().max().unwrap_or(0)));
                    save_picture(&format!("tick-{tick}-{name}"), b);
                }
            } else {
                if let Some(image) = &delivered.image {
                    save_picture(&format!("tick-{tick}-ordinary"), image);
                }
                first = Some(delivered);
            }
            out.object_end();
        }
    }
    out.array_end();
    out.bool("every_mode_gives_the_bytes_of_the_ordinary_mode", every_frame_equal);
    Ok(())
}

/// The lateness of ticks and the rate of frames at 60 ticks a second, with
/// recording and submission overlapping the next ticks and without.
pub fn pace(out: &mut Json, backend: Backend) -> Result<(), String> {
    const TICKS: u32 = 180;
    let period = Duration::from_secs_f64(1.0 / 60.0);
    out.string("loop", "180 ticks due every sixtieth of a second on the pacing clock; a tick that is due runs before a frame is started; a tick's lateness is the time between its due instant and its start");
    out.key("runs");
    out.array_start();
    for (policy, one_frame_a_tick) in [("one frame after each tick, when a frame of the world may start", true), ("frames back to back, whenever a frame of the world may start", false)] {
        for budget in [1_usize, 4] {
            for presenter in [Presenter::Thread, Presenter::SteppingThread] {
                let rig = Rig::new(backend, presenter)?;
                let mut world = RenderedWorld::new(&rig, 0, Size::Default, budget, budget);
                world.step(2);
                world.warm_up::<MathLayer>(&Request { content_hash: false, ..request(2, 1.0, false, Mode::ORDINARY) })?;
                let (mut lateness, mut frames, mut started_frames) = (Vec::new(), Vec::new(), 0_u64);
                let start = Instant::now();
                let (mut done, mut drawn_tick) = (0, 0);
                while done < TICKS {
                    let due = start + period * done;
                    let now = Instant::now();
                    let tick = world.world.tick();
                    if now >= due {
                        lateness.push((now - due).as_secs_f64() * 1e6);
                        world.step(1);
                        done += 1;
                    } else if (!one_frame_a_tick || drawn_tick != tick) && rig.instance.try_begin(0) {
                        let since_tick = (now + period - due).as_secs_f64() / period.as_secs_f64();
                        let frame = Request { tick, factor: since_tick.clamp(0.0, 1.0) as f32, reproducible: false, content_hash: false, mode: Mode::ORDINARY };
                        world.renderer.frame::<MathLayer>(&frame).map_err(|error| error.to_string())?;
                        started_frames += 1;
                        drawn_tick = tick;
                        if presenter == Presenter::SteppingThread {
                            rig.instance.drain();
                        }
                    } else {
                        std::thread::yield_now();
                    }
                    while let Ok(StreamEvent::Image(delivered)) = world.stream.try_recv() {
                        frames.push(delivered.times);
                    }
                }
                let elapsed = start.elapsed().as_secs_f64();
                while rig.instance.busy() {
                    if presenter == Presenter::SteppingThread {
                        rig.instance.serve();
                    }
                    std::thread::yield_now();
                }
                while let Ok(StreamEvent::Image(delivered)) = world.stream.try_recv() {
                    frames.push(delivered.times);
                }
                out.object_start();
                out.string("frames", policy);
                out.u64("worker_budget", budget as u64);
                out.string("presenter", if presenter == Presenter::Thread { "presentation thread: recording and submission overlap the next ticks" } else { "stepping thread: submission and read-back follow recording, no overlap" });
                out.bool("overlap", presenter == Presenter::Thread);
                write_summary(out, "tick_lateness_us", &lateness);
                out.decimal("tick_lateness_percentile_99_us", percentile(&lateness, 0.99));
                out.u64("frames_started", started_frames);
                out.u64("frames_delivered", frames.len() as u64);
                out.decimal("frames_per_second", frames.len() as f64 / elapsed);
                out.decimal("ticks_per_second", f64::from(TICKS) / elapsed);
                write_times(out, &frames);
                out.object_end();
            }
        }
    }
    out.array_end();
    Ok(())
}

/// 1, 4 and 16 worlds drawn offscreen through one presenter, each stepped
/// by its own thread with a worker budget of one.
pub fn shared_device(out: &mut Json, backend: Backend) -> Result<(), String> {
    let (frames_wanted, deadline) = if backend.draws_pixels() { (12, Duration::from_secs(20)) } else { (40, Duration::from_secs(60)) };
    out.string("worlds_each", "its own stepping thread, a worker budget of one, one tick then one frame of its four views, lossless: the step is held while the frame may not start");
    out.key("counts");
    out.array_start();
    let mut cost_at_one = None;
    for count in [1_u32, 4, 16] {
        let rig = Rig::new(backend, Presenter::Thread)?;
        let barrier = Barrier::new(count as usize);
        let results: Mutex<Vec<Result<Vec<FrameTimes>, String>>> = Mutex::new(Vec::new());
        let started = Mutex::new(None);
        std::thread::scope(|threads| {
            for id in 0..count {
                let (rig, barrier, results, started) = (&rig, &barrier, &results, &started);
                threads.spawn(move || {
                    let mut world = RenderedWorld::new(rig, id, Size::Default, 1, 1);
                    world.step(2);
                    let warm = world.warm_up::<MathLayer>(&Request { content_hash: false, ..request(2, 1.0, false, Mode::ORDINARY) });
                    barrier.wait();
                    let begun = *started.lock().unwrap().get_or_insert_with(Instant::now);
                    let mut frames = Vec::new();
                    let outcome = warm.and_then(|()| {
                        while frames.len() < frames_wanted && (frames.len() < 3 || begun.elapsed() < deadline) {
                            world.step(1);
                            let tick = world.world.tick();
                            let delivered = world.capture::<MathLayer>(&Request { tick, factor: 1.0, reproducible: false, content_hash: false, mode: Mode::ORDINARY })?;
                            frames.push(delivered.times);
                        }
                        Ok(frames)
                    });
                    results.lock().unwrap().push(outcome);
                });
            }
        });
        let mut per_world_cost = Vec::new();
        let mut all = Vec::new();
        for result in results.into_inner().unwrap() {
            let frames = result?;
            per_world_cost.push(frames.iter().map(|times| times.latency_us).sum::<f64>() / frames.len() as f64);
            all.extend(frames);
        }
        let cost = per_world_cost.iter().sum::<f64>() / per_world_cost.len() as f64;
        let processor = all.iter().map(|times| times.refresh.processor_us + times.prepare.processor_us + times.sort.processor_us + times.declare_us + times.record.processor_us).sum::<f64>() / all.len() as f64;
        let presenter = rig.instance.presenter_counters();
        out.object_start();
        out.u64("worlds", u64::from(count));
        out.u64("frames_measured_per_world", (all.len() / count as usize) as u64);
        out.decimal("frame_cost_per_world_us", cost);
        write_summary(out, "frame_cost_across_worlds_us", &per_world_cost);
        out.decimal("stages_processor_time_per_frame_us", processor);
        out.decimal("presenter_time_per_frame_us", presenter.busy_us / presenter.frames.max(1) as f64);
        out.u64("most_frames_in_flight", presenter.most_in_flight as u64);
        if count == 1 {
            cost_at_one = Some(cost);
        }
        if let Some(one) = cost_at_one {
            out.decimal("frame_cost_against_one_world", cost / one);
        }
        write_times(out, &all);
        write_memory(out, &rig.instance.port, count);
        out.object_end();
    }
    out.array_end();
    Ok(())
}

fn write_memory(out: &mut Json, port: &Port, worlds: u32) {
    let memory = port.memory();
    let sum = |owned_by_a_world: bool, class: MemoryClass, peak: bool| memory.iter().filter(|(owner, found, _, _)| owner.is_some() == owned_by_a_world && *found == class).map(|&(_, _, now, most)| if peak { most } else { now }).sum::<u64>();
    out.key("device_memory_bytes");
    out.object_start();
    out.u64("per_world_targets", sum(true, MemoryClass::Targets, true) / u64::from(worlds));
    out.u64("per_world_geometry_of_tile_blocks", sum(true, MemoryClass::Geometry, true) / u64::from(worlds));
    out.u64("instance_textures", sum(false, MemoryClass::Textures, true));
    out.u64("instance_geometry", sum(false, MemoryClass::Geometry, true));
    out.u64("instance_pipelines_payload", sum(false, MemoryClass::Pipelines, true));
    out.u64("instance_transient_targets_at_peak", sum(false, MemoryClass::Transient, true));
    out.object_end();
}

/// A loss simulated at the port: the streams end, the device is created
/// again, and the images that follow are those a world that lost nothing draws.
pub fn loss(out: &mut Json, backend: Backend) -> Result<(), String> {
    let rig = Rig::new(backend, Presenter::Thread)?;
    let frame = |tick| request(tick, 1.0, true, Mode::ORDINARY);
    let identity = |delivered: &Delivered| (hash_of(delivered), delivered.content_hash);
    // The control: a world that loses nothing, drawn at the same ticks.
    let mut control = RenderedWorld::new(&rig, 1, Size::Default, 4, 4);
    control.step(20);
    let mut expected = Vec::new();
    for tick in 20..=23 {
        expected.push(identity(&control.capture::<MathLayer>(&frame(tick))?));
        control.step(1);
    }
    let mut world = RenderedWorld::new(&rig, 0, Size::Default, 4, 4);
    world.step(20);
    let mut before = Vec::new();
    before.push(identity(&world.capture::<MathLayer>(&frame(20))?));
    world.step(1);
    before.push(identity(&world.capture::<MathLayer>(&frame(21))?));
    world.step(1);
    let (created_before, built_before) = (rig.instance.port.counters().created, world.renderer.blocks_built);
    // A frame is on its way when the adapter starts answering "lost".
    world.start::<MathLayer>(&frame(22))?;
    rig.instance.port.simulate_loss();
    let ended = loop {
        match world.stream.recv_timeout(Duration::from_secs(30)) {
            Ok(StreamEvent::Ended(reason)) => break Ok(reason),
            Ok(StreamEvent::Image(_)) => {}
            Err(error) => break Err(format!("the stream did not end: {error}")),
        }
    };
    out.bool("the_stream_ended", ended.is_ok());
    out.string("the_stream_said", &ended.clone().unwrap_or_else(|reason| reason));
    let refused = world.start::<MathLayer>(&frame(22));
    out.bool("a_frame_asked_while_lost_is_refused", refused.is_err());
    // The capability tries to create the device again, a few times, at growing intervals.
    let mut recovered = Err("not tried".to_string());
    for attempt in 0..4 {
        std::thread::sleep(Duration::from_millis(10 << attempt));
        recovered = rig.instance.recover(0);
        if recovered.is_ok() {
            break;
        }
    }
    out.bool("the_device_was_created_again", recovered.is_ok());
    recovered?;
    world.subscribe();
    let mut after = Vec::new();
    after.push(identity(&world.capture::<MathLayer>(&frame(22))?));
    world.step(1);
    after.push(identity(&world.capture::<MathLayer>(&frame(23))?));
    let counters = rig.instance.port.counters();
    out.u64("frames_dropped", rig.instance.presenter_counters().dropped);
    out.u64("stale_handles_refused", counters.stale_refused);
    out.u64("losses", counters.losses);
    out.u64("resources_created_again", counters.created - created_before);
    out.u64("tile_blocks_built_again", world.renderer.blocks_built - built_before);
    out.bool("the_images_before_the_loss_are_the_control_s", before == expected[..2]);
    out.bool("the_images_after_the_recovery_are_the_control_s", after == expected[2..]);
    for (name, (image, content)) in ["tick_22_after", "tick_23_after"].into_iter().zip(&after) {
        write_optional_hex(out, &format!("{name}_image_hash"), *image);
        write_optional_hex(out, &format!("{name}_content_hash"), *content);
    }
    Ok(())
}

/// A frame asked reproducible after ten thousand ticks unwatched, against
/// the same tick's frame in a world watched from its first tick.
pub fn unwatched(out: &mut Json, backend: Backend) -> Result<(), String> {
    const TICKS: u32 = 10_000;
    let stride = if backend.draws_pixels() { 500 } else { 10 };
    let rig = Rig::new(backend, Presenter::Thread)?;
    let mut watched = RenderedWorld::new(&rig, 0, Size::Default, 4, 4);
    let mut watched_frames = 0_u64;
    while watched.world.tick() < TICKS {
        watched.step(stride);
        let tick = watched.world.tick();
        watched.capture::<MathLayer>(&Request { tick, factor: 1.0, reproducible: false, content_hash: false, mode: Mode::ORDINARY })?;
        watched_frames += 1;
    }
    let mut unseen = RenderedWorld::new(&rig, 1, Size::Default, 4, 4);
    unseen.step(TICKS);
    let reproducible = request(TICKS, 1.0, true, Mode::ORDINARY);
    let interactive = watched.capture::<MathLayer>(&request(TICKS, 1.0, false, Mode::ORDINARY))?;
    let from_watched = watched.capture::<MathLayer>(&reproducible)?;
    let from_unseen = unseen.capture::<MathLayer>(&reproducible)?;
    out.u64("ticks", u64::from(TICKS));
    out.u64("frames_the_watched_world_drew_before", watched_frames);
    out.u64("ticks_between_two_of_them", u64::from(stride));
    out.u64("tile_blocks_built_in_the_watched_world", watched.renderer.blocks_built);
    out.u64("tile_blocks_built_in_the_unwatched_world", unseen.renderer.blocks_built);
    out.bool("the_watched_world_holds_a_history", watched.renderer.has_history());
    write_optional_hex(out, "watched_image_hash", hash_of(&from_watched));
    write_optional_hex(out, "unwatched_image_hash", hash_of(&from_unseen));
    write_optional_hex(out, "watched_content_hash", from_watched.content_hash);
    write_optional_hex(out, "unwatched_content_hash", from_unseen.content_hash);
    out.bool("the_two_reproducible_frames_are_the_same", from_watched.image == from_unseen.image && from_watched.content_hash == from_unseen.content_hash);
    out.bool("the_interactive_frame_of_the_watched_world_read_its_history", interactive.stamp.history);
    out.bool("the_interactive_frame_differs_from_the_reproducible_one", interactive.content_hash != from_watched.content_hash);
    if let Some(image) = &from_unseen.image {
        save_picture("tick-10000-unwatched", image);
    }
    Ok(())
}

/// The time from a shader's new payload being resident to the frame that shows it.
pub fn shader_payload(out: &mut Json, backend: Backend) -> Result<(), String> {
    let rig = Rig::new(backend, Presenter::Thread)?;
    let mut world = RenderedWorld::new(&rig, 0, Size::Default, 4, 4);
    world.step(30);
    let frame = request(30, 0.5, true, Mode::ORDINARY);
    let mut periods = Vec::new();
    let mut before = None;
    for _ in 0..5 {
        let delivered = world.capture::<MathLayer>(&frame)?;
        periods.push(delivered.times.latency_us);
        before = Some(delivered);
    }
    let before = before.expect("five frames were drawn");
    let importing = Instant::now();
    let payload = rig.instance.port.import_shader(1).map_err(|error| error.to_string())?;
    let import_us = importing.elapsed().as_secs_f64() * 1e6;
    out.string("payload_form", payload.form);
    out.u64("payload_bytes", (payload.vertex.len() + payload.fragment.len()) as u64);
    // The payload becomes resident while a frame of the world is on its way.
    let in_progress = world.start::<MathLayer>(&frame)?;
    rig.instance.shader_resident(1, payload);
    let resident = Instant::now();
    let mut frames_between = 0_u64;
    let shown = loop {
        match world.next_event(Duration::from_secs(120))? {
            StreamEvent::Image(delivered) if delivered.stamp.shader_variant == 1 => break *delivered,
            StreamEvent::Image(delivered) => {
                frames_between += 1;
                if delivered.number >= in_progress {
                    world.start::<MathLayer>(&frame)?;
                }
            }
            StreamEvent::Ended(reason) => return Err(reason),
        }
    };
    let shown_us = resident.elapsed().as_secs_f64() * 1e6;
    let installed = rig.instance.shader_installed().ok_or("the presenter installed no payload")?;
    out.decimal("import_us", import_us);
    out.decimal("pipelines_built_us", installed.pipelines_built_us);
    out.u64("variant_installed", u64::from(installed.variant));
    out.decimal("resident_to_cache_entry_published_us", (installed.published - installed.resident).as_secs_f64() * 1e6);
    out.decimal("resident_to_the_image_that_shows_it_us", shown_us);
    out.u64("images_delivered_in_between_with_the_old_payload", frames_between);
    out.decimal("frame_start_to_image_median_us", summary(&periods)[1]);
    write_optional_hex(out, "image_hash_before", hash_of(&before));
    write_optional_hex(out, "image_hash_after", hash_of(&shown));
    out.bool("the_image_changed", before.image != shown.image);
    out.bool("the_content_hash_is_unchanged", before.content_hash == shown.content_hash);
    if let Some(image) = &shown.image {
        save_picture("tick-30-second-shader-payload", image);
    }
    Ok(())
}

/// One step of the lifetimes measurement, as it is reported.
pub struct LifetimeStep {
    pub name: &'static str,
    pub fields: Vec<(String, String)>,
}

impl LifetimeStep {
    fn new(name: &'static str) -> LifetimeStep {
        LifetimeStep { name, fields: Vec::new() }
    }

    fn set(&mut self, key: &str, value: impl std::fmt::Display) {
        self.fields.push((key.to_string(), value.to_string()));
    }

    fn messages(&mut self, port: &Port) {
        let messages = port.validation_messages().unwrap_or_default();
        self.set("validation_messages", messages.len());
        if let Some(first) = messages.first() {
            self.set("first_validation_message", first.chars().take(400).collect::<String>());
        }
    }
}

/// What the lifetimes measurement draws with.
struct LifetimeScene {
    target: crate::port::Handle,
    pipelines: crate::port::Handle,
    textures: [Source; 5],
}

impl LifetimeScene {
    /// Declares a frame of `draws` draws from `buffer`, records and submits
    /// it without waiting, and returns its number and its submission.
    fn submit(&self, port: &Port, buffer: crate::port::Handle, draws: usize) -> Result<(u64, u64), String> {
        let frame = port.declare_frame();
        let run = || {
            let pass = port.open_pass(frame, &PassSpec { target: self.target, clear: CLEAR_COLOR, pipelines: self.pipelines, textures: self.textures, parts: 1 })?;
            let draw = |index: usize| PortDraw {
                buffer,
                first_vertex: 0,
                vertex_count: 6,
                blended: index % 2 == 1,
                constants: DrawConstants { transform: scale_translate(0.75, 0.75, 0.0, 0.0), tint: [1.0, 0.5, 0.25, 0.5], texture: TEXTURE_WHITE, sampler: SAMPLER_NEAREST },
            };
            port.record_part(frame, pass, 0, &(0..draws).map(draw).collect::<Vec<_>>())?;
            port.close_pass(pass)?;
            port.submit(frame, &[pass], &[self.target])
        };
        match run() {
            Ok(submission) => Ok((frame, submission)),
            Err(error) => {
                port.frame_done(frame);
                Err(error.to_string())
            }
        }
    }

    fn finish(&self, port: &Port, (frame, submission): (u64, u64)) -> Result<(), String> {
        while !port.finished(submission).map_err(|error| error.to_string())? {
            std::thread::yield_now();
        }
        port.take_images(submission).map_err(|error| error.to_string())?;
        port.frame_done(frame);
        Ok(())
    }
}

/// Releases ordered rightly, then wrongly on purpose, under the bound of
/// three frames in flight. `report` is called after each step, so that what
/// was found is kept if a wrong order ends the process.
pub fn lifetimes(backend: Backend, mut report: impl FnMut(&LifetimeStep)) -> Result<(), String> {
    let port = backend.open()?;
    let text = |error: crate::port::PortError| error.to_string();
    let scene = Scene::build();
    let mut textures = Vec::new();
    for texture in &scene.textures {
        textures.push(Source::Texture(port.create_texture(texture).map_err(text)?));
    }
    let drawing = LifetimeScene {
        target: port.create_target(64, 64, Some(0), MemoryClass::Targets).map_err(text)?,
        pipelines: port.create_pipelines(&port.import_shader(0).map_err(text)?).map_err(text)?,
        textures: [textures[0], textures[1], textures[2], textures[3], textures[4]],
    };
    let quad = unit_quad();
    let buffer = || port.create_vertex_buffer(vertex_bytes(&quad), Some(0)).map_err(text);

    // A resource released while three frames that name it are in flight.
    let mut step = LifetimeStep::new("released_under_three_frames_in_flight");
    let released = buffer()?;
    let mut flights = Vec::new();
    for _ in 0..FRAMES_IN_FLIGHT {
        flights.push(drawing.submit(&port, released, 64)?);
    }
    let destroyed_before = port.counters().destroyed;
    port.release(released).map_err(text)?;
    step.set("frames_in_flight_at_the_release", flights.len());
    step.set("destroyed_at_the_release", port.counters().destroyed - destroyed_before);
    let mut destroyed_after_each = Vec::new();
    for flight in flights {
        drawing.finish(&port, flight)?;
        destroyed_after_each.push((port.counters().destroyed - destroyed_before).to_string());
    }
    step.set("destroyed_after_each_frame_finished", destroyed_after_each.join(", "));
    step.set("destroyed_only_after_the_last_frame", destroyed_after_each == ["0", "0", "1"]);
    step.set("port_assertions_tripped", port.counters().queued_named + port.counters().destroyed_under_a_frame);
    step.messages(&port);
    report(&step);

    // A command that names a resource already queued for destruction: the
    // port refuses it, and the device never sees it.
    let mut step = LifetimeStep::new("named_after_its_release");
    let holder = buffer()?;
    let queued = buffer()?;
    let pending = drawing.submit(&port, holder, 4)?;
    port.release(queued).map_err(text)?;
    let named = drawing.submit(&port, queued, 4);
    step.set("the_port_refused_the_command", named.is_err());
    step.set("the_port_said", named.err().unwrap_or_default());
    step.set("port_assertion_tripped", port.counters().queued_named);
    drawing.finish(&port, pending)?;
    port.release(holder).map_err(text)?;
    step.messages(&port);
    report(&step);

    // A resource destroyed at once under a frame the device has finished but
    // whose completion nobody has observed.
    let mut step = LifetimeStep::new("destroyed_under_a_frame_whose_completion_was_not_observed");
    let victim = buffer()?;
    let flight = drawing.submit(&port, victim, 4)?;
    std::thread::sleep(Duration::from_millis(200));
    step.set("port_assertion_tripped", port.destroy_now(victim).map_err(text)?);
    step.messages(&port);
    drawing.finish(&port, flight)?;
    report(&step);

    // A resource destroyed at once, right after the submission of a long frame.
    let mut step = LifetimeStep::new("destroyed_right_after_the_submission_of_a_long_frame");
    step.set("reached", true);
    report(&step);
    let victim = buffer()?;
    let flight = drawing.submit(&port, victim, 4_000)?;
    step.set("port_assertion_tripped", port.destroy_now(victim).map_err(text)?);
    drawing.finish(&port, flight)?;
    step.set("the_process_went_on", true);
    step.messages(&port);
    report(&step);
    port.wait_idle().map_err(text)?;
    Ok(())
}

/// A real removal of the device, where the interface offers a call that
/// removes one: what the interface then says, what a frame asked next meets,
/// and whether the way back of a simulated loss serves a real one.
pub fn removal(out: &mut Json, backend: Backend) -> Result<(), String> {
    let rig = Rig::new(backend, Presenter::Thread)?;
    let frame = |tick| request(tick, 1.0, true, Mode::ORDINARY);
    let identity = |delivered: &Delivered| (hash_of(delivered), delivered.content_hash);
    let mut control = RenderedWorld::new(&rig, 1, Size::Default, 4, 4);
    control.step(20);
    let mut expected = Vec::new();
    for tick in 20..=22 {
        expected.push(identity(&control.capture::<MathLayer>(&frame(tick))?));
        control.step(1);
    }
    let mut world = RenderedWorld::new(&rig, 0, Size::Default, 4, 4);
    world.step(20);
    let before = identity(&world.capture::<MathLayer>(&frame(20))?);
    out.bool("the_image_before_is_the_control_s", before == expected[0]);
    match rig.instance.port.remove_device() {
        Ok(called) => out.string("called", &called),
        Err(reason) => {
            out.bool("tried", false);
            out.string("why_not", &reason.to_string());
            return Ok(());
        }
    }
    out.bool("tried", true);
    // The interface is asked for a while what became of its device.
    let asked = Instant::now();
    let mut reason = rig.instance.port.removed_reason();
    while reason.is_none() && asked.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
        reason = rig.instance.port.removed_reason();
    }
    out.bool("the_interface_reports_the_removal", reason.is_some());
    out.string("the_interface_says", reason.as_deref().unwrap_or("nothing: the device is not reported removed"));
    world.step(1);
    match world.capture::<MathLayer>(&frame(21)) {
        Ok(delivered) => {
            out.string("a_frame_asked_after_the_call", "was drawn");
            out.bool("with_the_control_s_bytes", identity(&delivered) == expected[1]);
        }
        Err(error) => out.string("a_frame_asked_after_the_call", &format!("was dropped: {error}")),
    }
    if reason.is_none() {
        return Ok(());
    }
    // The capability takes the interface's word for a loss and comes back
    // the way it does from a simulated one.
    rig.instance.port.simulate_loss();
    let ended = loop {
        match world.stream.recv_timeout(Duration::from_secs(30)) {
            Ok(StreamEvent::Ended(reason)) => break Ok(reason),
            Ok(StreamEvent::Image(_)) => {}
            Err(error) => break Err(format!("the stream did not end: {error}")),
        }
    };
    out.bool("the_stream_ended", ended.is_ok());
    let mut recovered = Err("not tried".to_string());
    for attempt in 0..6 {
        std::thread::sleep(Duration::from_millis(20 << attempt));
        recovered = rig.instance.recover(0);
        if recovered.is_ok() {
            out.u64("attempts_to_create_the_device_again", attempt + 1);
            break;
        }
    }
    out.bool("the_device_was_created_again", recovered.is_ok());
    if let Err(reason) = &recovered {
        out.string("the_creation_said", reason);
        return Ok(());
    }
    world.subscribe();
    let mut after = Vec::new();
    after.push(identity(&world.capture::<MathLayer>(&frame(21))?));
    world.step(1);
    after.push(identity(&world.capture::<MathLayer>(&frame(22))?));
    out.bool("the_images_after_the_recovery_are_the_control_s", after == expected[1..]);
    write_optional_hex(out, "tick_22_after_image_hash", after[1].0);
    Ok(())
}
