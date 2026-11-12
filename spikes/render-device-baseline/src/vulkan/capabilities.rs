//! What a Vulkan device reports for each item of the baseline and beyond it,
//! read from its features (with the chained 1.1, 1.2 and 1.3 structures and
//! the extension structures it lists), its limits, its queue families and
//! the properties of the compressed formats. Throwaway, never product code.

use std::ffi::c_void;

use super::ffi::*;
use super::gpu::Gpu;
use crate::backend::{Capability, Reported};

/// The chained feature structures a device answered, by structure type.
struct ChainedFeatures {
    blocks: Vec<Box<FeatureBlock>>,
    /// Why no chained structure was read, when none was.
    unavailable: Option<String>,
}

impl ChainedFeatures {
    /// Queries every chained structure the device's version and extension
    /// list allow, in one call of `vkGetPhysicalDeviceFeatures2`.
    fn query(gpu: &Gpu) -> ChainedFeatures {
        let physical = &gpu.physical;
        let version = physical.api_version;
        let Some(functions) = gpu.instance.functions11.as_ref().filter(|_| version >= API_VERSION_1_1) else {
            let reason = format!("the device is used at Vulkan {}, which has no vkGetPhysicalDeviceFeatures2", version_text(version));
            return ChainedFeatures { blocks: Vec::new(), unavailable: Some(reason) };
        };
        let mut types = Vec::new();
        if version >= API_VERSION_1_2 {
            types.extend([structure::PHYSICAL_DEVICE_VULKAN_1_1_FEATURES, structure::PHYSICAL_DEVICE_VULKAN_1_2_FEATURES]);
        } else {
            if physical.has_extension("VK_EXT_descriptor_indexing") {
                types.push(structure::PHYSICAL_DEVICE_DESCRIPTOR_INDEXING_FEATURES);
            }
            if physical.has_extension("VK_KHR_shader_atomic_int64") {
                types.push(structure::PHYSICAL_DEVICE_SHADER_ATOMIC_INT64_FEATURES);
            }
        }
        if version >= API_VERSION_1_3 {
            types.push(structure::PHYSICAL_DEVICE_VULKAN_1_3_FEATURES);
        }
        for (extension, structure_type) in [
            ("VK_EXT_mesh_shader", structure::PHYSICAL_DEVICE_MESH_SHADER_FEATURES),
            ("VK_KHR_ray_query", structure::PHYSICAL_DEVICE_RAY_QUERY_FEATURES),
            ("VK_KHR_acceleration_structure", structure::PHYSICAL_DEVICE_ACCELERATION_STRUCTURE_FEATURES),
        ] {
            if physical.has_extension(extension) {
                types.push(structure_type);
            }
        }
        let mut blocks: Vec<Box<FeatureBlock>> = types.into_iter().map(|structure_type| Box::new(FeatureBlock::new(structure_type))).collect();
        for position in (1..blocks.len()).rev() {
            let next = &mut *blocks[position] as *mut FeatureBlock as *mut c_void;
            blocks[position - 1].p_next = next;
        }
        let mut head = FeatureBlock::new(structure::PHYSICAL_DEVICE_FEATURES_2);
        head.p_next = blocks.first_mut().map_or(std::ptr::null_mut(), |block| &mut **block as *mut FeatureBlock as *mut c_void);
        // SAFETY: every structure of the chain is one the device's version or
        // extension list allows, boxed so that the pointers stay valid, and
        // large enough for the booleans the driver writes.
        unsafe { (functions.get_physical_device_features2)(physical.handle, &mut head) };
        ChainedFeatures { blocks, unavailable: None }
    }

    fn block(&self, structure_type: u32) -> Option<&FeatureBlock> {
        self.blocks.iter().find(|block| block.s_type == structure_type).map(|block| &**block)
    }
}

fn capability(name: &'static str, reported: Reported, evidence: String) -> Capability {
    Capability { name, reported, evidence }
}

fn yes_no(value: bool) -> Reported {
    if value { Reported::Yes } else { Reported::No }
}

