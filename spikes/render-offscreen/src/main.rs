//! Offscreen rendering spike, for the rendering research. Throwaway, never
//! product code.
//!
//! The spike measures, instead of assuming, that a fixed scene renders offscreen
//! with no window and no display server on every cell of the research CI, and
//! what the images are worth as references: the scene (one triangle per alive
//! entity of the reference workload's small population after 100 ticks) is
//! rendered twice from scratch by each graphics API the machine offers at run
//! time (Vulkan through the loader, the software device on the Linux cells;
//! Direct3D 11 on the WARP software device and on the hardware adapter on
//! Windows; Metal on macOS), read back, hashed as a whole and per 32-pixel
//! block, and written as a PNG next to the measurements. The binary loads every
//! graphics library at run time and reads its own imports to show that it
//! links none; it records whether a display variable was set and how many
//! sockets it holds after rendering.

#[cfg(windows)]
mod d3d11;
mod imports;
mod loader;
#[cfg(target_os = "macos")]
mod metal;
mod scene;
mod vulkan;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use reference_workload::{Json, write_platform_and_build};

use scene::{Frame, Scene};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "render-offscreen";
/// Each API renders the scene this many times from scratch.
const RUNS: usize = 2;

/// One frame's facts.
pub struct FrameResult {
    pub hash: u64,
    pub lit_pixels: usize,
    pub render_us: f64,
    pub readback_us: f64,
}

/// What one graphics API path reports.
pub struct Outcome {
    pub api: &'static str,
    pub available: bool,
    pub reason: String,
    pub loader: String,
    pub shader_compiler: String,
    pub shader_compile_us: f64,
    pub device_name: String,
    pub device_type: String,
    pub driver: String,
    pub device_us: f64,
    pub pipeline_us: f64,
    pub frames: Vec<FrameResult>,
    pub frame: Option<Frame>,
    pub notes: Vec<String>,
}

impl Outcome {
    pub fn new(api: &'static str) -> Outcome {
        Outcome {
            api,
            available: false,
            reason: String::new(),
            loader: String::new(),
            shader_compiler: String::new(),
            shader_compile_us: 0.0,
            device_name: String::new(),
            device_type: String::new(),
            driver: String::new(),
            device_us: 0.0,
            pipeline_us: 0.0,
            frames: Vec::new(),
            frame: None,
            notes: Vec::new(),
        }
    }

    pub fn frames_identical(&self) -> bool {
        self.frames.len() == RUNS && self.frames.windows(2).all(|pair| pair[0].hash == pair[1].hash)
    }

    fn write(&self, out: &mut Json, image_path: Option<&Path>) {
        out.object_start();
        out.string("api", self.api);
        out.bool("available", self.available);
        if !self.available {
            out.string("reason", &self.reason);
        }
        out.string("loader", &self.loader);
        out.string("shader_compiler", &self.shader_compiler);
        out.decimal("shader_compile_us", self.shader_compile_us);
        out.string("device_name", &self.device_name);
        out.string("device_type", &self.device_type);
        out.string("driver", &self.driver);
        out.decimal("device_us", self.device_us);
        out.decimal("pipeline_us", self.pipeline_us);
        out.key("frames");
        out.array_start();
        for frame in &self.frames {
            out.object_start();
            out.hex("hash", frame.hash);
            out.u64("lit_pixels", frame.lit_pixels as u64);
            out.decimal("render_us", frame.render_us);
            out.decimal("readback_us", frame.readback_us);
            out.object_end();
        }
        out.array_end();
        out.bool("frames_identical", self.frames_identical());
        match &self.frame {
            Some(frame) => {
                out.hex("image_hash", frame.hash());
                out.u64("lit_pixels", frame.lit_pixels() as u64);
                out.key("block_hashes");
                out.array_start();
                for hash in frame.block_hashes() {
                    out.object_start();
                    out.hex("hash", hash);
                    out.object_end();
                }
                out.array_end();
            }
            None => out.string("image_hash", "none"),
        }
        match image_path {
            Some(path) => out.string("image", &path.display().to_string()),
            None => out.string("image", "none"),
        }
        out.key("notes");
        out.array_start();
        for note in &self.notes {
            out.object_start();
            out.string("note", note);
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
}

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
    match sockets_before {
        Some(count) => out.u64("open_sockets_before_rendering", count),
        None => out.string("open_sockets_before_rendering", "not readable on this platform"),
    }
    match open_sockets() {
        Some(count) => out.u64("open_sockets_after_rendering", count),
        None => out.string("open_sockets_after_rendering", "not readable on this platform"),
    }
    out.key("vulkan_icd_files");
    out.array_start();
    for directory in ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"] {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                out.object_start();
                out.string("file", &entry.path().display().to_string());
                out.object_end();
            }
        }
    }
    out.array_end();
    match vulkan::find_glsl_compiler() {
        Ok((name, version)) => out.string("glsl_compiler", &format!("{name}: {version}")),
        Err(error) => out.string("glsl_compiler", &error),
    }
    out.object_end();
}

