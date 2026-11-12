//! The scene's shaders compiled ahead, on the machine that runs the spike,
//! by its platform's own compiler run as a tool: SPIR-V by the glslang
//! command-line tool from the device baseline's GLSL, DXIL by the DirectX
//! shader compiler of the Windows SDK, a Metal library by the Metal
//! compiler of Xcode. The measuring process runs the tools as child
//! processes and opens no compiler itself. Throwaway, never product code.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::safe::Bindings;

/// The four forms the specification names, and the SPIR-V of the native path.
#[cfg(test)]
pub const FORMS: [&str; 5] = ["spirv", "spirv_of_the_native_path", "dxil", "metal_library", "metal_text"];

/// What compiling one form ahead gave.
pub struct Made {
    pub form: &'static str,
    /// The tool with its version line, or the source the text was read from.
    pub tool: String,
    /// The bytes written, or the reason nothing was.
    pub outcome: Result<u64, String>,
}

fn baseline_shaders() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("render-device-baseline").join("shaders")
}

fn own_shaders() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders")
}

/// The scene's WGSL for the bindings given.
pub fn wgsl(bindings: Bindings) -> Result<String, String> {
    let path = own_shaders().join(match bindings {
        Bindings::Arrays => "scene.wgsl",
        Bindings::Separate => "scene_separate.wgsl",
    });
    fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The bindings a form of shaders is written for on an adapter: the WGSL
/// follows what the adapter offers, the Metal sources compiled ahead bind
/// separately, the two other forms bind arrays.
pub fn bindings_of(form: &str, features: wgpu::Features) -> Result<Bindings, String> {
    match form {
        "wgsl" => Ok(Bindings::offered_by(features)),
        "metal_library" | "metal_text" => Ok(Bindings::Separate),
        _ if Bindings::offered_by(features) == Bindings::Arrays => Ok(Bindings::Arrays),
        _ => Err("the source compiled ahead binds an array of textures, which wgpu does not offer on this adapter".to_string()),
    }
}

fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let output = command.output().map_err(|error| format!("{what} could not be run: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let text = format!("{} {}", String::from_utf8_lossy(&output.stdout).trim(), String::from_utf8_lossy(&output.stderr).trim());
        Err(format!("{what} failed: {}", text.split_whitespace().collect::<Vec<_>>().join(" ")))
    }
}

fn version_line(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
}

fn total_size(paths: &[PathBuf]) -> Result<u64, String> {
    paths.iter().map(|path| fs::metadata(path).map(|metadata| metadata.len()).map_err(|error| format!("{}: {error}", path.display()))).sum()
}

/// SPIR-V from GLSL: the fragment shader is the device baseline's, and the
/// vertex shader is this spike's for wgpu (`form` "spirv") or the
/// baseline's own, which negates y (`form` "spirv_of_the_native_path").
fn spirv(work: &Path, form: &'static str) -> Made {
    let Some((compiler, version)) = ["glslangValidator", "glslang"].iter().find_map(|name| version_line(Command::new(name).arg("--version")).map(|version| (*name, version))) else {
        return Made { form, tool: "none".to_string(), outcome: Err("no GLSL compiler on the path (glslangValidator or glslang)".to_string()) };
    };
    let vertex = if form == "spirv" { own_shaders().join("scene_for_wgpu.vert") } else { baseline_shaders().join("scene.vert") };
    let outputs = [(work.join(format!("{form}.vert.spv")), vertex), (work.join(format!("{form}.frag.spv")), baseline_shaders().join("scene.frag"))];
    let made = outputs.iter().try_for_each(|(output, source)| run(Command::new(compiler).arg("-V").arg(source).arg("-o").arg(output), compiler));
    Made { form, tool: format!("{compiler}: {version}"), outcome: made.and_then(|()| total_size(&[outputs[0].0.clone(), outputs[1].0.clone()])) }
}

/// The DirectX shader compiler: on the path, or the newest one of the
/// Windows SDK for this processor architecture.
fn find_dxc() -> Option<PathBuf> {
    if version_line(Command::new("dxc").arg("--version")).is_some() {
        return Some(PathBuf::from("dxc"));
    }
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = fs::read_dir(kits).ok()?.flatten().map(|entry| entry.path().join(architecture).join("dxc.exe")).filter(|path| path.is_file()).collect();
    versions.sort();
    versions.pop()
}

fn dxil(work: &Path) -> Made {
    if !cfg!(windows) {
        return Made { form: "dxil", tool: "none".to_string(), outcome: Err("DXIL is made on Windows by the DirectX shader compiler of the Windows SDK".to_string()) };
    }
    let Some(compiler) = find_dxc() else {
        return Made { form: "dxil", tool: "none".to_string(), outcome: Err("no DirectX shader compiler (dxc.exe) on the path or in the Windows SDK".to_string()) };
    };
    let version = version_line(Command::new(&compiler).arg("--version")).unwrap_or_default();
    let source = own_shaders().join("scene_for_wgpu.hlsl");
    let outputs = [(work.join("scene_vertex.dxil"), "vs_6_0", crate::safe::VERTEX_ENTRY), (work.join("scene_fragment.dxil"), "ps_6_0", crate::safe::FRAGMENT_ENTRY)];
    let made = outputs.iter().try_for_each(|(output, profile, entry)| run(Command::new(&compiler).arg("-nologo").arg("-T").arg(profile).arg("-E").arg(entry).arg("-Fo").arg(output).arg(&source), "dxc"));
    Made { form: "dxil", tool: format!("{}: {version}", compiler.display()), outcome: made.and_then(|()| total_size(&[outputs[0].0.clone(), outputs[1].0.clone()])) }
}

fn metal_library(work: &Path) -> Made {
    if !cfg!(target_os = "macos") {
        return Made { form: "metal_library", tool: "none".to_string(), outcome: Err("a Metal library is made on macOS by the Metal compiler of Xcode".to_string()) };
    }
    let version = version_line(Command::new("xcrun").args(["-sdk", "macosx", "metal", "--version"])).unwrap_or_else(|| "xcrun metal gave no version".to_string());
    let source = own_shaders().join("scene_for_wgpu.metal");
    let (intermediate, library) = (work.join("scene.air"), work.join("scene.metallib"));
    let made = run(Command::new("xcrun").args(["-sdk", "macosx", "metal", "-c"]).arg(&source).arg("-o").arg(&intermediate), "xcrun metal")
        .and_then(|()| run(Command::new("xcrun").args(["-sdk", "macosx", "metallib"]).arg(&intermediate).arg("-o").arg(&library), "xcrun metallib"));
    Made { form: "metal_library", tool: format!("xcrun metal and metallib: {version}"), outcome: made.and_then(|()| total_size(&[library])) }
}

fn metal_text(work: &Path) -> Made {
    let source = own_shaders().join("scene_for_wgpu.metal");
    let outcome = fs::copy(&source, work.join("scene.metal")).map_err(|error| format!("{}: {error}", source.display()));
    Made { form: "metal_text", tool: "the source as written; the platform compiles it at load".to_string(), outcome }
}

/// Compiles every form this machine can make into the work directory.
pub fn compile(work: &Path) -> Vec<Made> {
    vec![spirv(work, "spirv"), spirv(work, "spirv_of_the_native_path"), dxil(work), metal_library(work), metal_text(work)]
}

/// A form compiled ahead, read back from the work directory.
pub enum Loaded {
    SpirV { vertex: Vec<u32>, fragment: Vec<u32> },
    Dxil { vertex: Vec<u8>, fragment: Vec<u8> },
    MetalLibrary(Vec<u8>),
    MetalText(String),
}

impl Loaded {
    pub fn shaders(&self) -> crate::safe::Shaders<'_> {
        match self {
            Loaded::SpirV { vertex, fragment } => crate::safe::Shaders::SpirV { vertex, fragment },
            Loaded::Dxil { vertex, fragment } => crate::safe::Shaders::Dxil { vertex, fragment },
            Loaded::MetalLibrary(library) => crate::safe::Shaders::MetalLibrary(library),
            Loaded::MetalText(text) => crate::safe::Shaders::MetalText(text),
        }
    }
}

