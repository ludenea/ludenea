//! Device baseline spike, for the rendering research. Throwaway, never
//! product code.
//!
//! The spike measures, instead of assuming, what the devices a machine offers
//! give a renderer through Direct3D 12, Vulkan and Metal, each reached through
//! the system's loader: whether a device opens with no window, which items of
//! an assumed baseline it reports and which capabilities beyond it, whether a
//! scene with filtered textures, blending, depth and multisampling renders to
//! the same bytes twice, in a second process, under the settings of a software
//! device and whether its commands were recorded by one thread or by four,
//! how far two devices differ on it, whether the interface's validation layer
//! delivers its messages to the program, whether a texel written through a
//! view that converts nothing arrives with its bytes, and what a lit scene
//! costs written to one image and to a wide geometry buffer.
//!
//! `backend.rs` states what each interface implements; `scene.rs` builds the
//! inputs. Run with a cell label, the spike prints one JSON document. Run
//! with `--compare <directory>`, it compares the scene images found under the
//! directory (the artifacts of several cells) and prints a Markdown table.

mod backend;
#[cfg(windows)]
mod d3d12;
mod imports;
mod loader;
#[cfg(target_os = "macos")]
mod metal;
mod platform;
mod scene;
mod vulkan;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use reference_workload::{Json, fnv1a, write_platform_and_build};

use backend::{Api, BASELINE, BEYOND, Capability, Device, DeviceInfo, LitVariant, Recording, Reported, SceneRender, Setting};
use scene::{Difference, Image, Scene};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "render-device-baseline";
/// Each variant of the lit scene is timed this many times.
const LIT_REPETITIONS: usize = 5;
/// A child process that has not ended after this long is stopped.
const CHILD_TIMEOUT: Duration = Duration::from_secs(180);

fn shader_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders")
}

/// The directory the research CI publishes: `out/<cell>/` under the workspace
/// when the spike runs from there, the temporary directory otherwise.
fn output_dir(cell: &str) -> PathBuf {
    let candidate = Path::new("out").join(cell);
    if fs::create_dir_all(&candidate).is_ok() {
        return candidate;
    }
    let fallback = env::temp_dir().join(format!("{SPIKE}-{cell}"));
    let _ = fs::create_dir_all(&fallback);
    fallback
}

/// Every graphics interface of this platform, loaded or with the reason it is
/// unavailable.
fn open_apis(shaders: &Path, work: &Path) -> Vec<(&'static str, Result<Box<dyn Api>, String>)> {
    #[allow(unused_mut, reason = "the platform interfaces below are pushed on their platform only")]
    let mut apis = vec![("vulkan", vulkan::open(shaders, work))];
    #[cfg(windows)]
    apis.push(("direct3d12", d3d12::open(shaders, work)));
    #[cfg(target_os = "macos")]
    apis.push(("metal", metal::open(shaders, work)));
    apis
}

/// A name for files: lowercase letters and digits, everything else a dash.
fn file_label(text: &str) -> String {
    let mut label = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            label.push(character.to_ascii_lowercase());
        } else if !label.ends_with('-') {
            label.push('-');
        }
    }
    label.trim_matches('-').chars().take(60).collect()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first lines of a text, for a report.
fn excerpt(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).take(12).collect();
    lines.join(" / ").chars().take(1_500).collect()
}

fn parse_lines(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|line| line.split_once('=')).map(|(key, value)| (key.trim().to_string(), value.trim().to_string())).collect()
}

/// What a child process of the spike left: whether it ended by itself, how,
/// the `key=value` lines of its standard output and its standard error.
struct ChildRun {
    completed: bool,
    status: String,
    lines: Vec<(String, String)>,
    standard_error: String,
}

impl ChildRun {
    fn value(&self, key: &str) -> Option<&str> {
        self.lines.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }
}

