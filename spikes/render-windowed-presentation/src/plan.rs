//! The plan of a machine: which measurements run on it, each in a child
//! process, and the document they make. A cell of the research CI runs what
//! needs no real display, no hardware device and no person; a machine with a
//! display, named on the command line, also runs the pace and the long
//! synchronized runs. Throwaway, never product code.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::imports::Imports;
use crate::json::Json;
use crate::presenter::{self, DeviceInfo};

pub struct Options {
    pub cell: String,
    /// The machine has a real display: the pace and the long synchronized
    /// runs are measured too.
    pub with_display: bool,
    /// Sections to run; empty runs them all.
    pub only: Vec<String>,
    /// Frames of a run "for ten thousand frames".
    pub frames: u64,
}

/// The display a child process runs under: its name and its environment.
#[derive(Clone)]
struct Display {
    name: String,
    set: Vec<(String, String)>,
    unset: Vec<&'static str>,
}

impl Display {
    #[cfg(not(target_os = "linux"))]
    fn session() -> Display {
        Display { name: "the desktop session of the machine".to_string(), set: Vec::new(), unset: Vec::new() }
    }
}

/// The display servers a Linux cell is given: a virtual X11 display and a
/// Wayland compositor that needs no display, each a process of its own.
#[cfg(target_os = "linux")]
struct Servers {
    xvfb: Option<std::process::Child>,
    weston: Option<std::process::Child>,
    runtime: PathBuf,
    notes: Json,
}

#[cfg(target_os = "linux")]
impl Servers {
    const X_DISPLAY: &'static str = ":99";
    const WAYLAND_SOCKET: &'static str = "ludenea-spike";

