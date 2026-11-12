//! The chains the spike compares, each from a shader's source to the forms
//! a device takes. Throwaway, never product code.
//!
//! - **Slang**: its compiler run as a tool, one invocation per stage and
//!   form; it calls the DirectX shader compiler for DXIL and Apple's
//!   compiler for a Metal library.
//! - **A front end to SPIR-V with a translator**: the glslang command-line
//!   tool makes the SPIR-V, which is the Vulkan form; SPIRV-Cross or naga,
//!   linked into the spike, translates it to HLSL and to Metal text; the
//!   DirectX shader compiler makes DXIL from the HLSL and Apple's compiler a
//!   Metal library from the text.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use spirv_cross2::spirv;

/// The four forms a device takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    SpirV,
    Dxil,
    MetalText,
    MetalLibrary,
}

impl Form {
    pub const ALL: [Form; 4] = [Form::SpirV, Form::Dxil, Form::MetalText, Form::MetalLibrary];

    pub fn name(self) -> &'static str {
        match self {
            Form::SpirV => "spirv",
            Form::Dxil => "dxil",
            Form::MetalText => "metal_text",
            Form::MetalLibrary => "metal_library",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Form::SpirV => "spv",
            Form::Dxil => "dxil",
            Form::MetalText => "metal",
            Form::MetalLibrary => "metallib",
        }
    }
}

/// The translators of the second kind of chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Translator {
    SpirvCross,
    Naga,
}

/// A chain from a source to the four forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chain {
    Slang,
    Translated(Translator),
}

impl Chain {
    pub const ALL: [Chain; 3] = [Chain::Slang, Chain::Translated(Translator::SpirvCross), Chain::Translated(Translator::Naga)];

    pub fn name(self) -> &'static str {
        match self {
            Chain::Slang => "slang",
            Chain::Translated(Translator::SpirvCross) => "glslang_and_spirv_cross",
            Chain::Translated(Translator::Naga) => "glslang_and_naga",
        }
    }
}

/// A shader stage, as the tools name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute,
    Mesh,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::Vertex => "vertex",
            Stage::Fragment => "fragment",
            Stage::Compute => "compute",
            Stage::Mesh => "mesh",
        }
    }

    /// The extension the glslang tool reads the stage from.
    fn glsl_extension(self) -> &'static str {
        match self {
            Stage::Vertex => "vert",
            Stage::Fragment => "frag",
            Stage::Compute => "comp",
            Stage::Mesh => "mesh",
        }
    }

    fn hlsl_profile(self, recent: bool) -> String {
        let prefix = match self {
            Stage::Vertex => "vs",
            Stage::Fragment => "ps",
            Stage::Compute => "cs",
            Stage::Mesh => "ms",
        };
        format!("{prefix}_{}", if recent { "6_5" } else { "6_0" })
    }

    fn execution_model(self) -> spirv::ExecutionModel {
        match self {
            Stage::Vertex => spirv::ExecutionModel::Vertex,
            Stage::Fragment => spirv::ExecutionModel::Fragment,
            Stage::Compute => spirv::ExecutionModel::GLCompute,
            Stage::Mesh => spirv::ExecutionModel::MeshEXT,
        }
    }
}

/// One tool the spike runs, as found on this machine.
pub struct Tool {
    pub name: &'static str,
    pub license: &'static str,
    /// The executable, or the reason none was found.
    pub found: Result<PathBuf, String>,
    pub version: String,
    /// Where the executable comes from: a published release the setup
    /// fetched, the path, or a system kit.
    pub origin: String,
}

pub struct Tools {
    pub slangc: Tool,
    pub glslang: Tool,
    pub dxc: Tool,
    pub metal: Tool,
}

/// Where the setup script puts the tools it fetches from their published
/// releases, relative to the workspace the spike runs from.
const TOOLS_DIRECTORY: &str = "target/shader-tools";

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Runs a command; an error holds the start of what it wrote.
pub fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let output = command.output().map_err(|error| format!("{what} could not be run: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let text = one_line(&format!("{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
        Err(format!("{what} failed: {}", text.chars().take(700).collect::<String>()))
    }
}

fn first_line(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
}

/// The files under a directory, a few levels deep, whose name is one of
/// those given.
fn files_named(directory: &Path, names: &[&str], depth: u32, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                files_named(&path, names, depth - 1, found);
            }
        } else if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| names.contains(&name)) {
            found.push(path);
        }
    }
}

