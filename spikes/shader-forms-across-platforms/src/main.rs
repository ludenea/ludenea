//! Shading forms spike, for the rendering research. Throwaway, never product
//! code.
//!
//! One small shader set goes through the two kinds of chain the research
//! compares: Slang's compiler run as a tool, and a front end to SPIR-V (the
//! glslang tool) with a translator, run with each of SPIRV-Cross and naga,
//! the DirectX shader compiler making DXIL from the HLSL either writes and
//! Apple's compiler a Metal library from the Metal text. The spike records
//! which of the four forms a device takes each chain produces on this
//! machine, whether an import gives the same bytes twice, which payloads a
//! change of the lighting module changes, what each chain says of a shader's
//! stages and parameters, whether it tells which inputs a surface function
//! uses, and the time of each import.
//!
//! Run with a cell label, the spike prints one JSON document.

mod chains;
mod device;
mod json;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use reference_workload::{Json, fnv1a, write_platform_and_build};

use chains::{Chain, Form, Placement, PlacementKind, Requirements, SlangImport, Stage, Tools, Translator};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "shader-forms-across-platforms";
/// The line of the lighting module the change test rewrites, and its
/// replacement, in both source languages.
const LIGHTING_LINE: &str = "SPECULAR_STRENGTH = 0.25";
const LIGHTING_LINE_CHANGED: &str = "SPECULAR_STRENGTH = 0.5";
/// The four surface functions of the set, with whether each reads the view
/// and whether its material moves vertices.
const SURFACES: [(&str, bool, bool); 4] = [("plain", false, false), ("view", true, false), ("computed", false, false), ("moving", false, true)];

fn own_shaders() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders")
}

fn baseline_shaders() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("render-device-baseline").join("shaders")
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

/// Where the GLSL of a stage comes from.
enum Glsl {
    /// A file of the device baseline spike, unchanged.
    Baseline(&'static str),
    /// A file of this spike.
    Own(&'static str),
    /// The lighting module, a surface and a template, composed by text.
    Composed { surface: &'static str, template: &'static str },
}

struct StageSpec {
    stage: Stage,
    slang_entry: &'static str,
    glsl: Glsl,
}

/// One shader of the set, in both source languages.
struct Unit {
    name: String,
    slang_source: &'static str,
    /// The surface a lit shader is composed with.
    surface: Option<&'static str>,
    /// Mesh shaders and ray queries ask recent versions of the targets.
    recent: bool,
    stages: Vec<StageSpec>,
    requirements: Requirements,
}

fn lit_stages(surface: &'static str, vertex_entry: &'static str, fragment_entry: &'static str, fragment_template: &'static str) -> Vec<StageSpec> {
    vec![
        StageSpec { stage: Stage::Vertex, slang_entry: vertex_entry, glsl: Glsl::Composed { surface, template: "lit.vert.template" } },
        StageSpec { stage: Stage::Fragment, slang_entry: fragment_entry, glsl: Glsl::Composed { surface, template: fragment_template } },
    ]
}

/// The shader set: the device baseline's scene with the bindings its native
/// interfaces expect, a lit surface composed with the lighting module in
/// four variants, a compute shader, a mesh shader and a ray query.
fn units() -> Vec<Unit> {
    let scene = Unit {
        name: "scene".to_string(),
        slang_source: "scene.slang",
        surface: None,
        recent: false,
        stages: vec![
            StageSpec { stage: Stage::Vertex, slang_entry: "scene_vertex", glsl: Glsl::Baseline("scene.vert") },
            StageSpec { stage: Stage::Fragment, slang_entry: "scene_fragment", glsl: Glsl::Baseline("scene.frag") },
        ],
        requirements: Requirements {
            placements: vec![
                Placement { set: 0, binding: 0, kind: PlacementKind::Texture, index: 0, count: 5 },
                Placement { set: 0, binding: 1, kind: PlacementKind::Sampler, index: 0, count: 3 },
            ],
            push_constants_first: true,
            vertex_semantics: vec![(0, "POSITION"), (1, "TEXCOORD"), (2, "COLOR")],
        },
    };
    let mut units = vec![scene];
    for (surface, _, _) in SURFACES {
        units.push(Unit {
            name: format!("lit_{surface}"),
            slang_source: "lit.slang",
            surface: Some(surface),
            recent: false,
            stages: lit_stages(surface, "lit_vertex", "lit_fragment", "lit.frag.template"),
            requirements: Requirements::default(),
        });
    }
    let single = |name: &str, slang_source, stage, slang_entry, glsl, recent| Unit {
        name: name.to_string(),
        slang_source,
        surface: None,
        recent,
        stages: vec![StageSpec { stage, slang_entry, glsl }],
        requirements: Requirements::default(),
    };
    units.push(single("compute", "compute.slang", Stage::Compute, "compute_main", Glsl::Baseline("compute.comp"), false));
    units.push(single("mesh", "mesh.slang", Stage::Mesh, "mesh_main", Glsl::Own("triangle.mesh"), true));
    units.push(single("ray_query", "ray_query.slang", Stage::Compute, "ray_main", Glsl::Own("ray_query.comp"), true));
    units
}

/// A surface function alone behind a stub entry point, per surface: what an
/// importer compiles to ask a chain which inputs the surface itself uses.
fn probes() -> Vec<Unit> {
    SURFACES
        .iter()
        .map(|&(surface, _, _)| Unit {
            name: format!("probe_{surface}"),
            slang_source: "probe.slang",
            surface: Some(surface),
            recent: false,
            stages: lit_stages(surface, "probe_vertex", "probe_fragment", "probe.frag.template"),
            requirements: Requirements::default(),
        })
        .collect()
}

/// The lighting module as an import reads it: its GLSL text, and the
/// directory that holds its Slang module.
struct Lighting {
    label: &'static str,
    glsl: String,
    slang_directory: PathBuf,
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The lighting module as written, or with one constant changed.
fn lighting(work: &Path, changed: bool) -> Result<Lighting, String> {
    let glsl = read(&own_shaders().join("glsl").join("lighting.glsl"))?;
    let slang = read(&own_shaders().join("slang").join("lighting.slang"))?;
    if !glsl.contains(LIGHTING_LINE) || !slang.contains(LIGHTING_LINE) {
        return Err("the lighting module no longer holds the line the change test rewrites".to_string());
    }
    let label = if changed { "changed" } else { "original" };
    let (glsl, slang) = if changed { (glsl.replace(LIGHTING_LINE, LIGHTING_LINE_CHANGED), slang.replace(LIGHTING_LINE, LIGHTING_LINE_CHANGED)) } else { (glsl, slang) };
    // Slang looks for an imported module beside the importing file first, so
    // the whole Slang set is copied with the module as this import reads it:
    // a search path alone would leave the original in use.
    let slang_directory = work.join(format!("slang-{label}"));
    copy_tree(&own_shaders().join("slang"), &slang_directory)?;
    fs::write(slang_directory.join("lighting.slang"), slang).map_err(|error| format!("{}: {error}", slang_directory.display()))?;
    Ok(Lighting { label, glsl, slang_directory })
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))?.flatten() {
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            fs::copy(&source, &target).map_err(|error| format!("{}: {error}", source.display()))?;
        }
    }
    Ok(())
}

