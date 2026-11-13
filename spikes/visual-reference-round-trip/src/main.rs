//! Visual reference round trip spike, for the game testing research.
//! Throwaway, never product code.
//!
//! The spike checks, on files that travel between jobs and cells, the layout
//! of a visual reference (a content part, a byte part per device key, a pixel
//! file) and the three comparisons of a visual test, and measures what a
//! reference costs. It stands on the device baseline spike's scene and native
//! path and measures nothing of a renderer: its content hash is the hash of
//! the draw list its own code builds. On Mesa's software device on x86_64 it
//! also captures the frame with the device held to a stated processor
//! capability (`capability.rs`).
//!
//! Run with a cell label, it prints one JSON document and writes its pending
//! parts under `out/<cell>/pending/`. `frame.rs` builds the frame and the
//! changes brought about on purpose, `reference.rs` holds the files,
//! `comparison.rs` the three comparisons and `pixels.rs` the pixel file.

mod capability;
mod comparison;
mod frame;
mod pixels;
mod reference;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use reference_workload::{Json, fnv1a, write_platform_and_build};
use render_device_baseline::backend::{Api, DeviceInfo, DeviceKind, Recording, Setting};
use render_device_baseline::platform;
use render_device_baseline::scene::Image;

use comparison::{Capture, Compared, Fact, Outcome, Pending, Rendered, Run, bounded_difference, byte_equality, content_equality, review_group};
use frame::{AdapterFault, Change, Frame};
use pixels::{Bounds, Numbers};
use reference::{BytePart, ContentPart, ReferenceSet, pixel_file_name, write_accepted};

const SCHEMA_VERSION: u64 = 2;
const SPIKE: &str = "visual-reference-round-trip";
/// The device thread count the run states, set in the environment before any
/// device exists: the count is part of the key and cannot be set afterwards.
const STATED_DEVICE_THREADS: &str = "4";
/// The numbers the spike's test states for its bounded difference.
const BOUNDS: Bounds = Bounds { largest: 16, second: 4, allowed_beyond_second: 1_024 };
/// The three target sizes a capture is timed at.
const SIZES: [[u32; 2]; 3] = [[256, 256], [1_024, 1_024], [1_920, 1_080]];
const REPETITIONS: usize = 5;
const CHILD_TIMEOUT: Duration = Duration::from_secs(600);