/// A tool the setup fetched, then the same tool on the path.
fn fetched_or_on_path(directory: &str, names: &[&str], prefer: &str) -> Option<(PathBuf, String)> {
    let mut found = Vec::new();
    files_named(&Path::new(TOOLS_DIRECTORY).join(directory), names, 5, &mut found);
    found.sort();
    let fetched = found.iter().find(|path| path.to_string_lossy().replace('\\', "/").contains(prefer)).or(found.first());
    if let Some(path) = fetched {
        return Some((fs::canonicalize(path).unwrap_or_else(|_| path.clone()), "a published release, fetched by the setup".to_string()));
    }
    names.iter().find(|name| Command::new(name).arg("--version").output().is_ok()).map(|name| (PathBuf::from(name), "the path".to_string()))
}

pub fn find_tools() -> Tools {
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "x64",
    };
    let tool = |name, license, found: Option<(PathBuf, String)>, version_argument: &str, absent: &str| match found {
        Some((path, origin)) => {
            let version = first_line(Command::new(&path).arg(version_argument)).unwrap_or_else(|| "no version line".to_string());
            Tool { name, license, found: Ok(path), version, origin }
        }
        None => Tool { name, license, found: Err(absent.to_string()), version: String::new(), origin: String::new() },
    };
    let slangc = tool(
        "slangc",
        "Apache-2.0 WITH LLVM-exception",
        fetched_or_on_path("slang", &["slangc", "slangc.exe"], "/bin/"),
        "-version",
        "no Slang compiler: the setup fetched none and none is on the path",
    );
    let glslang = tool(
        "glslang",
        "BSD-3-Clause, BSD-2-Clause, MIT and Apache-2.0 parts",
        fetched_or_on_path("glslang", &["glslang", "glslang.exe", "glslangValidator", "glslangValidator.exe"], "/bin/"),
        "--version",
        "no glslang tool: the setup fetched none and none is on the path",
    );
    let dxc = tool(
        "dxc",
        "University of Illinois/NCSA (LLVM release license)",
        fetched_or_on_path("dxc", &["dxc", "dxc.exe"], &format!("/{architecture}/")),
        "--version",
        "no DirectX shader compiler: Microsoft publishes none for this platform, and none is on the path",
    );
    let metal = if cfg!(target_os = "macos") {
        match first_line(Command::new("xcrun").args(["-sdk", "macosx", "metal", "--version"])) {
            Some(version) if version.to_ascii_lowercase().contains("metal") => {
                Tool { name: "metal", license: "part of Xcode", found: Ok(PathBuf::from("xcrun")), version, origin: "Xcode, through xcrun".to_string() }
            }
            other => Tool { name: "metal", license: "part of Xcode", found: Err(format!("xcrun finds no Metal compiler: {}", other.unwrap_or_default())), version: String::new(), origin: String::new() },
        }
    } else {
        match fetched_or_on_path("metal", &["metal", "metal.exe"], "/bin/") {
            Some((path, origin)) => {
                let version = first_line(Command::new(&path).arg("--version")).unwrap_or_default();
                Tool { name: "metal", license: "Apple's Metal developer tools", found: Ok(path), version, origin }
            }
            None => Tool {
                name: "metal",
                license: "Apple's Metal developer tools",
                found: Err(if cfg!(windows) {
                    "Apple's Metal developer tools for Windows are not installed: Apple gives them behind a sign-in, a step no runner can take".to_string()
                } else {
                    "Apple publishes no Metal compiler for this platform".to_string()
                }),
                version: String::new(),
                origin: String::new(),
            },
        }
    };
    Tools { slangc, glslang, dxc, metal }
}

fn tool_path<'a>(tool: &'a Tool) -> Result<&'a Path, String> {
    tool.found.as_deref().map_err(Clone::clone)
}

/// What one invocation of the Slang compiler is asked.
pub struct SlangImport<'a> {
    /// The source file with the entry point.
    pub source: &'a Path,
    /// The module search path, in order.
    pub search: &'a [PathBuf],
    pub entry: &'a str,
    pub stage: Stage,
    /// Mesh shaders and ray queries ask a recent profile of Direct3D.
    pub recent: bool,
    /// Where the reflection of this invocation is written, when asked.
    pub reflection: Option<&'a Path>,
}

