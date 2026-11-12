//! Backend comparison spike, for the rendering research. Throwaway, never
//! product code.
//!
//! The device baseline's scene is rendered through three paths on every
//! device of the machine: natively, by the device baseline spike's own code
//! over Vulkan, Direct3D 12 and Metal; through wgpu at its lower level
//! (`wgpu-hal`); and through wgpu at its safe level (`wgpu`). The spike
//! measures what a layer adds beneath a render device port: what the binary
//! imports, which adapter wgpu calls its fallback, whether command buffers
//! recorded on four workers and submitted by a fifth thread give the bytes
//! of a serial recording, whether shaders compiled ahead enter with no
//! compiler opened by the process, the threads each path holds, the time to
//! record and submit 10 000 draws, whether the bytes are those of the native
//! path, whether a compiled pipeline can be saved and loaded, and whether
//! the interface's validation layer still reaches the program.
//!
//! Each path runs in its own child process per device, so that the threads
//! and the libraries counted are those of one path. Run with a cell label,
//! the spike prints one JSON document.

mod ahead;
mod lower;
mod safe;
mod workload;

use std::env;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use reference_workload::{Json, write_platform_and_build};
use render_device_baseline::backend::{Api, DeviceInfo, Recording};
#[cfg(windows)]
use render_device_baseline::d3d12;
#[cfg(target_os = "macos")]
use render_device_baseline::metal;
use render_device_baseline::scene::{HEIGHT, Image, Scene, WIDTH};
use render_device_baseline::{imports, platform, vulkan};
use wgpu_hal as hal;

use safe::{Recorded, SceneOnDevice, Shaders};
use workload::{MANY_DRAWS, MANY_DRAWS_REPETITIONS, block_on};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "render-backend-comparison";
const WGPU_VERSION: &str = "30.0.1";
/// A child process that has not ended after this long is stopped.
const CHILD_TIMEOUT: Duration = Duration::from_secs(300);
/// What the child writes to its standard error before a deliberate misuse.
const MISUSE_MARK: &str = "--- the deliberate misuse follows";

/// The flags of the instance the measurements run on: those wgpu gives a
/// build of this profile, with no validation layer and no debugging.
fn measured_flags() -> wgpu::InstanceFlags {
    wgpu::InstanceFlags::from_build_config()
}

fn validation_flags() -> wgpu::InstanceFlags {
    measured_flags() | wgpu::InstanceFlags::VALIDATION | wgpu::InstanceFlags::DEBUG
}

fn baseline_shader_dir() -> PathBuf {
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

/// Every native interface of this platform, loaded or with the reason it is
/// unavailable.
fn open_apis(work: &Path) -> Vec<(&'static str, Result<Box<dyn Api>, String>)> {
    let shaders = baseline_shader_dir();
    #[allow(unused_mut, reason = "the platform interfaces below are pushed on their platform only")]
    let mut apis = vec![("vulkan", vulkan::open(&shaders, work))];
    #[cfg(windows)]
    apis.push(("direct3d12", d3d12::open(&shaders, work)));
    #[cfg(target_os = "macos")]
    apis.push(("metal", metal::open(&shaders, work)));
    apis
}

/// A name for files: lowercase letters and digits, everything else a dash.
fn file_label(text: &str) -> String {
    let mut label = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            label.push(character.to_ascii_lowercase());
        } else if !label.ends_with('-') {
            label.push('-');
        }
    }
    label.trim_matches('-').chars().take(60).collect()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first lines of a text, for a report.
fn excerpt(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).take(8).collect();
    lines.join(" / ").chars().take(1_200).collect()
}

fn parse_lines(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|line| line.split_once('=')).map(|(key, value)| (key.trim().to_string(), value.trim().to_string())).collect()
}

// ---------------------------------------------------------------------------
// The child's side: every task prints `key=value` lines.

fn say(key: &str, value: impl Display) {
    println!("{key}={}", one_line(&value.to_string()));
}

/// Set by the tasks that only count threads: they render the scene once and
/// skip the timed workload, so that the counts are those of one device and
/// one scene on every path.
static COUNTING_THREADS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn counting_threads() -> bool {
    COUNTING_THREADS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The threads of the process at a step, in the tasks that count them.
fn say_threads(step: &str) {
    if !counting_threads() {
        return;
    }
    match platform::thread_count() {
        Some(threads) => say(&format!("threads_{step}"), threads),
        None => say(&format!("threads_{step}"), "not readable on this platform"),
    }
}

/// The shader compiler libraries the process holds at a step, by file name.
fn say_compilers(step: &str) {
    let key = format!("shader_compilers_{step}");
    match workload::loaded_shader_compilers() {
        Ok(found) if found.is_empty() => say(&key, "none"),
        Ok(found) => say(&key, found.iter().map(|path| path.rsplit(['/', '\\']).next().unwrap_or(path)).collect::<Vec<_>>().join(", ")),
        Err(reason) => say(&key, reason),
    }
}

fn say_times(prefix: &str, samples: &[f64]) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    say(&format!("{prefix}_fastest"), format!("{:.1}", sorted.first().copied().unwrap_or(0.0)));
    say(&format!("{prefix}_median"), format!("{:.1}", sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)));
}

/// The warnings and errors the `log` crate received since the last call.
fn say_log(prefix: &str) {
    let records: Vec<workload::LogRecord> = workload::drain_log().into_iter().filter(|record| record.level <= log::Level::Warn).collect();
    say(&format!("{prefix}_messages"), records.len());
    if let Some(first) = records.first() {
        say(&format!("{prefix}_first"), format!("[{}] {}: {}", first.level, first.target, first.message.chars().take(600).collect::<String>()));
    }
}

fn describe(info: &wgpu::AdapterInfo) -> String {
    format!("{}|{}|0x{:04x}|0x{:04x}|{}|{}", one_line(&info.name), safe::device_type_name(info.device_type), info.vendor, info.device, one_line(&info.driver), one_line(&info.driver_info))
}