fn sample_counts(flags: u32) -> String {
    let counts: Vec<String> = (0..7).filter(|bit| flags & (1 << bit) != 0).map(|bit| (1_u32 << bit).to_string()).collect();
    counts.join(", ")
}

fn format_name(format: u32) -> String {
    const BC: [&str; 16] = [
        "BC1_RGB_UNORM", "BC1_RGB_SRGB", "BC1_RGBA_UNORM", "BC1_RGBA_SRGB", "BC2_UNORM", "BC2_SRGB", "BC3_UNORM", "BC3_SRGB",
        "BC4_UNORM", "BC4_SNORM", "BC5_UNORM", "BC5_SNORM", "BC6H_UFLOAT", "BC6H_SFLOAT", "BC7_UNORM", "BC7_SRGB",
    ];
    const ETC2: [&str; 10] = [
        "ETC2_R8G8B8_UNORM", "ETC2_R8G8B8_SRGB", "ETC2_R8G8B8A1_UNORM", "ETC2_R8G8B8A1_SRGB", "ETC2_R8G8B8A8_UNORM", "ETC2_R8G8B8A8_SRGB",
        "EAC_R11_UNORM", "EAC_R11_SNORM", "EAC_R11G11_UNORM", "EAC_R11G11_SNORM",
    ];
    const ASTC_BLOCKS: [&str; 14] = ["4x4", "5x4", "5x5", "6x5", "6x6", "8x5", "8x6", "8x8", "10x5", "10x6", "10x8", "10x10", "12x10", "12x12"];
    match format {
        131..=146 => BC[(format - 131) as usize].to_string(),
        147..=156 => ETC2[(format - 147) as usize].to_string(),
        157..=184 => {
            let offset = (format - 157) as usize;
            format!("ASTC_{}_{}", ASTC_BLOCKS[offset / 2], if offset % 2 == 0 { "UNORM" } else { "SRGB" })
        }
        _ => format!("format {format}"),
    }
}

/// One compression family: whether the feature bit is set, and which of its
/// formats the device samples with optimal tiling.
fn compression(gpu: &Gpu, name: &'static str, feature_name: &str, feature_bit: usize, formats: std::ops::RangeInclusive<u32>) -> Capability {
    let enabled = gpu.core_features[feature_bit] != 0;
    let total = formats.clone().count();
    let sampled: Vec<String> = formats
        .filter(|&format| gpu.physical.format_features(&gpu.instance, format).optimal_tiling_features & FORMAT_FEATURE_SAMPLED_IMAGE != 0)
        .map(format_name)
        .collect();
    let list = if sampled.is_empty() { "none".to_string() } else { sampled.join(", ") };
    capability(name, yes_no(enabled), format!("{feature_name} = {}; {} of {total} formats sampled with optimal tiling: {list}", u32::from(enabled), sampled.len()))
}

fn limit(gpu: &Gpu, offset: usize) -> u32 {
    gpu.physical.properties.u32_at(offset)
}