/// One stage of one shader through the Slang compiler, to one form.
pub fn slang(tools: &Tools, import: &SlangImport, form: Form, output: &Path) -> Result<Vec<u8>, String> {
    let slangc = tool_path(&tools.slangc)?;
    let mut command = Command::new(slangc);
    command.arg(import.source);
    for directory in import.search {
        command.arg("-I").arg(directory);
    }
    // No line directive: they would write the path of the source into a
    // payload of text, and the same source read from two directories would
    // then give two payloads.
    command.args(["-entry", import.entry, "-stage", import.stage.name(), "-line-directive-mode", "none"]);
    match form {
        Form::SpirV => {
            command.args(["-target", "spirv", "-fvk-invert-y"]);
        }
        Form::Dxil => {
            let dxc = tool_path(&tools.dxc).map_err(|reason| format!("Slang makes DXIL through the DirectX shader compiler: {reason}"))?;
            command.args(["-target", "dxil", "-profile", if import.recent { "sm_6_5" } else { "sm_6_0" }]);
            // The library Slang loads sits beside the executable on Windows
            // and in a sibling directory in the Linux release.
            if let Some(directory) = dxc.parent() {
                let library = directory.parent().map(|parent| parent.join("lib")).filter(|library| library.is_dir());
                command.arg("-dxc-path").arg(library.as_deref().unwrap_or(directory));
            }
        }
        Form::MetalText => {
            command.args(["-target", "metal"]);
        }
        Form::MetalLibrary => {
            tool_path(&tools.metal).map_err(|reason| format!("Slang makes a Metal library through Apple's compiler: {reason}"))?;
            command.args(["-target", "metallib"]);
        }
    }
    if let Some(reflection) = import.reflection {
        command.arg("-reflection-json").arg(reflection);
    }
    command.arg("-o").arg(output);
    let _ = fs::remove_file(output);
    run(&mut command, "slangc")?;
    fs::read(output).map_err(|error| format!("slangc wrote no {}: {error}", output.display()))
}

/// GLSL text to SPIR-V through the glslang tool.
pub fn glslang(tools: &Tools, text: &str, stage: Stage, recent: bool, work: &Path, label: &str) -> Result<Vec<u8>, String> {
    let glslang = tool_path(&tools.glslang)?;
    let source = work.join(format!("{label}.{}", stage.glsl_extension()));
    let output = work.join(format!("{label}.{}.spv", stage.glsl_extension()));
    fs::write(&source, text).map_err(|error| format!("{}: {error}", source.display()))?;
    let _ = fs::remove_file(&output);
    let mut command = Command::new(glslang);
    command.arg("-V");
    if recent {
        command.args(["--target-env", "vulkan1.2"]);
    }
    command.arg(&source).arg("-o").arg(&output);
    run(&mut command, "glslang")?;
    fs::read(&output).map_err(|error| format!("glslang wrote no {}: {error}", output.display()))
}

fn words(spirv: &[u8]) -> Result<Vec<u32>, String> {
    if spirv.len() % 4 != 0 || spirv.len() < 20 {
        return Err(format!("not a SPIR-V module ({} bytes)", spirv.len()));
    }
    Ok(spirv.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect())
}

/// Where a resource of the SPIR-V goes in the translated text, for a shader
/// that has to meet a device's own bindings: its set and binding, then its
/// Direct3D register and its Metal index.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub set: u32,
    pub binding: u32,
    pub kind: PlacementKind,
    pub index: u32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementKind {
    Texture,
    Sampler,
}

