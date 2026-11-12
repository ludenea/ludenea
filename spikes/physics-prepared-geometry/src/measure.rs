//! What is measured for one shape kind: the ladder (the time to build the
//! shape against the time to read its prepared form, the form's bytes and
//! their hash, at three input sizes), the form file of the smallest size, and
//! the behavior (a world whose shape was read from a form, stepped against
//! the world that built the shape). Throwaway, never product code.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use reference_workload::{first_divergent_tick, fnv1a, run_hash};

use crate::kinds::{PreparedKind, milliseconds_since};
use crate::worlds::step_hashes;

/// The sizes of a run: the full measurement, a quick local check, or a test.
pub struct Settings {
    pub quick: bool,
    /// Input elements per ladder size, the smallest first.
    pub ladder: [usize; 3],
    pub steps: usize,
    /// Dynamic bodies of a behavior world.
    pub bodies: usize,
    /// Repetitions of a timed build or read.
    pub repetitions: usize,
}

/// A directory of forms written on another cell and committed with the spike.
pub struct OtherCell {
    pub name: String,
    pub directory: PathBuf,
}

/// The median, the fastest and the slowest of the repetitions, in milliseconds.
pub struct Timing {
    pub median: f64,
    pub fastest: f64,
    pub slowest: f64,
}

impl Timing {
    pub fn of(mut samples: Vec<f64>) -> Timing {
        assert!(!samples.is_empty(), "a timing needs a sample");
        samples.sort_by(f64::total_cmp);
        Timing { median: samples[samples.len() / 2], fastest: samples[0], slowest: samples[samples.len() - 1] }
    }
}

pub struct LadderRow {
    pub elements: usize,
    pub input_hash: u64,
    pub built: usize,
    pub build: Timing,
    pub form_bytes: usize,
    pub form_hash: u64,
    pub read: Timing,
    pub same_bytes_twice: bool,
    pub same_bytes_after_a_round_trip: bool,
}

pub struct SourceRow {
    pub source: String,
    pub read_succeeded: bool,
    pub form_bytes_equal_local: bool,
    /// The count of bytes that differ from the local form; -1 when the sizes
    /// differ or nothing was read.
    pub bytes_differing_from_local: i64,
    pub first_differing_step: i64,
    pub run_hash: u64,
}

/// The count of bytes that differ between two forms of one size, or -1 when
/// their sizes differ: a form that differs by a few bytes is not the same
/// finding as a form that differs throughout.
fn differing_bytes(form: &[u8], local_form: &[u8]) -> i64 {
    if form.len() != local_form.len() {
        return -1;
    }
    form.iter().zip(local_form).filter(|(byte, local)| byte != local).count() as i64
}

pub struct Behavior {
    pub steps: usize,
    /// Every body of the world, the fixed one included: all are hashed.
    pub bodies: usize,
    pub sources: Vec<SourceRow>,
}

pub struct KindReport {
    pub kind: &'static str,
    pub variant: &'static str,
    /// "yes" when a shape was built with no world, written and read back.
    pub prepared_form: String,
    pub how: String,
    pub form_file: &'static str,
    pub form_file_bytes: usize,
    pub form_file_written: bool,
    pub ladder: Vec<LadderRow>,
    pub behavior: Option<Behavior>,
}

impl KindReport {
    pub fn measured(&self) -> bool {
        self.behavior.is_some()
    }
}

/// One ladder size: the row, with the shape and its form for the caller that
/// keeps those of the smallest size.
fn ladder_row<K: PreparedKind>(kind: &K, elements: usize, repetitions: usize) -> Result<(LadderRow, K::Shape, Vec<u8>), String> {
    let input = kind.input(elements);
    let input_hash = kind.input_hash(&input);

    let mut build_samples = Vec::with_capacity(repetitions);
    let (first_milliseconds, shape) = kind.build(&input)?;
    build_samples.push(first_milliseconds);
    for _ in 1..repetitions {
        build_samples.push(kind.build(&input)?.0);
    }
    let form = kind.form(&shape)?;

    // A second build in the same process must give the same bytes.
    let (_, second) = kind.build(&input)?;
    let same_bytes_twice = kind.form(&second)? == form;
    drop(second);

    let mut read_samples = Vec::with_capacity(repetitions);
    let start = Instant::now();
    let read_back = kind.read(&form);
    read_samples.push(milliseconds_since(start));
    let read_back = read_back?;
    for _ in 1..repetitions {
        let start = Instant::now();
        let again = kind.read(&form);
        read_samples.push(milliseconds_since(start));
        drop(again?);
    }
    // The form read back and written again must give the same bytes.
    let same_bytes_after_a_round_trip = kind.form(&read_back)? == form;

    let row = LadderRow {
        elements,
        input_hash,
        built: kind.built(&shape),
        build: Timing::of(build_samples),
        form_bytes: form.len(),
        form_hash: fnv1a(&form),
        read: Timing::of(read_samples),
        same_bytes_twice,
        same_bytes_after_a_round_trip,
    };
    Ok((row, shape, form))
}

