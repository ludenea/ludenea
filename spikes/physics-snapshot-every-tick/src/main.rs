//! Physics snapshot at every tick: a throwaway spike of the networking
//! research, never product code.
//!
//! Takes three parts after a tick of a physics world (the library's image,
//! the tables that map an entity to its library handle, the canonical bytes of
//! the components), restores them in place for a correction of several
//! ticks, and measures what that costs beside the fixed step, in Box2D v3.2.0
//! (a C program this one runs as a child, see `box2d/`) and in Rapier 3D
//! 0.36.1 (this program): a take, a whole correction inside one frame at
//! several cadences and depths, the memory of the ring, the image after a
//! churn of bodies and of a sleeping world, the continuation after a restore
//! at every frame, and a correction by write-back that needs no image.
//!
//! Prints one JSON document on standard output. See README.md.

mod experiments;
mod scenes;
mod sim;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use reference_workload::{Json, write_platform_and_build};

use experiments::{
    CORRECTION_FORMS, Config, CorrectionResult, ForwardResult, Summary, WriteBackResult, run_asleep, run_churn,
    run_continuation, run_corrections, run_forward, run_write_back,
};
use scenes::{LIBRARY_SCENES, SCENE_KINDS};
use sim::{FIXED_STEP_MS, IMPULSES_PER_TICK, make_pool};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "physics-snapshot-every-tick";
const BOX2D_PROGRAM: &str = "physics-snapshot-every-tick-box2d";

fn number(out: &mut Json, key: &str, value: f64) {
    out.raw(key, &format!("{value:.4}"));
}

fn summary(out: &mut Json, name: &str, summary: Summary) {
    number(out, &format!("{name}_median"), summary.median);
    number(out, &format!("{name}_p99"), summary.p99);
    number(out, &format!("{name}_max"), summary.max);
}

fn write_forward(out: &mut Json, r: &ForwardResult) {
    out.object_start();
    out.string("scene", r.scene.name());
    out.u64("bodies", r.bodies as u64);
    out.u64("workers", r.workers as u64);
    out.u64("ticks", r.ticks as u64);
    summary(out, "step_ms", r.step);
    summary(out, "read_back_ms", r.sync);
    summary(out, "take_image_ms", r.image);
    summary(out, "take_tables_ms", r.tables);
    summary(out, "take_canonical_ms", r.canonical);
    summary(out, "take_ms", r.take);
    summary(out, "tick_and_take_ms", r.tick_and_take);
    let one_take = r.take_bytes.median;
    let bodies = r.bodies as f64;
    out.u64("image_bytes", r.image_bytes.median as u64);
    out.u64("image_bytes_largest", r.image_bytes.max as u64);
    out.u64("tables_bytes", r.tables_bytes as u64);
    out.u64("canonical_bytes", r.canonical_bytes as u64);
    out.u64("take_bytes", one_take as u64);
    out.raw("image_bytes_per_body", &format!("{:.1}", r.image_bytes.median / bodies));
    out.raw("tables_bytes_per_body", &format!("{:.1}", r.tables_bytes as f64 / bodies));
    out.raw("canonical_bytes_per_body", &format!("{:.1}", r.canonical_bytes as f64 / bodies));
    out.u64("ring_slots", r.ring_slots as u64);
    out.u64("ring_bytes", r.ring_bytes as u64);
    number(out, "ring_over_ten_takes", if one_take > 0.0 { r.ring_bytes as f64 / (10.0 * one_take) } else { 0.0 });
    out.hex("final_hash", r.final_hash);
    out.object_end();
}

fn write_correction(out: &mut Json, r: &CorrectionResult) {
    out.object_start();
    out.string("scene", r.scene.name());
    out.u64("bodies", r.bodies as u64);
    out.u64("workers", r.workers as u64);
    out.string("takes", r.form.name);
    out.u64("cadence", r.form.cadence as u64);
    out.u64("depth", r.form.depth as u64);
    out.u64("ticks_applied_again_at_most", r.ticks_again_at_most as u64);
    out.u64("corrections", r.corrections_run as u64);
    out.bool("settled_before_the_end", r.settled_before_the_end);
    out.u64("frames_over_the_fixed_step", r.exceeding as u64);
    summary(out, "frame_ms", r.frame);
    summary(out, "restore_ms", r.restore);
    summary(out, "tick_again_ms", r.tick_again);
    out.bool("fits_one_fixed_step", r.frame.p99 <= FIXED_STEP_MS);
    out.u64("ring_slots", r.ring_slots as u64);
    out.u64("ring_bytes", r.ring_bytes as u64);
    out.hex("final_hash", r.final_hash);
    match r.continuation {
        Some(equal) => out.bool("final_hash_equals_the_uninterrupted_world", equal),
        None => out.raw("final_hash_equals_the_uninterrupted_world", "null"),
    }
    out.object_end();
}