/// Runs one function of this file over wgpu's lower level for the interface
/// named, on the platforms where wgpu builds that interface.
macro_rules! with_hal {
    ($interface:expr, $function:ident($($argument:expr),*)) => {
        match $interface {
            #[cfg(not(target_vendor = "apple"))]
            "vulkan" => $function::<hal::api::Vulkan>($($argument),*),
            #[cfg(windows)]
            "direct3d12" => $function::<hal::api::Dx12>($($argument),*),
            #[cfg(target_vendor = "apple")]
            "metal" => $function::<hal::api::Metal>($($argument),*),
            other => Err(format!("wgpu's lower level has no interface named {other} on this platform")),
        }
    };
}

fn list_lower<A: hal::Api>(interface: &str) -> Result<(), String> {
    let (_instance, adapters) = lower::instance::<A>(measured_flags())?;
    say(&format!("lower_{interface}_count"), adapters.len());
    for (index, exposed) in adapters.iter().enumerate() {
        say(&format!("lower_{interface}_{index}"), describe(&exposed.info));
    }
    Ok(())
}

/// The adapters wgpu lists at each level and the adapter it calls its
/// fallback, per interface.
fn child_list() -> Result<(), String> {
    workload::collect_log(log::LevelFilter::Warn);
    say_threads("start");
    // Whether the system has a Vulkan loader at all, asked of the system's own
    // loader and closed at once: a machine without one is a case the spike
    // reports, and wgpu is then expected to list no adapter.
    let loader_names: &[&str] = if cfg!(windows) { &["vulkan-1.dll"] } else if cfg!(target_os = "macos") { &["libvulkan.1.dylib", "libvulkan.dylib"] } else { &["libvulkan.so.1", "libvulkan.so"] };
    match render_device_baseline::loader::Library::open(loader_names) {
        Ok(_) => say("vulkan_loader", "present"),
        Err(reason) => say("vulkan_loader", format!("absent: {reason}")),
    }
    for (interface, backend) in safe::BACKENDS {
        let instance = safe::instance(measured_flags(), wgpu::Backends::from(backend));
        let adapters = safe::adapters(&instance, backend);
        say(&format!("safe_{interface}_count"), adapters.len());
        for (index, adapter) in adapters.iter().enumerate() {
            say(&format!("safe_{interface}_{index}"), describe(&adapter.get_info()));
        }
        let options = wgpu::RequestAdapterOptions { force_fallback_adapter: true, ..Default::default() };
        match block_on(instance.request_adapter(&options)) {
            Ok(adapter) => say(&format!("fallback_{interface}"), describe(&adapter.get_info())),
            Err(error) => say(&format!("fallback_{interface}_error"), error),
        }
        if let Err(reason) = with_hal!(interface, list_lower(interface)) {
            say(&format!("lower_{interface}_error"), reason);
        }
    }
    say_threads("end");
    say_log("log");
    Ok(())
}

fn save_rgba(work: &Path, label: &str, name: &str, image: &Image) {
    let _ = fs::write(work.join(format!("{label}-{name}.rgba")), &image.rgba);
}

/// The scene in both recordings, then the many draws workload timed, on one
/// path of wgpu. `render` renders a scene by its recording; its second
/// argument says whether the scene is the many draws workload.
fn say_scene_and_many_draws(work: &Path, label: &str, mut render: impl FnMut(&Scene, bool, Recorded) -> Result<safe::Rendered, String>) -> Result<(), String> {
    let scene = Scene::build();
    let mut hashes = Vec::new();
    for recorded in Recorded::ALL {
        let rendered = render(&scene, false, recorded)?;
        say(&format!("scene_{}_hash", recorded.name()), format!("{:016x}", rendered.image.hash()));
        save_rgba(work, label, recorded.name(), &rendered.image);
        hashes.push(rendered.image.hash());
    }
    say("recordings_identical", hashes.windows(2).all(|pair| pair[0] == pair[1]));
    say_threads("rendered");
    if counting_threads() {
        return Ok(());
    }

    let many = workload::many_draws_scene();
    for recorded in Recorded::ALL {
        // One render that is not timed: the first use of a pipeline may
        // compile it.
        let warm = render(&many, true, recorded)?;
        let (mut record, mut submit, mut wait) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..MANY_DRAWS_REPETITIONS {
            let rendered = render(&many, true, recorded)?;
            record.push(rendered.recorded_us);
            submit.push(rendered.submitted_us - rendered.recorded_us);
            wait.push(rendered.completed_us - rendered.submitted_us);
        }
        say_times(&format!("many_{}_record_us", recorded.name()), &record);
        say_times(&format!("many_{}_submit_us", recorded.name()), &submit);
        say_times(&format!("many_{}_wait_us", recorded.name()), &wait);
        say(&format!("many_{}_hash", recorded.name()), format!("{:016x}", warm.image.hash()));
        if recorded == Recorded::Serial {
            save_rgba(work, label, "many", &warm.image);
        }
    }
    Ok(())
}