fn baseline(gpu: &Gpu) -> Vec<Capability> {
    let instance = &gpu.instance;
    let physical = &gpu.physical;
    let color_attachments = limit(gpu, property::MAX_COLOR_ATTACHMENTS);
    let depth_format = physical.format_features(instance, FORMAT_D32_SFLOAT).optimal_tiling_features;
    let depth_attachment = depth_format & FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT != 0;
    let (color_samples, depth_samples) = (limit(gpu, property::FRAMEBUFFER_COLOR_SAMPLE_COUNTS), limit(gpu, property::FRAMEBUFFER_DEPTH_SAMPLE_COUNTS));
    let compute_family = physical.families.iter().any(|family| family.queue_flags & QUEUE_COMPUTE != 0);
    let storage_image = physical.format_features(instance, FORMAT_R32_UINT).optimal_tiling_features & FORMAT_FEATURE_STORAGE_IMAGE != 0;
    let dynamic_indexing = gpu.core_features[feature::SHADER_SAMPLED_IMAGE_ARRAY_DYNAMIC_INDEXING] != 0;
    let sampled_images = limit(gpu, property::MAX_PER_STAGE_DESCRIPTOR_SAMPLED_IMAGES);
    let timestamp_bits = gpu.timestamp_bits();
    let timestamp_period = physical.properties.f32_at(property::TIMESTAMP_PERIOD);
    let host_visible = gpu.host_visible_memory();
    vec![
        capability(
            "several_color_attachments_and_depth",
            yes_no(color_attachments >= 5 && depth_attachment),
            format!("maxColorAttachments = {color_attachments} (the widest pass of the spike writes 5), D32_SFLOAT depth attachment with optimal tiling = {}", u32::from(depth_attachment)),
        ),
        capability(
            "multisampling_4_samples",
            yes_no(color_samples & SAMPLE_COUNT_4 != 0 && depth_samples & SAMPLE_COUNT_4 != 0),
            format!("framebufferColorSampleCounts = {{{}}}, framebufferDepthSampleCounts = {{{}}}", sample_counts(color_samples), sample_counts(depth_samples)),
        ),
        capability(
            "compute_passes",
            yes_no(compute_family),
            format!("a queue family with the compute bit = {}, maxComputeWorkGroupInvocations = {}", u32::from(compute_family), limit(gpu, property::MAX_COMPUTE_WORK_GROUP_INVOCATIONS)),
        ),
        capability(
            "indirect_draws",
            yes_no(limit(gpu, property::MAX_DRAW_INDIRECT_COUNT) >= 1),
            format!(
                "vkCmdDrawIndirect is core 1.0; maxDrawIndirectCount = {}, multiDrawIndirect = {}, drawIndirectFirstInstance = {}",
                limit(gpu, property::MAX_DRAW_INDIRECT_COUNT),
                gpu.core_features[feature::MULTI_DRAW_INDIRECT],
                gpu.core_features[feature::DRAW_INDIRECT_FIRST_INSTANCE]
            ),
        ),
        capability(
            "storage_buffers",
            yes_no(limit(gpu, property::MAX_PER_STAGE_DESCRIPTOR_STORAGE_BUFFERS) >= 2),
            format!("maxPerStageDescriptorStorageBuffers = {}, maxStorageBufferRange = {}", limit(gpu, property::MAX_PER_STAGE_DESCRIPTOR_STORAGE_BUFFERS), limit(gpu, property::MAX_STORAGE_BUFFER_RANGE)),
        ),
        capability(
            "storage_textures",
            yes_no(storage_image && limit(gpu, property::MAX_PER_STAGE_DESCRIPTOR_STORAGE_IMAGES) >= 1),
            format!("R32_UINT storage image with optimal tiling = {}, maxPerStageDescriptorStorageImages = {}", u32::from(storage_image), limit(gpu, property::MAX_PER_STAGE_DESCRIPTOR_STORAGE_IMAGES)),
        ),
        capability(
            "arrays_of_textures",
            yes_no(dynamic_indexing && sampled_images >= 5),
            format!("shaderSampledImageArrayDynamicIndexing = {}, maxPerStageDescriptorSampledImages = {sampled_images}", u32::from(dynamic_indexing)),
        ),
        capability(
            "timestamp_queries",
            yes_no(timestamp_bits > 0),
            format!(
                "timestampValidBits = {timestamp_bits} on the graphics and compute queue family, timestampComputeAndGraphics = {}, timestampPeriod = {timestamp_period} ns",
                limit(gpu, property::TIMESTAMP_COMPUTE_AND_GRAPHICS)
            ),
        ),
        capability(
            "asynchronous_read_back",
            yes_no(host_visible.is_some()),
            match host_visible {
                Some((index, coherent)) => format!("vkGetFenceStatus is core 1.0; host-visible memory type {index}, coherent = {}", u32::from(coherent)),
                None => "no host-visible memory type".to_string(),
            },
        ),
    ]
}

fn flag(block: &FeatureBlock, position: usize) -> bool {
    block.bits[position] != 0
}