fn glsl_text(spec: &StageSpec, lighting: &Lighting) -> Result<String, String> {
    match spec.glsl {
        Glsl::Baseline(file) => read(&baseline_shaders().join(file)),
        Glsl::Own(file) => read(&own_shaders().join("glsl").join(file)),
        Glsl::Composed { surface, template } => {
            let directory = own_shaders().join("glsl");
            Ok(format!("#version 450\n{}\n{}\n{}", lighting.glsl, read(&directory.join(format!("surface_{surface}.glsl")))?, read(&directory.join(template))?))
        }
    }
}

/// One import: a stage of a shader through a chain to a form.
struct Import {
    chain: Chain,
    unit: String,
    stage: Stage,
    form: Form,
    outcome: Result<Vec<u8>, String>,
    milliseconds: f64,
}

impl Import {
    fn id(&self) -> String {
        format!("{} {} {}", self.unit, self.stage.name(), self.form.name())
    }
}

/// The front end's SPIR-V of a stage, for the translated chains.
fn front_end(tools: &Tools, unit: &Unit, spec: &StageSpec, lighting: &Lighting, work: &Path) -> Result<Vec<u8>, String> {
    let label = format!("front-{}-{}-{}", unit.name, spec.stage.name(), lighting.label);
    chains::glslang(tools, &glsl_text(spec, lighting)?, spec.stage, unit.recent, work, &label)
}

fn import(tools: &Tools, chain: Chain, unit: &Unit, spec: &StageSpec, form: Form, lighting: &Lighting, work: &Path, reflection: Option<&Path>) -> Import {
    let label = format!("{}-{}-{}-{}-{}", chain.name(), unit.name, spec.stage.name(), form.name(), lighting.label);
    let started = Instant::now();
    let outcome = match chain {
        Chain::Slang => {
            let source = lighting.slang_directory.join(unit.slang_source);
            let mut search = vec![lighting.slang_directory.clone()];
            search.extend(unit.surface.map(|surface| lighting.slang_directory.join("surfaces").join(surface)));
            let request = SlangImport { source: &source, search: &search, entry: spec.slang_entry, stage: spec.stage, recent: unit.recent, reflection };
            chains::slang(tools, &request, form, &work.join(format!("{label}.{}", form.extension())))
        }
        Chain::Translated(translator) => front_end(tools, unit, spec, lighting, work).and_then(|spirv| match form {
            Form::SpirV => Ok(spirv),
            Form::Dxil => chains::to_hlsl(translator, &spirv, spec.stage, unit.recent, &unit.requirements).and_then(|hlsl| chains::dxil(tools, &hlsl, spec.stage, unit.recent, work, &label)),
            Form::MetalText => chains::to_metal_text(translator, &spirv, spec.stage, &unit.requirements).map(|metal| metal.text.into_bytes()),
            Form::MetalLibrary => chains::to_metal_text(translator, &spirv, spec.stage, &unit.requirements).and_then(|metal| chains::metal_library(tools, &metal.text, work, &label)),
        }),
    };
    Import { chain, unit: unit.name.clone(), stage: spec.stage, form, outcome, milliseconds: started.elapsed().as_secs_f64() * 1_000.0 }
}