/// The safe level on one adapter with one form of shaders.
fn child_safe(interface: &str, index: usize, form: &str, work: &Path, label: &str) -> Result<(), String> {
    workload::collect_log(log::LevelFilter::Warn);
    say_threads("start");
    let backend = safe::backend_named(interface).ok_or_else(|| format!("no interface named {interface}"))?;
    let instance = safe::instance(measured_flags(), wgpu::Backends::from(backend));
    say_threads("instance");
    let adapters = safe::adapters(&instance, backend);
    say_threads("adapters");
    say_compilers("adapters");
    let adapter = adapters.get(index).ok_or_else(|| format!("wgpu lists no adapter {index} for {interface}"))?;
    say("adapter", describe(&adapter.get_info()));

    let loaded = if form == "wgsl" { None } else { Some(ahead::load(work, form)?) };
    let bindings = ahead::bindings_of(form, adapter.features())?;
    say("bindings", bindings.name());
    let wgsl = ahead::wgsl(bindings)?;
    let shaders = loaded.as_ref().map_or(Shaders::Wgsl(&wgsl), ahead::Loaded::shaders);
    let cache_offered = adapter.features().contains(wgpu::Features::PIPELINE_CACHE);
    let mut extra = if shaders.is_compiled_ahead() { wgpu::Features::PASSTHROUGH_SHADERS } else { wgpu::Features::empty() };
    if cache_offered {
        extra |= wgpu::Features::PIPELINE_CACHE;
    }
    say("pipeline_cache_offered", cache_offered);
    let (device, queue, errors) = safe::open(adapter, bindings, extra)?;
    say_threads("device");
    say_compilers("device");

    let mut current: Option<(bool, SceneOnDevice)> = None;
    let mut first_creation = true;
    say_scene_and_many_draws(work, label, |scene, many, recorded| {
        if current.as_ref().is_none_or(|(held, _)| *held != many) {
            let started = Instant::now();
            current = Some((many, SceneOnDevice::create(&device, &queue, &errors, scene, bindings, &shaders, None)?));
            if first_creation {
                say("create_us", format!("{:.1}", started.elapsed().as_secs_f64() * 1_000_000.0));
                say_compilers("pipelines");
                first_creation = false;
            }
        }
        let (_, on_device) = current.as_ref().ok_or("no scene on the device")?;
        on_device.render(&device, &queue, &errors, scene, recorded)
    })?;
    drop(current);

    if cache_offered && form == "wgsl" && !counting_threads() {
        match safe::pipeline_cache_round_trip(&device, &queue, &errors, &Scene::build(), bindings, &shaders) {
            Ok(round_trip) => {
                say("pipeline_cache_saved_bytes", round_trip.saved_bytes);
                say("pipeline_cache_cold_us", format!("{:.1}", round_trip.cold_us));
                say("pipeline_cache_loaded_us", format!("{:.1}", round_trip.loaded_us));
                say("pipeline_cache_bytes_after_load", round_trip.bytes_after_load);
            }
            Err(reason) => say("pipeline_cache_error", reason),
        }
    }
    drop((device, queue));
    say_threads("device_dropped");
    drop(adapters);
    drop(instance);
    say_threads("instance_dropped");
    say_log("log");
    Ok(())
}

/// The lower level on one adapter with one form of shaders compiled ahead.
fn child_lower<A: hal::Api>(index: usize, form: &str, work: &Path, label: &str) -> Result<(), String> {
    workload::collect_log(log::LevelFilter::Warn);
    say_threads("start");
    let (instance, mut adapters) = lower::instance::<A>(measured_flags())?;
    say_threads("adapters");
    say_compilers("adapters");
    if index >= adapters.len() {
        return Err(format!("wgpu's lower level lists no adapter {index}"));
    }
    let exposed = adapters.swap_remove(index);
    drop(adapters);
    say("adapter", describe(&exposed.info));
    say("pipeline_cache_offered", exposed.features.contains(wgpu::Features::PIPELINE_CACHE));
    let loaded = ahead::load(work, form)?;
    let shaders = loaded.shaders();
    let bindings = ahead::bindings_of(form, exposed.features)?;
    say("bindings", bindings.name());
    let hal::OpenDevice { device, queue } = lower::open(&exposed, bindings)?;
    say_threads("device");
    say_compilers("device");

    let mut current: Option<(bool, lower::LowerScene<A>)> = None;
    let mut first_creation = true;
    let outcome = say_scene_and_many_draws(work, label, |scene, many, recorded| {
        if current.as_ref().is_none_or(|(held, _)| *held != many) {
            if let Some((_, previous)) = current.take() {
                // SAFETY: every submission that named the scene has completed.
                unsafe { previous.destroy(&device) };
            }
            let started = Instant::now();
            // SAFETY: the device and the queue are used by this thread alone.
            current = Some((many, unsafe { lower::LowerScene::<A>::create(&device, &queue, scene, bindings, &shaders) }?));
            if first_creation {
                say("create_us", format!("{:.1}", started.elapsed().as_secs_f64() * 1_000_000.0));
                say_compilers("pipelines");
                first_creation = false;
            }
        }
        let (_, on_device) = current.as_mut().ok_or("no scene on the device")?;
        // SAFETY: the scene was created on this device and queue.
        unsafe { on_device.render(&device, &queue, scene, recorded) }
    });
    if let Some((_, scene)) = current.take() {
        // SAFETY: every submission that named the scene has completed.
        unsafe { scene.destroy(&device) };
    }
    outcome?;
    drop(queue);
    drop(device);
    say_threads("device_dropped");
    drop(exposed);
    drop(instance);
    say_threads("instance_dropped");
    say_log("log");
    Ok(())
}

/// The native path on one device: the device baseline spike's own code.
fn child_native(interface: &str, index: usize, work: &Path, label: &str) -> Result<(), String> {
    say_threads("start");
    let (_, api) = open_apis(work).into_iter().find(|(name, _)| *name == interface).ok_or_else(|| format!("no native interface named {interface} on this platform"))?;
    let api = api?;
    say_threads("loader");
    let scene = Scene::build();
    {
        let mut device = api.open(index, false)?;
        say_threads("device");
        say_compilers("device");
        let mut hashes = Vec::new();
        for recording in Recording::ALL {
            let render = device.render_scene(&scene, recording)?;
            say(&format!("scene_{}_hash", recording.name()), format!("{:016x}", render.image.hash()));
            save_rgba(work, label, recording.name(), &render.image);
            hashes.push(render.image.hash());
        }
        say("recordings_identical", hashes.windows(2).all(|pair| pair[0] == pair[1]));
        say_compilers("pipelines");
        say_threads("rendered");
    }
    say_threads("device_dropped");

    // A second device: Direct3D 12's keeps the resources of its first scene.
    let many = workload::many_draws_scene();
    if !counting_threads() {
        let mut device = api.open(index, false)?;
        for (recording, name) in [(Recording::SerialOneBuffer, Recorded::Serial.name()), (Recording::ChunkedFourThreads, Recorded::FourWorkersOneSubmitter.name())] {
            let warm = device.render_scene(&many, recording)?;
            let (mut record, mut submit, mut wait) = (Vec::new(), Vec::new(), Vec::new());
            for _ in 0..MANY_DRAWS_REPETITIONS {
                let render = device.render_scene(&many, recording)?;
                record.push(render.recorded_us);
                submit.push(render.submitted_us - render.recorded_us);
                wait.push(render.render_us - render.submitted_us);
            }
            say_times(&format!("many_{name}_record_us"), &record);
            say_times(&format!("many_{name}_submit_us"), &submit);
            say_times(&format!("many_{name}_wait_us"), &wait);
            say(&format!("many_{name}_hash"), format!("{:016x}", warm.image.hash()));
            if recording == Recording::SerialOneBuffer {
                save_rgba(work, label, "many", &warm.image);
            }
        }
    }
    drop(api);
    say_threads("loader_dropped");
    Ok(())
}

