//! What a Metal device says of the assumed baseline and of what lies beyond
//! it: the GPU families it supports, its own properties, and what a small
//! kernel or a second queue shows when built on it.
//!
//! Metal has no query for several items every device of macOS has (compute
//! passes, indirect draws, buffers and textures a shader writes): for those
//! the evidence is the GPU family the device reports, whose feature table in
//! Apple's documentation holds the item, and the answer is "not reported"
//! where the device names no family. Throwaway, never product code.

use crate::backend::{Capability, Reported};

use super::gpu::{self, Gpu};
use super::objc::{Id, Owned, answers, class, class_name, flag, one_line, unsigned, yes_no};
use super::timestamps::{self, StageTimestamps};
use super::values;

const BC_FORMATS: &str = "BC1_RGBA, BC1_RGBA_sRGB, BC2_RGBA, BC2_RGBA_sRGB, BC3_RGBA, BC3_RGBA_sRGB, BC4_RUnorm, BC4_RSnorm, BC5_RGUnorm, BC5_RGSnorm, BC6H_RGBFloat, BC6H_RGBUfloat, BC7_RGBAUnorm, BC7_RGBAUnorm_sRGB";
const ETC2_FORMATS: &str = "EAC_R11Unorm, EAC_R11Snorm, EAC_RG11Unorm, EAC_RG11Snorm, EAC_RGBA8, EAC_RGBA8_sRGB, ETC2_RGB8, ETC2_RGB8_sRGB, ETC2_RGB8A1, ETC2_RGB8A1_sRGB";
const ASTC_BLOCKS: &str = "4x4, 5x4, 5x5, 6x5, 6x6, 8x5, 8x6, 8x8, 10x5, 10x6, 10x8, 10x10, 12x10, 12x12";
/// The lowest Apple family whose feature table holds the high dynamic range ASTC formats.
const APPLE_FAMILY_WITH_ASTC_HDR: usize = 6;
/// The lowest Apple family the feature tables still describe.
const APPLE_FAMILY_OF_THE_TABLES: usize = 2;

/// The GPU families a device says it supports.
pub struct Families {
    /// Whether the device answers `supportsFamily:` at all.
    answered: bool,
    /// The highest Apple family, or zero.
    apple: usize,
    mac2: bool,
    metal3: bool,
    /// The highest Common family, or zero.
    common: usize,
}

impl Families {
    pub fn read(device: Id) -> Families {
        let supports = |family: isize| send!(u8; device, "supportsFamily:", isize: family).is_ok_and(|answer| answer != 0);
        let highest = |first: isize, count: usize| (1..=count).filter(|number| supports(first + *number as isize - 1)).max().unwrap_or(0);
        Families {
            answered: answers(device, "supportsFamily:"),
            apple: highest(values::GPU_FAMILY_APPLE_1, values::GPU_FAMILY_APPLE_LAST_KNOWN),
            mac2: supports(values::GPU_FAMILY_MAC_2),
            metal3: supports(values::GPU_FAMILY_METAL_3),
            common: highest(values::GPU_FAMILY_COMMON_1, values::GPU_FAMILY_COMMON_LAST_KNOWN),
        }
    }

    /// The families in words, as the device's version of the interface.
    pub fn text(&self) -> String {
        if !self.answered {
            return "the device does not answer supportsFamily:".to_string();
        }
        let apple = if self.apple > 0 { format!("Apple{} (the highest of Apple1 to Apple{})", self.apple, values::GPU_FAMILY_APPLE_LAST_KNOWN) } else { "no Apple family".to_string() };
        let common = if self.common > 0 { format!("Common{}", self.common) } else { "no Common family".to_string() };
        format!("supportsFamily: {apple}, Mac2 {}, Metal3 {}, {common}", yes_no(self.mac2), yes_no(self.metal3))
    }

    /// Whether the device names a family whose feature table Apple publishes.
    fn names_a_documented_family(&self) -> bool {
        self.apple >= APPLE_FAMILY_OF_THE_TABLES || self.mac2
    }
}

fn item(name: &'static str, reported: Reported, evidence: String) -> Capability {
    Capability { name, reported, evidence }
}

fn yes_or_no(answer: bool) -> Reported {
    if answer { Reported::Yes } else { Reported::No }
}

