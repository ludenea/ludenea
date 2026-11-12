//! What a Direct3D 12 device reports of the baseline and beyond it, read with
//! `CheckFeatureSupport`, the timestamp frequency of its queue and the
//! creation of a second queue and of a draw command signature. Throwaway,
//! never product code.

use std::ptr;

use super::ffi::*;
use super::gpu::{Gpu, create_queue};
use crate::backend::{Capability, Reported};

pub fn level_name(level: u32) -> String {
    format!("{}_{}", level >> 12, (level >> 8) & 0xf)
}

/// The highest feature level the device reports.
pub fn feature_level(gpu: &Gpu) -> Result<u32, String> {
    // A runtime that does not know the newest level refuses the whole list:
    // the list is shortened from the top until it is accepted.
    let mut last_error = String::new();
    for known in (1..=FEATURE_LEVELS.len()).rev() {
        let mut data = FeatureLevelsData { count: known as u32, requested: FEATURE_LEVELS.as_ptr(), highest: 0 };
        match gpu.feature(FEATURE_FEATURE_LEVELS, &mut data, "D3D12_FEATURE_FEATURE_LEVELS") {
            Ok(()) => return Ok(data.highest),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// The highest shader model the device reports, as 0x66 for 6.6.
pub fn shader_model(gpu: &Gpu) -> Result<u32, String> {
    // The call fails for a model newer than the runtime knows: the request
    // starts at the newest model written here and steps down.
    let mut last_error = String::new();
    for requested in (0x60..=0x69).rev().chain([0x51]) {
        let mut data = ShaderModelData { highest: requested };
        match gpu.feature(FEATURE_SHADER_MODEL, &mut data, "D3D12_FEATURE_SHADER_MODEL") {
            Ok(()) => return Ok(data.highest),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

pub fn model_name(model: u32) -> String {
    format!("{}.{}", model >> 4, model & 0xf)
}

/// Whether the device's memory is unified with the processor's.
pub fn unified_memory(gpu: &Gpu) -> Result<bool, String> {
    let mut data = ArchitectureData::default();
    gpu.feature(FEATURE_ARCHITECTURE, &mut data, "D3D12_FEATURE_ARCHITECTURE")?;
    Ok(data.unified_memory != 0)
}

fn format_support(gpu: &Gpu, format: u32) -> Result<u32, String> {
    let mut data = FormatSupportData { format, ..Default::default() };
    gpu.feature(FEATURE_FORMAT_SUPPORT, &mut data, &format!("D3D12_FEATURE_FORMAT_SUPPORT, format {format}"))?;
    Ok(data.support1)
}

pub fn quality_levels(gpu: &Gpu, format: u32, samples: u32) -> Result<u32, String> {
    let mut data = MultisampleQualityLevelsData { format, sample_count: samples, ..Default::default() };
    gpu.feature(FEATURE_MULTISAMPLE_QUALITY_LEVELS, &mut data, &format!("D3D12_FEATURE_MULTISAMPLE_QUALITY_LEVELS, format {format}"))?;
    Ok(data.quality_levels)
}

pub fn timestamp_frequency(gpu: &Gpu) -> Result<u64, String> {
    let mut frequency = 0_u64;
    // SAFETY: the queue is live; the call writes one 64-bit value.
    check(unsafe { com!(gpu.queue.raw(), slot::queue::GET_TIMESTAMP_FREQUENCY, fn(*mut u64) -> Hresult, &mut frequency) }, "ID3D12CommandQueue::GetTimestampFrequency")?;
    Ok(frequency)
}

/// The draw command signature `ExecuteIndirect` takes, sixteen bytes per draw.
pub fn draw_signature(gpu: &Gpu) -> Result<Com, String> {
    let argument = IndirectArgumentDesc { kind: INDIRECT_ARGUMENT_TYPE_DRAW, data: [0; 3] };
    let desc = CommandSignatureDesc { byte_stride: 16, argument_count: 1, arguments: &argument, node_mask: 0 };
    let mut signature: Object = ptr::null_mut();
    // SAFETY: the device is live; a signature of draws alone takes no root
    // signature; the call writes an ID3D12CommandSignature.
    let result = unsafe {
        com!(gpu.raw(), slot::device::CREATE_COMMAND_SIGNATURE, fn(*const CommandSignatureDesc, Object, *const Guid, *mut Object) -> Hresult, &desc, ptr::null_mut(), &IID_COMMAND_SIGNATURE, &mut signature)
    };
    created(result, signature, "ID3D12Device::CreateCommandSignature (draw)")
}

fn item(name: &'static str, reported: Reported, evidence: String) -> Capability {
    Capability { name, reported, evidence }
}

fn yes_if(condition: bool) -> Reported {
    if condition { Reported::Yes } else { Reported::No }
}

/// Everything read from the device once, for the items below.
struct Facts {
    level: Result<u32, String>,
    model: Result<u32, String>,
    options: Result<OptionsData, String>,
    options1: Result<Options1Data, String>,
    options5: Result<Options5Data, String>,
    options7: Result<Options7Data, String>,
    options9: Result<Options9Data, String>,
    options11: Result<Options11Data, String>,
}

fn options<T: Default>(gpu: &Gpu, feature: u32, what: &str) -> Result<T, String> {
    let mut data = T::default();
    gpu.feature(feature, &mut data, what)?;
    Ok(data)
}

impl Facts {
    fn read(gpu: &Gpu) -> Facts {
        Facts {
            level: feature_level(gpu),
            model: shader_model(gpu),
            options: options(gpu, FEATURE_OPTIONS, "D3D12_FEATURE_D3D12_OPTIONS"),
            options1: options(gpu, FEATURE_OPTIONS1, "D3D12_FEATURE_D3D12_OPTIONS1"),
            options5: options(gpu, FEATURE_OPTIONS5, "D3D12_FEATURE_D3D12_OPTIONS5"),
            options7: options(gpu, FEATURE_OPTIONS7, "D3D12_FEATURE_D3D12_OPTIONS7"),
            options9: options(gpu, FEATURE_OPTIONS9, "D3D12_FEATURE_D3D12_OPTIONS9"),
            options11: options(gpu, FEATURE_OPTIONS11, "D3D12_FEATURE_D3D12_OPTIONS11"),
        }
    }

    fn level_text(&self) -> String {
        match &self.level {
            Ok(level) => format!("feature level {}", level_name(*level)),
            Err(error) => error.clone(),
        }
    }
}

/// The support bits of a format that are named in `wanted`, as text, and
/// whether all of them are set.
fn format_evidence(gpu: &Gpu, format: u32, name: &str, wanted: &[(u32, &str)]) -> (bool, String) {
    match format_support(gpu, format) {
        Ok(bits) => {
            let present: Vec<&str> = wanted.iter().filter(|(bit, _)| bits & bit != 0).map(|(_, label)| *label).collect();
            (present.len() == wanted.len(), format!("{name} support1 = {bits:#x} ({})", if present.is_empty() { "none of the needed bits".to_string() } else { present.join(", ") }))
        }
        Err(error) => (false, format!("{name}: {error}")),
    }
}

pub fn read(gpu: &Gpu) -> Vec<Capability> {
    let facts = Facts::read(gpu);
    let mut items = baseline(gpu, &facts);
    items.extend(beyond(gpu, &facts));
    items
}

fn baseline(gpu: &Gpu, facts: &Facts) -> Vec<Capability> {
    let level_ok = facts.level.as_ref().is_ok_and(|level| *level >= FEATURE_LEVEL_11_0);
    let level = facts.level_text();
    let binding_tier = facts.options.as_ref().map(|options| options.resource_binding_tier);
    let binding = match &binding_tier {
        Ok(tier) => format!("ResourceBindingTier = {tier}"),
        Err(error) => error.to_string(),
    };

    let (srgb_ok, srgb) = format_evidence(gpu, FORMAT_R8G8B8A8_UNORM_SRGB, "R8G8B8A8_UNORM_SRGB", &[(FORMAT_SUPPORT_RENDER_TARGET, "render target"), (FORMAT_SUPPORT_MULTISAMPLE_RENDER_TARGET, "multisample render target"), (FORMAT_SUPPORT_MULTISAMPLE_RESOLVE, "multisample resolve")]);
    let (half_ok, half) = format_evidence(gpu, FORMAT_R16G16B16A16_FLOAT, "R16G16B16A16_FLOAT", &[(FORMAT_SUPPORT_RENDER_TARGET, "render target")]);
    let (depth_ok, depth) = format_evidence(gpu, FORMAT_D32_FLOAT, "D32_FLOAT", &[(FORMAT_SUPPORT_DEPTH_STENCIL, "depth stencil"), (FORMAT_SUPPORT_MULTISAMPLE_RENDER_TARGET, "multisample render target")]);
    let attachments = item(
        "several_color_attachments_and_depth",
        yes_if(level_ok && srgb_ok && half_ok && depth_ok),
        format!("{level}: Direct3D 12 fixes D3D12_SIMULTANEOUS_RENDER_TARGET_COUNT = 8 for every device; {srgb}; {half}; {depth}"),
    );

    let quality = |format: u32, name: &str| match quality_levels(gpu, format, 4) {
        Ok(levels) => (levels > 0, format!("{name} at 4 samples: {levels} quality levels")),
        Err(error) => (false, error),
    };
    let (color_ok, color) = quality(FORMAT_R8G8B8A8_UNORM_SRGB, "R8G8B8A8_UNORM_SRGB");
    let (depth_samples_ok, depth_samples) = quality(FORMAT_D32_FLOAT, "D32_FLOAT");
    let multisampling = item("multisampling_4_samples", yes_if(color_ok && depth_samples_ok), format!("{color}; {depth_samples}"));

    let compute = item("compute_passes", yes_if(level_ok), format!("{level}: compute shaders (cs_5_0 and up) are part of every feature level from 11_0, the least Direct3D 12 creates a device at"));

    let signature = draw_signature(gpu);
    let indirect = item(
        "indirect_draws",
        yes_if(signature.is_ok()),
        match &signature {
            Ok(_) => "ExecuteIndirect is part of the core interface; a command signature of one D3D12_INDIRECT_ARGUMENT_TYPE_DRAW argument, stride 16, was created".to_string(),
            Err(error) => error.clone(),
        },
    );

    let storage_buffers = item("storage_buffers", yes_if(level_ok && binding_tier.is_ok()), format!("{level}: unordered access views of buffers are part of every feature level from 11_0; {binding}"));
    let (uav_ok, uav) = format_evidence(gpu, FORMAT_R32_UINT, "R32_UINT", &[(FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW, "typed unordered access view")]);
    let storage_textures = item("storage_textures", yes_if(level_ok && uav_ok), format!("{level}; {uav}"));
    let arrays = item(
        "arrays_of_textures",
        yes_if(binding_tier.as_ref().is_ok_and(|tier| *tier >= 1)),
        format!("{binding}: tier 1 and above bind a table of shader resource views indexed in the shader (at least 128 per stage at tier 1)"),
    );

    let frequency = timestamp_frequency(gpu);
    let timestamps = item(
        "timestamp_queries",
        yes_if(frequency.is_ok()),
        match &frequency {
            Ok(frequency) => format!("the direct queue's GetTimestampFrequency = {frequency} ticks per second; D3D12_QUERY_HEAP_TYPE_TIMESTAMP queries on a direct queue are core"),
            Err(error) => error.clone(),
        },
    );
    let read_back = item(
        "asynchronous_read_back",
        Reported::Yes,
        "D3D12_HEAP_TYPE_READBACK buffers and ID3D12Fence::GetCompletedValue are core: the copy is submitted and its fence polled without blocking".to_string(),
    );
    vec![attachments, multisampling, compute, indirect, storage_buffers, storage_textures, arrays, timestamps, read_back]
}

fn beyond(gpu: &Gpu, facts: &Facts) -> Vec<Capability> {
    let mut sampled = Vec::new();
    let mut refused = Vec::new();
    let mut errors = Vec::new();
    for (format, name) in BLOCK_COMPRESSED_FORMATS {
        match format_support(gpu, format) {
            Ok(bits) if bits & FORMAT_SUPPORT_SHADER_SAMPLE != 0 => sampled.push(name),
            Ok(_) => refused.push(name),
            Err(error) => errors.push(error),
        }
    }
    let mut evidence = format!("formats whose support includes D3D12_FORMAT_SUPPORT1_SHADER_SAMPLE: {}", if sampled.is_empty() { "none".to_string() } else { sampled.join(", ") });
    if !refused.is_empty() {
        evidence.push_str(&format!("; without sampling: {}", refused.join(", ")));
    }
    if !errors.is_empty() {
        evidence.push_str(&format!("; {}", errors.join("; ")));
    }
    let compression_bc = item("texture_compression_bc", yes_if(refused.is_empty() && errors.is_empty()), evidence);
    let compression_etc2 = item(
        "texture_compression_etc2",
        Reported::NotReported,
        "DXGI names no ETC2 format: Direct3D 12 has no way to ask a device for it, nor to create such a texture".to_string(),
    );
    let compression_astc = item(
        "texture_compression_astc",
        Reported::NotReported,
        "the public DXGI format list names no ASTC format: Direct3D 12 has no way to ask a device for it, nor to create such a texture".to_string(),
    );

    let atomics = atomics_64_bit(facts);
    let count = item(
        "indirect_draw_count_from_a_buffer",
        Reported::Yes,
        "ExecuteIndirect takes a count buffer (pCountBuffer and its offset) on every device: a draw count read from a buffer is part of the core call".to_string(),
    );
    let unbounded = match &facts.options {
        Ok(options) => item(
            "resource_arrays_without_a_fixed_bound",
            yes_if(options.resource_binding_tier >= 2),
            format!("ResourceBindingTier = {}: tier 2 and above let a table of shader resource views span the whole heap with an unbounded range, tier 3 also for constant buffer and unordered access views", options.resource_binding_tier),
        ),
        Err(error) => item("resource_arrays_without_a_fixed_bound", Reported::NotReported, error.clone()),
    };
    let mesh = match &facts.options7 {
        Ok(options) => item("mesh_shaders", yes_if(options.mesh_shader_tier >= 10), format!("D3D12_OPTIONS7 MeshShaderTier = {} (10 is tier 1)", options.mesh_shader_tier)),
        Err(error) => item("mesh_shaders", Reported::NotReported, error.clone()),
    };
    let rays = match &facts.options5 {
        Ok(options) => item("ray_queries", yes_if(options.raytracing_tier >= 11), format!("D3D12_OPTIONS5 RaytracingTier = {} (inline ray queries need tier 1.1, the value 11)", options.raytracing_tier)),
        Err(error) => item("ray_queries", Reported::NotReported, error.clone()),
    };
    let second_queue = match create_queue(gpu.raw(), COMMAND_LIST_TYPE_COMPUTE) {
        Ok(_queue) => item("second_queue", Reported::Yes, "a second command queue of type D3D12_COMMAND_LIST_TYPE_COMPUTE was created beside the direct queue".to_string()),
        Err(error) => item("second_queue", Reported::No, format!("a compute queue beside the direct queue: {error}")),
    };
    vec![compression_bc, compression_etc2, compression_astc, atomics, count, unbounded, mesh, rays, second_queue]
}

/// 64-bit atomics: on raw and structured buffers they come with shader model
/// 6.6 on a device with 64-bit integer operations; on typed resources, group
/// shared memory and resources in a descriptor heap they have flags of their own.
fn atomics_64_bit(facts: &Facts) -> Capability {
    let (Ok(model), Ok(options1)) = (&facts.model, &facts.options1) else {
        let reason = [facts.model.as_ref().err(), facts.options1.as_ref().err()].into_iter().flatten().cloned().collect::<Vec<_>>().join("; ");
        return item("atomics_64_bit", Reported::NotReported, reason);
    };
    let mut evidence = vec![format!("highest shader model {}", model_name(*model)), format!("D3D12_OPTIONS1 Int64ShaderOps = {}", options1.int64_shader_ops)];
    match &facts.options9 {
        Ok(options) => {
            evidence.push(format!("D3D12_OPTIONS9 AtomicInt64OnTypedResourceSupported = {}", options.atomic_int64_on_typed_resource_supported));
            evidence.push(format!("AtomicInt64OnGroupSharedSupported = {}", options.atomic_int64_on_group_shared_supported));
        }
        Err(error) => evidence.push(error.clone()),
    }
    match &facts.options11 {
        Ok(options) => evidence.push(format!("D3D12_OPTIONS11 AtomicInt64OnDescriptorHeapResourceSupported = {}", options.atomic_int64_on_descriptor_heap_resource_supported)),
        Err(error) => evidence.push(error.clone()),
    }
    evidence.push("shader model 6.6 with Int64ShaderOps gives 64-bit atomics on raw and structured buffers".to_string());
    item("atomics_64_bit", yes_if(*model >= 0x66 && options1.int64_shader_ops != 0), evidence.join("; "))
}
