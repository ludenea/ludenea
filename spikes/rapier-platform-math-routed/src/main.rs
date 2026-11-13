//! Rapier's worlds with its calls to the platform's math library routed to
//! the pure Rust library, for the physics research. Throwaway, never product
//! code.
//!
//! The earlier spike `rapier-platform-math-calls` found five calls Rapier 3D
//! 0.36.1 makes to the platform's math library on the path of a step with its
//! determinism feature on, and worlds that reach them hashing differently
//! from one cell to the next. This spike builds the same worlds, from that
//! spike's own source, against copies of the three libraries in which those
//! calls go to the pure Rust library the feature already uses (`patches/`,
//! applied by `setup.sh`). It reports their hashes at the same ticks and,
//! where a debugger is at hand, the calls their steps still make to the
//! platform's math library.
//!
//! Run with a cell label, it prints one JSON document. `--scene <name>
//! [ticks]` steps one world and prints its hashes, which is what the
//! debugger runs.

// The worlds and the count of the calls are the earlier spike's source,
// compiled here against the patched libraries: the same source, so the same
// worlds and the same breakpoints.
#[allow(dead_code)]
#[path = "../../rapier-platform-math-calls/src/reach.rs"]
mod reach;
#[allow(dead_code)]
#[path = "../../rapier-platform-math-calls/src/scenes.rs"]
mod scenes;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use reference_workload::{Fnv1a, Json, write_platform_and_build};

use scenes::{KINDS, Kind, Ran, TICKS};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "rapier-platform-math-routed";

/// The patches, as the build that made this program read them.
const PATCHES: [(&str, &str); 3] = [
    ("rapier3d-0.36.1", include_str!("../patches/rapier3d-0.36.1.patch")),
    ("parry3d-0.31.1", include_str!("../patches/parry3d-0.31.1.patch")),
    ("glamx-0.3.1", include_str!("../patches/glamx-0.3.1.patch")),
];
const PATCHED: [&str; 3] = ["rapier3d", "parry3d", "glamx"];
const LOCK_FILE: &str = include_str!("../Cargo.lock");

/// What a world gave with the libraries as published, on the five cells of
/// the research CI: its hash before the first step, one value everywhere, and
/// its final hashes with the cells that ended on each. Recorded from the
/// second run of the earlier spike (run 43450145269 of the research CI).
struct Published {
    kind: Kind,
    feature: bool,
    initial: u64,
    finals: &'static [(u64, &'static str)],
}

const ALL: &str = "linux-x86_64, linux-arm64, windows-x86_64, windows-arm64, macos-arm64";

const PUBLISHED: [Published; 7] = [
    Published { kind: Kind::Control, feature: true, initial: 0xb034_33b2_a9fd_a599, finals: &[(0x9787_20a6_934c_0050, ALL)] },
    Published {
        kind: Kind::JointLimits,
        feature: true,
        initial: 0xb3e7_4c3a_da1b_dbb5,
        finals: &[(0x831b_23d4_0845_9d46, "linux-x86_64, linux-arm64, windows-arm64"), (0xb7aa_90e8_b7a1_1b06, "windows-x86_64, macos-arm64")],
    },
    Published { kind: Kind::JointLimits, feature: false, initial: 0xb3e7_4c3a_da1b_dbb5, finals: &[(0xa890_594c_53a0_0ad1, ALL)] },
    Published {
        kind: Kind::MotorTargets,
        feature: true,
        initial: 0xbed9_d23f_75e8_08fa,
        finals: &[(0x8692_e31f_07f9_ac09, "linux-x86_64, linux-arm64, windows-arm64"), (0x7c60_3291_4166_4112, "windows-x86_64"), (0xbed8_4443_8121_46df, "macos-arm64")],
    },
    Published { kind: Kind::MotorTargets, feature: false, initial: 0xbed9_d23f_75e8_08fa, finals: &[(0x4f5d_5c00_da01_6a20, ALL)] },
    Published {
        kind: Kind::SeveralColliders,
        feature: true,
        initial: 0xc461_50ac_c266_daa1,
        finals: &[(0xe005_2200_e96c_2ea3, "linux-x86_64, linux-arm64"), (0xa05f_f51e_5255_774e, "windows-x86_64"), (0xa3d7_edd2_4272_3715, "windows-arm64, macos-arm64")],
    },
    Published {
        kind: Kind::BallsOnHulls,
        feature: true,
        initial: 0x579c_fe84_4869_96ff,
        finals: &[(0xea1a_f476_0a7d_6c60, "linux-x86_64, linux-arm64"), (0x3727_c169_10e5_4471, "windows-x86_64, windows-arm64"), (0x0398_64f8_7afb_d153, "macos-arm64")],
    },
];

