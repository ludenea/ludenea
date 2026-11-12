//! Prepared collision geometry: a throwaway spike of the assets and content
//! pipelines research, never product code.
//!
//! Collision shapes built from large inputs are the one part of a physics
//! world an importer could prepare ahead: this spike measures, with the two
//! physics libraries under study, whether a prepared form of such a shape
//! exists, whether its bytes and the steps of a world that reads it are the
//! same on every cell, and what building it outside the step saves.
//!
//! This program is the Rapier 3D half (a triangle mesh with its acceleration
//! structure, with and without the fixed internal edges flag, and a convex
//! hull) and the parent of the Box2D half: it runs the C program the setup
//! script builds and embeds its report. It writes the prepared forms of the
//! smallest ladder size under `out/<cell>/forms/` and reads those committed
//! under `forms-from-other-cells/<cell>/`.
//!
//! Usage: `physics-prepared-geometry <cell> [--quick]`, from the spikes
//! directory. Prints one JSON document on standard output and its progress on
//! standard error. See README.md.

mod box2d;
mod inputs;
mod kinds;
mod measure;
mod worlds;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use reference_workload::{Json, write_platform_and_build};

use kinds::{ConvexHull, TerrainMesh};
use measure::{KindReport, OtherCell, Settings, Timing, measure};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "physics-prepared-geometry";
const RAPIER_VERSION: &str = "0.36.1";
const PARRY_VERSION: &str = "0.31.1";
const FIXED_STEP_MS: f64 = 1_000.0 / 60.0;
const LADDER: [usize; 3] = [1_000, 10_000, 100_000];
const STEPS: usize = 600;
const BODIES: usize = 150;

/// The sizes of a run; a quick run divides the ladder, the steps and the
/// bodies by ten. A timed build or read is repeated 7 times in an optimized
/// build and 3 times in an unoptimized one.
fn settings(quick: bool) -> Settings {
    let divisor = if quick { 10 } else { 1 };
    Settings {
        quick,
        ladder: LADDER.map(|elements| elements / divisor),
        steps: STEPS / divisor,
        bodies: BODIES / divisor,
        repetitions: if cfg!(debug_assertions) { 3 } else { 7 },
    }
}

/// Where this run writes its forms: `out/<cell>/forms/` under the working
/// directory, which the research CI publishes when the spike runs from the
/// spikes directory, the temporary directory otherwise.
fn forms_directory(cell: &str) -> PathBuf {
    let candidate = Path::new("out").join(cell).join("forms");
    if fs::create_dir_all(&candidate).is_ok() {
        return candidate;
    }
    let fallback = env::temp_dir().join(format!("{SPIKE}-{cell}")).join("forms");
    if let Err(error) = fs::create_dir_all(&fallback) {
        eprintln!("{}: {error}", fallback.display());
    }
    fallback
}

/// The forms other cells wrote, as committed with the spike: one directory
/// per cell, in the order of their names.
fn other_cells(root: &Path) -> Vec<OtherCell> {
    let mut cells: Vec<OtherCell> = fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .map(|directory| OtherCell { name: directory.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(), directory })
                .collect()
        })
        .unwrap_or_default();
    cells.sort_by(|a, b| a.name.cmp(&b.name));
    cells
}

/// A decimal with four digits after the point: a tenth of a microsecond for
/// a duration in milliseconds.
fn fine(out: &mut Json, key: &str, value: f64) {
    out.raw(key, &format!("{value:.4}"));
}

fn write_timing(out: &mut Json, prefix: &str, timing: &Timing) {
    fine(out, &format!("{prefix}_ms_median"), timing.median);
    fine(out, &format!("{prefix}_ms_fastest"), timing.fastest);
    fine(out, &format!("{prefix}_ms_slowest"), timing.slowest);
}