/// What wgpu's translator wrote for the scene's WGSL on one adapter, as its
/// log gives it, saved next to the measurements.
fn child_generated(interface: &str, index: usize, output: &Path) -> Result<(), String> {
    workload::collect_log(log::LevelFilter::Debug);
    let backend = safe::backend_named(interface).ok_or_else(|| format!("no interface named {interface}"))?;
    let instance = safe::instance(measured_flags(), wgpu::Backends::from(backend));
    let adapters = safe::adapters(&instance, backend);
    let adapter = adapters.get(index).ok_or_else(|| format!("wgpu lists no adapter {index} for {interface}"))?;
    let bindings = safe::Bindings::offered_by(adapter.features());
    let (device, queue, errors) = safe::open(adapter, bindings, wgpu::Features::empty())?;
    workload::drain_log();
    let wgsl = ahead::wgsl(bindings)?;
    SceneOnDevice::create(&device, &queue, &errors, &Scene::build(), bindings, &Shaders::Wgsl(&wgsl), None)?;
    let generated: Vec<String> = workload::drain_log().into_iter().filter(|record| record.message.contains("Naga generated shader")).map(|record| record.message).collect();
    say("generated_shaders", generated.len());
    if !generated.is_empty() {
        let path = output.join(format!("{SPIKE}-generated-{interface}.txt"));
        fs::write(&path, generated.join("\n\n")).map_err(|error| format!("{}: {error}", path.display()))?;
        say("generated_file", path.display());
    }
    Ok(())
}

/// A view in a format its texture was not created for, made through the
/// interface beneath a device of the safe level, which wgpu's own
/// validation never sees.
fn safe_misuse<A: hal::Api>(device: &wgpu::Device, texture: &wgpu::Texture) -> Result<String, String> {
    use hal::Device as _;
    let what = "created, beneath wgpu, a view in the format R32Float of a texture created as Rgba8UnormSrgb with no other view format, then destroyed it";
    let descriptor = hal::TextureViewDescriptor {
        label: Some("misuse"),
        format: wgpu::TextureFormat::R32Float,
        dimension: wgpu::TextureViewDimension::D2,
        usage: wgpu::TextureUses::RESOURCE,
        range: wgpu::ImageSubresourceRange::default(),
    };
    // SAFETY: the two handles are those of live objects of the safe level,
    // used here by this thread alone; the call is the misuse under test and
    // the view, when one is returned, is destroyed at once.
    unsafe {
        let raw_device = device.as_hal::<A>().ok_or("the device gave no handle of this interface")?;
        let raw_texture = texture.as_hal::<A>().ok_or("the texture gave no handle of this interface")?;
        match raw_device.create_texture_view(&raw_texture, &descriptor) {
            Ok(view) => {
                raw_device.destroy_texture_view(view);
                Ok(what.to_string())
            }
            Err(error) => Ok(format!("{what}; the creation returned an error: {error}")),
        }
    }
}

/// The interface's validation layer beneath the safe level.
fn child_safe_validation(interface: &str, index: usize) -> Result<(), String> {
    workload::collect_log(log::LevelFilter::Warn);
    let backend = safe::backend_named(interface).ok_or_else(|| format!("no interface named {interface}"))?;
    let instance = safe::instance(validation_flags(), wgpu::Backends::from(backend));
    let adapters = safe::adapters(&instance, backend);
    let adapter = adapters.get(index).ok_or_else(|| format!("wgpu lists no adapter {index} for {interface}"))?;
    let bindings = safe::Bindings::offered_by(adapter.features());
    let (device, queue, errors) = safe::open(adapter, bindings, wgpu::Features::empty())?;
    let scene = Scene::build();
    let wgsl = ahead::wgsl(bindings)?;
    let rendered = SceneOnDevice::create(&device, &queue, &errors, &scene, bindings, &Shaders::Wgsl(&wgsl), None)
        .and_then(|on_device| Recorded::ALL.iter().try_for_each(|&recorded| on_device.render(&device, &queue, &errors, &scene, recorded).map(|_| ())));
    say("scene_rendered", rendered.is_ok());
    if let Err(reason) = rendered {
        say("scene_error", reason);
    }
    say_log("clean");
    eprintln!("{MISUSE_MARK}");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("misused"),
        size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    say("misuse", with_hal!(interface, safe_misuse(&device, &texture))?);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    say_log("misuse");
    say("wgpu_errors_after_misuse", errors.take().len());
    Ok(())
}

/// The interface's validation layer beneath the lower level.
fn child_lower_validation<A: hal::Api>(index: usize, form: &str, work: &Path) -> Result<(), String> {
    workload::collect_log(log::LevelFilter::Warn);
    let (instance, mut adapters) = lower::instance::<A>(validation_flags())?;
    if index >= adapters.len() {
        return Err(format!("wgpu's lower level lists no adapter {index}"));
    }
    let exposed = adapters.swap_remove(index);
    drop(adapters);
    let loaded = ahead::load(work, form)?;
    let bindings = ahead::bindings_of(form, exposed.features)?;
    let hal::OpenDevice { device, queue } = lower::open(&exposed, bindings)?;
    let scene = Scene::build();
    // SAFETY: the device and the queue are used by this thread alone, and
    // the scene is destroyed once its submissions have completed.
    unsafe {
        let mut on_device = lower::LowerScene::<A>::create(&device, &queue, &scene, bindings, &loaded.shaders())?;
        let rendered = Recorded::ALL.iter().try_for_each(|&recorded| on_device.render(&device, &queue, &scene, recorded).map(|_| ()));
        say("scene_rendered", rendered.is_ok());
        if let Err(reason) = rendered {
            say("scene_error", reason);
        }
        say_log("clean");
        eprintln!("{MISUSE_MARK}");
        say("misuse", on_device.misuse(&device));
        say_log("misuse");
        on_device.destroy(&device);
    }
    drop(queue);
    drop(device);
    drop(exposed);
    drop(instance);
    Ok(())
}