fn published(kind: Kind, feature: bool) -> Option<&'static Published> {
    PUBLISHED.iter().find(|published| published.kind == kind && published.feature == feature)
}

/// The hash of a patch's text, whatever line endings the checkout gave it.
fn patch_hash(text: &str) -> u64 {
    let mut hash = Fnv1a::new();
    for line in text.lines() {
        hash.write(line.trim_end_matches('\r').as_bytes());
        hash.write(b"\n");
    }
    hash.finish()
}

/// Whether a lock file takes a package from a path and not from a registry:
/// its entry is there and names no source. With the build locked to it, the
/// three libraries are then the patched copies.
fn locked_to_a_path(lock: &str, package: &str) -> bool {
    let name = format!("name = \"{package}\"");
    let mut entries = lock.split("[[package]]").filter(|entry| entry.lines().any(|line| line.trim_end() == name));
    match (entries.next(), entries.next()) {
        (Some(entry), None) => !entry.lines().any(|line| line.starts_with("source = ")),
        _ => false,
    }
}

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
    if let Some(published) = published(ran.kind, ran.feature) {
        out.bool("starts_as_with_the_published_libraries", ran.initial_hash == published.initial);
        out.string("final_hashes_with_the_published_libraries", &published.finals.iter().map(|(hash, cells)| format!("{hash:#018x} on {cells}")).collect::<Vec<_>>().join("; "));
        let same = published.finals.iter().find(|(hash, _)| *hash == ran.final_hash()).map(|(_, cells)| *cells);
        out.string("the_published_libraries_ended_on_this_final_hash_on", same.unwrap_or("no cell"));
    }
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
    out.string("libraries", "rapier3d 0.36.1 with enhanced-determinism, parry3d 0.31.1 and glamx 0.3.1, each as published with this spike's patch applied; the patched calls go to libm 0.2.16");
    out.key("patches");
    out.object_start();
    for (name, text) in PATCHES {
        out.hex(name, patch_hash(text));
    }
    out.object_end();
    out.bool("the_lock_file_takes_the_three_libraries_from_the_patched_copies", PATCHED.iter().all(|package| locked_to_a_path(LOCK_FILE, package)));
    out.u64("ticks", TICKS as u64);

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
    fn every_world_and_every_twin_has_what_the_published_libraries_gave() {
        for kind in KINDS {
            let record = published(kind, true).unwrap_or_else(|| panic!("{} has no record", kind.name()));
            assert!(!record.finals.is_empty());
            assert_eq!(published(kind, false).is_some(), kind.has_a_twin(), "{}", kind.name());
        }
        assert_eq!(PUBLISHED.len(), KINDS.len() + KINDS.iter().filter(|kind| kind.has_a_twin()).count());
    }

    #[test]
    fn a_world_and_its_twin_start_alike_with_the_published_libraries_too() {
        for kind in KINDS.into_iter().filter(|kind| kind.has_a_twin()) {
            assert_eq!(published(kind, true).unwrap().initial, published(kind, false).unwrap().initial);
        }
    }

    #[test]
    fn a_patch_hashes_alike_with_either_line_ending() {
        assert_eq!(patch_hash("--- a\n+++ b\n+x\n"), patch_hash("--- a\r\n+++ b\r\n+x\r\n"));
        assert_ne!(patch_hash("+x\n"), patch_hash("+y\n"));
        for (name, text) in PATCHES {
            assert!(text.contains("ComplexField"), "{name} routes nothing");
        }
    }

    #[test]
    fn a_package_with_a_source_is_not_locked_to_a_path() {
        let lock = "[[package]]\nname = \"rapier3d\"\nversion = \"0.36.1\"\ndependencies = [\n \"parry3d\",\n]\n\n[[package]]\nname = \"parry3d\"\nversion = \"0.31.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"00\"\n";
        assert!(locked_to_a_path(lock, "rapier3d"));
        assert!(!locked_to_a_path(lock, "parry3d"));
        assert!(!locked_to_a_path(lock, "glamx"));
    }

    #[test]
    fn this_build_is_locked_to_the_patched_copies() {
        for package in PATCHED {
            assert!(locked_to_a_path(LOCK_FILE, package), "{package} comes from a registry");
        }
    }

    #[test]
    fn every_world_has_a_name_a_scene_can_be_asked_by() {
        for kind in KINDS {
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
        }
    }
}