/// Runs this executable again as a child for one device: a second process is
/// the only way to render under another environment, and it keeps a misuse
/// that ends the process away from the measurements.
fn run_child(task: &str, api: &str, index: usize, work: &Path, label: &str, environment: &[(String, String)], affinity_cpus: Option<u32>) -> ChildRun {
    let failed = |status: String| ChildRun { completed: false, status, lines: Vec::new(), standard_error: String::new() };
    let (output_path, error_path) = (work.join(format!("{label}-{task}.out")), work.join(format!("{label}-{task}.err")));
    let files = fs::File::create(&output_path).and_then(|output| fs::File::create(&error_path).map(|error| (output, error)));
    let (output, error) = match files {
        Ok(files) => files,
        Err(reason) => return failed(format!("the child's output files could not be created: {reason}")),
    };
    let executable = match env::current_exe() {
        Ok(executable) => executable,
        Err(reason) => return failed(format!("the executable's path is unknown: {reason}")),
    };
    let mut command = Command::new(executable);
    command.arg("--child").arg(task).arg(api).arg(index.to_string()).arg(work).arg(label).arg(affinity_cpus.unwrap_or(0).to_string());
    command.envs(environment.iter().map(|(key, value)| (key.as_str(), value.as_str())));
    command.stdin(Stdio::null()).stdout(output).stderr(error);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(reason) => return failed(format!("the child could not be started: {reason}")),
    };
    let started = Instant::now();
    let (completed, status) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (true, status.to_string()),
            Ok(None) if started.elapsed() > CHILD_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break (false, format!("stopped after {} seconds without ending", CHILD_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(reason) => break (false, format!("the child could not be waited for: {reason}")),
        }
    };
    ChildRun {
        completed,
        status,
        lines: parse_lines(&fs::read_to_string(&output_path).unwrap_or_default()),
        standard_error: String::from_utf8_lossy(&fs::read(&error_path).unwrap_or_default()).into_owned(),
    }
}

/// The child's side: `--child <task> <interface> <device index> <work
/// directory> <label> <processors, or 0>`. It prints `key=value` lines.
fn child(arguments: &[String]) -> i32 {
    let [task, api_name, index, work, label, affinity] = arguments else {
        eprintln!("usage: --child <task> <interface> <device index> <work directory> <label> <processors>");
        return 2;
    };
    let (Ok(index), Ok(affinity)) = (index.parse::<usize>(), affinity.parse::<u32>()) else {
        eprintln!("the device index and the processor count are numbers");
        return 2;
    };
    if affinity > 0 {
        println!("affinity={}", platform::limit_affinity(affinity));
    }
    let work = Path::new(work);
    let scene = Scene::build();
    let Some((_, api)) = open_apis(&shader_dir(), work).into_iter().find(|(name, _)| name == api_name) else {
        println!("error=no interface named {api_name} on this platform");
        return 1;
    };
    let api = match api {
        Ok(api) => api,
        Err(reason) => {
            println!("error={}", one_line(&reason));
            return 1;
        }
    };
    match task.as_str() {
        "scene" => child_scene(api.as_ref(), index, &scene, work, label),
        "validation" => child_validation(api.as_ref(), index, &scene),
        _ => {
            eprintln!("unknown task {task}");
            2
        }
    }
}

fn child_scene(api: &dyn Api, index: usize, scene: &Scene, work: &Path, label: &str) -> i32 {
    let mut device = match api.open(index, false) {
        Ok(device) => device,
        Err(reason) => {
            println!("error={}", one_line(&reason));
            return 1;
        }
    };
    if let Some(info) = api.devices().into_iter().find(|info| info.index == index) {
        println!("device={}", one_line(&info.name));
    }
    println!("settings={}", one_line(&device.settings()));
    for recording in Recording::ALL {
        match device.render_scene(scene, recording) {
            Ok(render) => {
                let _ = fs::write(work.join(format!("{label}-{}.rgba", recording.name())), &render.image.rgba);
                println!("hash.{}={:016x}", recording.name(), render.image.hash());
            }
            Err(reason) => println!("error.{}={}", recording.name(), one_line(&reason)),
        }
    }
    if let Some(threads) = platform::thread_count() {
        println!("threads={threads}");
    }
    0
}