/// Steps a world whose shape is read from the bytes of a form and compares
/// it with the hashes of the world that built its shape.
fn source_row<K: PreparedKind>(
    kind: &K,
    source: String,
    bytes: Result<Vec<u8>, String>,
    local_form: &[u8],
    reference: &[u64],
    settings: &Settings,
) -> SourceRow {
    let mut row = SourceRow {
        source,
        read_succeeded: false,
        form_bytes_equal_local: false,
        bytes_differing_from_local: -1,
        first_differing_step: -1,
        run_hash: 0,
    };
    let world = bytes
        .and_then(|bytes| {
            row.form_bytes_equal_local = bytes == local_form;
            row.bytes_differing_from_local = differing_bytes(&bytes, local_form);
            kind.read(&bytes)
        })
        .and_then(|shape| kind.world(shape, settings.ladder[0], settings.bodies));
    match world {
        Ok(mut world) => {
            let hashes = step_hashes(&mut world, settings.steps);
            row.read_succeeded = true;
            row.first_differing_step = first_divergent_tick(reference, &hashes).map_or(-1, |step| step as i64);
            row.run_hash = run_hash(&hashes);
        }
        Err(error) => eprintln!("rapier {}, {}: {error}", kind.label(), row.source),
    }
    row
}

/// The worlds of one kind: built locally, read from the form this run wrote
/// (through its file when it was written), and read from each other cell's.
fn behavior<K: PreparedKind>(
    kind: &K,
    shape: K::Shape,
    form: &[u8],
    own_file: Option<&Path>,
    other_cells: &[OtherCell],
    settings: &Settings,
) -> Result<Behavior, String> {
    let mut world = kind.world(shape, settings.ladder[0], settings.bodies)?;
    let bodies = world.bodies.len();
    let reference = step_hashes(&mut world, settings.steps);
    drop(world);

    let mut sources = vec![SourceRow {
        source: String::from("built_locally"),
        read_succeeded: true,
        form_bytes_equal_local: true,
        bytes_differing_from_local: 0,
        first_differing_step: -1,
        run_hash: run_hash(&reference),
    }];
    let read_file = |path: &Path| fs::read(path).map_err(|error| format!("{}: {error}", path.display()));
    let own_bytes = own_file.map_or_else(|| Ok(form.to_vec()), read_file);
    sources.push(source_row(kind, String::from("read_from_this_run"), own_bytes, form, &reference, settings));
    for cell in other_cells {
        let bytes = read_file(&cell.directory.join(kind.file()));
        sources.push(source_row(kind, format!("read_from_{}", cell.name), bytes, form, &reference, settings));
    }
    for source in &sources {
        eprintln!("rapier {} world, {}: first differing step {}", kind.label(), source.source, source.first_differing_step);
    }
    Ok(Behavior { steps: settings.steps, bodies, sources })
}

fn measure_into<K: PreparedKind>(
    kind: &K,
    settings: &Settings,
    forms_directory: Option<&Path>,
    other_cells: &[OtherCell],
    report: &mut KindReport,
) -> Result<(), String> {
    let mut smallest = None;
    for elements in settings.ladder {
        let (row, shape, form) = ladder_row(kind, elements, settings.repetitions)?;
        eprintln!(
            "rapier {} {} elements: build {:.4} ms, read {:.4} ms, {} bytes",
            kind.label(),
            row.elements,
            row.build.median,
            row.read.median,
            row.form_bytes
        );
        report.ladder.push(row);
        if smallest.is_none() {
            smallest = Some((shape, form));
        }
    }
    let (shape, form) = smallest.ok_or("the ladder is empty")?;
    // Reaching this line is the first step's answer: at every size a shape
    // was built with no world, written, and read back.
    report.prepared_form = String::from("yes");
    report.how = kind.how(&shape);

    report.form_file_bytes = form.len();
    let own_file = forms_directory.map(|directory| directory.join(kind.file()));
    if let Some(path) = &own_file {
        fs::write(path, &form).map_err(|error| format!("{}: {error}", path.display()))?;
        report.form_file_written = true;
    }
    report.behavior = Some(behavior(kind, shape, &form, own_file.as_deref(), other_cells, settings)?);
    Ok(())
}

/// Measures one kind. A failure is recorded in the report, never fatal: the
/// other kinds and the other library are still measured and published.
pub fn measure<K: PreparedKind>(kind: &K, settings: &Settings, forms_directory: Option<&Path>, other_cells: &[OtherCell]) -> KindReport {
    let mut report = KindReport {
        kind: kind.kind(),
        variant: kind.variant(),
        prepared_form: String::from("not established"),
        how: String::new(),
        form_file: kind.file(),
        form_file_bytes: 0,
        form_file_written: false,
        ladder: Vec::new(),
        behavior: None,
    };
    if let Err(error) = measure_into(kind, settings, forms_directory, other_cells, &mut report) {
        eprintln!("rapier {}: not measured in full: {error}", kind.label());
        if report.prepared_form != "yes" {
            report.prepared_form = format!("not established: {error}");
        }
        report.how = format!("{} Not measured in full: {error}", report.how).trim().to_string();
    }
    report
}
