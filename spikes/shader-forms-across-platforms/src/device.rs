//! The device baseline's scene drawn natively with the shaders a chain
//! made, in place of those the native path compiles itself: with the forms
//! made on this machine in this run, and with the forms made on another
//! cell and committed under `forms-from-other-cells/`. Each image is
//! compared, byte for byte, with the image the native path's own shaders
//! give on the same device. Throwaway, never product code.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use reference_workload::Json;
use render_device_baseline::backend::{Api, Recording, SceneShaders};
#[cfg(windows)]
use render_device_baseline::d3d12;
#[cfg(target_os = "macos")]
use render_device_baseline::metal;
use render_device_baseline::scene::Scene;
use render_device_baseline::vulkan;

use crate::chains::Form;

/// The scene's two stages in one form, from one chain, made somewhere.
pub struct Candidate {
    /// The cell that made the form: this run, or the name of another cell.
    pub origin: String,
    pub chain: String,
    pub form: Form,
    pub vertex: Vec<u8>,
    pub fragment: Vec<u8>,
}

fn baseline_shaders() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("render-device-baseline").join("shaders")
}

/// The forms other cells made, as committed with the spike: one directory
/// per cell, one per chain, the scene's two stages per form.
pub fn from_other_cells(own_cell: &str) -> Vec<Candidate> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("forms-from-other-cells");
    let mut candidates = Vec::new();
    let directories = |path: &Path| -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(path).map(|entries| entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect()).unwrap_or_default();
        found.sort();
        found
    };
    let name = |path: &Path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    for cell in directories(&root) {
        if name(&cell) == own_cell {
            continue;
        }
        for chain in directories(&cell) {
            for form in Form::ALL {
                let stage = |stage: &str| fs::read(chain.join(format!("scene.{stage}.{}", form.extension())));
                if let (Ok(vertex), Ok(fragment)) = (stage("vertex"), stage("fragment")) {
                    candidates.push(Candidate { origin: name(&cell), chain: name(&chain), form, vertex, fragment });
                }
            }
        }
    }
    candidates
}

/// The forms an interface's device takes.
fn takes(interface: &str, form: Form) -> bool {
    match interface {
        "vulkan" => form == Form::SpirV,
        "direct3d12" => form == Form::Dxil,
        "metal" => form == Form::MetalText || form == Form::MetalLibrary,
        _ => false,
    }
}

/// The names a chain may give the scene's two functions in a Metal form:
/// the entry points as written in Slang, and the names SPIRV-Cross gives.
const METAL_ENTRY_NAMES: [(&str, &str); 3] = [("scene_vertex", "scene_fragment"), ("main0", "main0"), ("main", "main")];