fn child_validation(api: &dyn Api, index: usize, scene: &Scene) -> i32 {
    let mut device = match api.open(index, true) {
        Ok(device) => device,
        Err(reason) => {
            println!("layer=false");
            println!("reason={}", one_line(&reason));
            return 0;
        }
    };
    println!("layer=true");
    println!("delivery={}", one_line(&device.validation_delivery()));
    // Every recording runs under the layer: the chunked ones use calls the
    // single buffer does not.
    let failures: Vec<String> = Recording::ALL
        .iter()
        .filter_map(|&recording| device.render_scene(scene, recording).err().map(|reason| format!("{}: {}", recording.name(), one_line(&reason))))
        .collect();
    println!("scene_rendered={}", failures.is_empty());
    if !failures.is_empty() {
        println!("scene_error={}", failures.join("; "));
    }
    let clean = device.validation_messages();
    println!("clean_messages={}", clean.len());
    if let Some(first) = clean.first() {
        println!("clean_first={}", one_line(first));
    }
    println!("misuse={}", one_line(&device.misuse()));
    let reported = device.validation_messages();
    println!("misuse_messages={}", reported.len());
    if let Some(first) = reported.first() {
        println!("misuse_first={}", one_line(first));
    }
    0
}

fn write_capabilities(out: &mut Json, capabilities: &[Capability]) {
    for (key, names) in [("baseline", &BASELINE[..]), ("beyond_the_baseline", &BEYOND[..])] {
        out.key(key);
        out.array_start();
        for name in names {
            out.object_start();
            out.string("name", name);
            match capabilities.iter().find(|capability| capability.name == *name) {
                Some(capability) => {
                    out.string("reported", capability.reported.name());
                    out.string("evidence", &capability.evidence);
                }
                None => {
                    out.string("reported", "not answered by the spike");
                    out.string("evidence", "");
                }
            }
            out.object_end();
        }
        out.array_end();
    }
    let reported = |name: &str| capabilities.iter().any(|capability| capability.name == name && capability.reported == Reported::Yes);
    out.bool("every_baseline_item_reported", BASELINE.iter().all(|name| reported(name)));
}

fn mismatches(actual: &[u32], expected: &[u32]) -> u64 {
    if actual.len() != expected.len() {
        return actual.len().max(expected.len()) as u64;
    }
    actual.iter().zip(expected).filter(|(a, b)| a != b).count() as u64
}

fn write_compute(out: &mut Json, device: &mut dyn Device, scene: &Scene) {
    out.key("compute");
    out.object_start();
    match device.compute(scene) {
        Ok(output) => {
            let buffer = mismatches(&output.buffer, &scene.compute_expected_buffer());
            let texture = mismatches(&output.texture, &scene.compute_expected_texture());
            out.bool("ran", true);
            out.u64("storage_buffer_mismatches", buffer);
            out.u64("storage_texture_mismatches", texture);
            out.bool("exact", buffer == 0 && texture == 0);
        }
        Err(reason) => {
            out.bool("ran", false);
            out.string("reason", &reason);
        }
    }
    out.object_end();
}

fn write_difference(out: &mut Json, difference: Option<Difference>) {
    match difference {
        Some(difference) => {
            out.u64("differing_pixels", difference.differing_pixels);
            out.u64("differing_bytes", difference.differing_bytes);
            out.string("largest_difference_per_channel", &format!("{:?}", difference.largest));
        }
        None => out.string("difference", "the two images have different sizes"),
    }
}

/// A rectangle of an image as an image of its own.
fn crop(image: &Image, left: u32, width: u32) -> Image {
    let mut rgba = Vec::with_capacity((width * image.height * 4) as usize);
    for y in 0..image.height {
        for x in left..left + width {
            rgba.extend_from_slice(image.pixel(x, y));
        }
    }
    Image { width, height: image.height, rgba }
}

fn write_image(out: &mut Json, output: &Path, name: &str, image: &Image) {
    let path = output.join(format!("{SPIKE}-{name}.png"));
    match fs::write(&path, image.png()) {
        Ok(()) => out.string("image", &path.display().to_string()),
        Err(reason) => out.string("image", &format!("not written: {reason}")),
    }
}