    fn start() -> Servers {
        let runtime = std::env::temp_dir().join(format!("ludenea-spike-runtime-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&runtime);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700));
        }
        let wait_for = |path: &Path| {
            let started = Instant::now();
            while !path.exists() && started.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(50));
            }
            path.exists()
        };
        let mut notes = Json::object();
        let xvfb = Command::new("Xvfb").args([Self::X_DISPLAY, "-screen", "0", "1280x720x24", "-nolisten", "tcp"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        let xvfb = match xvfb {
            Ok(child) => {
                let up = wait_for(Path::new("/tmp/.X11-unix/X99"));
                notes.set("virtual_x11_display", if up { "Xvfb, a display of 1280 by 720 with no window manager".to_string() } else { "Xvfb started and opened no socket within ten seconds".to_string() });
                Some(child)
            }
            Err(error) => {
                notes.set("virtual_x11_display", format!("not started: {error}"));
                None
            }
        };
        let weston = Command::new("weston")
            .args(["--backend=headless", &format!("--socket={}", Self::WAYLAND_SOCKET), "--width=1280", "--height=720"])
            .env("XDG_RUNTIME_DIR", &runtime)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let weston = match weston {
            Ok(child) => {
                let up = wait_for(&runtime.join(Self::WAYLAND_SOCKET));
                notes.set("compositor_with_no_display", if up { "weston with its headless backend, an output of 1280 by 720".to_string() } else { "weston started and opened no socket within ten seconds".to_string() });
                Some(child)
            }
            Err(error) => {
                notes.set("compositor_with_no_display", format!("not started: {error}"));
                None
            }
        };
        Servers { xvfb, weston, runtime, notes }
    }

    fn runtime(&self) -> String {
        self.runtime.to_string_lossy().into_owned()
    }

    fn none(&self) -> Display {
        Display { name: "no display".to_string(), set: Vec::new(), unset: vec!["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE"] }
    }

    fn x11(&self) -> Display {
        Display { name: "a virtual X11 display".to_string(), set: vec![("DISPLAY".to_string(), Self::X_DISPLAY.to_string())], unset: vec!["WAYLAND_DISPLAY", "XDG_SESSION_TYPE"] }
    }

    fn wayland(&self) -> Display {
        Display { name: "a Wayland compositor with no display".to_string(), set: vec![("WAYLAND_DISPLAY".to_string(), Self::WAYLAND_SOCKET.to_string()), ("XDG_RUNTIME_DIR".to_string(), self.runtime())], unset: vec!["DISPLAY", "XDG_SESSION_TYPE"] }
    }

    fn wayland_that_fails_beside_x11(&self) -> Display {
        Display {
            name: "a Wayland connection that fails beside a running X11 server".to_string(),
            set: vec![("WAYLAND_DISPLAY".to_string(), "ludenea-absent".to_string()), ("XDG_RUNTIME_DIR".to_string(), self.runtime()), ("DISPLAY".to_string(), Self::X_DISPLAY.to_string())],
            unset: vec!["XDG_SESSION_TYPE"],
        }
    }

    fn windowed(&self) -> Vec<Display> {
        vec![self.x11(), self.wayland()]
    }
}

#[cfg(target_os = "linux")]
impl Drop for Servers {
    fn drop(&mut self) {
        for child in [&mut self.xvfb, &mut self.weston].into_iter().flatten() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

/// Runs a program to its end or to its limit and gives the JSON object it
/// printed last, with the display it ran under, or what happened instead.
fn run(program: &Path, words: &[String], display: &Display, limit: Duration) -> Json {
    let mut command = Command::new(program);
    command.args(words).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for name in &display.unset {
        command.env_remove(name);
    }
    for (name, value) in &display.set {
        command.env(name, value);
    }
    let failure = |status: String, standard_error: &str, printed: &str| {
        let tail = |text: &str| text.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect::<String>();
        Json::object().with("completed", false).with("under", display.name.clone()).with("status", status).with("arguments", words.join(" ")).with("standard_error", tail(standard_error.trim())).with("printed", tail(printed.trim()))
    };
    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failure(format!("not started: {error}"), "", ""),
    };
    let reader = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    };
    let printed = child.stdout.take().map(|pipe| reader(Box::new(pipe)));
    let errors = child.stderr.take().map(|pipe| reader(Box::new(pipe)));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let printed = printed.and_then(|thread| thread.join().ok()).unwrap_or_default();
    let errors = errors.and_then(|thread| thread.join().ok()).unwrap_or_default();
    let object = printed.lines().rev().map(str::trim).find(|line| line.starts_with('{') && line.ends_with('}'));
    match (status, object) {
        (Some(status), Some(object)) if status.success() => Json::Raw(format!("{{\"under\":{},{}", Json::from(display.name.clone()).print(), &object[1..])),
        (Some(status), _) => failure(format!("ended with {status}"), &errors, &printed),
        (None, _) => failure(format!("killed after {} seconds without an end", limit.as_secs()), &errors, &printed),
    }
}

fn words(scenario: &str, pairs: &[(&str, &str)]) -> Vec<String> {
    let mut words = vec!["--child".to_string(), scenario.to_string()];
    words.extend(pairs.iter().map(|(key, value)| format!("{key}={value}")));
    words
}

/// Whether a child's answer says the presentation mode asked is not offered.
fn not_offered(result: &Json) -> bool {
    matches!(result, Json::Raw(text) if text.contains("\"error\":\"not offered"))
}

fn imports_of(path: &Path, what: &str) -> Json {
    let report = Json::object().with("binary", what);
    match Imports::of_file(path) {
        Ok(imports) => {
            let denied = imports.denied();
            report.with("format", imports.format).with("windowing_graphics_or_audio_libraries", if denied.is_empty() { "none".to_string() } else { denied.join(", ") }).with("imports", imports.libraries.join(", "))
        }
        Err(error) => report.with("error", error),
    }
}

pub fn run_plan(options: &Options) -> Json {
    let program = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("render-windowed-presentation"));
    let beside = |name: &str| program.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let wanted = |section: &str| options.only.is_empty() || options.only.iter().any(|only| only == section);
    let frames = options.frames.to_string();
    let mut facts = Json::object()
        .with("cell", options.cell.clone())
        .with("spike", "render-windowed-presentation")
        .with("with_a_real_display", options.with_display)
        .with("frames_of_a_long_run", options.frames)
        .with(
            "libraries",
            Json::object()
                .with("winit", format!("{} ({})", crate::host_winit::VERSION, crate::host_winit::LICENSE))
                .with("sdl3", format!("{} ({}), through {}", crate::host_sdl::version(), crate::host_sdl::LICENSE, crate::host_sdl::BINDING))
                .with("wgpu", format!("{} ({})", crate::wgpu_surface::VERSION, crate::wgpu_surface::LICENSE)),
        );
    #[cfg(windows)]
    facts.set("display", crate::win32::display_facts());

    #[cfg(target_os = "linux")]
    let servers = Servers::start();
    #[cfg(target_os = "linux")]
    facts.set("display_servers", servers.notes.clone());
    #[cfg(target_os = "linux")]
    let displays = servers.windowed();
    #[cfg(not(target_os = "linux"))]
    let displays = vec![Display::session()];

