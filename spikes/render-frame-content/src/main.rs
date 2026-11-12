//! Frame content spike of the rendering research. Throwaway, never product code.
//!
//! The stages of a frame (extract, refresh, prepare, declare and compile,
//! record, submit, read back) at the size of the reference workload, on the
//! runtime skeleton's world, above a render device port: first with a device
//! that draws nothing, then with each device of the machine that needs no
//! graphics processor, through the native path of the device baseline spike.
//! See README.md for the hypotheses and the method.
//!
//! The parent process measures on the device that draws nothing and starts
//! one child process per device and per kind of measurement, so that a
//! driver that ends a process ends one measurement. Usage:
//!
//! - `render-frame-content <cell>`: every measurement; prints the report.
//! - `render-frame-content <cell> content-device-only`: no device of the machine.
//! - `render-frame-content --child device <interface> <index> <report> <pictures> <label>`
//! - `render-frame-content --child lifetimes <interface> <index> <report>`

mod graph;
mod harness;
mod instance;
mod math;
mod pool;
mod port;
mod prepare;
mod renderer;
mod scenarios;
mod slices;
mod visual;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use reference_workload::{Json, write_platform_and_build};
use render_device_baseline::backend::{Api, DeviceInfo, DeviceKind};

use harness::{Backend, SharedApi};
use scenarios::LifetimeStep;

/// Every graphics interface of this platform, loaded or with the reason it is unavailable.
fn open_apis() -> Vec<(&'static str, Result<Box<dyn Api>, String>)> {
    let shaders = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../render-device-baseline/shaders");
    let work = std::env::temp_dir();
    #[allow(unused_mut, reason = "the platform interfaces below are pushed on their platform only")]
    let mut apis = vec![("vulkan", render_device_baseline::vulkan::open(&shaders, &work))];
    #[cfg(windows)]
    apis.push(("direct3d12", render_device_baseline::d3d12::open(&shaders, &work)));
    #[cfg(target_os = "macos")]
    apis.push(("metal", render_device_baseline::metal::open(&shaders, &work)));
    apis
}

/// One measurement as a JSON object, with the seconds it took; an error is
/// recorded in its place.
fn measured(run: impl FnOnce(&mut Json) -> Result<(), String>) -> String {
    let started = Instant::now();
    let mut out = Json::new();
    out.object_start();
    if let Err(reason) = run(&mut out) {
        // What was written before the error may be cut short: it is left out.
        out = Json::new();
        out.object_start();
        out.string("error", &reason);
    }
    out.decimal("seconds", started.elapsed().as_secs_f64());
    out.object_end();
    out.finish()
}

fn object_of(entries: &[(String, String)]) -> String {
    let mut out = Json::new();
    out.object_start();
    for (key, value) in entries {
        out.raw(key, value);
    }
    out.object_end();
    out.finish()
}

/// A text as a JSON string.
fn quoted(text: &str) -> String {
    let mut out = Json::new();
    out.object_start();
    out.string("text", text);
    out.object_end();
    let object = out.finish();
    object["{\"text\":".len()..object.len() - 1].to_string()
}

fn device_facts(info: &DeviceInfo, settings: &str) -> String {
    let mut out = Json::new();
    out.object_start();
    out.string("name", &info.name);
    out.string("kind", info.kind.name());
    out.string("driver", &info.driver);
    out.string("interface_version", &info.interface_version);
    out.string("vendor_id", &format!("0x{:04x}", info.vendor_id));
    out.string("device_id", &format!("0x{:04x}", info.device_id));
    out.string("settings", settings);
    out.object_end();
    out.finish()
}

fn step_json(step: &LifetimeStep) -> String {
    let mut out = Json::new();
    out.object_start();
    out.string("step", step.name);
    for (key, value) in &step.fields {
        match value.as_str() {
            "true" => out.bool(key, true),
            "false" => out.bool(key, false),
            number if number.parse::<u64>().is_ok() => out.u64(key, number.parse().unwrap()),
            text => out.string(key, text),
        }
    }
    out.object_end();
    out.finish()
}

fn array_of(items: &[String]) -> String {
    format!("[{}]", items.join(","))
}