fn output_dir(cell: &str) -> PathBuf {
    let candidate = Path::new("out").join(cell);
    if fs::create_dir_all(&candidate).is_ok() {
        return candidate;
    }
    let fallback = env::temp_dir().join(format!("{SPIKE}-{cell}"));
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The interface the cells' devices are reached through on this platform:
/// Direct3D 12 on Windows, Metal on macOS, Vulkan elsewhere.
fn open_api(work: &Path) -> Result<Box<dyn Api>, String> {
    let shaders = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("render-device-baseline").join("shaders");
    #[cfg(windows)]
    return render_device_baseline::d3d12::open(&shaders, work);
    #[cfg(target_os = "macos")]
    return render_device_baseline::metal::open(&shaders, work);
    #[cfg(not(any(windows, target_os = "macos")))]
    render_device_baseline::vulkan::open(&shaders, work)
}

/// The device a pipeline's cell asks for: a software device, or failing that
/// the device a virtual machine presents, or failing that the first listed.
fn choose_device(api: &dyn Api) -> Option<DeviceInfo> {
    let devices = api.devices();
    [DeviceKind::Software, DeviceKind::Virtual].iter().find_map(|kind| devices.iter().find(|info| info.kind == *kind).cloned()).or_else(|| devices.first().cloned())
}

/// The processor of this machine as the system names it: recorded beside the
/// key, since the machines of a hosted cell are not one model.
fn host_processor() -> String {
    let said = |command: &mut Command| command.stdin(Stdio::null()).output().ok().filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    let named = if cfg!(target_os = "linux") {
        fs::read_to_string("/proc/cpuinfo").ok().and_then(|text| {
            let field = |name: &str| text.lines().find(|line| line.starts_with(name)).and_then(|line| line.split_once(':')).map(|(_, value)| value.trim().to_string());
            field("model name").or_else(|| Some(format!("implementer {}, part {}, variant {}, revision {}", field("CPU implementer")?, field("CPU part")?, field("CPU variant")?, field("CPU revision")?)))
        })
    } else if cfg!(target_os = "macos") {
        said(Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"]))
    } else {
        said(Command::new("reg").args(["query", r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0", "/v", "ProcessorNameString"]))
            .and_then(|text| text.lines().find_map(|line| line.split_once("REG_SZ")).map(|(_, value)| value.to_string()))
            .or_else(|| env::var("PROCESSOR_IDENTIFIER").ok())
    };
    named.map(|name| one_line(&name)).filter(|name| !name.is_empty()).unwrap_or_else(|| "not reported".to_string())
}

/// The key a byte part is held under: the interface, the device, its
/// implementation and version, its settings (the stated thread count among
/// them where the device reads one) and the processor architecture.
fn key_of(api: &dyn Api, info: &DeviceInfo, settings: &str) -> String {
    [api.name(), &info.name, &info.driver, &info.interface_version, settings, env::consts::ARCH].join(" | ")
}

/// One capture on a device created from scratch: the device baseline's
/// Direct3D 12 path keeps a scene's resources for the life of a device, so a
/// frame that differs is drawn by a device of its own on every interface.
fn capture(api: &dyn Api, info: &DeviceInfo, frame: &Frame, fault: AdapterFault, processor: &str) -> Result<Capture, String> {
    let mut device = api.open(info.index, false)?;
    let render = device.render_scene(&frame.device_scene(fault), Recording::SerialOneBuffer)?;
    let key = key_of(api, info, &device.settings());
    Ok(Capture {
        content: ContentPart::of(&frame.content(), &frame.target, Frame::fixture_hash(), frame.tick),
        rendered: Some(Rendered { key, image: render.image, payloads: frame.payloads(), host_processor: processor.to_string() }),
    })
}

fn write_numbers(out: &mut Json, numbers: &Numbers) {
    out.key("numbers");
    out.object_start();
    out.string("largest_per_channel", &format!("{:?}", numbers.largest_per_channel));
    out.u64("differing_pixels", numbers.differing_pixels);
    out.u64("beyond_the_second_number", numbers.beyond_second);
    out.u64("beyond_the_largest_number", numbers.beyond_largest);
    out.string("rectangle", &format!("{:?}", numbers.rectangle));
    out.object_end();
}

fn verdict(outcome: &Outcome) -> &'static str {
    match (outcome.compared, outcome.passed) {
        (Compared::NotCompared, true) => "not compared",
        (Compared::NotCompared, false) => "not compared, fails on the content",
        (Compared::NotMade, true) => "not made",
        (Compared::NotMade, false) => "not made, fails on the content",
        (_, true) => "passes",
        (_, false) => "fails",
    }
}

fn pending_names(pending: &[Pending]) -> String {
    if pending.is_empty() { "none".to_string() } else { pending.iter().map(|pending| pending.name()).collect::<Vec<_>>().join(", ") }
}

fn write_outcome(out: &mut Json, name: &str, outcome: &Outcome) {
    out.key(name);
    out.object_start();
    out.string("verdict", verdict(outcome));
    out.string("compared", outcome.compared.name());
    out.bool("passed", outcome.passed);
    out.string("cause", &if outcome.facts.is_empty() { "nothing differs".to_string() } else { outcome.facts.iter().map(Fact::name).collect::<Vec<_>>().join("; ") });
    out.string("review_group", &review_group(&outcome.facts));
    out.string("pending", &pending_names(&outcome.pending));
    if let Some(numbers) = &outcome.numbers {
        write_numbers(out, numbers);
    }
    out.object_end();
}

fn listed_files(directory: &Path) -> String {
    let mut names: Vec<String> = fs::read_dir(directory).map(|entries| entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect()).unwrap_or_default();
    names.sort();
    if names.is_empty() { "none".to_string() } else { names.join(", ") }
}

/// The three comparisons of one capture against one set.
struct Compared3 {
    content: Outcome,
    bytes: Outcome,
    bounded: Outcome,
}

fn compare(capture: &Capture, set: &ReferenceSet, run: &Run) -> Compared3 {
    Compared3 { content: content_equality(capture, set, run), bytes: byte_equality(capture, set, run), bounded: bounded_difference(capture, set, &BOUNDS, run) }
}

fn write_compared(out: &mut Json, compared: &Compared3) {
    write_outcome(out, "content_equality", &compared.content);
    write_outcome(out, "byte_equality", &compared.bytes);
    write_outcome(out, "bounded_difference", &compared.bounded);
}

/// The reference sets the repository carries, by the cell that wrote each,
/// and this cell's own whether or not it exists yet.
fn carried_sets(cell: &str) -> Vec<(String, PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("references");
    let mut names: Vec<String> = fs::read_dir(&root).map(|entries| entries.flatten().filter(|entry| entry.path().is_dir()).filter_map(|entry| entry.file_name().into_string().ok()).collect()).unwrap_or_default();
    let own = format!("from-{cell}");
    if !names.contains(&own) {
        names.push(own);
    }
    names.sort();
    names.into_iter().map(|name| (name.clone(), root.join(name))).collect()
}

/// Reads each carried set as a later job does and compares this cell's
/// capture with it, as a run that takes references. Returns whether every
/// carried set compared as the design says.
fn write_carried_references(out: &mut Json, cell: &str, output: &Path, base: &Capture) -> bool {
    let rendered = base.rendered.as_ref().expect("the cell's device draws");
    let mut as_designed = true;
    out.key("carried_references");
    out.array_start();
    for (name, directory) in carried_sets(cell) {
        out.object_start();
        out.string("set", &name);
        let own = name == format!("from-{cell}");
        out.bool("written_by_this_cell", own);
        let pending = output.join("pending").join(&name);
        match ReferenceSet::read(&directory) {
            Err(reason) => {
                out.string("error", &reason);
                as_designed = false;
            }
            Ok(set) => {
                let exists = set.content.is_some() || !set.bytes.is_empty();
                out.bool("exists", exists);
                let under_this_key = set.byte_part(&rendered.key);
                out.bool("byte_part_under_this_key", under_this_key.is_some());
                out.key("byte_parts");
                out.array_start();
                for part in &set.bytes {
                    out.object_start();
                    out.string("key", &part.key);
                    out.string("host_processor", &part.host_processor);
                    out.hex("frame_bytes_hash", part.frame_bytes_hash);
                    out.bool("same_key", part.key == rendered.key);
                    out.bool("same_bytes", part.frame_bytes_hash == rendered.image.hash());
                    out.bool("same_host_processor", part.host_processor == rendered.host_processor);
                    out.bool("pixel_file_read", set.pixel_file(part).is_some_and(|file| pixels::read_strict(&file).is_ok_and(|image| image.hash() == part.frame_bytes_hash)));
                    out.object_end();
                }
                out.array_end();
                let compared = compare(base, &set, &Run { pending: &pending, takes_references: true });
                write_compared(out, &compared);
                // The design: a set that does not exist fails the three and
                // leaves its parts; a part under this key gives equal bytes;
                // a part under another key alone is not compared, the content
                // is equal and a pending byte part is left.
                let expected = if !exists {
                    !compared.content.passed && !compared.bytes.passed && !compared.bounded.passed
                } else if under_this_key.is_some() {
                    compared.content.passed && compared.bytes.passed && compared.bytes.compared == Compared::Equal
                } else {
                    compared.content.passed && compared.bytes.passed && compared.bytes.compared == Compared::NotCompared && compared.bytes.pending == [Pending::BytePartUnderANewKey]
                };
                out.bool("as_designed", expected);
                as_designed &= expected;
            }
        }
        out.string("pending_files", &listed_files(&pending));
        out.object_end();
    }
    out.array_end();
    as_designed
}

/// What a case of the causes table expects of the three comparisons.
struct Expected {
    content_passes: bool,
    bytes: (Compared, bool),
    bytes_pending: &'static [Pending],
    /// Nothing where the bounded difference is decided on its numbers.
    bounded: Option<(Compared, bool)>,
    /// Whether the pending part falls in the review group of a key that
    /// moved and nothing else.
    key_moved_alone: bool,
}

struct Case {
    name: &'static str,
    what: &'static str,
    change: Change,
    fault: AdapterFault,
    /// The reference the capture is compared with: accepted under this key,
    /// accepted under another key alone, or absent.
    reference: &'static str,
    /// Another target size than the reference's.
    size: Option<[u32; 2]>,
    /// The content device: no picture.
    draws: bool,
    expected: Expected,
}

const DECIDED_ON_ITS_NUMBERS: Option<(Compared, bool)> = None;

fn cases() -> Vec<Case> {
    let case = |name, what, change, reference, expected| Case { name, what, change, fault: AdapterFault::None, reference, size: None, draws: true, expected };
    let fails = (Compared::Different, false);
    vec![
        case(
            "nothing_changed",
            "the frame the reference was taken from, under its key",
            Change::None,
            "accepted",
            Expected { content_passes: true, bytes: (Compared::Equal, true), bytes_pending: &[], bounded: Some((Compared::Equal, true)), key_moved_alone: false },
        ),
        case(
            "reference_does_not_exist",
            "no part of the set exists",
            Change::None,
            "absent",
            Expected { content_passes: false, bytes: fails, bytes_pending: &[Pending::BytePart], bounded: Some(fails), key_moved_alone: false },
        ),
        Case {
            size: Some([512, 512]),
            ..case(
                "target_size_differs",
                "the frame is captured at another target size than the reference's",
                Change::None,
                "accepted",
                Expected { content_passes: false, bytes: fails, bytes_pending: &[Pending::BytePart], bounded: Some(fails), key_moved_alone: false },
            )
        },
        case(
            "item_moved_inside_the_view",
            "one triangle is moved six pixels inside the view",
            Change::ItemMovedInsideTheView,
            "accepted",
            Expected { content_passes: false, bytes: fails, bytes_pending: &[Pending::BytePart], bounded: DECIDED_ON_ITS_NUMBERS, key_moved_alone: false },
        ),
        case(
            "item_moved_outside_every_view",
            "a triangle that lies beyond the view is moved further away",
            Change::ItemMovedOutsideEveryView,
            "accepted",
            Expected { content_passes: false, bytes: (Compared::Equal, true), bytes_pending: &[Pending::ContentRefresh], bounded: Some((Compared::Equal, true)), key_moved_alone: false },
        ),
        case(
            "texture_encoded_again",
            "a texture is encoded again to other bytes, its part in the content unchanged",
            Change::TextureEncodedAgain,
            "accepted",
            Expected { content_passes: true, bytes: fails, bytes_pending: &[Pending::BytePart], bounded: DECIDED_ON_ITS_NUMBERS, key_moved_alone: false },
        ),
        case(
            "texture_encoded_again_where_nothing_samples",
            "a texel no draw samples is encoded to other bytes",
            Change::TextureEncodedAgainWhereNothingSamples,
            "accepted",
            Expected { content_passes: true, bytes: (Compared::Equal, true), bytes_pending: &[Pending::ByteRefresh], bounded: Some((Compared::Equal, true)), key_moved_alone: false },
        ),
        Case {
            fault: AdapterFault::BlendStateSetWrongly,
            ..case(
                "adapter_sets_a_blend_state_wrongly",
                "the adapter beneath the content draws a blended item with blending off",
                Change::None,
                "accepted",
                Expected { content_passes: true, bytes: fails, bytes_pending: &[Pending::BytePart], bounded: DECIDED_ON_ITS_NUMBERS, key_moved_alone: false },
            )
        },
        case(
            "run_under_another_key",
            "the set holds a byte part under another key alone",
            Change::None,
            "accepted under another key",
            Expected { content_passes: true, bytes: (Compared::NotCompared, true), bytes_pending: &[Pending::BytePartUnderANewKey], bounded: Some((Compared::Equal, true)), key_moved_alone: true },
        ),
        case(
            "key_moved_with_a_changed_item",
            "the set holds a byte part under another key alone, and one triangle is moved inside the view",
            Change::ItemMovedInsideTheView,
            "accepted under another key",
            Expected { content_passes: false, bytes: (Compared::NotCompared, false), bytes_pending: &[Pending::BytePartUnderANewKey], bounded: DECIDED_ON_ITS_NUMBERS, key_moved_alone: false },
        ),
        Case {
            draws: false,
            ..case(
                "device_draws_nothing",
                "the content device: the frame is a content hash and no picture",
                Change::None,
                "accepted",
                Expected { content_passes: true, bytes: (Compared::NotMade, true), bytes_pending: &[], bounded: Some((Compared::NotMade, true)), key_moved_alone: false },
            )
        },
    ]
}

/// Brings about each fact of the causes table and checks the verdicts of the
/// three comparisons against its row. Returns the count of cases that were
/// not reported as designed.
fn write_causes(out: &mut Json, api: &dyn Api, info: &DeviceInfo, base: &Capture, processor: &str, work: &Path) -> u64 {
    let rendered = base.rendered.as_ref().expect("the cell's device draws");
    let file = pixels::write(&rendered.image);
    let part = |key: String| BytePart { key, frame_bytes_hash: rendered.image.hash(), payloads: rendered.payloads.clone(), pixel_file: Some(pixel_file_name(&file)), base: true, host_processor: processor.to_string(), note: None };
    let sets = work.join("cases");
    let accepted = write_accepted(&sets.join("accepted"), &base.content, &part(rendered.key.clone()), &file);
    // A key that moved, brought about by naming the part's key otherwise: the
    // part and its picture are those of this device.
    let other = write_accepted(&sets.join("accepted under another key"), &base.content, &part(format!("{} | another version, named on purpose", rendered.key)), &file);
    let mut unexpected = 0;
    out.key("causes");
    out.array_start();
    for case in cases() {
        out.object_start();
        out.string("case", case.name);
        out.string("what", case.what);
        let frame = match case.size {
            Some([width, height]) => Frame::build_at(case.change, width, height),
            None => Frame::build(case.change),
        };
        let captured = if case.draws {
            capture(api, info, &frame, case.fault, processor)
        } else {
            Ok(Capture { content: ContentPart::of(&frame.content(), &frame.target, Frame::fixture_hash(), frame.tick), rendered: None })
        };
        let set = accepted.clone().and(other.clone()).and_then(|()| ReferenceSet::read(&sets.join(case.reference)));
        match (captured, set) {
            (Ok(captured), Ok(set)) => {
                let pending = work.join("pending").join(case.name);
                let compared = compare(&captured, &set, &Run { pending: &pending, takes_references: true });
                write_compared(out, &compared);
                out.string("pending_files", &listed_files(&pending));
                let bounded_as_expected = match case.expected.bounded {
                    Some(expected) => (compared.bounded.compared, compared.bounded.passed) == expected,
                    // Decided on its numbers: made against the base, and the
                    // verdict is what the numbers give under the bounds.
                    None => compared.bounded.numbers.is_some_and(|numbers| numbers.within(&BOUNDS) == compared.bounded.passed),
                };
                let group_as_expected = (review_group(&compared.bytes.facts) == "the key moved") == case.expected.key_moved_alone;
                let as_designed = compared.content.passed == case.expected.content_passes
                    && (compared.bytes.compared, compared.bytes.passed) == case.expected.bytes
                    && compared.bytes.pending == case.expected.bytes_pending
                    && bounded_as_expected
                    && group_as_expected;
                out.bool("as_designed", as_designed);
                unexpected += u64::from(!as_designed);
            }
            (Err(reason), _) | (_, Err(reason)) => {
                out.string("error", &one_line(&reason));
                unexpected += 1;
            }
        }
        out.object_end();
    }
    out.array_end();
    unexpected
}

/// The pixel file: an independent decoder, the strict reader, a file a tool
/// rewrote and a file whose pixels changed. Returns whether all four held.
fn write_pixel_file(out: &mut Json, image: &Image, work: &Path) -> bool {
    let file = pixels::write(image);
    let (plain, rewritten) = (work.join("plain.png"), work.join("rewritten.png"));
    let mut holds = true;
    out.key("pixel_file");
    out.object_start();
    out.string("decoder", pixels::decoder_name());
    out.string("rewriter", pixels::rewriter_name());
    let read_back = fs::write(&plain, &file).and_then(|()| fs::read(&plain)).ok().and_then(|read| pixels::read_strict(&read).ok().map(|decoded| decoded == *image && read == file));
    out.bool("read_back_by_the_spike_to_the_bytes_it_wrote", read_back == Some(true));
    holds &= read_back == Some(true);
    match pixels::decode_with_the_tool(&plain, work) {
        Ok(decoded) => {
            out.bool("decoder_gives_the_same_pixels", decoded == *image);
            holds &= decoded == *image;
        }
        Err(reason) => {
            out.string("decoder_error", &reason);
            holds = false;
        }
    }
    match pixels::rewrite_with_the_tool(&plain, &rewritten, work).and_then(|()| fs::read(&rewritten).map_err(|error| error.to_string())) {
        Ok(other) => {
            out.u64("rewritten_file_bytes", other.len() as u64);
            let same_pixels = pixels::decode_with_the_tool(&rewritten, work).is_ok_and(|decoded| decoded == *image);
            out.bool("rewritten_file_decodes_to_the_same_pixels", same_pixels);
            let refusal = pixels::read_strict(&other).err();
            out.bool("rewritten_file_refused_by_the_strict_reader", refusal.is_some());
            out.string("refusal", refusal.as_deref().unwrap_or("none"));
            holds &= same_pixels && refusal.is_some();
        }
        Err(reason) => {
            out.string("rewriter_error", &reason);
            holds = false;
        }
    }
    match pixels::with_one_pixel_changed(&file) {
        Ok(changed) => {
            let read = pixels::read_strict(&changed);
            out.bool("changed_pixel_file_read_by_the_strict_reader", read.is_ok());
            let caught = read.is_ok_and(|read| read.hash() != image.hash());
            out.bool("changed_pixel_caught_by_the_hash_of_the_byte_part", caught);
            holds &= caught;
        }
        Err(reason) => {
            out.string("changed_pixel_error", &reason);
            holds = false;
        }
    }
    out.object_end();
    holds
}

/// What a pixel file weighs at each target size: as written, compressed in
/// the file by the platform's tool, and in the repository's packed form, for
/// one picture and for two revisions of it.
fn write_weights(out: &mut Json, api: &dyn Api, info: &DeviceInfo, processor: &str, work: &Path) {
    out.key("pixel_file_weight");
    out.array_start();
    for [width, height] in SIZES {
        out.object_start();
        out.u64("width", u64::from(width));
        out.u64("height", u64::from(height));
        let picture = |change: Change, name: &str| -> Result<PathBuf, String> {
            let captured = capture(api, info, &Frame::build_at(change, width, height), AdapterFault::None, processor)?;
            let path = work.join(name);
            fs::write(&path, pixels::write(&captured.rendered.expect("the cell's device draws").image)).map_err(|error| error.to_string())?;
            Ok(path)
        };
        match (picture(Change::None, "weight-first.png"), picture(Change::ItemMovedInsideTheView, "weight-second.png")) {
            (Ok(first), Ok(second)) => {
                out.u64("as_written_bytes", fs::metadata(&first).map(|metadata| metadata.len()).unwrap_or(0));
                let compressed = work.join("weight-compressed.png");
                match pixels::rewrite_with_the_tool(&first, &compressed, work).and_then(|()| fs::metadata(&compressed).map_err(|error| error.to_string())) {
                    Ok(metadata) => out.u64("compressed_in_the_file_bytes", metadata.len()),
                    Err(reason) => out.string("compressed_in_the_file_error", &reason),
                }
                match pixels::packed_size(&[&first], work) {
                    Ok(size) => out.u64("packed_bytes", size),
                    Err(reason) => out.string("packed_error", &reason),
                }
                match pixels::packed_size(&[&first, &second], work) {
                    Ok(size) => out.u64("packed_two_revisions_bytes", size),
                    Err(reason) => out.string("packed_two_revisions_error", &reason),
                }
            }
            (Err(reason), _) | (_, Err(reason)) => out.string("error", &one_line(&reason)),
        }
        out.object_end();
    }
    out.array_end();
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

/// The child's side of a timing: `--child <device index> <width> <height>
/// <processors, or 0>`. It prints `name=value` lines.
fn child(arguments: &[String]) -> i32 {
    let parsed: Option<Vec<u32>> = arguments.iter().map(|argument| argument.parse().ok()).collect();
    let Some(&[index, width, height, affinity]) = parsed.as_deref() else {
        eprintln!("usage: --child <device index> <width> <height> <processors>");
        return 2;
    };
    if affinity > 0 {
        println!("affinity={}", platform::limit_affinity(affinity));
    }
    let work = env::temp_dir().join(format!("{SPIKE}-child-{}", std::process::id()));
    let _ = fs::create_dir_all(&work);
    let status = match timed_captures(&work, index as usize, width, height) {
        Ok(()) => 0,
        Err(reason) => {
            println!("error={}", one_line(&reason));
            1
        }
    };
    let _ = fs::remove_dir_all(&work);
    status
}

fn timed_captures(work: &Path, index: usize, width: u32, height: u32) -> Result<(), String> {
    let api = open_api(work)?;
    let info = api.devices().into_iter().find(|info| info.index == index).ok_or("no device at that index")?;
    let frame = Frame::build_at(Change::None, width, height);
    let scene = frame.device_scene(AdapterFault::None);
    let mut device = api.open(index, false)?;
    println!("key={}", one_line(&key_of(api.as_ref(), &info, &device.settings())));
    let (mut renders, mut read_backs, mut hashes, mut contents, mut differences) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut last = None;
    for _ in 0..REPETITIONS {
        let render = device.render_scene(&scene, Recording::SerialOneBuffer)?;
        renders.push(render.render_us);
        read_backs.push(render.readback_us);
        let started = Instant::now();
        let hash = render.image.hash();
        hashes.push(started.elapsed().as_secs_f64() * 1_000_000.0);
        let started = Instant::now();
        let content = frame.content().hash;
        contents.push(started.elapsed().as_secs_f64() * 1_000_000.0);
        // The bounded difference of two pictures a tenth of whose pixels differ.
        let mut other = render.image.clone();
        for pixel in other.rgba.as_chunks_mut::<4>().0.iter_mut().step_by(10) {
            pixel[0] ^= 3;
        }
        let started = Instant::now();
        let numbers = pixels::numbers(&render.image, &other, &BOUNDS).ok_or("two pictures of one size")?;
        differences.push(started.elapsed().as_secs_f64() * 1_000_000.0);
        last = Some((hash, content, numbers.differing_pixels));
    }
    let (hash, content, differing) = last.ok_or("no repetition")?;
    println!("hash={hash:016x}");
    println!("content={content:016x}");
    println!("differing={differing}");
    let captures: Vec<f64> = (0..REPETITIONS).map(|repetition| renders[repetition] + read_backs[repetition] + hashes[repetition]).collect();
    for (name, mut values) in [("render", renders), ("read_back", read_backs), ("hash", hashes), ("capture", captures), ("content_hash", contents), ("bounded_difference", differences)] {
        println!("{name}_fastest_us={:.3}", values.iter().copied().fold(f64::INFINITY, f64::min));
        println!("{name}_median_us={:.3}", median(&mut values));
    }
    if let Some(threads) = platform::thread_count() {
        println!("threads={threads}");
    }
    Ok(())
}

/// Runs this executable again for one timing: another process is the only
/// way to give a software device another thread count.
fn run_child(index: usize, [width, height]: [u32; 2], setting: &Setting, work: &Path) -> Result<Vec<(String, String)>, String> {
    let arguments = ["--child".to_string(), index.to_string(), width.to_string(), height.to_string(), setting.affinity_cpus.unwrap_or(0).to_string()];
    run_process(&arguments, &setting.environment, work)
}

/// Runs this executable again with the arguments and the environment given,
/// and returns the `name=value` lines it printed.
fn run_process(arguments: &[String], environment: &[(String, String)], work: &Path) -> Result<Vec<(String, String)>, String> {
    let (output_path, error_path) = (work.join("child.out"), work.join("child.err"));
    let output = fs::File::create(&output_path).map_err(|error| error.to_string())?;
    let error = fs::File::create(&error_path).map_err(|error| error.to_string())?;
    let mut command = Command::new(env::current_exe().map_err(|error| error.to_string())?);
    command.args(arguments);
    command.envs(environment.iter().map(|(name, value)| (name.as_str(), value.as_str())));
    let mut child = command.stdin(Stdio::null()).stdout(output).stderr(error).spawn().map_err(|error| format!("the child could not be started: {error}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > CHILD_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("stopped after {} seconds without ending", CHILD_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(reason) => return Err(format!("the child could not be waited for: {reason}")),
        }
    };
    let lines: Vec<(String, String)> = fs::read_to_string(&output_path).unwrap_or_default().lines().filter_map(|line| line.split_once('=')).map(|(name, value)| (name.to_string(), value.to_string())).collect();
    if let Some((_, reason)) = lines.iter().find(|(name, _)| name == "error") {
        return Err(reason.clone());
    }
    if !status.success() {
        return Err(format!("the child ended with {status}: {}", one_line(&fs::read_to_string(&error_path).unwrap_or_default()).chars().take(300).collect::<String>()));
    }
    Ok(lines)
}

/// One capture (the render, the read-back, the hash) at three target sizes,
/// as the run started it and under each thread setting the device has.
fn write_costs(out: &mut Json, api: &dyn Api, info: &DeviceInfo, work: &Path) {
    let mut settings = vec![Setting { label: "as_the_run_states".to_string(), what: format!("the run's own environment, which states {STATED_DEVICE_THREADS} device threads where the device reads a count"), environment: Vec::new(), affinity_cpus: None }];
    settings.extend(api.software_settings(info).into_iter().filter(|setting| ["one_thread", "four_threads", "one_processor", "four_processors"].contains(&setting.label.as_str())));
    out.key("capture_costs");
    out.array_start();
    for setting in &settings {
        for size in SIZES {
            out.object_start();
            out.string("setting", &setting.label);
            out.string("what", &setting.what);
            out.u64("width", u64::from(size[0]));
            out.u64("height", u64::from(size[1]));
            match run_child(info.index, size, setting, work) {
                Ok(lines) => {
                    let megapixels = f64::from(size[0]) * f64::from(size[1]) / 1_000_000.0;
                    for (name, value) in &lines {
                        match value.parse::<f64>() {
                            Ok(number) if name.ends_with("_us") => {
                                out.decimal(name, number);
                                if name.starts_with("bounded_difference") {
                                    out.decimal(&name.replace("_us", "_us_per_million_pixels"), number / megapixels);
                                }
                            }
                            _ => out.string(name, value),
                        }
                    }
                }
                Err(reason) => out.string("error", &reason),
            }
            out.object_end();
        }
    }
    out.array_end();
}

fn measure(cell: &str) {
    let output = output_dir(cell);
    let work = env::temp_dir().join(format!("{SPIKE}-work-{}", std::process::id()));
    let _ = fs::create_dir_all(&work);
    let processor = host_processor();

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.string("host_processor", &processor);
    out.string("stated_device_threads", STATED_DEVICE_THREADS);
    out.string("bounds", &format!("largest {}, second {}, {} pixels allowed beyond the second", BOUNDS.largest, BOUNDS.second, BOUNDS.allowed_beyond_second));

    let api = open_api(&work);
    let chosen = api.as_ref().ok().and_then(|api| choose_device(api.as_ref()));
    match (&api, chosen) {
        (Ok(api), Some(info)) => {
            let api = api.as_ref();
            out.key("device");
            out.object_start();
            out.string("interface", api.name());
            out.string("name", &info.name);
            out.string("kind", info.kind.name());
            out.string("driver", &info.driver);
            out.string("interface_version", &info.interface_version);
            out.object_end();
            let frame = Frame::build(Change::None);
            let content = frame.content();
            out.key("frame");
            out.object_start();
            out.hex("content_hash", content.hash);
            out.hex("fixture_state_hash", Frame::fixture_hash());
            out.string("target", &frame.target.text());
            out.string("passes", &content.passes.iter().map(|(name, hash)| format!("{name} {hash:016x}")).collect::<Vec<_>>().join(", "));
            out.u64("content_part_bytes", ContentPart::of(&content, &frame.target, Frame::fixture_hash(), frame.tick).text().len() as u64);
            out.object_end();
            match capture(api, &info, &frame, AdapterFault::None, &processor) {
                Ok(base) => {
                    let rendered = base.rendered.as_ref().expect("the cell's device draws");
                    out.string("key", &rendered.key);
                    out.hex("key_hash", fnv1a(rendered.key.as_bytes()));
                    out.hex("frame_bytes_hash", rendered.image.hash());
                    let _ = fs::write(output.join("frame.png"), pixels::write(&rendered.image));
                    let carried = write_carried_references(&mut out, cell, &output, &base);
                    let unexpected = write_causes(&mut out, api, &info, &base, &processor, &work);
                    let pixel_file = write_pixel_file(&mut out, &rendered.image, &work);
                    write_weights(&mut out, api, &info, &processor, &work);
                    write_costs(&mut out, api, &info, &work);
                    capability::write(&mut out, &info, &rendered.image, &output, &work);
                    out.key("hypotheses");
                    out.object_start();
                    out.bool("carried_references_compare_as_designed", carried);
                    out.u64("causes_not_reported_as_designed", unexpected);
                    out.bool("pixel_file_holds", pixel_file);
                    out.object_end();
                }
                Err(reason) => out.string("error", &format!("the frame could not be captured: {}", one_line(&reason))),
            }
        }
        (Ok(_), None) => out.string("error", "the interface lists no device"),
        (Err(reason), _) => out.string("error", &format!("the interface is unavailable: {}", one_line(reason))),
    }
    out.object_end();
    let _ = fs::remove_dir_all(&work);
    println!("{}", out.finish());
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--child") => std::process::exit(child(&arguments[1..])),
        Some("--held") => std::process::exit(capability::child(&arguments[1..])),
        Some("--difference") => std::process::exit(capability::difference(&arguments[1..])),
        _ => {}
    }
    // The run states its device thread count before any thread or device
    // exists; a count already in the environment is the run's and is kept.
    if env::var_os("LP_NUM_THREADS").is_none() {
        // SAFETY: the process has one thread, and nothing reads the
        // environment at the same time.
        unsafe { env::set_var("LP_NUM_THREADS", STATED_DEVICE_THREADS) };
    }
    measure(arguments.first().map(String::as_str).unwrap_or("local"));
}