/// The child's side: `--child <task> <interface> <adapter index> <form>
/// <work directory> <output directory> <label>`.
fn child(arguments: &[String]) -> i32 {
    let [task, interface, index, form, work, output, label] = arguments else {
        eprintln!("usage: --child <task> <interface> <adapter index> <form> <work directory> <output directory> <label>");
        return 2;
    };
    let Ok(index) = index.parse::<usize>() else {
        eprintln!("the adapter index is a number");
        return 2;
    };
    let (work, output, interface) = (Path::new(work), Path::new(output), interface.as_str());
    let task = match task.strip_suffix("-threads") {
        Some(counted) => {
            COUNTING_THREADS.store(true, std::sync::atomic::Ordering::Relaxed);
            counted
        }
        None => task.as_str(),
    };
    let outcome = match task {
        "list" => child_list(),
        "native" => child_native(interface, index, work, label),
        "safe" => child_safe(interface, index, form, work, label),
        "lower" => with_hal!(interface, child_lower(index, form, work, label)),
        "generated" => child_generated(interface, index, output),
        "safe-validation" => child_safe_validation(interface, index),
        "lower-validation" => with_hal!(interface, child_lower_validation(index, form, work)),
        _ => {
            eprintln!("unknown task {task}");
            return 2;
        }
    };
    match outcome {
        Ok(()) => 0,
        Err(reason) => {
            say("error", reason);
            1
        }
    }
}

// ---------------------------------------------------------------------------
// The parent's side.

/// What a child process of the spike left.
struct ChildRun {
    completed: bool,
    status: String,
    lines: Vec<(String, String)>,
    standard_error: String,
}

impl ChildRun {
    fn value(&self, key: &str) -> Option<&str> {
        self.lines.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }

    fn hash(&self, key: &str) -> Option<u64> {
        self.value(key).and_then(|text| u64::from_str_radix(text, 16).ok())
    }
}

struct Task<'a> {
    task: &'a str,
    interface: &'a str,
    index: usize,
    form: &'a str,
    label: &'a str,
    environment: &'a [(String, String)],
}

/// Runs this executable again as a child for one task.
fn run_child(task: &Task, work: &Path, output: &Path) -> ChildRun {
    let failed = |status: String| ChildRun { completed: false, status, lines: Vec::new(), standard_error: String::new() };
    let name = format!("{}-{}", task.label, task.task);
    let (output_path, error_path) = (work.join(format!("{name}.out")), work.join(format!("{name}.err")));
    let files = fs::File::create(&output_path).and_then(|out| fs::File::create(&error_path).map(|error| (out, error)));
    let (standard_output, standard_error) = match files {
        Ok(files) => files,
        Err(reason) => return failed(format!("the child's output files could not be created: {reason}")),
    };
    let executable = match env::current_exe() {
        Ok(executable) => executable,
        Err(reason) => return failed(format!("the executable's path is unknown: {reason}")),
    };
    let mut command = Command::new(executable);
    command.arg("--child").arg(task.task).arg(task.interface).arg(task.index.to_string()).arg(task.form).arg(work).arg(output).arg(task.label);
    command.envs(task.environment.iter().map(|(key, value)| (key.as_str(), value.as_str())));
    command.stdin(Stdio::null()).stdout(standard_output).stderr(standard_error);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(reason) => return failed(format!("the child could not be started: {reason}")),
    };
    let started = Instant::now();
    let (completed, status) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (true, status.to_string()),
            Ok(None) if started.elapsed() > CHILD_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break (false, format!("stopped after {} seconds without ending", CHILD_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(reason) => break (false, format!("the child could not be waited for: {reason}")),
        }
    };
    ChildRun {
        completed,
        status,
        lines: parse_lines(&fs::read_to_string(&output_path).unwrap_or_default()),
        standard_error: String::from_utf8_lossy(&fs::read(&error_path).unwrap_or_default()).into_owned(),
    }
}

/// A value of a child's line as JSON: a boolean, a whole number, a decimal
/// or a text. Hashes and descriptions stay texts.
fn write_value(out: &mut Json, key: &str, value: &str) {
    let textual = key.ends_with("hash") || key == "adapter" || key.ends_with("_first") || key == "misuse";
    if textual {
        out.string(key, value);
    } else if value == "true" || value == "false" {
        out.bool(key, value == "true");
    } else if let Ok(number) = value.parse::<u64>() {
        out.u64(key, number);
    } else if let Some(number) = value.parse::<f64>().ok().filter(|number| number.is_finite()) {
        out.decimal(key, number);
    } else {
        out.string(key, value);
    }
}

/// One run of a path on a device, as an object left open: what was asked,
/// how the child ended, every line it printed.
fn write_run(out: &mut Json, path: &str, shaders: &str, run: &ChildRun) {
    out.object_start();
    out.string("path", path);
    out.string("shaders", shaders);
    out.bool("completed", run.completed);
    out.string("status", &run.status);
    for (key, value) in &run.lines {
        write_value(out, key, value);
    }
    if !run.standard_error.trim().is_empty() {
        out.string("standard_error", &excerpt(&run.standard_error));
    }
}

/// A wgpu adapter as a child described it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Listed {
    index: usize,
    name: String,
    kind: String,
    vendor_id: u32,
    device_id: u32,
    driver: String,
}

fn parse_listed(index: usize, text: &str) -> Option<Listed> {
    let parts: Vec<&str> = text.split('|').collect();
    let identifier = |part: &str| u32::from_str_radix(part.trim_start_matches("0x"), 16).ok();
    Some(Listed {
        index,
        name: parts.first()?.to_string(),
        kind: parts.get(1)?.to_string(),
        vendor_id: identifier(parts.get(2)?)?,
        device_id: identifier(parts.get(3)?)?,
        driver: [parts.get(4).copied().unwrap_or(""), parts.get(5).copied().unwrap_or("")].join(" ").trim().to_string(),
    })
}