    let libraries = ["winit", "sdl3"];
    let paths: &[&str] = if cfg!(windows) {
        &["direct3d12", "vulkan", "wgpu-direct3d12", "wgpu-vulkan"]
    } else if cfg!(target_os = "macos") {
        &["metal", "wgpu-metal"]
    } else {
        &["vulkan", "wgpu-vulkan"]
    };
    // The devices each path lists.
    let mut devices: Vec<(&str, Vec<DeviceInfo>)> = Vec::new();
    let mut listed = Vec::new();
    for path in paths {
        match presenter::devices(path) {
            Ok(found) => {
                listed.push(Json::object().with("path", *path).with("devices", Json::List(found.iter().map(|device| Json::object().with("index", device.index).with("name", device.name.clone()).with("kind", device.kind.clone())).collect())));
                // A device listed twice under one name is measured once.
                let mut once: Vec<DeviceInfo> = Vec::new();
                for device in found {
                    if !once.iter().any(|kept| kept.name == device.name) {
                        once.push(device);
                    }
                }
                devices.push((path, once));
            }
            Err(error) => {
                listed.push(Json::object().with("path", *path).with("error", error));
                devices.push((path, Vec::new()));
            }
        }
    }
    facts.set("devices", Json::List(listed));
    let index = |device: &DeviceInfo| device.index.to_string();
    let soak_limit = Duration::from_secs(330);

    if wanted("probes") {
        facts.set(
            "imports",
            Json::List(vec![
                imports_of(&program, "the spike, which links winit, SDL3 and wgpu"),
                imports_of(&beside("windowing-probe-winit"), "a probe that links winit alone"),
                imports_of(&beside("windowing-probe-sdl3"), "a probe that links SDL3 alone"),
            ]),
        );
        let mut probes = Vec::new();
        #[cfg(target_os = "linux")]
        let probe_displays = vec![servers.none(), servers.x11(), servers.wayland(), servers.wayland_that_fails_beside_x11()];
        #[cfg(not(target_os = "linux"))]
        let probe_displays = displays.clone();
        for display in &probe_displays {
            for probe in ["windowing-probe-winit", "windowing-probe-sdl3"] {
                let first = run(&beside(probe), &[], display, Duration::from_secs(60));
                // A request the library refused twice in one process is made
                // once more by a new process that names X11 from its start.
                let refused_twice = matches!(&first, Json::Raw(text) if text.matches("error:").count() >= 2);
                probes.push(first);
                if refused_twice && display.set.iter().any(|(name, _)| name == "DISPLAY") {
                    let again = run(&beside(probe), &["--x11".to_string()], display, Duration::from_secs(60));
                    if let Json::Raw(text) = again {
                        probes.push(Json::Raw(format!("{{\"in_a_new_process\":true,{}", &text[1..])));
                    } else {
                        probes.push(again);
                    }
                }
            }
        }
        facts.set("windowing_probes", Json::List(probes));
        if !cfg!(target_os = "linux") {
            facts.set("no_display", "not run: a logged-on session of this platform always has a desktop; the case of no display is made on the Linux cells");
        }
    }

    if wanted("threads") {
        let mut runs = Vec::new();
        for display in &displays {
            runs.push(run(&program, &words("threads", &[("library", "winit"), ("path", "none")]), display, Duration::from_secs(120)));
            runs.push(run(&program, &words("threads", &[("library", "sdl3"), ("path", "none"), ("subsystems", "yes")]), display, Duration::from_secs(120)));
            for (path, found) in &devices {
                for device in found {
                    runs.push(run(&program, &words("threads", &[("library", "winit"), ("path", path), ("device", &index(device))]), display, Duration::from_secs(120)));
                }
            }
            if let Some((path, found)) = devices.iter().find(|(_, found)| !found.is_empty()) {
                runs.push(run(&program, &words("threads", &[("library", "sdl3"), ("path", path), ("device", &index(&found[0]))]), display, Duration::from_secs(120)));
            }
        }
        facts.set("threads", Json::List(runs));
    }

    if wanted("modal") {
        let display = &displays[0];
        facts.set(
            "move_and_resize_loop",
            Json::List(vec![
                run(&program, &words("modal", &[("library", "winit"), ("host_loop", "the host pumps")]), display, Duration::from_secs(60)),
                run(&program, &words("modal", &[("library", "sdl3"), ("host_loop", "the host pumps")]), display, Duration::from_secs(60)),
                run(&program, &words("modal", &[("library", "sdl3"), ("host_loop", "the library calls back")]), display, Duration::from_secs(60)),
            ]),
        );
    }