/// A feature of the 1.2 structure, or of the extension structure that
/// carries it on an older device.
fn feature12(chained: &ChainedFeatures, position: usize, extension_type: u32, extension_position: Option<usize>) -> Option<bool> {
    if let Some(block) = chained.block(structure::PHYSICAL_DEVICE_VULKAN_1_2_FEATURES) {
        return Some(flag(block, position));
    }
    chained.block(extension_type).zip(extension_position).map(|(block, position)| flag(block, position))
}

fn no_answer(chained: &ChainedFeatures, what: &str) -> String {
    match &chained.unavailable {
        Some(reason) => format!("{what}: {reason}"),
        None => format!("{what}: neither Vulkan 1.2 nor the extension is offered"),
    }
}

fn atomics(chained: &ChainedFeatures) -> Capability {
    let atomic_type = structure::PHYSICAL_DEVICE_SHADER_ATOMIC_INT64_FEATURES;
    let buffer = feature12(chained, feature12::SHADER_BUFFER_INT64_ATOMICS, atomic_type, Some(0));
    let shared = feature12(chained, feature12::SHADER_SHARED_INT64_ATOMICS, atomic_type, Some(1));
    match buffer.zip(shared) {
        Some((buffer, shared)) => capability("atomics_64_bit", yes_no(buffer && shared), format!("shaderBufferInt64Atomics = {}, shaderSharedInt64Atomics = {}", u32::from(buffer), u32::from(shared))),
        None => capability("atomics_64_bit", Reported::No, no_answer(chained, "VK_KHR_shader_atomic_int64 not listed")),
    }
}

fn draw_count(gpu: &Gpu, chained: &ChainedFeatures) -> Capability {
    let name = "indirect_draw_count_from_a_buffer";
    if let Some(block) = chained.block(structure::PHYSICAL_DEVICE_VULKAN_1_2_FEATURES) {
        let value = flag(block, feature12::DRAW_INDIRECT_COUNT);
        return capability(name, yes_no(value), format!("drawIndirectCount = {}", u32::from(value)));
    }
    let listed: Vec<&str> = ["VK_KHR_draw_indirect_count", "VK_AMD_draw_indirect_count"].into_iter().filter(|extension| gpu.physical.has_extension(extension)).collect();
    let evidence = if listed.is_empty() { no_answer(chained, "VK_KHR_draw_indirect_count not listed") } else { format!("extension listed: {}", listed.join(", ")) };
    capability(name, yes_no(!listed.is_empty()), evidence)
}

fn unbounded_arrays(chained: &ChainedFeatures) -> Capability {
    let name = "resource_arrays_without_a_fixed_bound";
    let indexing_type = structure::PHYSICAL_DEVICE_DESCRIPTOR_INDEXING_FEATURES;
    let shift = |position: usize| position.checked_sub(feature12::DESCRIPTOR_INDEXING_STRUCTURE_OFFSET);
    let read = |position: usize| feature12(chained, position, indexing_type, shift(position));
    let fields = [
        ("runtimeDescriptorArray", feature12::RUNTIME_DESCRIPTOR_ARRAY),
        ("descriptorBindingPartiallyBound", feature12::DESCRIPTOR_BINDING_PARTIALLY_BOUND),
        ("descriptorBindingVariableDescriptorCount", feature12::DESCRIPTOR_BINDING_VARIABLE_DESCRIPTOR_COUNT),
        ("shaderSampledImageArrayNonUniformIndexing", feature12::SHADER_SAMPLED_IMAGE_ARRAY_NON_UNIFORM_INDEXING),
    ];
    let values: Option<Vec<(&str, bool)>> = fields.iter().map(|&(field, position)| read(position).map(|value| (field, value))).collect();
    match values {
        Some(values) => {
            let mut evidence: Vec<String> = values.iter().map(|(field, value)| format!("{field} = {}", u32::from(*value))).collect();
            if let Some(value) = chained.block(structure::PHYSICAL_DEVICE_VULKAN_1_2_FEATURES).map(|block| flag(block, feature12::DESCRIPTOR_INDEXING)) {
                evidence.insert(0, format!("descriptorIndexing = {}", u32::from(value)));
            }
            capability(name, yes_no(values.iter().all(|(_, value)| *value)), evidence.join(", "))
        }
        None => capability(name, Reported::No, no_answer(chained, "VK_EXT_descriptor_indexing not listed")),
    }
}