/// What a translation has to respect beyond the SPIR-V itself.
#[derive(Clone, Debug, Default)]
pub struct Requirements {
    /// Resources placed where a device expects them; others are left to
    /// the translator.
    pub placements: Vec<Placement>,
    /// Whether the push constants are the constant buffer at register and
    /// buffer index 0.
    pub push_constants_first: bool,
    /// The HLSL semantic of each vertex input, by location.
    pub vertex_semantics: Vec<(u32, &'static str)>,
}

/// A translated shader: its text and the name its entry point took.
pub struct Translated {
    pub text: String,
    pub entry: String,
}

fn spirv_cross_error(error: impl std::fmt::Display) -> String {
    format!("SPIRV-Cross: {}", one_line(&error.to_string()))
}

fn spirv_cross_hlsl(spirv: &[u8], stage: Stage, recent: bool, requirements: &Requirements) -> Result<Translated, String> {
    use spirv_cross2::compile::hlsl;
    use spirv_cross2::{Compiler, Module, targets};
    let words = words(spirv)?;
    let mut compiler = Compiler::<targets::Hlsl>::new(Module::from_words(&words)).map_err(spirv_cross_error)?;
    let model = stage.execution_model();
    for placement in &requirements.placements {
        let register = Some(hlsl::RegisterBinding { register: placement.index, space: 0 });
        let target = match placement.kind {
            PlacementKind::Texture => hlsl::BindTarget { cbv: None, uav: None, srv: register, sampler: None },
            PlacementKind::Sampler => hlsl::BindTarget { cbv: None, uav: None, srv: None, sampler: register },
        };
        compiler.add_resource_binding(model, hlsl::ResourceBinding::from_qualified(placement.set, placement.binding), &target).map_err(spirv_cross_error)?;
    }
    if requirements.push_constants_first {
        let target = hlsl::BindTarget { cbv: Some(hlsl::RegisterBinding { register: 0, space: 0 }), uav: None, srv: None, sampler: None };
        compiler.add_resource_binding(model, hlsl::ResourceBinding::PushConstantBuffer, &target).map_err(spirv_cross_error)?;
    }
    if stage == Stage::Vertex {
        for (location, semantic) in &requirements.vertex_semantics {
            compiler.remap_vertex_attribute(*location, *semantic).map_err(spirv_cross_error)?;
        }
    }
    let mut options = hlsl::CompilerOptions::default();
    options.shader_model = if recent { hlsl::HlslShaderModel::ShaderModel6_5 } else { hlsl::HlslShaderModel::ShaderModel6_0 };
    // The SPIR-V was made for Vulkan, whose clip space has y down.
    options.common.flip_vertex_y = stage == Stage::Vertex;
    let artifact = compiler.compile(&options).map_err(spirv_cross_error)?;
    Ok(Translated { text: artifact.to_string(), entry: "main".to_string() })
}

fn spirv_cross_msl(spirv: &[u8], stage: Stage, requirements: &Requirements) -> Result<Translated, String> {
    use spirv_cross2::compile::msl;
    use spirv_cross2::{Compiler, Module, targets};
    let words = words(spirv)?;
    let mut compiler = Compiler::<targets::Msl>::new(Module::from_words(&words)).map_err(spirv_cross_error)?;
    let model = stage.execution_model();
    for placement in &requirements.placements {
        let count = std::num::NonZeroU32::new(placement.count);
        let target = match placement.kind {
            PlacementKind::Texture => msl::BindTarget { buffer: 0, texture: placement.index, sampler: 0, count },
            PlacementKind::Sampler => msl::BindTarget { buffer: 0, texture: 0, sampler: placement.index, count },
        };
        compiler.add_resource_binding(model, msl::ResourceBinding::from_qualified(placement.set, placement.binding), &target).map_err(spirv_cross_error)?;
    }
    if requirements.push_constants_first {
        let target = msl::BindTarget { buffer: 0, texture: 0, sampler: 0, count: None };
        compiler.add_resource_binding(model, msl::ResourceBinding::PushConstantBuffer, &target).map_err(spirv_cross_error)?;
    }
    let mut options = msl::CompilerOptions::default();
    options.version = msl::MslVersion::new(2, 3, 0);
    options.common.flip_vertex_y = stage == Stage::Vertex;
    let artifact = compiler.compile(&options).map_err(spirv_cross_error)?;
    Ok(Translated { text: artifact.to_string(), entry: "main0".to_string() })
}

fn naga_error(error: impl std::fmt::Debug) -> String {
    format!("naga: {}", one_line(&format!("{error:?}")).chars().take(600).collect::<String>())
}

/// The SPIR-V as a naga module with its analysis, y turned up as naga's
/// back ends expect.
pub fn naga_module(spirv: &[u8]) -> Result<(naga::Module, naga::valid::ModuleInfo), String> {
    let options = naga::front::spv::Options { adjust_coordinate_space: true, strict_capabilities: false, block_ctx_dump_prefix: None };
    let module = naga::front::spv::parse_u8_slice(spirv, &options).map_err(naga_error)?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).map_err(naga_error)?;
    Ok((module, info))
}