fn write_write_back(out: &mut Json, r: &WriteBackResult) {
    out.object_start();
    out.string("scene", r.scene.name());
    out.u64("bodies", r.bodies as u64);
    out.u64("workers", r.workers as u64);
    out.bool("identical_before_the_correction", r.identical_before_the_correction);
    out.i64("first_differing_tick_of_the_eight", r.first_differing_tick_of_the_eight);
    out.raw("largest_error_over_body_size", &format!("{:.6e}", r.largest_error_over_body_size));
    out.raw("largest_error_distance", &format!("{:.6e}", r.largest_error_distance));
    out.i64("first_tick_the_hashes_part", r.first_tick_the_hashes_part);
    out.i64("tick_they_meet_again", r.tick_they_meet_again);
    out.bool("equal_at_the_end", r.equal_at_the_end);
    number(out, "highest_body_uninterrupted", r.highest_uninterrupted);
    number(out, "highest_body_corrected", r.highest_corrected);
    out.object_end();
}

/// The Rapier half: every experiment, as one JSON object in the shape of the
/// Box2D half's report.
fn rapier_report(cell: &str, config: &Config, only: Option<&str>) -> String {
    let wanted = |section: &str| only.is_none_or(|only| only == section);
    let pools = [make_pool(1), make_pool(4)];
    let mut out = Json::new();
    out.object_start();
    out.string("library", "rapier3d");
    out.string("dimension", "3d");
    out.string("cell", cell);
    out.string("version", "0.36.1");
    out.string("license", "Apache-2.0");
    out.string("features", "enhanced-determinism, parallel, serde-serialize");
    out.string("image_written_by", "bincode 1.3.3 (MIT)");
    out.bool("quick", config.quick);
    out.string("architecture", std::env::consts::ARCH);
    out.string("compiler", "rustc, the pinned toolchain");
    out.key("settings");
    out.object_start();
    out.raw("time_step", &format!("{:.9}", 1.0 / 60.0));
    out.u64("solver_iterations", 4);
    number(&mut out, "fixed_step_ms", FIXED_STEP_MS);
    out.u64("impulses_per_tick", IMPULSES_PER_TICK as u64);
    out.u64("warm_up_ticks", config.warmup_ticks as u64);
    out.u64("take_ticks", config.forward_ticks as u64);
    out.u64("corrections", config.corrections as u64);
    out.u64("corrections_before_a_settled_verdict_stops_a_run", config.settle_after as u64);
    out.u64("continuation_frames", config.continuation_frames as u64);
    out.u64("churn_ticks", config.churn_ticks as u64);
    out.u64("bodies_of_the_claims", config.claim_bodies as u64);
    out.object_end();

    out.key("takes_after_every_tick");
    out.array_start();
    if wanted("takes") {
        for scene in SCENE_KINDS {
            for &bodies in &config.ladder {
                for pool in &pools {
                    let r = run_forward(config, scene, bodies, pool);
                    eprintln!(
                        "rapier takes {} {} bodies {} workers: step {:.3} ms, take {:.3} ms (p99 {:.3}), {:.0} bytes",
                        scene.name(),
                        r.bodies,
                        r.workers,
                        r.step.median,
                        r.take.median,
                        r.take.p99,
                        r.take_bytes.median
                    );
                    write_forward(&mut out, &r);
                }
            }
        }
    }
    out.array_end();

    out.key("corrections");
    out.array_start();
    if wanted("corrections") {
        for scene in SCENE_KINDS {
            for &bodies in &config.ladder {
                for form in &CORRECTION_FORMS {
                    let r = run_corrections(config, scene, bodies, &pools[1], form);
                    eprintln!(
                        "rapier corrections {} {} bodies, takes {}, depth {}: frame {:.3} ms (p99 {:.3}) over {}{}",
                        scene.name(),
                        r.bodies,
                        form.name,
                        form.depth,
                        r.frame.median,
                        r.frame.p99,
                        r.corrections_run,
                        if r.settled_before_the_end { ", settled" } else { "" }
                    );
                    write_correction(&mut out, &r);
                }
            }
        }
    }
    out.array_end();

    out.key("continuation");
    out.array_start();
    if wanted("continuation") {
        for scene in &LIBRARY_SCENES {
            for pool in &pools {
                let r = run_continuation(config, scene, pool);
                eprintln!(
                    "rapier continuation {} {} bodies {} workers: first differing tick {}, {:.1} s",
                    r.scene, r.bodies, r.workers, r.first_differing_tick, r.wall_seconds
                );
                out.object_start();
                out.string("scene", r.scene);
                out.u64("bodies", r.bodies as u64);
                out.u64("workers", r.workers as u64);
                out.u64("frames", r.frames as u64);
                out.u64("corrections_with_the_same_inputs", r.same_inputs_again as u64);
                out.u64("corrections_with_an_amended_input", r.amended as u64);
                out.u64("mispredicted_ticks", r.mispredicted_ticks as u64);
                out.u64("mispredicted_ticks_that_differed", r.mispredicted_ticks_that_differed as u64);
                out.u64("ticks_compared", r.ticks_compared as u64);
                out.i64("first_differing_tick", r.first_differing_tick);
                out.hex("final_hash", r.final_hash);
                out.hex("uninterrupted_final_hash", r.uninterrupted_final_hash);
                out.u64("image_bytes", r.image_bytes as u64);
                out.raw("wall_seconds", &format!("{:.1}", r.wall_seconds));
                out.object_end();
            }
        }
    }
    out.array_end();

    if wanted("churn") {
        let r = run_churn(config, &pools[1]);
        eprintln!(
            "rapier churn: image {} bytes after the churn, {} bytes fresh",
            r.churned_image_bytes, r.fresh_image_bytes
        );
        out.key("churn");
        out.object_start();
        out.string("scene", "bodies_in_motion");
        out.u64("bodies", r.bodies as u64);
        out.u64("ticks", r.ticks as u64);
        out.u64("destroyed_and_created_per_tick", r.changes_per_tick as u64);
        out.u64("churned_image_bytes", r.churned_image_bytes as u64);
        out.u64("fresh_image_bytes", r.fresh_image_bytes as u64);
        number(&mut out, "churned_over_fresh", r.churned_image_bytes as f64 / r.fresh_image_bytes as f64);
        out.u64("churned_take_bytes", r.churned_take_bytes as u64);
        out.u64("fresh_take_bytes", r.fresh_take_bytes as u64);
        out.bool("the_fresh_world_holds_the_same_components", r.same_components);
        out.object_end();
    } else {
        out.raw("churn", "null");
    }

    if wanted("asleep") {
        let r = run_asleep(config, &pools[1]);
        eprintln!(
            "rapier asleep: {} of {} bodies asleep after {} ticks, take {:.4} ms asleep and {:.4} ms awake",
            r.asleep_bodies, r.bodies, r.ticks_until_asleep, r.asleep_take.median, r.awake_take.median
        );
        out.key("asleep");
        out.object_start();
        out.string("scene", "pile_at_rest");
        out.u64("bodies", r.bodies as u64);
        out.u64("ticks_until_asleep", r.ticks_until_asleep as u64);
        out.u64("asleep_bodies", r.asleep_bodies as u64);
        out.u64("awake_bodies_after_waking", r.awake_bodies_after_waking as u64);
        summary(&mut out, "asleep_take_ms", r.asleep_take);
        summary(&mut out, "awake_take_ms", r.awake_take);
        out.u64("asleep_image_bytes", r.asleep_image_bytes as u64);
        out.u64("awake_image_bytes", r.awake_image_bytes as u64);
        out.u64("asleep_take_bytes", r.asleep_take_bytes as u64);
        out.u64("awake_take_bytes", r.awake_take_bytes as u64);
        let time = if r.awake_take.median > 0.0 { r.asleep_take.median / r.awake_take.median } else { 0.0 };
        number(&mut out, "asleep_over_awake_time", time);
        number(&mut out, "asleep_over_awake_bytes", r.asleep_take_bytes as f64 / r.awake_take_bytes as f64);
        out.object_end();
    } else {
        out.raw("asleep", "null");
    }

    out.key("write_back");
    out.array_start();
    if wanted("write_back") {
        for scene in SCENE_KINDS {
            for pool in &pools {
                let r = run_write_back(config, scene, pool);
                eprintln!(
                    "rapier write-back {} {} workers: hashes part at tick {}, largest error {:.3e} of a body",
                    scene.name(),
                    r.workers,
                    r.first_tick_the_hashes_part,
                    r.largest_error_over_body_size
                );
                write_write_back(&mut out, &r);
            }
        }
    }
    out.array_end();
    out.object_end();
    out.finish()
}