fn mesh_shaders(gpu: &Gpu, chained: &ChainedFeatures) -> Capability {
    let name = "mesh_shaders";
    let nvidia = if gpu.physical.has_extension("VK_NV_mesh_shader") { "; VK_NV_mesh_shader listed" } else { "" };
    match chained.block(structure::PHYSICAL_DEVICE_MESH_SHADER_FEATURES) {
        Some(block) => {
            let (task, mesh) = (flag(block, 0), flag(block, 1));
            capability(name, yes_no(task && mesh), format!("VK_EXT_mesh_shader listed: taskShader = {}, meshShader = {}{nvidia}", u32::from(task), u32::from(mesh)))
        }
        None if gpu.physical.has_extension("VK_EXT_mesh_shader") => capability(name, Reported::NotReported, no_answer(chained, "VK_EXT_mesh_shader listed but its features were not read")),
        None => capability(name, Reported::No, format!("VK_EXT_mesh_shader not listed{nvidia}")),
    }
}

fn ray_queries(gpu: &Gpu, chained: &ChainedFeatures) -> Capability {
    let name = "ray_queries";
    let query = chained.block(structure::PHYSICAL_DEVICE_RAY_QUERY_FEATURES).map(|block| flag(block, 0));
    let acceleration = chained.block(structure::PHYSICAL_DEVICE_ACCELERATION_STRUCTURE_FEATURES).map(|block| flag(block, 0));
    let listed = |extension: &str| if gpu.physical.has_extension(extension) { "listed" } else { "not listed" };
    let describe = |value: Option<bool>| value.map_or("not read".to_string(), |value| u32::from(value).to_string());
    let evidence = format!(
        "VK_KHR_ray_query {}, rayQuery = {}; VK_KHR_acceleration_structure {}, accelerationStructure = {}",
        listed("VK_KHR_ray_query"),
        describe(query),
        listed("VK_KHR_acceleration_structure"),
        describe(acceleration)
    );
    capability(name, yes_no(query == Some(true) && acceleration == Some(true)), evidence)
}

fn second_queue(gpu: &Gpu) -> Capability {
    let families = &gpu.physical.families;
    let several = families.iter().any(|family| family.queue_count > 1);
    let compute_only = families.iter().any(|family| family.queue_flags & QUEUE_COMPUTE != 0 && family.queue_flags & QUEUE_GRAPHICS == 0);
    let described: Vec<String> = families
        .iter()
        .enumerate()
        .map(|(index, family)| {
            let mut kinds = Vec::new();
            for (bit, kind) in [(QUEUE_GRAPHICS, "graphics"), (QUEUE_COMPUTE, "compute"), (0x4, "transfer")] {
                if family.queue_flags & bit != 0 {
                    kinds.push(kind);
                }
            }
            format!("family {index}: {} queue(s), {}", family.queue_count, kinds.join(" and "))
        })
        .collect();
    capability("second_queue", yes_no(several || compute_only), described.join("; "))
}

fn beyond(gpu: &Gpu) -> Vec<Capability> {
    let chained = ChainedFeatures::query(gpu);
    vec![
        compression(gpu, "texture_compression_bc", "textureCompressionBC", feature::TEXTURE_COMPRESSION_BC, 131..=146),
        compression(gpu, "texture_compression_etc2", "textureCompressionETC2", feature::TEXTURE_COMPRESSION_ETC2, 147..=156),
        compression(gpu, "texture_compression_astc", "textureCompressionASTC_LDR", feature::TEXTURE_COMPRESSION_ASTC_LDR, 157..=184),
        atomics(&chained),
        draw_count(gpu, &chained),
        unbounded_arrays(&chained),
        mesh_shaders(gpu, &chained),
        ray_queries(gpu, &chained),
        second_queue(gpu),
    ]
}

/// Every item of the baseline and beyond it, once each.
pub fn report(gpu: &Gpu) -> Vec<Capability> {
    let mut capabilities = baseline(gpu);
    capabilities.extend(beyond(gpu));
    capabilities
}