/// Every stage of every unit through every chain to every form.
fn import_all(tools: &Tools, units: &[Unit], lighting: &Lighting, work: &Path, reflections: Option<&Path>) -> Vec<Import> {
    let mut imports = Vec::new();
    for chain in Chain::ALL {
        for unit in units {
            for spec in &unit.stages {
                for form in Form::ALL {
                    let reflection = reflections.filter(|_| chain == Chain::Slang).map(|directory| directory.join(format!("slang-{}-{}-{}.json", unit.name, spec.stage.name(), form.name())));
                    imports.push(import(tools, chain, unit, spec, form, lighting, work, reflection.as_deref()));
                }
            }
        }
    }
    imports
}

fn write_tools(out: &mut Json, tools: &Tools) {
    out.key("tools");
    out.array_start();
    for tool in [&tools.slangc, &tools.glslang, &tools.dxc, &tools.metal] {
        out.object_start();
        out.string("name", tool.name);
        out.string("license", tool.license);
        match &tool.found {
            Ok(path) => {
                out.bool("found", true);
                out.string("path", &path.display().to_string());
                out.string("version", &tool.version);
                out.string("origin", &tool.origin);
            }
            Err(reason) => {
                out.bool("found", false);
                out.string("reason", reason);
            }
        }
        out.object_end();
    }
    for (name, version, license) in [("naga", "30.0.1", "MIT OR Apache-2.0"), ("SPIRV-Cross, through the spirv-cross2 crate", "0.7.2 (SPIRV-Cross at aa217ae)", "Apache-2.0; the crate MIT OR Apache-2.0")] {
        out.object_start();
        out.string("name", name);
        out.string("license", license);
        out.bool("found", true);
        out.string("version", version);
        out.string("origin", "a crate linked into the spike");
        out.object_end();
    }
    out.array_end();
}

fn write_imports(out: &mut Json, imports: &[Import]) {
    out.key("imports");
    out.array_start();
    for import in imports {
        out.object_start();
        out.string("chain", import.chain.name());
        out.string("unit", &import.unit);
        out.string("stage", import.stage.name());
        out.string("form", import.form.name());
        out.decimal("milliseconds", import.milliseconds);
        match &import.outcome {
            Ok(bytes) => {
                out.bool("made", true);
                out.u64("bytes", bytes.len() as u64);
                out.hex("hash", fnv1a(bytes));
            }
            Err(reason) => {
                out.bool("made", false);
                out.string("reason", reason);
            }
        }
        out.object_end();
    }
    out.array_end();

    out.key("forms_per_chain");
    out.array_start();
    for chain in Chain::ALL {
        for form in Form::ALL {
            let of_pair: Vec<&Import> = imports.iter().filter(|import| import.chain == chain && import.form == form).collect();
            let made: Vec<&&Import> = of_pair.iter().filter(|import| import.outcome.is_ok()).collect();
            out.object_start();
            out.string("chain", chain.name());
            out.string("form", form.name());
            out.u64("stages", of_pair.len() as u64);
            out.u64("made", made.len() as u64);
            out.bool("every_stage_made", made.len() == of_pair.len());
            out.string("not_made", &of_pair.iter().filter(|import| import.outcome.is_err()).map(|import| format!("{} {}", import.unit, import.stage.name())).collect::<Vec<_>>().join(", "));
            if let Some(reason) = of_pair.iter().find_map(|import| import.outcome.as_ref().err()) {
                out.string("first_reason", reason);
            }
            let mut times: Vec<f64> = made.iter().map(|import| import.milliseconds).collect();
            times.sort_by(f64::total_cmp);
            if !times.is_empty() {
                out.decimal("median_milliseconds", times[times.len() / 2]);
                out.decimal("slowest_milliseconds", times[times.len() - 1]);
            }
            out.object_end();
        }
    }
    out.array_end();
}

/// Whether an import gives the same bytes twice, per chain.
fn write_determinism(out: &mut Json, first: &[Import], second: &[Import]) {
    out.key("a_second_import");
    out.array_start();
    for chain in Chain::ALL {
        let pairs: Vec<(&Import, &Import)> = first.iter().zip(second).filter(|(a, b)| a.chain == chain && a.outcome.is_ok() && b.outcome.is_ok()).collect();
        let differing: Vec<String> = pairs.iter().filter(|(a, b)| a.outcome != b.outcome).map(|(a, _)| a.id()).collect();
        out.object_start();
        out.string("chain", chain.name());
        out.u64("payloads_compared", pairs.len() as u64);
        out.u64("payloads_that_differ", differing.len() as u64);
        out.bool("same_bytes_twice", differing.is_empty());
        out.string("differing", &differing.join(", "));
        out.object_end();
    }
    out.array_end();
}