fn write_kind(out: &mut Json, kind: &KindReport, forms_from_other_cells_present: bool) {
    out.object_start();
    out.string("kind", kind.kind);
    out.string("variant", kind.variant);
    out.string("prepared_form", &kind.prepared_form);
    out.string("how", &kind.how);
    out.bool("measured_in_full", kind.measured());
    out.string("form_file", kind.form_file);
    out.u64("form_file_bytes", kind.form_file_bytes as u64);
    out.bool("form_file_written", kind.form_file_written);
    out.key("ladder");
    out.array_start();
    for row in &kind.ladder {
        out.object_start();
        out.u64("elements", row.elements as u64);
        out.hex("input_hash", row.input_hash);
        out.u64("built", row.built as u64);
        write_timing(out, "build", &row.build);
        fine(out, "build_over_fixed_step", row.build.median / FIXED_STEP_MS);
        out.u64("form_bytes", row.form_bytes as u64);
        out.hex("form_hash", row.form_hash);
        write_timing(out, "read", &row.read);
        out.bool("same_bytes_twice", row.same_bytes_twice);
        out.bool("same_bytes_after_a_round_trip", row.same_bytes_after_a_round_trip);
        out.object_end();
    }
    out.array_end();
    out.key("behavior");
    out.object_start();
    if let Some(behavior) = &kind.behavior {
        out.u64("steps", behavior.steps as u64);
        out.u64("bodies", behavior.bodies as u64);
        out.bool("forms_from_other_cells_present", forms_from_other_cells_present);
        out.key("sources");
        out.array_start();
        for source in &behavior.sources {
            out.object_start();
            out.string("source", &source.source);
            out.bool("read_succeeded", source.read_succeeded);
            out.bool("form_bytes_equal_local", source.form_bytes_equal_local);
            out.i64("bytes_differing_from_local", source.bytes_differing_from_local);
            out.i64("first_differing_step", source.first_differing_step);
            out.hex("run_hash", source.run_hash);
            out.object_end();
        }
        out.array_end();
    }
    out.object_end();
    out.object_end();
}

fn write_report(
    cell: &str,
    settings: &Settings,
    forms_directory: &Path,
    other_cells: &[OtherCell],
    kinds: &[KindReport],
    box2d: &Result<String, String>,
) -> String {
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    out.string("profile", if cfg!(debug_assertions) { "dev" } else { "release" });
    out.bool("quick", settings.quick);
    out.bool("complete", box2d.is_ok() && kinds.iter().all(KindReport::measured));
    fine(&mut out, "fixed_step_ms", FIXED_STEP_MS);
    out.u64("repetitions", settings.repetitions as u64);
    write_platform_and_build(&mut out);
    out.string("forms_directory", &forms_directory.to_string_lossy().replace('\\', "/"));
    out.key("forms_from_other_cells");
    out.array_start();
    for other in other_cells {
        out.object_start();
        out.string("cell", &other.name);
        out.object_end();
    }
    out.array_end();
    out.key("rapier");
    out.object_start();
    out.string("version", RAPIER_VERSION);
    out.string("parry_version", PARRY_VERSION);
    out.string("license", "Apache-2.0");
    out.string("features", "enhanced-determinism, serde-serialize");
    out.string("serializer", "bincode 1.3.3");
    out.key("settings");
    out.object_start();
    out.raw("time_step", &format!("{}", 1.0f32 / 60.0));
    out.u64("threads", 1);
    out.u64("steps", settings.steps as u64);
    out.object_end();
    out.key("kinds");
    out.array_start();
    for kind in kinds {
        write_kind(&mut out, kind, !other_cells.is_empty());
    }
    out.array_end();
    out.object_end();
    match box2d {
        Ok(report) => out.raw("box2d", report),
        Err(reason) => {
            out.key("box2d");
            out.object_start();
            out.bool("ran", false);
            out.string("reason", reason);
            out.object_end();
        }
    }
    out.object_end();
    out.finish()
}