/// Where the setup script installed the Box2D half: under the working
/// directory when the spike runs from `spikes/`, beside the workspace
/// otherwise, or where the environment says.
fn box2d_program() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PHYSICS_SNAPSHOT_BOX2D") {
        return Some(PathBuf::from(path));
    }
    let relative = Path::new("build").join(BOX2D_PROGRAM).join("install").join("bin");
    let roots = [PathBuf::from("."), Path::new(env!("CARGO_MANIFEST_DIR")).join("..")];
    for root in roots {
        for name in [BOX2D_PROGRAM.to_string(), format!("{BOX2D_PROGRAM}.exe")] {
            let candidate = root.join(&relative).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Runs the Box2D half and returns its report, or why there is none.
fn box2d_report(cell: &str, quick: bool, only: Option<&str>) -> Result<String, String> {
    let program = box2d_program().ok_or("the Box2D program was not found; the setup script builds it")?;
    let mut command = Command::new(&program);
    command.arg(cell).stdout(Stdio::piped()).stderr(Stdio::inherit());
    if quick {
        command.arg("--quick");
    }
    if let Some(only) = only {
        command.args(["--only", only]);
    }
    let output = command.output().map_err(|error| format!("{} did not start: {error}", program.display()))?;
    if !output.status.success() {
        return Err(format!("{} ended with {}", program.display(), output.status));
    }
    let report = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    let report = report.trim();
    if !(report.starts_with('{') && report.ends_with('}')) {
        return Err("the Box2D program printed no JSON document".to_string());
    }
    Ok(report.to_string())
}

fn main() {
    let mut cell = String::from("unspecified");
    let mut quick = false;
    let mut only = None;
    let mut library = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--quick" => quick = true,
            "--only" => only = arguments.next(),
            "--library" => library = arguments.next(),
            _ => cell = argument,
        }
    }
    // A world without a handed-in pool would step on the global one: it is
    // restricted to one thread so that nothing steps on threads not counted.
    let _ = rayon::ThreadPoolBuilder::new().num_threads(1).build_global();
    let config = if quick { Config::quick() } else { Config::full() };
    let wants = |name: &str| library.as_deref().is_none_or(|library| library == name);

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    out.bool("quick", quick);
    out.raw("fixed_step_ms", &format!("{FIXED_STEP_MS:.4}"));
    write_platform_and_build(&mut out);
    let mut complete = true;
    if wants("box2d") {
        match box2d_report(&cell, quick, only.as_deref()) {
            Ok(report) => out.raw("box2d", &report),
            Err(reason) => {
                eprintln!("the Box2D half did not run: {reason}");
                complete = false;
                out.key("box2d");
                out.object_start();
                out.string("library", "box2d");
                out.bool("ran", false);
                out.string("reason", &reason);
                out.object_end();
            }
        }
    }
    if wants("rapier") {
        out.raw("rapier", &rapier_report(&cell, &config, only.as_deref()));
    }
    out.bool("complete", complete && library.is_none() && only.is_none());
    out.object_end();
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenes::build_scene;
    use crate::sim::{Take, fnv1a};

    #[test]
    fn fnv1a_reference_vectors() {
        assert_eq!(fnv1a(0xcbf2_9ce4_8422_2325, b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(0xcbf2_9ce4_8422_2325, b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn a_world_restored_and_stepped_again_is_the_world_never_restored() {
        let pool = make_pool(1);
        for scene in SCENE_KINDS {
            let mut a = build_scene(scene, 120, &pool, false, true);
            let mut b = build_scene(scene, 120, &pool, false, true);
            assert_eq!(a.count(), 120, "{}: the body count", scene.name());
            let mut take = Take::default();
            let mut at_take = 0;
            for tick in 1..=30 {
                a.tick(tick, false);
                b.tick(tick, false);
                assert_eq!(a.hash(), b.hash(), "{}: two runs differ at tick {tick}", scene.name());
                if tick == 20 {
                    a.take(&mut take, tick);
                    at_take = a.hash();
                }
            }
            let at_end = a.hash();
            a.restore(&take);
            assert_eq!(a.hash(), at_take, "{}: the restored components", scene.name());
            for tick in 21..=30 {
                a.tick(tick, false);
            }
            assert_eq!(a.hash(), at_end, "{}: restored and stepped again", scene.name());

            // A mispredicted input must change what follows.
            a.restore(&take);
            a.tick(21, true);
            b.restore(&take);
            b.tick(21, false);
            assert_ne!(a.hash(), b.hash(), "{}: a mispredicted input changed nothing", scene.name());
        }
    }

    #[test]
    fn a_quick_report_is_json_with_every_section() {
        let config = Config::quick();
        let report = rapier_report("test", &config, Some("asleep"));
        assert!(report.starts_with('{') && report.ends_with('}'));
        for key in ["takes_after_every_tick", "corrections", "continuation", "churn", "asleep", "write_back"] {
            assert!(report.contains(&format!("\"{key}\"")), "{key} is missing");
        }
    }
}