fn listed(list: &ChildRun, level: &str, interface: &str) -> Vec<Listed> {
    let count = list.value(&format!("{level}_{interface}_count")).and_then(|text| text.parse::<usize>().ok()).unwrap_or(0);
    (0..count).filter_map(|index| list.value(&format!("{level}_{interface}_{index}")).and_then(|text| parse_listed(index, text))).collect()
}

/// One device of an interface, with its position in each path's listing.
struct Device {
    name: String,
    kind: String,
    vendor_id: u32,
    device_id: u32,
    native: Option<DeviceInfo>,
    safe: Option<Listed>,
    lower: Option<Listed>,
}

/// The devices of one interface: those wgpu's safe level lists, each with
/// the first native device and the first adapter of the lower level that
/// carry its identifiers, then the native devices wgpu does not list. A
/// device an interface lists twice under one pair of identifiers is one.
fn devices_of(native: &[DeviceInfo], safe: &[Listed], lower: &[Listed]) -> Vec<Device> {
    let mut devices: Vec<Device> = Vec::new();
    let known = |devices: &[Device], vendor: u32, device: u32| devices.iter().any(|known| known.vendor_id == vendor && known.device_id == device);
    for adapter in safe {
        if known(&devices, adapter.vendor_id, adapter.device_id) {
            continue;
        }
        devices.push(Device {
            name: adapter.name.clone(),
            kind: adapter.kind.clone(),
            vendor_id: adapter.vendor_id,
            device_id: adapter.device_id,
            native: native.iter().find(|info| info.vendor_id == adapter.vendor_id && info.device_id == adapter.device_id).cloned(),
            safe: Some(adapter.clone()),
            lower: lower.iter().find(|other| other.vendor_id == adapter.vendor_id && other.device_id == adapter.device_id).cloned(),
        });
    }
    for info in native {
        if known(&devices, info.vendor_id, info.device_id) {
            continue;
        }
        devices.push(Device {
            name: info.name.clone(),
            kind: info.kind.name().to_string(),
            vendor_id: info.vendor_id,
            device_id: info.device_id,
            native: Some(info.clone()),
            safe: None,
            lower: lower.iter().find(|other| other.vendor_id == info.vendor_id && other.device_id == info.device_id).cloned(),
        });
    }
    devices
}

fn read_image(work: &Path, label: &str, name: &str) -> Option<Image> {
    let rgba = fs::read(work.join(format!("{label}-{name}.rgba"))).ok()?;
    (rgba.len() == (WIDTH * HEIGHT * 4) as usize).then_some(Image { width: WIDTH, height: HEIGHT, rgba })
}

/// An image with its rows in the opposite order.
fn upside_down(image: &Image) -> Image {
    let row = image.width as usize * 4;
    Image { width: image.width, height: image.height, rgba: image.rgba.chunks(row.max(1)).rev().flatten().copied().collect() }
}

/// How a path's image compares with the native path's on the same device.
fn write_against_native(out: &mut Json, key: &str, work: &Path, native_label: Option<&str>, label: &str, name: &str) {
    let own = read_image(work, label, name);
    let reference = native_label.and_then(|native_label| read_image(work, native_label, if name == "many" { "many" } else { Recording::SerialOneBuffer.name() }));
    match (own, reference) {
        (Some(own), Some(reference)) => {
            out.bool(&format!("{key}_bytes_equal_the_native_path"), own == reference);
            if own != reference
                && let Some(difference) = own.difference(&reference)
            {
                out.u64(&format!("{key}_differing_pixels"), difference.differing_pixels);
                out.string(&format!("{key}_largest_difference_per_channel"), &format!("{:?}", difference.largest));
                out.bool(&format!("{key}_is_the_native_image_upside_down"), own == upside_down(&reference));
            }
        }
        (Some(_), None) => out.string(&format!("{key}_bytes_equal_the_native_path"), "no native image of this device to compare with"),
        (None, _) => out.string(&format!("{key}_bytes_equal_the_native_path"), "not rendered"),
    }
}

/// The scene's image of one path as a PNG file next to the measurements,
/// once per distinct image of a device.
fn publish_image(output: &Path, written: &mut Vec<u64>, work: &Path, device_label: &str, label: &str) {
    if let Some(image) = read_image(work, label, Recorded::Serial.name()).or_else(|| read_image(work, label, Recording::SerialOneBuffer.name()))
        && !written.contains(&image.hash())
    {
        written.push(image.hash());
        let _ = fs::write(output.join(format!("{SPIKE}-scene-{device_label}-{:016x}.png", image.hash())), image.png());
    }
}

fn write_validation(out: &mut Json, path: &str, run: &ChildRun) {
    write_run(out, path, "validation layer switched on", run);
    let after_mark = run.standard_error.split_once(MISUSE_MARK).map(|(_, after)| after.lines().filter(|line| !line.trim().is_empty()).count() as u64);
    out.u64("standard_error_lines_after_the_misuse", after_mark.unwrap_or(0));
    let in_the_log = run.value("misuse_messages").and_then(|count| count.parse::<u64>().ok()).unwrap_or(0);
    out.bool("the_misuse_reaches_the_log", in_the_log > 0);
    out.bool("the_misuse_reaches_the_program", in_the_log > 0 || after_mark.unwrap_or(0) > 0);
    out.object_end();
}

fn write_imports(out: &mut Json) {
    out.key("imports");
    out.object_start();
    match env::current_exe().map_err(|error| error.to_string()).and_then(|path| imports::Imports::of_file(&path)) {
        Ok(imports) => {
            out.string("format", imports.format);
            out.string("libraries", &imports.libraries.join(", "));
            let denied = imports.denied();
            out.bool("imports_a_windowing_graphics_or_audio_library", !denied.is_empty());
            out.string("denied", &denied.join(", "));
        }
        Err(error) => out.string("error", &error),
    }
    out.object_end();
}