fn write_pass_through(out: &mut Json, device: &mut dyn Device, scene: &Scene, output: &Path, label: &str) {
    let side = scene::PASS_THROUGH_SIDE;
    out.key("pass_through");
    out.object_start();
    match device.pass_through(scene) {
        Ok(image) if image.width == side * 2 && image.height == side && image.rgba.len() == (side * side * 8) as usize => {
            let source = Image { width: side, height: side, rgba: scene.pass_through_source.clone() };
            out.bool("ran", true);
            for (key, half) in [("pass_through", crop(&image, 0, side)), ("converting", crop(&image, side, side))] {
                out.key(key);
                out.object_start();
                let difference = source.difference(&half);
                out.bool("every_texel_arrives_with_its_bytes", difference.is_some_and(|difference| difference.differing_pixels == 0));
                write_difference(out, difference);
                out.object_end();
            }
            write_image(out, output, &format!("pass-through-{label}"), &image);
        }
        Ok(image) => {
            out.bool("ran", false);
            out.string("reason", &format!("an image of {} by {} with {} bytes was returned", image.width, image.height, image.rgba.len()));
        }
        Err(reason) => {
            out.bool("ran", false);
            out.string("reason", &reason);
        }
    }
    out.object_end();
}

fn write_lit(out: &mut Json, device: &mut dyn Device, scene: &Scene, output: &Path, label: &str) {
    let side = scene::LIT_SIZE;
    out.key("lit_scene");
    out.array_start();
    for variant in LitVariant::ALL {
        out.object_start();
        out.string("variant", variant.name());
        out.u64("color_bits_per_pixel", variant.bits_per_pixel());
        match device.lit(scene, variant, LIT_REPETITIONS) {
            Ok(render) => {
                let mut sorted = render.microseconds.clone();
                sorted.sort_by(f64::total_cmp);
                out.bool("ran", true);
                out.decimal("fastest_us", sorted.first().copied().unwrap_or(0.0));
                out.decimal("median_us", sorted.get(sorted.len() / 2).copied().unwrap_or(0.0));
                out.string("repetitions_us", &render.microseconds.iter().map(|time| format!("{time:.0}")).collect::<Vec<_>>().join(", "));
                out.hex("output_hash", fnv1a(&render.output));
                let bytes_per_pixel = if variant.output_is_lit() { 8 } else { 4 };
                let complete = render.output.len() == (side * side * bytes_per_pixel) as usize;
                out.bool("output_has_the_expected_size", complete);
                if complete {
                    let picture = if variant.output_is_lit() {
                        Image::from_half_floats(side, side, &render.output)
                    } else {
                        Image { width: side, height: side, rgba: render.output }
                    };
                    let background = if variant.output_is_lit() { [0, 0, 0, 255] } else { [0, 0, 0, 0] };
                    out.u64("drawn_pixels", picture.rgba.as_chunks::<4>().0.iter().filter(|pixel| **pixel != background).count() as u64);
                    write_image(out, output, &format!("lit-{}-{label}", file_label(variant.name())), &picture);
                }
            }
            Err(reason) => {
                out.bool("ran", false);
                out.string("reason", &reason);
            }
        }
        out.object_end();
    }
    out.array_end();
}

fn write_render(out: &mut Json, prefix: &str, result: &Result<SceneRender, String>) {
    match result {
        Ok(render) => {
            out.hex(&format!("{prefix}_hash"), render.image.hash());
            out.decimal(&format!("{prefix}_render_us"), render.render_us);
            out.decimal(&format!("{prefix}_readback_us"), render.readback_us);
            out.u64(&format!("{prefix}_readback_polls"), render.readback_polls);
            match render.timestamp_ns {
                Some(elapsed) => out.u64(&format!("{prefix}_timestamps_ns"), elapsed),
                None => out.string(&format!("{prefix}_timestamps_ns"), "no timestamp taken"),
            }
        }
        Err(reason) => out.string(&format!("{prefix}_error"), reason),
    }
}

/// The scene's image on one device, kept for the comparison across devices.
struct Measured {
    label: String,
    image: Image,
}