fn words(path: &Path) -> Result<Vec<u32>, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() % 4 != 0 || bytes.len() < 20 {
        return Err(format!("{} is not a SPIR-V module ({} bytes)", path.display(), bytes.len()));
    }
    Ok(bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect())
}

pub fn load(work: &Path, form: &str) -> Result<Loaded, String> {
    let bytes = |name: &str| fs::read(work.join(name)).map_err(|error| format!("{name}: {error}"));
    match form {
        "spirv" | "spirv_of_the_native_path" => Ok(Loaded::SpirV { vertex: words(&work.join(format!("{form}.vert.spv")))?, fragment: words(&work.join(format!("{form}.frag.spv")))? }),
        "dxil" => Ok(Loaded::Dxil { vertex: bytes("scene_vertex.dxil")?, fragment: bytes("scene_fragment.dxil")? }),
        "metal_library" => Ok(Loaded::MetalLibrary(bytes("scene.metallib")?)),
        "metal_text" => fs::read_to_string(work.join("scene.metal")).map(Loaded::MetalText).map_err(|error| format!("scene.metal: {error}")),
        other => Err(format!("no form named {other}")),
    }
}

/// The forms of shaders compiled ahead an interface takes through wgpu.
pub fn forms_of(interface: &str) -> &'static [&'static str] {
    match interface {
        "vulkan" => &["spirv", "spirv_of_the_native_path"],
        "direct3d12" => &["dxil"],
        "metal" => &["metal_library", "metal_text"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_interface_takes_forms_the_spike_knows() {
        for interface in ["vulkan", "direct3d12", "metal"] {
            assert!(!forms_of(interface).is_empty());
            assert!(forms_of(interface).iter().all(|form| FORMS.contains(form)));
        }
        assert!(forms_of("other").is_empty());
        assert!(load(Path::new("."), "glsl").is_err());
    }

    #[test]
    fn the_sources_the_forms_are_made_from_exist() {
        assert!(wgsl(Bindings::Arrays).is_ok_and(|source| source.contains("binding_array")));
        assert!(wgsl(Bindings::Separate).is_ok_and(|source| source.contains("texture_4") && !source.contains("binding_array<")));
        assert!(own_shaders().join("scene_for_wgpu.vert").is_file());
        assert_eq!(bindings_of("metal_text", wgpu::Features::all()), Ok(Bindings::Separate));
        assert_eq!(bindings_of("wgsl", wgpu::Features::empty()), Ok(Bindings::Separate));
        assert_eq!(bindings_of("dxil", wgpu::Features::TEXTURE_BINDING_ARRAY), Ok(Bindings::Arrays));
        assert!(bindings_of("spirv", wgpu::Features::empty()).is_err());
        assert!(baseline_shaders().join("scene.vert").is_file());
        assert!(baseline_shaders().join("scene.frag").is_file());
        assert!(own_shaders().join("scene_for_wgpu.hlsl").is_file());
        assert!(own_shaders().join("scene_for_wgpu.metal").is_file());
    }
}