/// The lifetimes measurement as a JSON object, written to `report` after
/// each step when a path is given.
fn lifetimes_report(backend: Backend, head: &[(String, String)], report: Option<&Path>) -> String {
    let mut steps: Vec<String> = Vec::new();
    let assemble = |steps: &[String], error: Option<&str>| {
        let mut entries = head.to_vec();
        entries.push(("steps".to_string(), array_of(steps)));
        if let Some(error) = error {
            entries.push(("error".to_string(), quoted(error)));
        }
        object_of(&entries)
    };
    let outcome = scenarios::lifetimes(backend, |step| {
        // A step reported twice replaces its first report.
        let json = step_json(step);
        let prefix = format!("{{\"step\":{}", quoted(step.name));
        match steps.iter().position(|existing| existing.starts_with(&prefix)) {
            Some(position) => steps[position] = json,
            None => steps.push(json),
        }
        if let Some(report) = report {
            let _ = std::fs::write(report, assemble(&steps, None));
        }
    });
    let text = assemble(&steps, outcome.as_ref().err().map(String::as_str));
    if let Some(report) = report {
        let _ = std::fs::write(report, &text);
    }
    text
}

fn child(arguments: &[String]) {
    let [kind, interface, index, report, rest @ ..] = arguments else {
        eprintln!("usage: --child <device|lifetimes> <interface> <index> <report> [<pictures> <label>]");
        std::process::exit(2);
    };
    let index: usize = index.parse().expect("the device's index is a number");
    let api = open_apis().into_iter().find(|(name, _)| name == interface).and_then(|(_, api)| api.ok()).expect("the parent found this interface");
    let api: &'static dyn Api = Box::leak(api);
    let info = api.devices().into_iter().find(|device| device.index == index).expect("the parent found this device");
    let validation = kind == "lifetimes";
    let mut backend = Backend::Native { api: SharedApi(api), index, validation };
    let mut opened = backend.open();
    let mut layer = "on".to_string();
    if validation && let Err(reason) = &opened {
        // No layer on this machine: the port's own assertions are measured alone.
        layer = format!("absent: {reason}");
        backend = Backend::Native { api: SharedApi(api), index, validation: false };
        opened = backend.open();
    }
    let settings = opened.as_ref().map_err(String::clone).and_then(|port| port.settings().map_err(|error| error.to_string())).unwrap_or_else(|reason| format!("not opened: {reason}"));
    let mut entries = vec![("interface".to_string(), quoted(interface)), ("device".to_string(), device_facts(&info, &settings))];
    if validation {
        let delivery = opened.as_ref().map_err(String::clone).and_then(|port| port.validation_delivery().map_err(|error| error.to_string())).unwrap_or_else(|reason| format!("not opened: {reason}"));
        drop(opened);
        entries.push(("validation_layer".to_string(), quoted(&layer)));
        entries.push(("validation_delivery".to_string(), quoted(&delivery)));
        lifetimes_report(backend, &entries, Some(Path::new(report)));
        return;
    }
    drop(opened);
    if let [pictures, label] = rest {
        let _ = scenarios::PICTURES.set((PathBuf::from(pictures), label.clone()));
    }
    type Measurement = fn(&mut Json, Backend) -> Result<(), String>;
    let measurements: [(&str, Measurement); 7] = [
        ("modes", scenarios::modes),
        ("pace", scenarios::pace),
        ("shared_device", scenarios::shared_device),
        ("loss", scenarios::loss),
        ("unwatched", scenarios::unwatched),
        ("shader_payload", scenarios::shader_payload),
        ("removal", scenarios::removal),
    ];
    for (name, run) in measurements {
        entries.push((name.to_string(), measured(|out| run(out, backend))));
        // Written after each measurement: what was found is kept if a later one ends the process.
        let _ = std::fs::write(report, object_of(&entries));
    }
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
    label.trim_matches('-').to_string()
}