/// Which payloads a change of the lighting module changes, per chain. Three
/// kinds of payload are told apart: the fragment stage of a lit shader,
/// which calls the lighting; its vertex stage, composed with the module and
/// calling nothing of it; and the payloads of shaders composed with no
/// module at all.
fn write_lighting_change(out: &mut Json, original: &[Import], changed: &[Import]) {
    out.key("a_change_of_the_lighting_module");
    out.array_start();
    for chain in Chain::ALL {
        let pairs: Vec<(&Import, &Import)> = original.iter().zip(changed).filter(|(a, b)| a.chain == chain && a.outcome.is_ok() && b.outcome.is_ok()).collect();
        let calls = |import: &Import| import.unit.starts_with("lit_") && import.stage == Stage::Fragment;
        let composed_only = |import: &Import| import.unit.starts_with("lit_") && import.stage != Stage::Fragment;
        let count = |kind: &dyn Fn(&Import) -> bool, differs: bool| pairs.iter().filter(|(a, b)| kind(a) && (a.outcome != b.outcome) == differs).count() as u64;
        let apart = |import: &Import| !import.unit.starts_with("lit_");
        let ids = |kind: &dyn Fn(&Import) -> bool, differs: bool| pairs.iter().filter(|(a, b)| kind(a) && (a.outcome != b.outcome) == differs).map(|(a, _)| a.id()).collect::<Vec<_>>().join(", ");
        out.object_start();
        out.string("chain", chain.name());
        out.u64("payloads_compared", pairs.len() as u64);
        out.u64("that_call_the_module_changed", count(&calls, true));
        out.u64("that_call_the_module_unchanged", count(&calls, false));
        out.u64("composed_with_it_and_calling_nothing_of_it_changed", count(&composed_only, true));
        out.u64("composed_with_it_and_calling_nothing_of_it_unchanged", count(&composed_only, false));
        out.u64("of_shaders_apart_from_it_changed", count(&apart, true));
        out.u64("of_shaders_apart_from_it_unchanged", count(&apart, false));
        out.string("that_call_the_module_and_did_not_change", &ids(&calls, false));
        out.string("composed_only_and_changed", &ids(&composed_only, true));
        out.string("apart_and_changed", &ids(&apart, true));
        out.bool("every_payload_that_calls_it_changes", count(&calls, true) > 0 && count(&calls, false) == 0);
        out.bool("no_payload_apart_from_it_changes", count(&apart, true) == 0);
        out.bool("changes_every_payload_composed_with_it_and_nothing_else", !pairs.is_empty() && count(&calls, false) == 0 && count(&composed_only, false) == 0 && count(&apart, true) == 0);
        out.object_end();
    }
    out.array_end();
}

fn naga_stage(stage: naga::ShaderStage) -> String {
    format!("{stage:?}").to_ascii_lowercase()
}

/// A resource of a naga module by the kind the engine's reflection names.
fn naga_kind(module: &naga::Module, variable: &naga::GlobalVariable) -> String {
    fn of(module: &naga::Module, inner: &naga::TypeInner, space: naga::AddressSpace) -> String {
        match inner {
            naga::TypeInner::Image { class: naga::ImageClass::Storage { .. }, .. } => "storage_texture".to_string(),
            naga::TypeInner::Image { .. } => "texture".to_string(),
            naga::TypeInner::Sampler { .. } => "sampler".to_string(),
            naga::TypeInner::AccelerationStructure { .. } => "acceleration_structure".to_string(),
            naga::TypeInner::BindingArray { base, .. } => format!("array of {}", of(module, &module.types[*base].inner, space)),
            _ => match space {
                naga::AddressSpace::Uniform => "constants".to_string(),
                naga::AddressSpace::Storage { .. } => "storage_buffer".to_string(),
                naga::AddressSpace::Immediate => "per_draw_constants".to_string(),
                _ => "other".to_string(),
            },
        }
    }
    of(module, &module.types[variable.ty].inner, variable.space)
}

/// The stages and parameters of a SPIR-V module as naga reads them, in the
/// spike's reflection format: one line per stage and per parameter.
fn reflect_with_naga(spirv: &[u8]) -> Result<Vec<String>, String> {
    let (module, _) = chains::naga_module(spirv)?;
    let mut lines: Vec<String> = module.entry_points.iter().map(|entry| format!("stage {}", naga_stage(entry.stage))).collect();
    for (_, variable) in module.global_variables.iter() {
        let bound = variable.binding.is_some() || variable.space == naga::AddressSpace::Immediate;
        if bound {
            lines.push(format!("parameter {} {}", variable.name.as_deref().unwrap_or("unnamed"), naga_kind(&module, variable)));
        }
    }
    lines.sort();
    Ok(lines)
}

/// The same through SPIRV-Cross's reflection; `active_only` keeps the
/// parameters the entry point uses.
fn reflect_with_spirv_cross(spirv: &[u8], active_only: bool) -> Result<Vec<String>, String> {
    use spirv_cross2::reflect::ResourceType;
    use spirv_cross2::{Compiler, Module, targets};
    let failed = |error: spirv_cross2::SpirvCrossError| format!("SPIRV-Cross: {error}");
    if spirv.len() % 4 != 0 {
        return Err("not a SPIR-V module".to_string());
    }
    let words: Vec<u32> = spirv.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect();
    let compiler = Compiler::<targets::None>::new(Module::from_words(&words)).map_err(failed)?;
    let resources = if active_only { compiler.shader_resources_for_active_variables(compiler.active_interface_variables().map_err(failed)?).map_err(failed)? } else { compiler.shader_resources().map_err(failed)? };
    let mut lines = vec![format!("stage {}", format!("{:?}", compiler.execution_model().map_err(failed)?).to_ascii_lowercase())];
    for (kind, resource_type) in [
        ("constants", ResourceType::UniformBuffer),
        ("storage_buffer", ResourceType::StorageBuffer),
        ("texture", ResourceType::SeparateImage),
        ("sampler", ResourceType::SeparateSamplers),
        ("storage_texture", ResourceType::StorageImage),
        ("per_draw_constants", ResourceType::PushConstant),
        ("acceleration_structure", ResourceType::AccelerationStructure),
    ] {
        for resource in resources.resources_for_type(resource_type).map_err(failed)? {
            lines.push(format!("parameter {} {kind}", resource.name));
        }
    }
    lines.sort();
    Ok(lines)
}