fn measure_device(out: &mut Json, api: &dyn Api, info: &DeviceInfo, scene: &Scene, output: &Path, work: &Path) -> Option<Measured> {
    let label = file_label(&format!("{}-{}-{}", api.name(), info.index, info.name));
    out.object_start();
    out.u64("index", info.index as u64);
    out.string("name", &info.name);
    out.string("kind", info.kind.name());
    out.string("vendor_id", &format!("0x{:04x}", info.vendor_id));
    out.string("device_id", &format!("0x{:04x}", info.device_id));
    out.string("driver", &info.driver);
    out.string("interface_version", &info.interface_version);
    let mut device = match api.open(info.index, false) {
        Ok(device) => device,
        Err(reason) => {
            out.bool("opened", false);
            out.string("reason", &reason);
            out.object_end();
            return None;
        }
    };
    out.bool("opened", true);
    let settings = device.settings();
    out.string("settings", &settings);
    out.string("reference_key", &[api.name(), &info.name, &info.driver, &info.interface_version, &settings].join(" | "));
    write_capabilities(out, &device.capabilities());

    // The scene in its three recordings on this device, then on a second
    // device created from scratch in the same process.
    let first: Vec<Result<SceneRender, String>> = Recording::ALL.iter().map(|&recording| device.render_scene(scene, recording)).collect();
    write_compute(out, device.as_mut(), scene);
    write_pass_through(out, device.as_mut(), scene, output, &label);
    write_lit(out, device.as_mut(), scene, output, &label);
    drop(device);
    let second: Vec<Result<SceneRender, String>> = match api.open(info.index, false) {
        Ok(mut again) => Recording::ALL.iter().map(|&recording| again.render_scene(scene, recording)).collect(),
        Err(reason) => Recording::ALL.iter().map(|_| Err(format!("the second device did not open: {reason}"))).collect(),
    };
    let reference = first.iter().find_map(|result| result.as_ref().ok()).map(|render| render.image.clone());
    let mut hashes = Vec::new();

    out.key("scene");
    out.object_start();
    out.key("recordings");
    out.array_start();
    for (position, recording) in Recording::ALL.iter().enumerate() {
        out.object_start();
        out.string("recording", recording.name());
        for (prefix, result) in [("first_device", &first[position]), ("second_device", &second[position])] {
            hashes.extend(result.as_ref().ok().map(|render| render.image.hash()));
            write_render(out, prefix, result);
        }
        out.object_end();
    }
    out.array_end();
    if let Some(image) = &reference {
        out.hex("image_hash", image.hash());
        out.string("block_hashes", &image.block_hashes().iter().map(|hash| format!("{hash:016x}")).collect::<Vec<_>>().join(" "));
        out.u64("nearest_filtered_pixels_that_differ_from_their_texel", scene.nearest_mismatches(image));
        out.key("drawn_pixels_per_region");
        out.object_start();
        for region in &scene::REGIONS {
            out.u64(region.name, image.drawn_pixels(region.bounds));
        }
        out.object_end();
        write_image(out, output, &format!("scene-{label}"), image);
    }
    out.object_end();

    // The scene again in child processes: the same settings, then each
    // setting the software device has.
    let mut runs = vec![Setting { label: "second_process".to_string(), what: "the same settings in a second process".to_string(), environment: Vec::new(), affinity_cpus: None }];
    runs.extend(api.software_settings(info));
    out.key("runs");
    out.array_start();
    for run in &runs {
        let run_label = format!("{label}-{}", run.label);
        let child = run_child("scene", api.name(), info.index, work, &run_label, &run.environment, run.affinity_cpus);
        out.object_start();
        out.string("label", &run.label);
        out.string("what", &run.what);
        out.string("environment", &run.environment.iter().map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join(" "));
        out.u64("affinity_cpus", u64::from(run.affinity_cpus.unwrap_or(0)));
        out.bool("completed", child.completed);
        out.string("status", &child.status);
        for key in ["error", "device", "settings", "affinity", "threads"] {
            if let Some(value) = child.value(key) {
                out.string(key, value);
            }
        }
        out.key("recordings");
        out.array_start();
        for recording in Recording::ALL {
            out.object_start();
            out.string("recording", recording.name());
            match child.value(&format!("hash.{}", recording.name())).and_then(|text| u64::from_str_radix(text, 16).ok()) {
                Some(hash) => {
                    hashes.push(hash);
                    out.hex("hash", hash);
                    if let Some(reference) = &reference
                        && hash != reference.hash()
                    {
                        match fs::read(work.join(format!("{run_label}-{}.rgba", recording.name()))) {
                            Ok(rgba) => write_difference(out, reference.difference(&Image { width: scene::WIDTH, height: scene::HEIGHT, rgba })),
                            Err(reason) => out.string("difference", &format!("the child's image could not be read: {reason}")),
                        }
                    }
                }
                None => out.string("error", child.value(&format!("error.{}", recording.name())).unwrap_or("no hash reported")),
            }
            out.object_end();
        }
        out.array_end();
        if !child.standard_error.trim().is_empty() {
            out.string("standard_error", &excerpt(&child.standard_error));
        }
        out.object_end();
    }
    out.array_end();
    let expected = Recording::ALL.len() * (2 + runs.len());
    let rendered = hashes.len();
    hashes.sort_unstable();
    hashes.dedup();
    out.u64("renders_expected", expected as u64);
    out.u64("renders_completed", rendered as u64);
    out.u64("distinct_scene_hashes", hashes.len() as u64);
    out.bool("every_render_identical", rendered == expected && hashes.len() == 1);

    let validation = run_child("validation", api.name(), info.index, work, &format!("{label}-validation"), &api.validation_environment(), None);
    out.key("validation");
    out.object_start();
    out.bool("completed", validation.completed);
    out.string("status", &validation.status);
    for key in ["error", "reason", "delivery", "scene_error", "clean_first", "misuse", "misuse_first"] {
        if let Some(value) = validation.value(key) {
            out.string(key, value);
        }
    }
    for key in ["layer", "scene_rendered"] {
        if let Some(value) = validation.value(key) {
            out.bool(key, value == "true");
        }
    }
    for key in ["clean_messages", "misuse_messages"] {
        if let Some(count) = validation.value(key).and_then(|value| value.parse::<u64>().ok()) {
            out.u64(key, count);
        }
    }
    let delivered = validation.value("misuse_messages").and_then(|value| value.parse::<u64>().ok()).is_some_and(|count| count > 0);
    out.bool("the_misuse_is_reported_to_the_program", delivered);
    if !validation.standard_error.trim().is_empty() {
        out.string("standard_error", &excerpt(&validation.standard_error));
    }
    out.object_end();
    out.object_end();
    reference.map(|image| Measured { label, image })
}

