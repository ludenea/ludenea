//! Mesa's software device held to a stated processor capability.
//!
//! On the Linux x86_64 cell one key drew two pictures, by the host's
//! processor. This part captures the frame again, one child process per
//! setting, with the device held by its environment to a stated vector width
//! and to stated processor capabilities, and records the bytes each setting
//! draws beside what the device reads of the processor. Whether a setting
//! makes two hosts alike is read across runs: one run has one host.
//!
//! The settings are those of Mesa 25.2.8 as read in its sources:
//! `LP_NATIVE_VECTOR_WIDTH` sets the width of the vectors the generated code
//! uses (256 bits at most by default), and `GALLIUM_OVERRIDE_CPU_CAPS` clears
//! the capabilities above a named level before the device hands them to the
//! code generator. The processor's name, which the code generator takes from
//! the host and tunes for, has no setting. `GALLIVM_PERF=nopt` states no
//! capability: it removes the optimization passes, and is here to say whether
//! the hosts part in optimized code.

use std::fs;
use std::path::Path;

use reference_workload::Json;
use render_device_baseline::backend::{DeviceInfo, Recording, Setting};
use render_device_baseline::scene::Image;

use crate::frame::{AdapterFault, Change, Frame};
use crate::pixels;

/// Mesa's on-disk shader cache keys its entries on neither the vector width
/// nor the capabilities: every held capture compiles afresh.
const WITHOUT_THE_CACHE: (&str, &str) = ("MESA_SHADER_CACHE_DISABLE", "true");
const WIDTH: &str = "LP_NATIVE_VECTOR_WIDTH";
const CAPABILITIES: &str = "GALLIUM_OVERRIDE_CPU_CAPS";
const PERFORMANCE: &str = "GALLIVM_PERF";

/// The capabilities Mesa's processor detection hands to the code generator
/// on x86, as `/proc/cpuinfo` names them.
const CAPABILITIES_THE_DEVICE_READS: [&str; 16] =
    ["sse", "sse2", "pni", "ssse3", "sse4_1", "sse4_2", "avx", "f16c", "fma", "avx2", "avx512f", "avx512cd", "avx512bw", "avx512dq", "avx512vl", "avx512vbmi"];

/// Whether the measurement applies: Mesa's software device on x86_64, the
/// one architecture the capability levels exist for.
pub fn applies(info: &DeviceInfo) -> bool {
    cfg!(target_arch = "x86_64") && info.name.starts_with("llvmpipe")
}

pub fn settings() -> Vec<Setting> {
    let setting = |label: &str, what: &str, environment: &[(&str, &str)]| Setting {
        label: label.to_string(),
        what: what.to_string(),
        environment: environment.iter().chain([&WITHOUT_THE_CACHE]).map(|(name, value)| (name.to_string(), value.to_string())).collect(),
        affinity_cpus: None,
    };
    vec![
        setting("as_started", "the host's own capabilities and the default vector width, compiled afresh: the control", &[]),
        setting("vector_width_128", "128-bit vectors, the host's own capabilities", &[(WIDTH, "128")]),
        setting("vector_width_256", "256-bit vectors stated, the host's own capabilities", &[(WIDTH, "256")]),
        setting("capabilities_to_avx2", "capabilities held below AVX-512 (AVX2, FMA and F16C stay), 256-bit vectors", &[(CAPABILITIES, "avx"), (WIDTH, "256")]),
        setting("capabilities_to_avx2_width_128", "capabilities held below AVX-512, 128-bit vectors", &[(CAPABILITIES, "avx"), (WIDTH, "128")]),
        setting("capabilities_to_sse4", "capabilities held below AVX (no AVX, AVX2, FMA or F16C), 128-bit vectors", &[(CAPABILITIES, "sse4.1"), (WIDTH, "128")]),
        setting("capabilities_to_sse2", "capabilities held to SSE2, which every x86_64 processor has, 128-bit vectors", &[(CAPABILITIES, "sse2"), (WIDTH, "128")]),
        setting("no_optimization_passes", "the host's own capabilities, the generated code without optimization passes: states no capability", &[(PERFORMANCE, "nopt")]),
        setting("capabilities_to_sse2_no_optimization_passes", "capabilities held to SSE2, 128-bit vectors, without optimization passes", &[(CAPABILITIES, "sse2"), (WIDTH, "128"), (PERFORMANCE, "nopt")]),
    ]
}

/// What the device and its code generator read of the processor, from
/// `/proc/cpuinfo`: the vendor, family, model and stepping the code generator
/// names the processor from, and the capabilities the device hands it.
fn processor_facts() -> Vec<(&'static str, String)> {
    let text = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let field = |name: &str| {
        text.lines().find(|line| line.split_once(':').is_some_and(|(key, _)| key.trim() == name)).and_then(|line| line.split_once(':')).map(|(_, value)| crate::one_line(value)).unwrap_or_else(|| "not reported".to_string())
    };
    let flags = field("flags");
    let present: Vec<&str> = CAPABILITIES_THE_DEVICE_READS.iter().copied().filter(|capability| flags.split(' ').any(|flag| flag == *capability)).collect();
    vec![
        ("vendor", field("vendor_id")),
        ("family", field("cpu family")),
        ("model", field("model")),
        ("stepping", field("stepping")),
        ("name", field("model name")),
        ("capabilities_the_device_reads", present.join(" ")),
    ]
}