/// An item Metal has no query for and gives to every GPU family of macOS.
fn held_by_every_family(name: &'static str, families: &Families, what: &str) -> Capability {
    let reported = if families.names_a_documented_family() { Reported::Yes } else { Reported::NotReported };
    item(name, reported, format!("{what}. Metal has no query for it: the feature tables of every GPU family of macOS hold it, and the device reports {}", families.text()))
}

fn property_text(device: Id, name: &str) -> String {
    match unsigned(device, name) {
        Some(value) => format!("{name} = {value}"),
        None => format!("{name} is not answered"),
    }
}

fn flag_text(device: Id, name: &str) -> String {
    match flag(device, name) {
        Some(answer) => format!("{name} = {}", yes_no(answer)),
        None => format!("{name} is not answered"),
    }
}

fn several_color_attachments_and_depth(device: Id, families: &Families) -> Capability {
    let reported = if families.names_a_documented_family() { Reported::Yes } else { Reported::NotReported };
    let evidence = format!(
        "Metal has no property for the number of color attachments: the feature tables give 8 to every GPU family of macOS, with the 32-bit float depth format, and the device reports {}; {}",
        families.text(),
        flag_text(device, "isDepth24Stencil8PixelFormatSupported")
    );
    item("several_color_attachments_and_depth", reported, evidence)
}

fn multisampling_4_samples(device: Id) -> Capability {
    let counts: Vec<(usize, Option<bool>)> = [1, 2, 4, 8].into_iter().map(|count| (count, send!(u8; device, "supportsTextureSampleCount:", usize: count).ok().map(|answer| answer != 0))).collect();
    let four = counts.iter().find(|(count, _)| *count == 4).and_then(|(_, answer)| *answer);
    let listed: Vec<String> = counts.iter().map(|(count, answer)| format!("{count} {}", answer.map_or("not answered", yes_no))).collect();
    let reported = four.map_or(Reported::NotReported, yes_or_no);
    item("multisampling_4_samples", reported, format!("supportsTextureSampleCount: {}; {}", listed.join(", "), flag_text(device, "supports32BitMSAA")))
}

fn storage_textures(device: Id, families: &Families) -> Capability {
    let tier = match unsigned(device, "readWriteTextureSupport") {
        Some(tier) => format!("readWriteTextureSupport = {}", values::READ_WRITE_TEXTURE_TIER_NAMES.get(tier).copied().unwrap_or("an unknown tier")),
        None => "readWriteTextureSupport is not answered".to_string(),
    };
    held_by_every_family("storage_textures", families, &format!("A texture with the shader write usage is written by a kernel on every device; {tier} (tier 1 and above also read and write one 32-bit channel in one function)"))
}

fn arrays_of_textures(device: Id, families: &Families) -> Capability {
    held_by_every_family(
        "arrays_of_textures",
        families,
        &format!("An array of textures and an array of samplers are arguments of a shader function; {}, {}", property_text(device, "argumentBuffersSupport"), property_text(device, "maxArgumentBufferSamplerCount")),
    )
}

/// The timestamp counter set, the points where the device samples it, and
/// what the spike can take from it.
fn timestamp_queries(gpu: &Gpu) -> Capability {
    let device = gpu.device.id();
    let sets = timestamps::counter_set_names(device);
    let points = timestamps::sampling_points(device);
    let has_set = sets.as_ref().is_ok_and(|names| names.contains(&gpu.timestamp_set));
    let any_point = points.as_ref().is_some_and(|sampled| sampled.contains(&true));
    let sets_text = match &sets {
        Ok(names) if names.is_empty() => "counterSets lists nothing".to_string(),
        Ok(names) => format!("counterSets = {}", names.join(", ")),
        Err(reason) => reason.clone(),
    };
    let points_text = match &points {
        Some(sampled) => {
            let listed: Vec<String> = values::COUNTER_SAMPLING_POINT_NAMES.iter().zip(sampled).map(|(name, answer)| format!("{name} {}", yes_no(*answer))).collect();
            format!("supportsCounterSampling: {}", listed.join(", "))
        }
        None => "the device does not answer supportsCounterSampling:".to_string(),
    };
    let taken = match StageTimestamps::create(gpu) {
        Ok(_) => {
            let conversion = match gpu.clock.as_ref().and_then(|clock| clock.nanoseconds_per_tick(device)) {
                Some((nanoseconds, span)) => format!("one tick of the device's clock measured as {nanoseconds:.3} ns against the process's clock over {:.1} ms (sampleTimestamps:gpuTimestamp:)", span.as_secs_f64() * 1_000.0),
                None => "the device's clock could not be read beside the process's, so its ticks are taken as nanoseconds".to_string(),
            };
            format!("a sample buffer of two timestamps was created for the boundaries of a pass's stages; {conversion}")
        }
        Err(reason) => format!("the spike samples at the boundaries of a pass's stages only and takes no timestamp here ({reason}); {}", command_buffer_times(gpu)),
    };
    let reported = if sets.is_err() && points.is_none() { Reported::NotReported } else { yes_or_no(has_set && any_point) };
    item("timestamp_queries", reported, format!("{sets_text}; {points_text}; {taken}"))
}