/// Whether Windows is in its developer mode, read from the registry; the
/// reason where it cannot be read.
fn developer_mode() -> String {
    if !cfg!(windows) {
        return "not Windows".to_string();
    }
    let query = std::process::Command::new("reg").args(["query", r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock", "/v", "AllowDevelopmentWithoutDevLicense"]).output();
    match query {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            if text.contains("0x1") { "on".to_string() } else { "off".to_string() }
        }
        Ok(_) => "off (the registry holds no such value)".to_string(),
        Err(error) => format!("unknown: {error}"),
    }
}

fn open_apis(work: &Path) -> Vec<(&'static str, Result<Box<dyn Api>, String>)> {
    let shaders = baseline_shaders();
    #[allow(unused_mut, reason = "the platform interfaces below are pushed on their platform only")]
    let mut apis = vec![("vulkan", vulkan::open(&shaders, work))];
    #[cfg(windows)]
    apis.push(("direct3d12", d3d12::open(&shaders, work)));
    #[cfg(target_os = "macos")]
    apis.push(("metal", metal::open(&shaders, work)));
    apis
}

/// Draws the scene with one candidate on one device; returns the image's
/// bytes, the entry names that worked on Metal, and the time of the call.
fn draw(api: &dyn Api, index: usize, scene: &Scene, candidate: &Candidate) -> Result<(Vec<u8>, String, f64, Option<(f64, f64)>), String> {
    let names: &[(&str, &str)] = if api.name() == "metal" { &METAL_ENTRY_NAMES } else { &METAL_ENTRY_NAMES[..1] };
    let mut last = String::new();
    for (vertex_entry, fragment_entry) in names {
        let shaders = SceneShaders {
            vertex: candidate.vertex.clone(),
            fragment: candidate.fragment.clone(),
            vertex_entry: vertex_entry.to_string(),
            fragment_entry: fragment_entry.to_string(),
            metal_text: candidate.form == Form::MetalText,
        };
        let started = Instant::now();
        let outcome = api.open_with_scene_shaders(index, &shaders).and_then(|mut device| {
            let render = device.render_scene(scene, Recording::SerialOneBuffer)?;
            Ok((render.image.rgba, device.scene_shader_load().map(|load| (load.libraries_us, load.pipelines_us))))
        });
        match outcome {
            Ok((image, load)) => return Ok((image, format!("{vertex_entry}, {fragment_entry}"), started.elapsed().as_secs_f64() * 1_000.0, load)),
            Err(reason) => last = reason,
        }
    }
    Err(last)
}

/// Draws the scene natively with every candidate its interface takes, on
/// every device of the machine, and reports each against the reference.
pub fn write(out: &mut Json, candidates: &[Candidate], work: &Path) {
    let scene = Scene::build();
    out.key("the_scene_on_a_device");
    out.object_start();
    out.string("windows_developer_mode", &developer_mode());
    out.u64("candidates", candidates.len() as u64);
    out.key("interfaces");
    out.array_start();
    for (interface, api) in open_apis(work) {
        out.object_start();
        out.string("interface", interface);
        let api = match api {
            Ok(api) => api,
            Err(reason) => {
                out.bool("available", false);
                out.string("reason", &reason);
                out.object_end();
                continue;
            }
        };
        out.bool("available", true);
        out.key("devices");
        out.array_start();
        let mut seen = Vec::new();
        for info in api.devices() {
            // A device an interface lists twice is drawn on once.
            if seen.contains(&(info.vendor_id, info.device_id)) {
                continue;
            }
            seen.push((info.vendor_id, info.device_id));
            out.object_start();
            out.string("name", &info.name);
            out.string("kind", info.kind.name());
            out.string("driver", &info.driver);
            let started = Instant::now();
            let reference = api.open(info.index, false).and_then(|mut device| {
                let render = device.render_scene(&scene, Recording::SerialOneBuffer)?;
                Ok((render.image, device.scene_shader_load().map(|load| (load.libraries_us, load.pipelines_us))))
            });
            let reference = match reference {
                Ok((image, load)) => {
                    out.hex("reference_hash", image.hash());
                    out.decimal("reference_milliseconds", started.elapsed().as_secs_f64() * 1_000.0);
                    if let Some((libraries, pipelines)) = load {
                        out.decimal("reference_libraries_us", libraries);
                        out.decimal("reference_pipelines_us", pipelines);
                    }
                    Some(image)
                }
                Err(reason) => {
                    out.string("reference_error", &reason);
                    None
                }
            };
            out.key("draws");
            out.array_start();
            for candidate in candidates.iter().filter(|candidate| takes(interface, candidate.form)) {
                out.object_start();
                out.string("made_on", &candidate.origin);
                out.string("chain", &candidate.chain);
                out.string("form", candidate.form.name());
                match draw(api.as_ref(), info.index, &scene, candidate) {
                    Ok((rgba, names, milliseconds, load)) => {
                        out.bool("accepted_and_drawn", true);
                        out.string("entry_names", &names);
                        out.decimal("milliseconds", milliseconds);
                        if let Some((libraries, pipelines)) = load {
                            out.decimal("libraries_us", libraries);
                            out.decimal("pipelines_us", pipelines);
                        }
                        match &reference {
                            Some(reference) => {
                                let image = render_device_baseline::scene::Image { width: reference.width, height: reference.height, rgba };
                                out.hex("hash", image.hash());
                                out.bool("bytes_equal_the_reference", image == *reference);
                                if let Some(difference) = image.difference(reference).filter(|_| image != *reference) {
                                    out.u64("differing_pixels", difference.differing_pixels);
                                    out.string("largest_difference_per_channel", &format!("{:?}", difference.largest));
                                }
                            }
                            None => out.string("bytes_equal_the_reference", "no reference image"),
                        }
                    }
                    Err(reason) => {
                        out.bool("accepted_and_drawn", false);
                        out.string("reason", &reason.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(700).collect::<String>());
                    }
                }
                out.object_end();
            }
            out.array_end();
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();
    out.object_end();
}