/// The stages and parameters a Slang reflection document names, in the
/// spike's format, or the reason the document was not understood.
fn reflect_slang_document(text: &str) -> Result<Vec<String>, String> {
    let document = json::parse(text)?;
    let parameters = document.get("parameters").ok_or("the document has no list of parameters")?;
    let mut lines = Vec::new();
    for parameter in parameters.list() {
        let name = parameter.get("name").and_then(json::Value::text).unwrap_or("unnamed");
        let kind = parameter.get("type").map_or("untyped".to_string(), |kind| {
            let part = |key: &str| kind.get(key).and_then(json::Value::text).unwrap_or("");
            [part("kind"), part("baseShape"), part("access")].iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join(" ")
        });
        lines.push(format!("parameter {name} {kind}"));
    }
    for entry in document.get("entryPoints").map(json::Value::list).unwrap_or(&[]) {
        lines.push(format!("stage {} {}", entry.get("stage").and_then(json::Value::text).unwrap_or("unnamed"), entry.get("name").and_then(json::Value::text).unwrap_or("unnamed")));
    }
    lines.sort();
    Ok(lines)
}

/// Whether a Slang reflection document says a parameter is used: the value
/// of a member named `used` in the parameter or in its binding, if any.
fn slang_says_used(text: &str, parameter_name: &str) -> Result<Option<bool>, String> {
    let document = json::parse(text)?;
    let parameters = document.get("parameters").ok_or("the document has no list of parameters")?;
    let Some(parameter) = parameters.list().iter().find(|parameter| parameter.get("name").and_then(json::Value::text) == Some(parameter_name)) else {
        return Ok(None);
    };
    let flag = |value: &json::Value| match value.get("used") {
        Some(json::Value::Bool(used)) => Some(*used),
        Some(json::Value::Number(used)) => Some(*used != 0.0),
        _ => None,
    };
    let in_bindings = parameter.get("bindings").map(json::Value::list).unwrap_or(&[]).iter().find_map(flag);
    Ok(flag(parameter).or_else(|| parameter.get("binding").and_then(flag)).or(in_bindings))
}

fn find<'a>(imports: &'a [Import], chain: Chain, unit: &str, stage: Stage, form: Form) -> Option<&'a Import> {
    imports.iter().find(|import| import.chain == chain && import.unit == unit && import.stage == stage && import.form == form)
}

/// What each chain says of a shader's stages and parameters, per form.
fn write_reflection(out: &mut Json, units: &[Unit], imports: &[Import], reflections: &Path, tools: &Tools, work: &Path, lighting: &Lighting) {
    out.key("reflection");
    out.array_start();
    for unit in units {
        for spec in &unit.stages {
            // The translated chains reflect the one SPIR-V every form of
            // theirs is made from.
            let spirv = find(imports, Chain::Translated(Translator::Naga), &unit.name, spec.stage, Form::SpirV).and_then(|import| import.outcome.as_ref().ok());
            out.object_start();
            out.string("unit", &unit.name);
            out.string("stage", spec.stage.name());
            match spirv {
                Some(spirv) => {
                    for (key, lines) in [("from_the_front_end_through_naga", reflect_with_naga(spirv)), ("from_the_front_end_through_spirv_cross", reflect_with_spirv_cross(spirv, false))] {
                        match lines {
                            Ok(lines) => out.string(key, &lines.join("; ")),
                            Err(reason) => out.string(key, &format!("not read: {reason}")),
                        }
                    }
                    // Whether each translator keeps the parameters' names in
                    // the text it writes, the one thing the SPIR-V's
                    // reflection cannot say of the other forms.
                    let names: Vec<String> = reflect_with_spirv_cross(spirv, false).unwrap_or_default().iter().filter_map(|line| line.split(' ').nth(1).map(str::to_string)).collect();
                    for translator in [Translator::SpirvCross, Translator::Naga] {
                        let texts = [
                            ("hlsl", chains::to_hlsl(translator, spirv, spec.stage, unit.recent, &unit.requirements).map(|translated| translated.text)),
                            ("metal_text", chains::to_metal_text(translator, spirv, spec.stage, &unit.requirements).map(|translated| translated.text)),
                        ];
                        for (language, text) in texts {
                            let key = format!("{}_names_kept_in_{language}", Chain::Translated(translator).name());
                            match text {
                                Ok(text) => out.string(&key, &format!("{} of {}", names.iter().filter(|name| text.contains(name.as_str())).count(), names.len())),
                                Err(reason) => out.string(&key, &format!("not translated: {}", reason.chars().take(200).collect::<String>())),
                            }
                        }
                    }
                }
                None => out.string("from_the_front_end", "no SPIR-V was made"),
            }
            let mut per_form = Vec::new();
            for form in Form::ALL {
                let path = reflections.join(format!("slang-{}-{}-{}.json", unit.name, spec.stage.name(), form.name()));
                let made = find(imports, Chain::Slang, &unit.name, spec.stage, form).is_some_and(|import| import.outcome.is_ok());
                let lines = if made { read(&path).and_then(|text| reflect_slang_document(&text)) } else { Err("the form was not made".to_string()) };
                match &lines {
                    Ok(lines) => out.string(&format!("from_slang_for_{}", form.name()), &lines.join("; ")),
                    Err(reason) => out.string(&format!("from_slang_for_{}", form.name()), &format!("not read: {reason}")),
                }
                per_form.extend(lines.ok());
            }
            out.u64("slang_forms_reflected", per_form.len() as u64);
            out.bool("slang_names_the_same_for_every_form_reflected", per_form.len() > 1 && per_form.windows(2).all(|pair| pair[0] == pair[1]));
            out.object_end();
        }
    }
    out.array_end();
    let _ = (tools, work, lighting);
}