/// The unix sockets the process holds, read from `/proc/self/fd` on Linux: a
/// connection to a display server would be one of them.
fn open_sockets() -> Option<u64> {
    let entries = fs::read_dir("/proc/self/fd").ok()?;
    let mut sockets = 0;
    for entry in entries.flatten() {
        if let Ok(target) = fs::read_link(entry.path())
            && target.to_string_lossy().starts_with("socket:")
        {
            sockets += 1;
        }
    }
    Some(sockets)
}

fn write_session(out: &mut Json, sockets_before: Option<u64>) {
    out.key("session");
    out.object_start();
    out.bool("display_set", env::var_os("DISPLAY").is_some());
    out.bool("wayland_display_set", env::var_os("WAYLAND_DISPLAY").is_some());
    for (key, count) in [("open_sockets_before_rendering", sockets_before), ("open_sockets_after_rendering", open_sockets()), ("threads_after_rendering", platform::thread_count())] {
        match count {
            Some(count) => out.u64(key, count),
            None => out.string(key, "not readable on this platform"),
        }
    }
    out.object_end();
}

fn write_imports(out: &mut Json) {
    out.key("imports");
    out.object_start();
    match env::current_exe().map_err(|error| error.to_string()).and_then(|path| imports::Imports::of_file(&path)) {
        Ok(imports) => {
            out.string("format", imports.format);
            out.string("libraries", &imports.libraries.join(", "));
            let denied = imports.denied();
            out.bool("imports_a_windowing_graphics_or_audio_library", !denied.is_empty());
            out.string("denied", &denied.join(", "));
        }
        Err(error) => out.string("error", &error),
    }
    out.object_end();
}