fn write_scene(out: &mut Json, scene: &Scene) {
    out.key("scene");
    out.object_start();
    out.string("size", scene::SCENE_SIZE.name());
    out.u64("entities", scene.entities as u64);
    out.u64("alive", scene.alive as u64);
    out.u64("tick", u64::from(scene.tick));
    out.hex("state_hash", scene.state_hash);
    out.bool("matches_sequential_reference", scene.matches_sequential_reference);
    out.u64("triangles", scene.triangles() as u64);
    out.u64("vertices", scene.vertices.len() as u64);
    out.u64("vertex_bytes", scene.vertex_bytes().len() as u64);
    out.hex("vertex_buffer_hash", reference_workload::fnv1a(scene.vertex_bytes()));
    out.u64("width", u64::from(scene::WIDTH));
    out.u64("height", u64::from(scene::HEIGHT));
    out.decimal("build_us", scene.build_us);
    out.object_end();
}

/// Every graphics API path of this platform, tried in order; an API whose
/// loader is absent reports itself unavailable.
fn render_all(scene: &Scene, shaders: &Path, work: &Path) -> Vec<Outcome> {
    #[allow(unused_mut, reason = "the platform paths below push on their platform only")]
    let mut outcomes = vec![vulkan::render(scene, shaders, work, RUNS)];
    #[cfg(windows)]
    {
        outcomes.push(d3d11::render(scene, shaders, true, RUNS));
        outcomes.push(d3d11::render(scene, shaders, false, RUNS));
    }
    #[cfg(target_os = "macos")]
    outcomes.push(metal::render(scene, shaders, RUNS));
    outcomes
}

fn main() {
    let cell = env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let sockets_before = open_sockets();
    let output = output_dir(&cell);
    let work = env::temp_dir().join(format!("{SPIKE}-work-{}", std::process::id()));
    let _ = fs::create_dir_all(&work);
    let scene = Scene::build();
    let outcomes = render_all(&scene, &shader_dir(), &work);

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.u64("runs_per_api", RUNS as u64);
    write_scene(&mut out, &scene);
    out.key("renderers");
    out.array_start();
    for outcome in &outcomes {
        let image_path = outcome.frame.as_ref().and_then(|frame| {
            let path = output.join(format!("{SPIKE}-{}.png", outcome.api));
            fs::write(&path, frame.png()).ok().map(|_| path)
        });
        outcome.write(&mut out, image_path.as_deref());
    }
    out.array_end();
    out.u64("apis_available", outcomes.iter().filter(|o| o.available).count() as u64);
    out.bool("every_available_api_rendered_identical_frames", outcomes.iter().filter(|o| o.available).all(Outcome::frames_identical));
    write_session(&mut out, sockets_before);
    out.key("imports");
    out.object_start();
    match env::current_exe().map_err(|e| e.to_string()).and_then(|path| imports::Imports::of_file(&path)) {
        Ok(imports) => {
            out.string("format", imports.format);
            out.key("libraries");
            out.array_start();
            for name in &imports.libraries {
                out.object_start();
                out.string("name", name);
                out.object_end();
            }
            out.array_end();
            let denied = imports.denied();
            out.bool("imports_a_windowing_graphics_or_audio_library", !denied.is_empty());
            out.key("denied");
            out.array_start();
            for name in &denied {
                out.object_start();
                out.string("name", name);
                out.object_end();
            }
            out.array_end();
        }
        Err(error) => out.string("error", &error),
    }
    out.object_end();
    out.object_end();
    let _ = fs::remove_dir_all(&work);
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The graphics API this platform is expected to offer renders two
    /// identical frames with lit pixels; where no API is available the test
    /// states so and passes, since the absence is a fact the spike reports.
    #[test]
    fn the_platform_api_renders_two_identical_frames_or_reports_its_absence() {
        let scene = Scene::build();
        let work = env::temp_dir().join(format!("{SPIKE}-test-{}", std::process::id()));
        fs::create_dir_all(&work).unwrap();
        let outcomes = render_all(&scene, &shader_dir(), &work);
        let _ = fs::remove_dir_all(&work);
        assert!(!outcomes.is_empty());
        let mut rendered = 0;
        for outcome in &outcomes {
            if !outcome.available {
                eprintln!("{} unavailable here: {}", outcome.api, outcome.reason);
                continue;
            }
            rendered += 1;
            assert!(outcome.frames_identical(), "{}: {:?}", outcome.api, outcome.frames.iter().map(|f| f.hash).collect::<Vec<_>>());
            let frame = outcome.frame.as_ref().expect("a frame");
            assert_eq!(frame.rgba.len(), (scene::WIDTH * scene::HEIGHT * 4) as usize);
            let lit = frame.lit_pixels();
            assert!(lit > 1_000 && lit < 60_000, "{}: {lit} lit pixels", outcome.api);
            // The clear color is opaque black and every triangle a primary color.
            for pixel in frame.rgba.chunks_exact(4) {
                assert_eq!(pixel[3], 255, "{}: alpha", outcome.api);
                assert!(pixel.iter().take(3).all(|&c| c == 0 || c == 255), "{}: {pixel:?}", outcome.api);
            }
        }
        if cfg!(any(windows, target_os = "macos")) {
            assert!(rendered >= 1, "the platform's own API should render");
        }
    }

    #[test]
    fn the_output_directory_is_created_under_out() {
        let dir = output_dir("render-offscreen-test-cell");
        assert!(dir.is_dir());
        let _ = fs::remove_dir_all(&dir);
    }
}