/// Runs a child to its end or to a limit, and returns how it ended.
fn run_child(arguments: &[&str], environment: &[(String, String)], limit: Duration) -> String {
    let mut command = Command::new(std::env::current_exe().expect("the process knows its own path"));
    command.arg("--child").args(arguments).envs(environment.iter().cloned());
    let mut process = match command.spawn() {
        Ok(process) => process,
        Err(error) => return format!("not started: {error}"),
    };
    let started = Instant::now();
    loop {
        match process.try_wait() {
            Ok(Some(status)) => return if status.success() { "ended normally".to_string() } else { format!("ended with {status}") },
            Ok(None) if started.elapsed() > limit => {
                let _ = process.kill();
                let _ = process.wait();
                return format!("stopped after {} seconds without an end", limit.as_secs());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return format!("lost: {error}"),
        }
    }
}

fn child_report(path: &Path, ended: &str) -> String {
    let report = std::fs::read_to_string(path).ok().filter(|text| text.trim_start().starts_with('{')).unwrap_or_else(|| "{\"error\":\"the process left no report\"}".to_string());
    format!("{{\"process\":{},\"report\":{report}}}", quoted(ended))
}

fn parent(cell: &str, content_device_only: bool) {
    let pictures = PathBuf::from("out").join(cell);
    let _ = std::fs::create_dir_all(&pictures);
    let work = std::env::temp_dir();
    let mut out = Json::new();
    out.object_start();
    out.string("spike", "render-frame-content");
    out.string("cell", cell);
    write_platform_and_build(&mut out);

    out.key("content_device");
    out.object_start();
    out.raw("content_hashes", &measured(scenarios::content_hashes));
    out.raw("prepare_cost", &measured(scenarios::prepare_cost));
    out.raw("graph_cost", &measured(scenarios::graph_cost));
    out.raw("tile_extraction", &measured(scenarios::tile_extraction));
    out.raw("modes", &measured(|out| scenarios::modes(out, Backend::Content)));
    out.raw("pace", &measured(|out| scenarios::pace(out, Backend::Content)));
    out.raw("shared_device", &measured(|out| scenarios::shared_device(out, Backend::Content)));
    out.raw("loss", &measured(|out| scenarios::loss(out, Backend::Content)));
    out.raw("unwatched", &measured(|out| scenarios::unwatched(out, Backend::Content)));
    out.raw("shader_payload", &measured(|out| scenarios::shader_payload(out, Backend::Content)));
    out.raw("lifetimes", &lifetimes_report(Backend::Content, &[], None));
    out.object_end();

    // The devices that need no graphics processor; on the founder's machine,
    // every device.
    let every_device = cell.starts_with("founder");
    let (mut interfaces, mut devices, mut validations) = (Vec::new(), Vec::new(), Vec::new());
    for (name, api) in if content_device_only { Vec::new() } else { open_apis() } {
        let api = match api {
            Ok(api) => api,
            Err(reason) => {
                interfaces.push(format!("{{\"interface\":{},\"available\":false,\"reason\":{}}}", quoted(name), quoted(&reason)));
                continue;
            }
        };
        let listed = api.devices();
        interfaces.push(format!("{{\"interface\":{},\"available\":true,\"devices\":{}}}", quoted(name), listed.len()));
        let mut seen: Vec<String> = Vec::new();
        for info in listed {
            let wanted = every_device || matches!(info.kind, DeviceKind::Software | DeviceKind::Virtual);
            // Direct3D 12 lists its software adapter twice: one is measured.
            if !wanted || seen.contains(&info.name) {
                continue;
            }
            seen.push(info.name.clone());
            let label = format!("{name}-{}", file_label(&info.name));
            let index = info.index.to_string();
            let report = work.join(format!("render-frame-content-{}-{label}.json", std::process::id()));
            let _ = std::fs::remove_file(&report);
            let ended = run_child(&["device", name, &index, &report.to_string_lossy(), &pictures.to_string_lossy(), &label], &[], Duration::from_secs(1_500));
            devices.push(child_report(&report, &ended));
            let _ = std::fs::remove_file(&report);
            let ended = run_child(&["lifetimes", name, &index, &report.to_string_lossy()], &api.validation_environment(), Duration::from_secs(300));
            validations.push(child_report(&report, &ended));
            let _ = std::fs::remove_file(&report);
        }
    }
    out.raw("interfaces", &array_of(&interfaces));
    out.raw("devices", &array_of(&devices));
    out.raw("lifetimes_under_validation", &array_of(&validations));
    out.object_end();
    println!("{}", out.finish());
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--child") => child(&arguments[1..]),
        Some(cell) => parent(cell, arguments.get(1).is_some_and(|argument| argument == "content-device-only")),
        None => parent("local", false),
    }
}