fn measure(cell: &str) {
    let sockets_before = open_sockets();
    let output = output_dir(cell);
    let work = env::temp_dir().join(format!("{SPIKE}-work-{}", std::process::id()));
    let _ = fs::create_dir_all(&work);
    let scene = Scene::build();

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.key("inputs");
    out.object_start();
    out.hex("hash", scene.input_hash());
    out.u64("scene_vertices", scene.vertices.len() as u64);
    out.u64("scene_draws", scene.draws.len() as u64);
    out.string("textures", &scene.textures.iter().map(|texture| texture.name).collect::<Vec<_>>().join(", "));
    out.u64("lit_vertices", scene.lit_vertices.len() as u64);
    out.u64("lit_repetitions", LIT_REPETITIONS as u64);
    out.object_end();

    let mut measured = Vec::new();
    let mut devices_opened = 0;
    out.key("interfaces");
    out.array_start();
    for (name, api) in open_apis(&shader_dir(), &work) {
        out.object_start();
        out.string("interface", name);
        match api {
            Ok(api) => {
                out.bool("available", true);
                out.string("loader", &api.loader());
                out.string("shader_compiler", &api.shader_compiler());
                out.key("devices");
                out.array_start();
                for info in api.devices() {
                    if let Some(result) = measure_device(&mut out, api.as_ref(), &info, &scene, &output, &work) {
                        devices_opened += 1;
                        measured.push(result);
                    }
                }
                out.array_end();
            }
            Err(reason) => {
                out.bool("available", false);
                out.string("reason", &reason);
            }
        }
        out.object_end();
    }
    out.array_end();
    out.u64("devices_measured", devices_opened);

    out.key("differences_between_devices");
    out.array_start();
    for (position, a) in measured.iter().enumerate() {
        for b in &measured[position + 1..] {
            out.object_start();
            out.string("first", &a.label);
            out.string("second", &b.label);
            write_difference(&mut out, a.image.difference(&b.image));
            out.object_end();
        }
    }
    out.array_end();
    write_session(&mut out, sockets_before);
    write_imports(&mut out);
    out.object_end();
    let _ = fs::remove_dir_all(&work);
    println!("{}", out.finish());
}

/// Every scene image under a directory, by the name of the directory that
/// holds it and of the device.
fn scene_images(directory: &Path, depth: u32, found: &mut Vec<(String, Image)>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    let prefix = format!("{SPIKE}-scene-");
    for path in paths {
        if path.is_dir() {
            if depth > 0 {
                scene_images(&path, depth - 1, found);
            }
            continue;
        }
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(device) = name.strip_prefix(&prefix).and_then(|rest| rest.strip_suffix(".png")) else { continue };
        let cell = directory.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        match fs::read(&path).map_err(|error| error.to_string()).and_then(|bytes| Image::from_png(&bytes)) {
            Ok(image) => found.push((format!("{cell}: {device}"), image)),
            Err(reason) => eprintln!("{}: {reason}", path.display()),
        }
    }
}