/// What a command buffer offers in place of timestamp queries.
fn command_buffer_times(gpu: &Gpu) -> String {
    match gpu.command_buffer("the look at a command buffer's times") {
        Ok(command_buffer) if answers(command_buffer, "GPUStartTime") && answers(command_buffer, "GPUEndTime") => {
            "a command buffer has the GPUStartTime and GPUEndTime properties instead, in seconds for the whole buffer".to_string()
        }
        Ok(_) => "a command buffer has no GPUStartTime and GPUEndTime properties either".to_string(),
        Err(reason) => reason,
    }
}

fn texture_compression_bc(device: Id) -> Capability {
    match flag(device, "supportsBCTextureCompression") {
        Some(true) => item("texture_compression_bc", Reported::Yes, format!("supportsBCTextureCompression = yes; formats sampled: {BC_FORMATS}")),
        Some(false) => item("texture_compression_bc", Reported::No, "supportsBCTextureCompression = no; no BC format is sampled".to_string()),
        None => item("texture_compression_bc", Reported::NotReported, "the device does not answer supportsBCTextureCompression".to_string()),
    }
}

/// ETC2 and ASTC have no property on macOS: the feature tables give them to
/// the Apple GPU families and to no other.
fn texture_compression_of_apple_families(name: &'static str, families: &Families, formats: String) -> Capability {
    if !families.answered {
        return item(name, Reported::NotReported, families.text());
    }
    let source = format!("Metal has no property for these formats on macOS: the feature tables give them to the Apple GPU families only, and the device reports {}", families.text());
    if families.apple >= APPLE_FAMILY_OF_THE_TABLES { item(name, Reported::Yes, format!("{source}; formats sampled: {formats}")) } else { item(name, Reported::No, format!("{source}; none of these formats is sampled")) }
}

fn astc_formats(families: &Families) -> String {
    let high_range = if families.apple >= APPLE_FAMILY_WITH_ASTC_HDR { ", each also as _HDR" } else { "" };
    format!("ASTC_ with the blocks {ASTC_BLOCKS}, each as _LDR and _sRGB{high_range}")
}

/// Compiles a kernel that takes the maximum into a 64-bit atomic and builds
/// its pipeline: the compiler's answer and the device's are the evidence.
fn atomics_64_bit(gpu: &Gpu, source: &str) -> Capability {
    const FILE: &str = "atomics_64_bit.metal";
    let (library, message) = match gpu.compile(FILE, source) {
        Ok(compiled) => compiled,
        Err(reason) => return item("atomics_64_bit", Reported::No, format!("the compiler refused a kernel that calls atomic_max_explicit on an atomic_ulong: {}", one_line(&reason))),
    };
    let message = message.map_or("without a message".to_string(), |message| format!("with the message: {}", one_line(&message)));
    let pipeline = gpu::function(&library, FILE, "atomics_64_bit_probe").and_then(|function| gpu.compute_pipeline(&function, FILE));
    match pipeline {
        Ok(_) => item("atomics_64_bit", Reported::Yes, format!("the compiler accepted a kernel that calls atomic_max_explicit on an atomic_ulong, {message}, and the device built its compute pipeline")),
        Err(reason) => item("atomics_64_bit", Reported::No, format!("the compiler accepted a kernel that calls atomic_max_explicit on an atomic_ulong, {message}, but the device did not build its pipeline: {}", one_line(&reason))),
    }
}