fn main() {
    let mut cell = String::from("unspecified");
    let mut quick = false;
    for argument in env::args().skip(1) {
        if argument == "--quick" {
            quick = true;
        } else {
            cell = argument;
        }
    }
    let settings = settings(quick);
    let forms_directory = forms_directory(&cell);
    let other_cells_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("forms-from-other-cells");
    let other_cells = other_cells(&other_cells_root);

    let forms = Some(forms_directory.as_path());
    let kinds = [
        measure(&TerrainMesh { fixed_internal_edges: false }, &settings, forms, &other_cells),
        measure(&TerrainMesh { fixed_internal_edges: true }, &settings, forms, &other_cells),
        measure(&ConvexHull, &settings, forms, &other_cells),
    ];

    // A missing or failing Box2D program is recorded and the facts of the
    // Rapier half are still published.
    let box2d = box2d::run(&cell, quick, &forms_directory, &other_cells_root, &other_cells);
    if let Err(reason) = &box2d {
        eprintln!("box2d: not measured: {reason}");
    }
    println!("{}", write_report(&cell, &settings, &forms_directory, &other_cells, &kinds, &box2d));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::PreparedKind;
    use reference_workload::fnv1a;

    fn small() -> Settings {
        Settings { quick: true, ladder: [24, 48, 96], steps: 60, bodies: 6, repetitions: 2 }
    }

    /// A directory of this test's own under the temporary directory.
    fn scratch(name: &str) -> PathBuf {
        let directory = env::temp_dir().join(format!("{SPIKE}-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("a scratch directory");
        directory
    }

    fn assert_holds(report: &KindReport) {
        let name = format!("{} {}", report.kind, report.variant);
        assert_eq!(report.prepared_form, "yes", "{name}");
        assert_eq!(report.ladder.len(), 3, "{name}");
        for row in &report.ladder {
            assert!(row.built > 0 && row.form_bytes > 0, "{name}");
            assert!(row.same_bytes_twice, "{name}: two builds gave different bytes");
            assert!(row.same_bytes_after_a_round_trip, "{name}: the form read back and written again differs");
        }
        let behavior = report.behavior.as_ref().expect("a behavior");
        assert_eq!(behavior.bodies, 7, "{name}");
        for source in &behavior.sources {
            assert!(source.read_succeeded && source.form_bytes_equal_local, "{name} {}", source.source);
            assert_eq!(source.first_differing_step, -1, "{name} {}", source.source);
            assert_eq!(source.run_hash, behavior.sources[0].run_hash, "{name} {}", source.source);
        }
    }

    #[test]
    fn fnv1a_reference_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn a_timing_is_the_median_and_the_extremes() {
        let timing = Timing::of(vec![3.0, 1.0, 2.0]);
        assert_eq!((timing.fastest, timing.median, timing.slowest), (1.0, 2.0, 3.0));
    }

    #[test]
    fn a_quick_run_divides_the_sizes_by_ten() {
        let (full, quick) = (settings(false), settings(true));
        assert_eq!((full.ladder, full.steps, full.bodies), ([1_000, 10_000, 100_000], 600, 150));
        assert_eq!((quick.ladder, quick.steps, quick.bodies), ([100, 1_000, 10_000], 60, 15));
    }

    #[test]
    fn a_form_read_back_steps_like_the_built_shape() {
        let directory = scratch("own-forms");
        let reports = [
            measure(&TerrainMesh { fixed_internal_edges: false }, &small(), Some(&directory), &[]),
            measure(&TerrainMesh { fixed_internal_edges: true }, &small(), Some(&directory), &[]),
            measure(&ConvexHull, &small(), Some(&directory), &[]),
        ];
        for report in &reports {
            assert_holds(report);
            assert!(report.form_file_written);
            assert_eq!(report.behavior.as_ref().expect("a behavior").sources.len(), 2);
            let written = fs::metadata(directory.join(report.form_file)).expect("the form file").len();
            assert_eq!(written, report.form_file_bytes as u64);
        }
        // The two variants of the mesh are two forms.
        assert_ne!(reports[0].ladder[0].form_hash, reports[1].ladder[0].form_hash);
        assert_eq!(reports[0].ladder[0].input_hash, reports[1].ladder[0].input_hash);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn the_fixed_internal_edges_flag_adds_pseudo_normals_only() {
        let kind = TerrainMesh { fixed_internal_edges: true };
        let (_, mesh) = kind.build(&kind.input(24)).expect("a mesh");
        assert!(mesh.pseudo_normals().is_some());
        assert!(mesh.topology().is_none());
        let plain = TerrainMesh { fixed_internal_edges: false };
        let (_, mesh) = plain.build(&plain.input(24)).expect("a mesh");
        assert!(mesh.pseudo_normals().is_none());
    }

    #[test]
    fn a_form_from_another_cell_is_read_and_compared() {
        let kind = ConvexHull;
        let settings = small();
        let root = scratch("other-cells");
        let (same, changed, absent) = (root.join("same"), root.join("changed"), root.join("absent"));
        for directory in [&same, &changed, &absent] {
            fs::create_dir_all(directory).expect("a cell directory");
        }
        let (_, hull) = kind.build(&kind.input(settings.ladder[0])).expect("a hull");
        fs::write(same.join(kind.file()), kind.form(&hull).expect("a form")).expect("a form file");
        let (_, other) = kind.build(&kind.input(settings.ladder[1])).expect("a hull");
        fs::write(changed.join(kind.file()), kind.form(&other).expect("a form")).expect("a form file");

        let cells = other_cells(&root);
        assert_eq!(cells.iter().map(|cell| cell.name.as_str()).collect::<Vec<_>>(), ["absent", "changed", "same"]);
        let report = measure(&kind, &settings, None, &cells);
        let sources = &report.behavior.as_ref().expect("a behavior").sources;
        assert_eq!(sources.iter().map(|source| source.source.as_str()).collect::<Vec<_>>(), [
            "built_locally",
            "read_from_this_run",
            "read_from_absent",
            "read_from_changed",
            "read_from_same"
        ]);
        assert!(!sources[2].read_succeeded);
        assert_eq!(sources[2].bytes_differing_from_local, -1);
        assert!(sources[3].read_succeeded && !sources[3].form_bytes_equal_local);
        assert_eq!(sources[3].bytes_differing_from_local, -1, "a form of another size");
        assert_ne!(sources[3].first_differing_step, -1);
        assert!(sources[4].read_succeeded && sources[4].form_bytes_equal_local);
        assert_eq!((sources[4].bytes_differing_from_local, sources[4].first_differing_step), (0, -1));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_report_is_json_with_every_kind() {
        let settings = small();
        let kinds = [
            measure(&TerrainMesh { fixed_internal_edges: false }, &settings, None, &[]),
            measure(&TerrainMesh { fixed_internal_edges: true }, &settings, None, &[]),
            measure(&ConvexHull, &settings, None, &[]),
        ];
        let forms = Path::new("out").join("test").join("forms");

        let without_box2d = write_report("test", &settings, &forms, &[], &kinds, &Err(String::from("not \"built\"")));
        assert!(box2d::is_one_json_object(&without_box2d), "{without_box2d}");
        for expected in [
            "\"kind\":\"triangle_mesh\",\"variant\":\"no_flags\"",
            "\"kind\":\"triangle_mesh\",\"variant\":\"fixed_internal_edges\"",
            "\"kind\":\"convex_hull\",\"variant\":\"\"",
            "\"prepared_form\":\"yes\"",
            "\"complete\":false",
            "\"box2d\":{\"ran\":false,\"reason\":\"not \\\"built\\\"\"}",
            "\"forms_from_other_cells_present\":false",
            "\"forms_directory\":\"out/test/forms\"",
        ] {
            assert!(without_box2d.contains(expected), "{expected} is missing from {without_box2d}");
        }

        let child = String::from("{\"ran\":true,\"kinds\":[{\"kind\":\"chain\"}]}\n");
        let with_box2d = write_report("test", &settings, &forms, &[], &kinds, &Ok(child));
        assert!(box2d::is_one_json_object(&with_box2d), "{with_box2d}");
        assert!(with_box2d.contains("\"complete\":true"));
        assert!(with_box2d.ends_with("\"box2d\":{\"ran\":true,\"kinds\":[{\"kind\":\"chain\"}]}}"));
    }

    #[test]
    fn a_kind_that_cannot_be_measured_is_recorded() {
        // A ladder size of two points has no hull: the kind is reported as
        // not established and the run is not complete.
        let settings = Settings { quick: true, ladder: [2, 48, 96], steps: 5, bodies: 2, repetitions: 1 };
        let report = measure(&ConvexHull, &settings, None, &[]);
        assert!(!report.measured());
        assert!(report.prepared_form.starts_with("not established: "), "{}", report.prepared_form);
        let json = write_report("test", &settings, Path::new("forms"), &[], &[report], &Ok(String::from("{}")));
        assert!(box2d::is_one_json_object(&json));
        assert!(json.contains("\"complete\":false") && json.contains("\"measured_in_full\":false"));
    }
}