/// Groups the scene images found under a directory into sets of identical
/// bytes and prints how far the sets are from one another.
fn compare(directory: &Path) -> i32 {
    let mut images = Vec::new();
    scene_images(directory, 3, &mut images);
    if images.is_empty() {
        eprintln!("no scene image under {}", directory.display());
        return 1;
    }
    let mut sets: Vec<(Image, Vec<String>)> = Vec::new();
    for (label, image) in images {
        match sets.iter_mut().find(|(member, _)| *member == image) {
            Some((_, labels)) => labels.push(label),
            None => sets.push((image, vec![label])),
        }
    }
    println!("{} distinct image(s) of the scene:", sets.len());
    println!();
    for (position, (image, labels)) in sets.iter().enumerate() {
        println!("- image {} (`{:016x}`): {}", position + 1, image.hash(), labels.join("; "));
    }
    println!();
    println!("| Images | Differing pixels | Differing bytes | Largest difference per channel |");
    println!("| --- | --- | --- | --- |");
    for (position, (a, _)) in sets.iter().enumerate() {
        for (offset, (b, _)) in sets[position + 1..].iter().enumerate() {
            match a.difference(b) {
                Some(difference) => println!("| {} and {} | {} | {} | {:?} |", position + 1, position + offset + 2, difference.differing_pixels, difference.differing_bytes, difference.largest),
                None => println!("| {} and {} | different sizes | | |", position + 1, position + offset + 2),
            }
        }
    }
    0
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--child") => std::process::exit(child(&arguments[1..])),
        Some("--compare") => match arguments.get(1) {
            Some(directory) => std::process::exit(compare(Path::new(directory))),
            None => {
                eprintln!("usage: --compare <directory>");
                std::process::exit(2);
            }
        },
        Some(cell) => measure(cell),
        None => measure("unspecified"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every device this machine offers renders the scene with something
    /// drawn in each region and computes exactly, or its interface says why
    /// it is unavailable: the absence is a fact the spike reports.
    #[test]
    fn every_device_renders_and_computes_or_its_interface_reports_its_absence() {
        let scene = Scene::build();
        let work = env::temp_dir().join(format!("{SPIKE}-test-{}", std::process::id()));
        fs::create_dir_all(&work).unwrap();
        let mut measured = 0;
        for (name, api) in open_apis(&shader_dir(), &work) {
            let api = match api {
                Ok(api) => api,
                Err(reason) => {
                    eprintln!("{name} unavailable here: {reason}");
                    continue;
                }
            };
            for info in api.devices() {
                let mut device = match api.open(info.index, false) {
                    Ok(device) => device,
                    Err(reason) => {
                        eprintln!("{name}, {}: {reason}", info.name);
                        continue;
                    }
                };
                measured += 1;
                let capabilities = device.capabilities();
                for expected in BASELINE.iter().chain(&BEYOND) {
                    assert_eq!(capabilities.iter().filter(|capability| capability.name == *expected).count(), 1, "{name}, {}: {expected}", info.name);
                }
                let render = device.render_scene(&scene, Recording::SerialOneBuffer).unwrap_or_else(|reason| panic!("{name}, {}: {reason}", info.name));
                assert_eq!((render.image.width, render.image.height), (scene::WIDTH, scene::HEIGHT));
                assert_eq!(render.image.rgba.len(), (scene::WIDTH * scene::HEIGHT * 4) as usize);
                for region in &scene::REGIONS {
                    assert!(render.image.drawn_pixels(region.bounds) > 0, "{name}, {}: nothing drawn in {}", info.name, region.name);
                }
                assert_eq!(render.image.pixel(254, 110), scene::CLEAR_BYTES, "{name}, {}: the clear color", info.name);
                let output = device.compute(&scene).unwrap_or_else(|reason| panic!("{name}, {}: {reason}", info.name));
                assert_eq!(mismatches(&output.buffer, &scene.compute_expected_buffer()), 0, "{name}, {}", info.name);
                assert_eq!(mismatches(&output.texture, &scene.compute_expected_texture()), 0, "{name}, {}", info.name);
            }
        }
        let _ = fs::remove_dir_all(&work);
        if cfg!(any(windows, target_os = "macos")) {
            assert!(measured >= 1, "the platform's own interface should offer a device");
        }
    }

    #[test]
    fn labels_lines_and_excerpts_are_shaped_for_files_and_reports() {
        assert_eq!(file_label("direct3d12-0-Microsoft Basic Render Driver"), "direct3d12-0-microsoft-basic-render-driver");
        assert_eq!(file_label("  llvmpipe (LLVM 20.1.2, 256 bits)  "), "llvmpipe-llvm-20-1-2-256-bits");
        assert_eq!(one_line("a\r\n  b\tc"), "a b c");
        assert_eq!(excerpt("\n first \n\nsecond\n"), "first / second");
        let lines = parse_lines("hash.serial_one_buffer=00ff\nnoise\nthreads = 12\nreason=a=b\n");
        assert_eq!(lines, vec![("hash.serial_one_buffer".to_string(), "00ff".to_string()), ("threads".to_string(), "12".to_string()), ("reason".to_string(), "a=b".to_string())]);
        assert_eq!(mismatches(&[1, 2, 3], &[1, 0, 3]), 1);
        assert_eq!(mismatches(&[1], &[1, 2]), 2);
    }

    #[test]
    fn a_half_of_an_image_is_cropped_row_by_row() {
        let image = Image { width: 4, height: 2, rgba: (0..32).collect() };
        assert_eq!(crop(&image, 2, 2).rgba, vec![8, 9, 10, 11, 12, 13, 14, 15, 24, 25, 26, 27, 28, 29, 30, 31]);
    }

    #[test]
    fn the_output_directory_is_created_under_out() {
        let directory = output_dir("render-device-baseline-test-cell");
        assert!(directory.is_dir());
        let _ = fs::remove_dir_all(&directory);
    }
}