    if wanted("events") {
        let display = &displays[0];
        let mut runs = Vec::new();
        for library in libraries {
            for pump in ["1", "4", "16"] {
                runs.push(run(&program, &words("events", &[("library", library), ("pump_ms", pump), ("injection", "posted"), ("count", "200")]), display, Duration::from_secs(120)));
            }
            runs.push(run(&program, &words("events", &[("library", library), ("pump_ms", "4"), ("injection", "system"), ("count", "50")]), display, Duration::from_secs(120)));
        }
        facts.set("injected_events", Json::List(runs));
    }

    // A long run in the fastest mode the surface offers.
    let fastest = |pairs: &[(&str, &str)], display: &Display| -> Json {
        let mut result = Json::Null;
        for mode in ["unsynchronized", "latest", "synchronized"] {
            let mut pairs = pairs.to_vec();
            pairs.push(("mode", mode));
            result = run(&program, &words("soak", &pairs), display, soak_limit);
            if !not_offered(&result) {
                break;
            }
        }
        result
    };

    if wanted("soak") {
        let mut runs = Vec::new();
        for window in ["windowed", "borderless"] {
            for display in &displays {
                for library in libraries {
                    for (path, found) in &devices {
                        for device in found {
                            runs.push(fastest(&[("library", library), ("path", path), ("device", &index(device)), ("window", window), ("frames", &frames)], display));
                        }
                    }
                }
            }
        }
        facts.set("frames_through_gestures", Json::List(runs));
    }

    if wanted("fallback") {
        let mut runs = Vec::new();
        for display in &displays {
            for library in libraries {
                for (path, found) in &devices {
                    if let Some(device) = found.first() {
                        runs.push(fastest(&[("library", library), ("path", path), ("device", &index(device)), ("presenter", "platform-thread"), ("frames", &frames)], display));
                    }
                }
            }
        }
        facts.set("presenter_inside_the_platform_thread", Json::List(runs));
    }

    if wanted("modes") {
        let mut runs = Vec::new();
        for display in &displays {
            for (path, found) in &devices {
                for device in found {
                    let base = [("library", "winit"), ("path", *path), ("device", &index(device))];
                    let with = |extra: &[(&str, &str)]| {
                        let mut pairs = base.to_vec();
                        pairs.extend_from_slice(extra);
                        run(&program, &words("soak", &pairs), display, soak_limit)
                    };
                    runs.push(with(&[("mode", "synchronized"), ("frames", "1200")]));
                    if *path == "vulkan" {
                        runs.push(with(&[("mode", "synchronized"), ("frames", "1200"), ("acquire_wait_ms", "1000")]));
                    }
                    runs.push(with(&[("mode", "latest"), ("frames", "3000")]));
                }
            }
        }
        facts.set("presentation_modes", Json::List(runs));
    }

    if options.with_display {
        if wanted("long") {
            let mut runs = Vec::new();
            for (number, (path, found)) in devices.iter().enumerate() {
                if let Some(device) = found.first() {
                    let library = libraries[number % 2];
                    let mut pairs = vec![("library", library), ("path", *path), ("mode", "synchronized"), ("frames", frames.as_str()), ("limit_seconds", "420")];
                    let device = index(device);
                    pairs.push(("device", &device));
                    if *path == "vulkan" {
                        pairs.push(("acquire_wait_ms", "1000"));
                    }
                    runs.push(run(&program, &words("soak", &pairs), &displays[0], Duration::from_secs(450)));
                }
            }
            facts.set("synchronized_long_runs", Json::List(runs));
        }
        if wanted("pace") {
            let mut runs = Vec::new();
            for (path, found) in devices.iter().filter(|(path, _)| !path.starts_with("wgpu")) {
                if let Some(device) = found.first() {
                    runs.push(run(&program, &words("pace", &[("library", "winit"), ("path", path), ("device", &index(device)), ("seconds", "20"), ("rounds", "3")]), &displays[0], Duration::from_secs(1200)));
                }
            }
            facts.set("pace", Json::List(runs));
        }
    } else {
        facts.set("not_run_without_a_real_display", "the pace of a loop beside a presentation thread against a display's refresh, and ten thousand frames in step with a display: a cell of the research CI has no real display");
    }
    facts
}
