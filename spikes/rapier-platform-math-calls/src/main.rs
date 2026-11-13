//! Rapier's calls to the platform's math library on the path of a step, for
//! the physics research. Throwaway, never product code.
//!
//! With its determinism feature on, Rapier 3D 0.36.1 still calls the
//! platform's math library through methods on a plain `f32`, in five places a
//! step reaches. This spike measures whether the platforms differ on those
//! calls at all: on the functions themselves, against the pure Rust library
//! the feature routes the other calls to (`functions.rs`); on worlds built to
//! reach each call with many inputs, hashed at the same ticks on every cell
//! (`scenes.rs`); and, where a debugger is at hand, on the calls a world's
//! steps really make (`reach.rs`).
//!
//! Run with a cell label, it prints one JSON document. `--scene <name>
//! [ticks]` steps one world and prints its hashes, which is what the
//! debugger runs.

mod functions;
mod reach;
mod scenes;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use reference_workload::{Json, write_platform_and_build};

use scenes::{KINDS, Kind, Ran, TICKS};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "rapier-platform-math-calls";

fn output_dir(cell: &str) -> PathBuf {
    let candidate = Path::new("out").join(cell);
    if fs::create_dir_all(&candidate).is_ok() {
        return candidate;
    }
    let fallback = env::temp_dir().join(format!("{SPIKE}-{cell}"));
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn checkpoints_text(ran: &Ran) -> String {
    ran.checkpoints.iter().map(|(tick, hash)| format!("{tick}:{hash:016x}")).collect::<Vec<_>>().join(" ")
}

fn write_world(out: &mut Json, ran: &Ran) {
    out.object_start();
    out.string("world", ran.kind.name());
    out.string("what", ran.kind.what());
    out.string("variant", if ran.feature { "as described" } else { "the twin without the limits or the motors" });
    out.u64("bodies_hashed", ran.bodies as u64);
    out.u64("colliders", ran.colliders as u64);
    out.u64("joints", ran.joints as u64);
    out.hex("initial_hash", ran.initial_hash);
    out.string("checkpoints", &checkpoints_text(ran));
    out.hex("final_hash", ran.final_hash());
    out.hex("inertia_hash_after_the_build", ran.inertia_after_build.0);
    out.u64("bodies_with_a_turned_inertia_frame_after_the_build", ran.inertia_after_build.1 as u64);
    out.hex("inertia_hash_after_the_steps", ran.inertia_after_steps.0);
    out.u64("bodies_with_a_turned_inertia_frame_after_the_steps", ran.inertia_after_steps.1 as u64);
    out.u64("most_contact_pairs_of_a_ball_and_a_hull", ran.hull_contacts_peak.0 as u64);
    out.u64("most_contact_pairs_of_two_hulls", ran.hull_contacts_peak.1 as u64);
    out.object_end();
}

fn measure(cell: &str) {
    let output = output_dir(cell);
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.string("libraries", "rapier3d 0.36.1 with enhanced-determinism, parry3d 0.31.1, glamx 0.3.1, libm 0.2.16 as the reference");
    out.u64("ticks", TICKS as u64);

    functions::write(&mut out, &output);
    out.key("constants");
    out.object_start();
    functions::constants(&mut out, "half_centers_of_the_joint_limits", &scenes::limit_half_centers());
    functions::sines(&mut out, "half_targets_of_the_motors", &scenes::motor_half_targets());
    functions::constants(&mut out, "one_degree", &[scenes::one_degree()]);
    // The same call on a constant, as the collision library writes it: the
    // compiler may compute it when it builds, with no call left to make.
    let (sin, cos) = scenes::one_degree().sin_cos();
    out.string("one_degree_with_a_constant_argument", &format!("{:08x}:{:08x},{:08x}", scenes::one_degree().to_bits(), sin.to_bits(), cos.to_bits()));
    out.object_end();

    let mut acted = Vec::new();
    out.key("worlds");
    out.array_start();
    for kind in KINDS {
        let ran = scenes::run(kind, true, TICKS);
        write_world(&mut out, &ran);
        if kind.has_a_twin() {
            let twin = scenes::run(kind, false, TICKS);
            write_world(&mut out, &twin);
            acted.push((kind, ran.initial_hash == twin.initial_hash && ran.final_hash() != twin.final_hash()));
        }
    }
    out.array_end();
    out.key("the_feature_changes_what_its_world_does");
    out.object_start();
    for (kind, changes) in acted {
        out.bool(kind.name(), changes);
    }
    out.object_end();

    reach::write(&mut out, &output);
    out.object_end();
    println!("{}", out.finish());
}

/// `--scene <name> [ticks]`: one world, stepped, with its hashes on standard
/// output. A debugger counts what its steps call.
fn scene(arguments: &[String]) -> i32 {
    let Some(kind) = arguments.first().and_then(|name| Kind::from_name(name)) else {
        eprintln!("usage: --scene <{}> [ticks]", KINDS.map(Kind::name).join("|"));
        return 2;
    };
    let ticks = arguments.get(1).and_then(|ticks| ticks.parse().ok()).unwrap_or(TICKS);
    let ran = scenes::run(kind, true, ticks);
    println!("world={}", kind.name());
    println!("initial_hash={:016x}", ran.initial_hash);
    println!("checkpoints={}", checkpoints_text(&ran));
    0
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("--scene") {
        std::process::exit(scene(&arguments[1..]));
    }
    measure(arguments.first().map(String::as_str).unwrap_or("local"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_world_has_a_name_a_scene_can_be_asked_by() {
        for kind in KINDS {
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
            assert!(!kind.what().is_empty() && !kind.call().is_empty());
        }
    }
}