fn resource_arrays_without_a_fixed_bound(device: Id) -> Capability {
    match unsigned(device, "argumentBuffersSupport") {
        Some(tier) => {
            let second_tier = tier >= values::ARGUMENT_BUFFERS_TIER_2;
            let meaning = if second_tier { "tier 2: an argument buffer holds arrays of resources indexed at run time" } else { "tier 1: an argument buffer holds no array of resources indexed at run time" };
            item("resource_arrays_without_a_fixed_bound", yes_or_no(second_tier), format!("argumentBuffersSupport = {tier} ({meaning}); {}", property_text(device, "maxArgumentBufferSamplerCount")))
        }
        None => item("resource_arrays_without_a_fixed_bound", Reported::NotReported, "the device does not answer argumentBuffersSupport".to_string()),
    }
}

fn mesh_shaders(families: &Families) -> Capability {
    if !families.answered {
        return item("mesh_shaders", Reported::NotReported, families.text());
    }
    let descriptor_class = class("MTLMeshRenderPipelineDescriptor").is_ok();
    let evidence = format!(
        "Metal has no property for mesh shaders: the Metal3 family holds them; supportsFamily: Metal3 {}; the MTLMeshRenderPipelineDescriptor class {}",
        yes_no(families.metal3),
        if descriptor_class { "exists" } else { "does not exist" }
    );
    item("mesh_shaders", yes_or_no(families.metal3 && descriptor_class), evidence)
}

fn ray_queries(device: Id) -> Capability {
    let evidence = format!("{}, {}", flag_text(device, "supportsRaytracing"), flag_text(device, "supportsRaytracingFromRender"));
    item("ray_queries", flag(device, "supportsRaytracing").map_or(Reported::NotReported, yes_or_no), evidence)
}

/// A device has no fixed number of queues: the evidence is a second one, created.
fn second_queue(gpu: &Gpu) -> Capability {
    let second = send!(Id; gpu.device.id(), "newCommandQueue").and_then(|queue| Owned::adopt(queue, "the second newCommandQueue"));
    match second {
        Ok(queue) => item("second_queue", Reported::Yes, format!("a second command queue was created beside the first (class {}); a device states no number of queues", class_name(queue.id()))),
        Err(reason) => item("second_queue", Reported::No, reason),
    }
}

/// Every item of the baseline and of what lies beyond it, once each.
pub fn read(gpu: &Gpu, atomics_source: &str) -> Vec<Capability> {
    let device = gpu.device.id();
    let families = Families::read(device);
    vec![
        several_color_attachments_and_depth(device, &families),
        multisampling_4_samples(device),
        held_by_every_family("compute_passes", &families, "A compute command encoder dispatches kernels on every device"),
        held_by_every_family("indirect_draws", &families, "drawPrimitives:indirectBuffer:indirectBufferOffset: reads one draw's arguments from a buffer on every device"),
        held_by_every_family("storage_buffers", &families, &format!("A buffer is read and written by a kernel through a pointer in the device address space; {}", property_text(device, "maxBufferLength"))),
        storage_textures(device, &families),
        arrays_of_textures(device, &families),
        timestamp_queries(gpu),
        held_by_every_family("asynchronous_read_back", &families, "A blit encoder copies a texture to a buffer in shared storage and the command buffer's status is read without waiting"),
        texture_compression_bc(device),
        texture_compression_of_apple_families("texture_compression_etc2", &families, ETC2_FORMATS.to_string()),
        texture_compression_of_apple_families("texture_compression_astc", &families, astc_formats(&families)),
        atomics_64_bit(gpu, atomics_source),
        item(
            "indirect_draw_count_from_a_buffer",
            Reported::No,
            "the render command encoder has no draw call that reads its number of draws from a buffer: drawPrimitives:indirectBuffer:indirectBufferOffset: issues one draw. Draws whose number is decided on the device go through an indirect command buffer, another mechanism, which the spike does not exercise".to_string(),
        ),
        resource_arrays_without_a_fixed_bound(device),
        mesh_shaders(&families),
        ray_queries(device),
        second_queue(gpu),
    ]
}