fn measure_interface(out: &mut Json, interface: &str, api: Option<&Result<Box<dyn Api>, String>>, list: &ChildRun, made: &[ahead::Made], work: &Path, output: &Path) {
    out.object_start();
    out.string("interface", interface);
    let native_devices = match api {
        Some(Ok(api)) => {
            out.bool("native_path_available", true);
            out.string("native_loader", &api.loader());
            out.string("native_shader_compiler", &api.shader_compiler());
            api.devices()
        }
        Some(Err(reason)) => {
            out.bool("native_path_available", false);
            out.string("native_path_reason", reason);
            Vec::new()
        }
        None => {
            out.bool("native_path_available", false);
            out.string("native_path_reason", "the device baseline spike has no path for this interface on this platform");
            Vec::new()
        }
    };
    let (safe_adapters, lower_adapters) = (listed(list, "safe", interface), listed(list, "lower", interface));
    out.u64("adapters_at_the_safe_level", safe_adapters.len() as u64);
    out.u64("adapters_at_the_lower_level", lower_adapters.len() as u64);
    if let Some(reason) = list.value(&format!("lower_{interface}_error")) {
        out.string("lower_level_reason", reason);
    }
    out.key("fallback_adapter");
    out.object_start();
    match list.value(&format!("fallback_{interface}")).and_then(|text| parse_listed(0, text)) {
        Some(fallback) => {
            out.bool("given", true);
            out.string("name", &fallback.name);
            out.string("kind", &fallback.kind);
            out.string("driver", &fallback.driver);
            out.bool("is_a_software_device", fallback.kind == "software");
        }
        None => {
            out.bool("given", false);
            out.string("reason", list.value(&format!("fallback_{interface}_error")).unwrap_or("the listing child reported nothing"));
        }
    }
    out.object_end();

    let validation_environment: Vec<(String, String)> = match api {
        Some(Ok(api)) => api.validation_environment(),
        _ => Vec::new(),
    };
    let forms: Vec<&str> = ahead::forms_of(interface).iter().copied().filter(|form| made.iter().any(|made| made.form == *form && made.outcome.is_ok())).collect();
    let none: &[(String, String)] = &[];
    let mut generated_file = None;

    out.key("devices");
    out.array_start();
    for device in devices_of(&native_devices, &safe_adapters, &lower_adapters) {
        let device_label = file_label(&format!("{interface}-{}", device.name));
        out.object_start();
        out.string("name", &device.name);
        out.string("kind", &device.kind);
        out.string("vendor_id", &format!("0x{:04x}", device.vendor_id));
        out.string("device_id", &format!("0x{:04x}", device.device_id));
        if let Some(info) = &device.native {
            out.string("driver", &info.driver);
            out.string("interface_version", &info.interface_version);
        } else if let Some(adapter) = &device.safe {
            out.string("driver", &adapter.driver);
        }
        let mut written = Vec::new();
        out.key("runs");
        out.array_start();

        let native_label = format!("{device_label}-native");
        let mut native_reference = None;
        if let Some(info) = &device.native {
            let run = run_child(&Task { task: "native", interface, index: info.index, form: "none", label: &native_label, environment: none }, work, output);
            write_run(out, "native", "compiled at run time by the device baseline spike", &run);
            out.object_end();
            if run.hash(&format!("scene_{}_hash", Recording::SerialOneBuffer.name())).is_some() {
                native_reference = Some(native_label.as_str());
            }
            publish_image(output, &mut written, work, &device_label, &native_label);
        }

        let mut wgpu_run = |out: &mut Json, path: &str, task: &str, index: usize, form: &str| {
            let label = format!("{device_label}-{task}-{}", file_label(form));
            let run = run_child(&Task { task, interface, index, form, label: &label, environment: none }, work, output);
            write_run(out, path, form, &run);
            write_against_native(out, "scene", work, native_reference, &label, Recorded::Serial.name());
            write_against_native(out, "many_draws", work, native_reference, &label, "many");
            out.object_end();
            publish_image(output, &mut written, work, &device_label, &label);
        };
        if let Some(adapter) = &device.safe {
            wgpu_run(out, "wgpu_safe_level", "safe", adapter.index, "wgsl");
            for form in &forms {
                wgpu_run(out, "wgpu_safe_level", "safe", adapter.index, form);
            }
        }
        if let Some(adapter) = &device.lower {
            for form in &forms {
                wgpu_run(out, "wgpu_lower_level", "lower", adapter.index, form);
            }
        }
        out.array_end();

        // The threads of each path, counted in a process that opens one
        // device and renders the scene once.
        out.key("threads");
        out.array_start();
        let count_threads = |out: &mut Json, path: &str, task: &str, index: usize, form: &str| {
            let label = format!("{device_label}-{task}-counted");
            let run = run_child(&Task { task: &format!("{task}-threads"), interface, index, form, label: &label, environment: none }, work, output);
            write_run(out, path, form, &run);
            out.object_end();
        };
        if let Some(info) = &device.native {
            count_threads(out, "native", "native", info.index, "none");
        }
        if let Some(adapter) = &device.safe {
            count_threads(out, "wgpu_safe_level", "safe", adapter.index, "wgsl");
        }
        if let (Some(adapter), Some(form)) = (&device.lower, forms.first()) {
            count_threads(out, "wgpu_lower_level", "lower", adapter.index, form);
        }
        out.array_end();

        out.key("validation");
        out.array_start();
        if let Some(adapter) = &device.safe {
            let label = format!("{device_label}-safe");
            let run = run_child(&Task { task: "safe-validation", interface, index: adapter.index, form: "wgsl", label: &label, environment: &validation_environment }, work, output);
            write_validation(out, "wgpu_safe_level", &run);
            if generated_file.is_none() {
                let run = run_child(&Task { task: "generated", interface, index: adapter.index, form: "wgsl", label: &label, environment: none }, work, output);
                generated_file = Some(run.value("generated_file").unwrap_or("none logged").to_string());
            }
        }
        if let (Some(adapter), Some(form)) = (&device.lower, forms.first()) {
            let label = format!("{device_label}-lower");
            let run = run_child(&Task { task: "lower-validation", interface, index: adapter.index, form, label: &label, environment: &validation_environment }, work, output);
            write_validation(out, "wgpu_lower_level", &run);
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();
    out.string("shaders_generated_by_wgpu", generated_file.as_deref().unwrap_or("no adapter at the safe level"));
    out.object_end();
}

fn measure(cell: &str) {
    let output = output_dir(cell);
    let work = env::temp_dir().join(format!("{SPIKE}-work-{}", std::process::id()));
    let _ = fs::create_dir_all(&work);
    let scene = Scene::build();
    let many = workload::many_draws_scene();

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.key("wgpu");
    out.object_start();
    out.string("version", WGPU_VERSION);
    out.string("license", "MIT OR Apache-2.0");
    out.string("relationship", "candidate library, linked inside this spike only");
    out.string("instance_flags", &format!("{:?}", measured_flags()));
    out.string("instance_flags_under_validation", &format!("{:?}", validation_flags()));
    out.object_end();
    out.key("inputs");
    out.object_start();
    out.hex("scene_hash", scene.input_hash());
    out.u64("scene_draws", scene.draws.len() as u64);
    out.u64("many_draws", MANY_DRAWS as u64);
    out.u64("many_draws_vertices", many.vertices.len() as u64);
    out.u64("many_draws_repetitions", MANY_DRAWS_REPETITIONS as u64);
    out.object_end();

    let made = ahead::compile(&work);
    out.key("shaders_compiled_ahead");
    out.array_start();
    for form in &made {
        out.object_start();
        out.string("form", form.form);
        out.string("tool", &form.tool);
        match &form.outcome {
            Ok(bytes) => {
                out.bool("made", true);
                out.u64("bytes", *bytes);
            }
            Err(reason) => {
                out.bool("made", false);
                out.string("reason", reason);
            }
        }
        out.object_end();
    }
    out.array_end();

    let none: &[(String, String)] = &[];
    let list = run_child(&Task { task: "list", interface: "all", index: 0, form: "none", label: "list", environment: none }, &work, &output);
    out.key("listing");
    write_run(&mut out, "wgpu_both_levels", "none", &list);
    out.object_end();

    let apis = open_apis(&work);
    out.key("interfaces");
    out.array_start();
    for (interface, _) in safe::BACKENDS {
        let api = apis.iter().find(|(name, _)| *name == interface).map(|(_, api)| api);
        let listed_by_wgpu = !listed(&list, "safe", interface).is_empty() || !listed(&list, "lower", interface).is_empty();
        if api.is_some() || listed_by_wgpu {
            measure_interface(&mut out, interface, api, &list, &made, &work, &output);
        }
    }
    out.array_end();
    write_imports(&mut out);
    out.object_end();
    let _ = fs::remove_dir_all(&work);
    println!("{}", out.finish());
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--child") => std::process::exit(child(&arguments[1..])),
        Some(cell) => measure(cell),
        None => measure("unspecified"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render_device_baseline::backend::DeviceKind;

    fn info(index: usize, name: &str, vendor_id: u32, device_id: u32) -> DeviceInfo {
        DeviceInfo { index, name: name.to_string(), kind: DeviceKind::Software, vendor_id, device_id, driver: String::new(), interface_version: String::new() }
    }

    #[test]
    fn an_adapter_line_is_parsed_and_a_malformed_one_refused() {
        let adapter = parse_listed(2, "Microsoft Basic Render Driver|software|0x1414|0x008c|10.0.26100.9818|").unwrap();
        assert_eq!((adapter.index, adapter.name.as_str(), adapter.kind.as_str()), (2, "Microsoft Basic Render Driver", "software"));
        assert_eq!((adapter.vendor_id, adapter.device_id, adapter.driver.as_str()), (0x1414, 0x8c, "10.0.26100.9818"));
        assert_eq!(parse_listed(0, "llvmpipe|software|what|0x0000"), None);
        assert_eq!(parse_listed(0, "llvmpipe"), None);
    }

    #[test]
    fn a_device_listed_twice_is_one_and_a_native_only_device_is_kept() {
        let native = [info(0, "hardware", 0x10de, 1), info(1, "software", 0x1414, 0x8c), info(2, "software", 0x1414, 0x8c), info(3, "native only", 7, 7)];
        let safe = [parse_listed(0, "hardware|discrete|0x10de|0x0001|d|").unwrap(), parse_listed(1, "software|software|0x1414|0x008c|d|").unwrap()];
        let lower = [parse_listed(0, "software|software|0x1414|0x008c|d|").unwrap()];
        let devices = devices_of(&native, &safe, &lower);
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[1].native.as_ref().map(|info| info.index), Some(1));
        assert_eq!(devices[1].lower.as_ref().map(|adapter| adapter.index), Some(0));
        assert!(devices[0].lower.is_none());
        assert!(devices[2].safe.is_none() && devices[2].native.is_some());
    }

    #[test]
    fn a_childs_values_keep_their_kind() {
        let mut out = Json::new();
        out.object_start();
        write_value(&mut out, "scene_serial_hash", "1224191051008123");
        write_value(&mut out, "threads_start", "4");
        write_value(&mut out, "create_us", "12.5");
        write_value(&mut out, "recordings_identical", "true");
        write_value(&mut out, "shader_compilers_device", "none");
        out.object_end();
        assert_eq!(out.finish(), r#"{"scene_serial_hash":"1224191051008123","threads_start":4,"create_us":12.500,"recordings_identical":true,"shader_compilers_device":"none"}"#);
    }

    #[test]
    fn labels_and_lines_are_shaped_for_files_and_reports() {
        assert_eq!(file_label("direct3d12-Microsoft Basic Render Driver"), "direct3d12-microsoft-basic-render-driver");
        assert_eq!(one_line("a\r\n  b\tc"), "a b c");
        assert_eq!(parse_lines("a=1\nnoise\nb = x=y\n"), vec![("a".to_string(), "1".to_string()), ("b".to_string(), "x=y".to_string())]);
    }
}