/// Whether each chain tells which inputs a surface function uses: whether it
/// reads the view, and whether its material's vertex code reads anything.
fn write_inputs_used(out: &mut Json, tools: &Tools, lighting: &Lighting, work: &Path, reflections: &Path, lit: &[Import]) {
    let probe_units = probes();
    out.key("inputs_a_surface_function_uses");
    out.array_start();
    for ((surface, reads_view, moves_vertices), probe) in SURFACES.iter().zip(&probe_units) {
        out.object_start();
        out.string("surface", surface);
        out.bool("reads_the_view", *reads_view);
        out.bool("moves_vertices", *moves_vertices);
        let mut answers: Vec<(String, Option<bool>, Option<bool>, String)> = Vec::new();

        // naga, on the lit shader itself: its analysis says, per function,
        // which global each function reads, through the functions it calls.
        let lit_unit = format!("lit_{surface}");
        let per_function = |stage: Stage, function_prefix: &str, global: Option<&str>| -> Result<bool, String> {
            let spirv = find(lit, Chain::Translated(Translator::Naga), &lit_unit, stage, Form::SpirV).and_then(|import| import.outcome.as_ref().ok()).ok_or("no SPIR-V of the lit shader")?;
            let (module, info) = chains::naga_module(spirv)?;
            let (handle, _) = module.functions.iter().find(|(_, function)| function.name.as_deref().is_some_and(|name| name == function_prefix || name.starts_with(&format!("{function_prefix}(")))).ok_or_else(|| format!("no function named {function_prefix} in the module"))?;
            let function_info = &info[handle];
            Ok(module.global_variables.iter().any(|(global_handle, variable)| !function_info[global_handle].is_empty() && global.is_none_or(|wanted| variable.name.as_deref() == Some(wanted))))
        };
        let view = per_function(Stage::Fragment, "surface", Some("view"));
        let moves = per_function(Stage::Vertex, "surface_offset", None);
        let note = [view.as_ref().err(), moves.as_ref().err()].into_iter().flatten().cloned().collect::<Vec<_>>().join("; ");
        answers.push(("glslang_and_naga, per function of the lit shader".to_string(), view.ok(), moves.ok(), note));

        // SPIRV-Cross and Slang, on a probe: the parameters the stub entry
        // point around the surface function uses.
        let probe_spirv = |chain: Chain, stage: Stage| {
            let spec = probe.stages.iter().find(|spec| spec.stage == stage).expect("a probe has both stages");
            let reflection = reflections.join(format!("slang-{}-{}-spirv.json", probe.name, stage.name()));
            import(tools, chain, probe, spec, Form::SpirV, lighting, work, Some(&reflection)).outcome
        };
        let active = |spirv: Result<Vec<u8>, String>, wanted: Option<&str>| -> Result<bool, String> {
            let lines = reflect_with_spirv_cross(&spirv?, true)?;
            Ok(lines.iter().any(|line| line.starts_with("parameter ") && wanted.is_none_or(|wanted| line.split(' ').nth(1).is_some_and(|name| name.to_ascii_lowercase().contains(wanted)))))
        };
        let cross = Chain::Translated(Translator::SpirvCross);
        let (view, moves) = (active(probe_spirv(cross, Stage::Fragment), Some("view")), active(probe_spirv(cross, Stage::Vertex), None));
        let note = [view.as_ref().err(), moves.as_ref().err()].into_iter().flatten().cloned().collect::<Vec<_>>().join("; ");
        answers.push(("glslang_and_spirv_cross, on a probe".to_string(), view.ok(), moves.ok(), note));

        let slang_fragment = probe_spirv(Chain::Slang, Stage::Fragment);
        let slang_vertex = probe_spirv(Chain::Slang, Stage::Vertex);
        let document = |stage: Stage| read(&reflections.join(format!("slang-{}-{}-spirv.json", probe.name, stage.name())));
        let said = |made: &Result<Vec<u8>, String>, stage: Stage, parameter: &str| -> Result<Option<bool>, String> {
            made.as_ref().map_err(Clone::clone)?;
            slang_says_used(&document(stage)?, parameter)
        };
        let view = said(&slang_fragment, Stage::Fragment, "view");
        let moves = said(&slang_vertex, Stage::Vertex, "material");
        let note = match (&view, &moves) {
            (Ok(None), _) | (_, Ok(None)) => "the reflection document has no member that says whether a parameter is used".to_string(),
            _ => [view.as_ref().err(), moves.as_ref().err()].into_iter().flatten().cloned().collect::<Vec<_>>().join("; "),
        };
        answers.push(("slang, its reflection document on a probe".to_string(), view.ok().flatten(), moves.ok().flatten(), note));
        // The same probe's SPIR-V read as any SPIR-V is: what an importer
        // can still do where the document does not say.
        let (view, moves) = (active(slang_fragment, Some("view")), active(slang_vertex, None));
        let note = [view.as_ref().err(), moves.as_ref().err()].into_iter().flatten().cloned().collect::<Vec<_>>().join("; ");
        answers.push(("slang, the SPIR-V of a probe read through SPIRV-Cross".to_string(), view.ok(), moves.ok(), note));

        out.key("answers");
        out.array_start();
        for (how, view, moves, note) in answers {
            out.object_start();
            out.string("how", &how);
            match view {
                Some(view) => out.bool("reads_the_view", view),
                None => out.string("reads_the_view", "not told"),
            }
            match moves {
                Some(moves) => out.bool("moves_vertices", moves),
                None => out.string("moves_vertices", "not told"),
            }
            out.bool("as_the_source_has_it", view == Some(*reads_view) && moves == Some(*moves_vertices));
            if !note.is_empty() {
                out.string("note", &note.chars().take(500).collect::<String>());
            }
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();
}

/// Where Apple's compiler is present, whether it accepts each payload of
/// Metal text a chain made, and the time it takes: the compile such a
/// payload costs the machine that loads it.
fn write_metal_text_check(out: &mut Json, tools: &Tools, imports: &[Import], work: &Path) {
    out.key("metal_text_through_apple_s_compiler");
    out.array_start();
    if tools.metal.found.is_ok() {
        for chain in Chain::ALL {
            let texts: Vec<&Import> = imports.iter().filter(|import| import.chain == chain && import.form == Form::MetalText && import.outcome.is_ok()).collect();
            let mut times = Vec::new();
            let mut refused = Vec::new();
            for import in &texts {
                let text = String::from_utf8_lossy(import.outcome.as_ref().map_or(&[][..], Vec::as_slice)).into_owned();
                let started = Instant::now();
                match chains::metal_library(tools, &text, work, &format!("check-{}-{}-{}", chain.name(), import.unit, import.stage.name())) {
                    Ok(_) => times.push(started.elapsed().as_secs_f64() * 1_000.0),
                    Err(reason) => refused.push(format!("{} {}: {}", import.unit, import.stage.name(), reason.chars().take(260).collect::<String>())),
                }
            }
            times.sort_by(f64::total_cmp);
            out.object_start();
            out.string("chain", chain.name());
            out.u64("texts", texts.len() as u64);
            out.u64("accepted", times.len() as u64);
            out.string("refused", &refused.join(" / "));
            if !times.is_empty() {
                out.decimal("median_milliseconds", times[times.len() / 2]);
                out.decimal("slowest_milliseconds", times[times.len() - 1]);
            }
            out.object_end();
        }
    }
    out.array_end();
}

/// The scene's payloads next to the measurements, for the machines that
/// render a form made elsewhere.
fn publish_scene_forms(output: &Path, imports: &[Import]) -> u64 {
    let directory = output.join("forms");
    let mut published = 0;
    for import in imports.iter().filter(|import| import.unit == "scene") {
        if let Ok(bytes) = &import.outcome {
            let chain = directory.join(import.chain.name());
            if fs::create_dir_all(&chain).is_ok() && fs::write(chain.join(format!("scene.{}.{}", import.stage.name(), import.form.extension())), bytes).is_ok() {
                published += 1;
            }
        }
    }
    published
}

fn measure(cell: &str) -> Result<String, String> {
    let output = output_dir(cell);
    let work = env::temp_dir().join(format!("{SPIKE}-work-{}", std::process::id()));
    fs::create_dir_all(&work).map_err(|error| format!("{}: {error}", work.display()))?;
    let reflections = output.join("reflection");
    fs::create_dir_all(&reflections).map_err(|error| format!("{}: {error}", reflections.display()))?;
    let tools = chains::find_tools();
    let units = units();
    let (original, changed) = (lighting(&work, false)?, lighting(&work, true)?);

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    write_tools(&mut out, &tools);
    out.key("shader_set");
    out.array_start();
    for unit in &units {
        out.object_start();
        out.string("unit", &unit.name);
        out.string("stages", &unit.stages.iter().map(|spec| spec.stage.name()).collect::<Vec<_>>().join(", "));
        out.bool("composed_with_the_lighting_module", unit.surface.is_some());
        out.object_end();
    }
    out.array_end();

    let first = import_all(&tools, &units, &original, &work, Some(&reflections));
    let second = import_all(&tools, &units, &original, &work, None);
    let after_change = import_all(&tools, &units, &changed, &work, None);
    write_imports(&mut out, &first);
    write_determinism(&mut out, &first, &second);
    write_lighting_change(&mut out, &first, &after_change);
    write_reflection(&mut out, &units, &first, &reflections, &tools, &work, &original);
    write_inputs_used(&mut out, &tools, &original, &work, &reflections, &first);
    out.u64("scene_payloads_published", publish_scene_forms(&output, &first));
    // The scene drawn natively with the forms of Slang and of the chain with
    // SPIRV-Cross, made here and on other cells. naga's HLSL binds samplers
    // through a heap of its own and its Metal text binds arrays through
    // argument buffers, which the native path does not implement: its forms
    // are not drawn here (its SPIR-V form is the front end's, the same bytes
    // as the other translated chain's).
    write_metal_text_check(&mut out, &tools, &first, &work);
    let mut candidates = device::from_other_cells(cell);
    for chain in [Chain::Slang, Chain::Translated(Translator::SpirvCross)] {
        for form in Form::ALL {
            let stage = |stage: Stage| find(&first, chain, "scene", stage, form).and_then(|import| import.outcome.clone().ok());
            if let (Some(vertex), Some(fragment)) = (stage(Stage::Vertex), stage(Stage::Fragment)) {
                candidates.push(device::Candidate { origin: "this run".to_string(), chain: chain.name().to_string(), form, vertex, fragment });
            }
        }
    }
    device::write(&mut out, &candidates, &work);
    out.object_end();
    let _ = fs::remove_dir_all(&work);
    Ok(out.finish())
}

fn main() {
    let cell = env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    match measure(&cell) {
        Ok(document) => println!("{document}"),
        Err(reason) => {
            eprintln!("{reason}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shader_set_has_the_stated_units_and_sources() {
        let units = units();
        let names: Vec<&str> = units.iter().map(|unit| unit.name.as_str()).collect();
        assert_eq!(names, ["scene", "lit_plain", "lit_view", "lit_computed", "lit_moving", "compute", "mesh", "ray_query"]);
        assert_eq!(units.iter().map(|unit| unit.stages.len()).sum::<usize>(), 13);
        for unit in units.iter().chain(&probes()) {
            assert!(own_shaders().join("slang").join(unit.slang_source).is_file(), "{}", unit.slang_source);
            if let Some(surface) = unit.surface {
                assert!(own_shaders().join("slang").join("surfaces").join(surface).join("surface.slang").is_file());
            }
        }
    }

    #[test]
    fn a_lit_stage_is_composed_of_the_module_the_surface_and_the_template() {
        let work = env::temp_dir().join(format!("{SPIKE}-test-{}", std::process::id()));
        fs::create_dir_all(&work).unwrap();
        let (original, changed) = (lighting(&work, false).unwrap(), lighting(&work, true).unwrap());
        let units = units();
        let lit = units.iter().find(|unit| unit.name == "lit_view").unwrap();
        let fragment = glsl_text(&lit.stages[1], &original).unwrap();
        assert!(fragment.starts_with("#version 450\n"));
        for part in ["vec3 light_surface(", "Surface surface(SurfaceInput inputs)", "out_color = vec4(light_surface(surface(inputs), world_position), 1.0);", LIGHTING_LINE] {
            assert!(fragment.contains(part), "{part}");
        }
        let after = glsl_text(&lit.stages[1], &changed).unwrap();
        assert!(after.contains(LIGHTING_LINE_CHANGED) && !after.contains(LIGHTING_LINE));
        assert!(read(&changed.slang_directory.join("lighting.slang")).unwrap().contains(LIGHTING_LINE_CHANGED));
        assert!(read(&original.slang_directory.join("lighting.slang")).unwrap().contains(LIGHTING_LINE));
        assert!(changed.slang_directory.join("surfaces").join("view").join("surface.slang").is_file());
        // A shader that is not composed with the module does not read it.
        let scene = glsl_text(&units[0].stages[1], &changed).unwrap();
        assert!(!scene.contains("SPECULAR_STRENGTH"));
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn a_slang_reflection_document_is_reduced_to_stages_and_parameters() {
        let document = r#"{"parameters": [{"name": "view", "binding": {"kind": "descriptorTableSlot", "index": 0, "used": 0}, "type": {"kind": "constantBuffer"}}, {"name": "base_color_image", "type": {"kind": "resource", "baseShape": "texture2D"}}], "entryPoints": [{"name": "lit_fragment", "stage": "fragment"}]}"#;
        assert_eq!(reflect_slang_document(document).unwrap(), ["parameter base_color_image resource texture2D", "parameter view constantBuffer", "stage fragment lit_fragment"]);
        assert_eq!(slang_says_used(document, "view"), Ok(Some(false)));
        assert_eq!(slang_says_used(document, "base_color_image"), Ok(None));
        assert_eq!(slang_says_used(document, "absent"), Ok(None));
        assert!(reflect_slang_document("[]").is_err());
    }
}