fn naga_hlsl(spirv: &[u8], recent: bool) -> Result<Translated, String> {
    use naga::back::hlsl;
    let (module, info) = naga_module(spirv)?;
    // The writer ends the process when a module has per-draw constants and
    // no place was named for them, so one is always named.
    let options = hlsl::Options {
        shader_model: if recent { hlsl::ShaderModel::V6_5 } else { hlsl::ShaderModel::V6_0 },
        fake_missing_bindings: true,
        immediates_target: Some(hlsl::BindTarget::default()),
        ..Default::default()
    };
    let pipeline = hlsl::PipelineOptions::default();
    let mut text = String::new();
    let reflection = hlsl::Writer::new(&mut text, &options, &pipeline).write(&module, &info, None).map_err(naga_error)?;
    let entry = reflection.entry_point_names.into_iter().next().ok_or("naga: the module has no entry point")?.map_err(naga_error)?;
    Ok(Translated { text, entry })
}

fn naga_msl(spirv: &[u8]) -> Result<Translated, String> {
    use naga::back::msl;
    let (module, info) = naga_module(spirv)?;
    let options = msl::Options { lang_version: (2, 3), fake_missing_bindings: true, ..Default::default() };
    let (text, translation) = msl::write_string(&module, &info, &options, &msl::PipelineOptions::default()).map_err(naga_error)?;
    let entry = translation.entry_point_names.into_iter().next().ok_or("naga: the module has no entry point")?.map_err(naga_error)?;
    Ok(Translated { text, entry })
}

/// A translation run so that a translator that panics on a module fails
/// that import and not the whole measurement.
fn guarded(translate: impl FnOnce() -> Result<Translated, String> + std::panic::UnwindSafe) -> Result<Translated, String> {
    std::panic::catch_unwind(translate).unwrap_or_else(|panic| {
        let message = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string())).unwrap_or_default();
        Err(format!("the translator panicked: {}", one_line(&message)))
    })
}

pub fn to_hlsl(translator: Translator, spirv: &[u8], stage: Stage, recent: bool, requirements: &Requirements) -> Result<Translated, String> {
    guarded(|| match translator {
        Translator::SpirvCross => spirv_cross_hlsl(spirv, stage, recent, requirements),
        Translator::Naga => naga_hlsl(spirv, recent),
    })
}

pub fn to_metal_text(translator: Translator, spirv: &[u8], stage: Stage, requirements: &Requirements) -> Result<Translated, String> {
    guarded(|| match translator {
        Translator::SpirvCross => spirv_cross_msl(spirv, stage, requirements),
        Translator::Naga => naga_msl(spirv),
    })
}

/// DXIL from HLSL through the DirectX shader compiler.
pub fn dxil(tools: &Tools, hlsl: &Translated, stage: Stage, recent: bool, work: &Path, label: &str) -> Result<Vec<u8>, String> {
    let dxc = tool_path(&tools.dxc)?;
    let source = work.join(format!("{label}.hlsl"));
    let output = work.join(format!("{label}.dxil"));
    fs::write(&source, &hlsl.text).map_err(|error| format!("{}: {error}", source.display()))?;
    let _ = fs::remove_file(&output);
    run(Command::new(dxc).arg("-nologo").arg("-T").arg(stage.hlsl_profile(recent)).arg("-E").arg(&hlsl.entry).arg("-Fo").arg(&output).arg(&source), "dxc")?;
    fs::read(&output).map_err(|error| format!("dxc wrote no {}: {error}", output.display()))
}

/// A Metal library from Metal text through Apple's compiler.
pub fn metal_library(tools: &Tools, text: &str, work: &Path, label: &str) -> Result<Vec<u8>, String> {
    let metal = tool_path(&tools.metal)?;
    let (source, intermediate, output) = (work.join(format!("{label}.metal")), work.join(format!("{label}.air")), work.join(format!("{label}.metallib")));
    fs::write(&source, text).map_err(|error| format!("{}: {error}", source.display()))?;
    let _ = fs::remove_file(&output);
    if cfg!(target_os = "macos") {
        run(Command::new("xcrun").args(["-sdk", "macosx", "metal", "-c"]).arg(&source).arg("-o").arg(&intermediate), "xcrun metal")?;
        run(Command::new("xcrun").args(["-sdk", "macosx", "metallib"]).arg(&intermediate).arg("-o").arg(&output), "xcrun metallib")?;
    } else {
        run(Command::new(metal).arg("-c").arg(&source).arg("-o").arg(&intermediate), "metal")?;
        run(Command::new(metal.with_file_name("metallib")).arg(&intermediate).arg("-o").arg(&output), "metallib")?;
    }
    fs::read(&output).map_err(|error| format!("the Metal compiler wrote no {}: {error}", output.display()))
}