/// The child's side: `--held <device index> <picture path>`. One capture on
/// a device created under the environment the parent gave; it prints the key
/// and the hash of the bytes and writes the picture.
pub fn child(arguments: &[String]) -> i32 {
    let (Some(index), Some(picture)) = (arguments.first().and_then(|argument| argument.parse::<usize>().ok()), arguments.get(1)) else {
        eprintln!("usage: --held <device index> <picture path>");
        return 2;
    };
    let work = std::env::temp_dir().join(format!("{}-held-{}", crate::SPIKE, std::process::id()));
    let _ = fs::create_dir_all(&work);
    let status = match held_capture(&work, index, Path::new(picture)) {
        Ok(()) => 0,
        Err(reason) => {
            println!("error={}", crate::one_line(&reason));
            1
        }
    };
    let _ = fs::remove_dir_all(&work);
    status
}

fn held_capture(work: &Path, index: usize, picture: &Path) -> Result<(), String> {
    let api = crate::open_api(work)?;
    let info = api.devices().into_iter().find(|info| info.index == index).ok_or("no device at that index")?;
    let frame = Frame::build(Change::None);
    let mut device = api.open(index, false)?;
    let render = device.render_scene(&frame.device_scene(AdapterFault::None), Recording::SerialOneBuffer)?;
    println!("key={}", crate::one_line(&crate::key_of(api.as_ref(), &info, &device.settings())));
    println!("hash={:016x}", render.image.hash());
    fs::write(picture, pixels::write(&render.image)).map_err(|error| format!("the picture could not be written: {error}"))
}

/// `--difference <pixel file> <pixel file>`: the numbers of the difference
/// between two pictures the spike wrote, for pictures taken from two runs.
pub fn difference(arguments: &[String]) -> i32 {
    let read = |path: &String| fs::read(path).map_err(|error| error.to_string()).and_then(|file| pixels::read_strict(&file));
    let (Some(a), Some(b)) = (arguments.first(), arguments.get(1)) else {
        eprintln!("usage: --difference <pixel file> <pixel file>");
        return 2;
    };
    match (read(a), read(b)) {
        (Ok(a), Ok(b)) => match pixels::numbers(&a, &b, &crate::BOUNDS) {
            Some(numbers) => {
                println!("differing_pixels={} of {}", numbers.differing_pixels, u64::from(a.width) * u64::from(a.height));
                println!("largest_per_channel={:?}", numbers.largest_per_channel);
                println!("beyond_the_second_number={}", numbers.beyond_second);
                println!("rectangle={:?}", numbers.rectangle);
                0
            }
            None => {
                eprintln!("the two pictures are not of one size");
                1
            }
        },
        (Err(reason), _) | (_, Err(reason)) => {
            eprintln!("a picture could not be read: {reason}");
            1
        }
    }
}

/// One held capture per setting, each against the run's own capture, with
/// the pictures kept under `held/` in the run's output.
pub fn write(out: &mut Json, info: &DeviceInfo, base: &Image, output: &Path, work: &Path) {
    out.key("processor_capability");
    out.object_start();
    out.bool("applies", applies(info));
    if !applies(info) {
        out.string("why", "the capability levels are those of Mesa's software device on x86_64");
        out.object_end();
        return;
    }
    out.key("processor");
    out.object_start();
    for (name, value) in processor_facts() {
        out.string(name, &value);
    }
    out.object_end();
    let pictures = output.join("held");
    let _ = fs::create_dir_all(&pictures);
    let mut hashes = Vec::new();
    out.key("settings");
    out.array_start();
    for setting in settings() {
        out.object_start();
        out.string("setting", &setting.label);
        out.string("what", &setting.what);
        out.string("environment", &setting.environment.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join(" "));
        let picture = pictures.join(format!("{}.png", setting.label));
        let arguments = ["--held".to_string(), info.index.to_string(), picture.to_string_lossy().into_owned()];
        match crate::run_process(&arguments, &setting.environment, work) {
            Ok(lines) => {
                let value = |name: &str| lines.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone()).unwrap_or_default();
                let hash = value("hash");
                out.string("key", &value("key"));
                out.string("frame_bytes_hash", &hash);
                out.bool("same_bytes_as_the_run", hash == format!("{:016x}", base.hash()));
                match fs::read(&picture).map_err(|error| error.to_string()).and_then(|file| pixels::read_strict(&file)) {
                    Ok(held) => match pixels::numbers(base, &held, &crate::BOUNDS) {
                        Some(numbers) => crate::write_numbers(out, &numbers),
                        None => out.string("picture_error", "not the size of the run's capture"),
                    },
                    Err(reason) => out.string("picture_error", &reason),
                }
                hashes.push(hash);
            }
            Err(reason) => out.string("error", &reason),
        }
        out.object_end();
    }
    out.array_end();
    hashes.sort();
    hashes.dedup();
    out.u64("distinct_pictures", hashes.len() as u64);
    out.object_end();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_is_named_once_and_compiles_afresh() {
        let settings = settings();
        let mut labels: Vec<&str> = settings.iter().map(|setting| setting.label.as_str()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), settings.len());
        for setting in &settings {
            assert!(setting.environment.iter().any(|(name, value)| (name.as_str(), value.as_str()) == WITHOUT_THE_CACHE), "{}", setting.label);
            assert!(setting.label.chars().all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'), "{}", setting.label);
        }
    }

    #[test]
    fn the_capability_levels_are_those_mesa_reads() {
        let levels = ["nosse", "sse", "sse2", "sse3", "ssse3", "sse4.1", "avx"];
        for setting in settings() {
            for (name, value) in &setting.environment {
                if name == CAPABILITIES {
                    assert!(levels.contains(&value.as_str()), "{value}");
                }
            }
        }
    }
}
