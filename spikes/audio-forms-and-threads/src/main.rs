//! Audio forms and threads spike: the measurements. Throwaway, never product
//! code. One JSON document on standard output.
//!
//! The parent process never touches the output library: it measures the
//! offline form and the simulation's hashes, then starts two children of
//! itself, one that takes the null form and reports what it has loaded, one
//! that opens a device stream and reports what it finds.

mod sys;

use std::env;
use std::io::Read;
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use audio_forms_and_threads::{
    Command, Counting, Decoder, EMITTERS, Emitter, Mixer, Producer, STREAMED_VOICE, Stream, Wave, commands_between, extract, frames_of_tick, inside_the_mixer, mixer_allocations, ring,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use reference_workload::{Fnv1a, Grid, Json, Parameters, SEQUENTIAL_PHASES, Size, State, Timing, run_sequential, tick, write_platform_and_build};

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "audio-forms-and-threads";
const TICKS: u32 = 600;
const DEVICE_TICKS: u32 = 90;
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);
const SETTLE: Duration = Duration::from_millis(400);

struct Checks {
    every_check_holds: bool,
}

impl Checks {
    fn record(&mut self, out: &mut Json, name: &str, holds: bool) {
        out.bool(name, holds);
        self.every_check_holds &= holds;
    }
}

fn parameters(ticks: u32) -> Parameters {
    Parameters::new(Size::Small, ticks, false)
}

/// The one-shot sounds and the streamed sound that the script adds to the
/// emitters: a function of the tick alone.
fn scripted(tick_index: u32, wave: Wave, at: u64, push: &mut impl FnMut(Command) -> bool) {
    if tick_index == 1 {
        push(Command::Start { voice: STREAMED_VOICE, wave: Wave::Streamed, millihertz: 0, gain: 0.02, pan: 0.0, at });
    }
    let voice = 32 + (tick_index / 10) % 16;
    if tick_index % 10 == 0 {
        push(Command::Start { voice, wave, millihertz: 440_000 + 1_000 * tick_index, gain: 0.03, pan: 0.0, at });
    }
    if tick_index % 10 == 5 && tick_index > 10 {
        push(Command::Stop { voice: 32 + (tick_index / 10 - 1) % 16, at });
    }
}

struct OfflineRun {
    tick_hashes: Vec<u64>,
    frames: u64,
    samples_hash: u64,
    commands: u64,
    allocations_in_the_mixer: u64,
    counts_follow_the_ticks: bool,
}

/// The offline form: after each tick, the changes are handed to the mixer and
/// the frames of that tick are mixed on the calling thread. Never waits.
fn offline(ticks: u32, wave: Wave, rate: u64, numerator: u64, denominator: u64) -> OfflineRun {
    let p = parameters(ticks);
    let mut state = State::initialize(&p);
    let mut grid = Grid::new();
    let mut timing = Timing::new(&SEQUENTIAL_PHASES);
    let (mut producer, consumer) = ring(1_024);
    let mut mixer = Mixer::new(rate as u32, consumer, Stream::Synchronous(Decoder::new()));
    let most = (rate * numerator / denominator + 1) as usize;
    let mut buffer = vec![0.0f32; most * 2];
    let mut samples = Fnv1a::new();
    let mut run = OfflineRun { tick_hashes: vec![state.hash()], frames: 0, samples_hash: 0, commands: 0, allocations_in_the_mixer: 0, counts_follow_the_ticks: true };
    let mut previous: Option<[Emitter; EMITTERS]> = None;
    let allocations_before = mixer_allocations();
    for t in 1..=ticks {
        tick(&mut state, &mut grid, t, false, &mut timing);
        run.tick_hashes.push(state.hash());
        let frames = frames_of_tick(u64::from(t - 1), rate, numerator, denominator);
        let at = mixer.position();
        let current = extract(&state);
        let mut push = |command| producer.push(command);
        run.commands += commands_between(previous.as_ref(), &current, wave, at, frames as u32, &mut push);
        scripted(t, wave, at, &mut push);
        previous = Some(current);
        let out = &mut buffer[..frames as usize * 2];
        inside_the_mixer(|| mixer.mix(out));
        for sample in out.iter() {
            samples.write(&sample.to_bits().to_le_bytes());
        }
        run.frames += frames;
        run.counts_follow_the_ticks &= mixer.position() == u64::from(t) * rate * numerator / denominator;
    }
    run.allocations_in_the_mixer = mixer_allocations() - allocations_before;
    run.samples_hash = samples.finish();
    run.counts_follow_the_ticks &= mixer.commands_dropped == 0;
    run
}

/// The simulation alone, and the simulation with a sink that takes the null
/// form: nothing is extracted and nothing is mixed.
fn plain(ticks: u32, null_sink: bool) -> Vec<u64> {
    let p = parameters(ticks);
    let mut state = State::initialize(&p);
    let mut grid = Grid::new();
    let mut timing = Timing::new(&SEQUENTIAL_PHASES);
    let mut hashes = vec![state.hash()];
    let sink: Option<fn(&State)> = null_sink.then_some(|_| {});
    for t in 1..=ticks {
        tick(&mut state, &mut grid, t, false, &mut timing);
        hashes.push(state.hash());
        if let Some(sink) = sink {
            sink(&state);
        }
    }
    hashes
}

fn offline_form(out: &mut Json, checks: &mut Checks) {
    out.key("offline_count");
    out.array_start();
    for (rate, numerator, denominator) in [(48_000u64, 1u64, 60u64), (44_100, 1, 60), (48_000, 1, 50), (48_000, 1, 90), (44_100, 1, 64)] {
        let per_tick: Vec<u64> = (0..u64::from(TICKS)).map(|t| frames_of_tick(t, rate, numerator, denominator)).collect();
        let total: u64 = per_tick.iter().sum();
        let resumed: Vec<u64> = (300..u64::from(TICKS)).map(|t| frames_of_tick(t, rate, numerator, denominator)).collect();
        let run = offline(TICKS, Wave::Triangle, rate, numerator, denominator);
        out.object_start();
        out.u64("rate", rate);
        out.string("step", &format!("{numerator}/{denominator} s"));
        out.bool("the_step_divides_the_rate", rate * numerator % denominator == 0);
        out.u64("fewest_frames_in_a_tick", *per_tick.iter().min().unwrap_or(&0));
        out.u64("most_frames_in_a_tick", *per_tick.iter().max().unwrap_or(&0));
        out.u64("frames_over_the_run", total);
        checks.record(out, "the_total_is_the_floor_of_rate_times_elapsed_time", total == u64::from(TICKS) * rate * numerator / denominator && run.frames == total);
        checks.record(out, "the_mixer_position_after_each_tick_follows_the_tick_index", run.counts_follow_the_ticks);
        checks.record(out, "a_run_resumed_at_tick_300_mixes_the_same_frames_per_tick", resumed[..] == per_tick[300..]);
        out.object_end();
    }
    out.array_end();

    out.key("offline_samples");
    out.object_start();
    out.u64("ticks", u64::from(TICKS));
    out.string("voices", "sixteen emitters from the simulation, one-shot sounds every ten ticks, one streamed sound decoded as the mixer pulls");
    for (name, wave) in [("triangle_in_exact_arithmetic", Wave::Triangle), ("the_platform_sine", Wave::PlatformSine)] {
        let first = offline(TICKS, wave, 48_000, 1, 60);
        let second = offline(TICKS, wave, 48_000, 1, 60);
        out.key(name);
        out.object_start();
        out.hex("samples_hash", first.samples_hash);
        out.u64("frames", first.frames);
        out.u64("commands_from_the_emitters", first.commands);
        checks.record(out, "two_runs_give_the_same_samples", first.samples_hash == second.samples_hash);
        checks.record(out, "the_mixer_allocates_nothing", first.allocations_in_the_mixer == 0 && second.allocations_in_the_mixer == 0);
        out.u64("allocations_in_the_mixer", first.allocations_in_the_mixer);
        out.object_end();
    }
    out.object_end();

    let reference = run_sequential(&parameters(TICKS));
    let alone = plain(TICKS, false);
    let null = plain(TICKS, true);
    let mixed = offline(TICKS, Wave::Triangle, 48_000, 1, 60);
    out.key("forms_and_the_simulation");
    out.object_start();
    out.hex("final_hash", reference.final_hash());
    checks.record(out, "the_simulation_alone_gives_the_reference_hashes", alone == reference.tick_hashes);
    checks.record(out, "the_null_form_gives_the_reference_hashes", null == reference.tick_hashes);
    checks.record(out, "the_offline_form_gives_the_reference_hashes", mixed.tick_hashes == reference.tick_hashes);
    out.object_end();
}

fn mixer_cost(out: &mut Json) {
    let (mut producer, consumer) = ring(256);
    let mut mixer = Mixer::new(48_000, consumer, Stream::None);
    for voice in 0..64u32 {
        producer.push(Command::Start { voice, wave: Wave::Triangle, millihertz: 110_000 + 20_000 * voice, gain: 0.01, pan: 0.0, at: 0 });
    }
    let mut buffer = [0.0f32; 1_024];
    let mut best = u128::MAX;
    for _ in 0..200 {
        let started = Instant::now();
        mixer.mix(&mut buffer);
        best = best.min(started.elapsed().as_nanos());
        std::hint::black_box(&buffer);
    }
    out.key("mixer_cost");
    out.object_start();
    out.u64("voices", 64);
    out.u64("frames_per_buffer", 512);
    out.u64("best_ns_per_buffer", best as u64);
    out.decimal("share_of_the_buffer_duration_percent", best as f64 / (512.0 / 48_000.0 * 1e9) * 100.0);
    out.object_end();
}

// ------------------------------------------------------------------ children

fn write_list(out: &mut Json, name: &str, items: &[String]) {
    let quoted: Vec<String> = items.iter().map(|item| format!("\"{}\"", item.replace('\\', "/").replace('"', "'"))).collect();
    out.raw(name, &format!("[{}]", quoted.join(",")));
}

/// The child that takes the null form: it opens nothing and mixes nothing.
fn null_form_probe() -> String {
    let hashes = plain(TICKS, true);
    let mut out = Json::new();
    out.object_start();
    out.i64("threads", sys::thread_count());
    out.u64("loaded_modules", sys::loaded_module_count() as u64);
    write_list(&mut out, "loaded_audio_modules", &sys::loaded_audio_modules());
    out.hex("final_hash", *hashes.last().expect("a run has hashes"));
    out.object_end();
    out.finish()
}

#[derive(Default)]
struct StreamFacts {
    callbacks: AtomicU64,
    frames: AtomicU64,
    fewest_frames: AtomicU64,
    most_frames: AtomicU64,
    position: AtomicU64,
    playback_lead_ns: AtomicU64,
    policy: AtomicI64,
    priority: AtomicI64,
    underruns: AtomicU64,
    commands_dropped: AtomicU64,
}

fn stop_decoder(stop: &AtomicBool, decoder: thread::JoinHandle<()>) {
    stop.store(true, Ordering::Relaxed);
    let _ = decoder.join();
}

/// The child that opens a device stream through the output library.
fn device_probe() -> String {
    let mut out = Json::new();
    out.object_start();

    // The decoding thread is the spike's own: it starts before the first
    // count, so that the counts that follow are the library's and the
    // platform's.
    let (mut decoded, samples) = ring::<f32>(48_000);
    let stop = Arc::new(AtomicBool::new(false));
    let decoder = {
        let stop = stop.clone();
        thread::Builder::new()
            .name("spike-decoder".to_string())
            .spawn(move || {
                let mut decoder = Decoder::new();
                let mut next = decoder.next_sample();
                while !stop.load(Ordering::Relaxed) {
                    while decoded.push(next) {
                        next = decoder.next_sample();
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            })
            .expect("a thread can be started")
    };
    thread::sleep(Duration::from_millis(50));

    out.i64("threads_before_the_library", sys::thread_count());
    write_list(&mut out, "audio_modules_before_the_library", &sys::loaded_audio_modules());

    let host = cpal::default_host();
    out.string("host", host.id().name());
    out.i64("threads_with_the_host", sys::thread_count());

    let Some(device) = host.default_output_device() else {
        out.string("outcome", "no default output device");
        out.i64("threads_after_the_lookup", sys::thread_count());
        write_list(&mut out, "audio_modules_after_the_lookup", &sys::loaded_audio_modules());
        drop(host);
        thread::sleep(SETTLE);
        out.i64("threads_after_the_host_is_dropped", sys::thread_count());
        stop_decoder(&stop, decoder);
        out.object_end();
        return out.finish();
    };
    out.string("device", &device.id().map_or_else(|error| format!("unnamed: {error}"), |id| id.to_string()));
    let supported = match device.default_output_config() {
        Ok(supported) => supported,
        Err(error) => {
            out.string("outcome", "no default output configuration");
            out.string("error", &format!("{:?}: {error}", error.kind()));
            out.i64("threads_after_the_failure", sys::thread_count());
            stop_decoder(&stop, decoder);
            out.object_end();
            return out.finish();
        }
    };
    out.string("default_sample_format", &supported.sample_format().to_string());
    out.u64("default_rate", u64::from(supported.sample_rate()));
    out.u64("default_channels", u64::from(supported.channels()));
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate;
    let channels = usize::from(config.channels);

    let (producer, consumer) = ring::<Command>(1_024);
    let mut mixer = Mixer::new(rate, consumer, Stream::Ring(samples));
    let facts = Arc::new(StreamFacts { fewest_frames: AtomicU64::new(u64::MAX), ..StreamFacts::default() });
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut scratch = vec![0.0f32; 4_096 * 2];
    let callback_facts = facts.clone();
    let callback_errors = errors.clone();
    let built = device.build_output_stream(
        config,
        move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
            inside_the_mixer(|| {
                let facts = &callback_facts;
                let frames = data.len() / channels;
                for block in data.chunks_mut(4_096 * channels) {
                    let block_frames = block.len() / channels;
                    let stereo = &mut scratch[..block_frames * 2];
                    mixer.mix(stereo);
                    for (frame, pair) in block.chunks_mut(channels).zip(stereo.chunks_exact(2)) {
                        for (channel, sample) in frame.iter_mut().enumerate() {
                            *sample = if channel < 2 { pair[channel] } else { 0.0 };
                        }
                    }
                }
                if facts.callbacks.fetch_add(1, Ordering::Relaxed) == 0 {
                    let (policy, priority) = sys::scheduling_of_this_thread();
                    facts.policy.store(i64::from(policy), Ordering::Relaxed);
                    facts.priority.store(i64::from(priority), Ordering::Relaxed);
                }
                facts.frames.fetch_add(frames as u64, Ordering::Relaxed);
                facts.fewest_frames.fetch_min(frames as u64, Ordering::Relaxed);
                facts.most_frames.fetch_max(frames as u64, Ordering::Relaxed);
                facts.position.store(mixer.position(), Ordering::Relaxed);
                facts.underruns.store(mixer.stream_underruns, Ordering::Relaxed);
                facts.commands_dropped.store(mixer.commands_dropped, Ordering::Relaxed);
                let stamp = info.timestamp();
                facts.playback_lead_ns.store(stamp.playback.duration_since(stamp.callback).as_nanos() as u64, Ordering::Relaxed);
            });
        },
        move |error: cpal::Error| {
            if let Ok(mut errors) = callback_errors.lock() {
                errors.push(format!("{:?}: {error}", error.kind()));
            }
        },
        None,
    );
    let stream = match built {
        Ok(stream) => stream,
        Err(error) => {
            out.string("outcome", "the stream could not be built");
            out.string("error", &format!("{:?}: {error}", error.kind()));
            out.i64("threads_after_the_failure", sys::thread_count());
            write_list(&mut out, "audio_modules_after_the_failure", &sys::loaded_audio_modules());
            stop_decoder(&stop, decoder);
            out.object_end();
            return out.finish();
        }
    };
    out.i64("threads_with_the_stream_built", sys::thread_count());
    if let Err(error) = stream.play() {
        out.string("start_error", &format!("{:?}: {error}", error.kind()));
    }

    // The device form: the simulation steps, paced, and hands its changes to
    // the stream's mixer with a target fifty milliseconds ahead.
    let hashes = drive(DEVICE_TICKS, rate, producer, &facts);
    let reference = run_sequential(&parameters(DEVICE_TICKS));
    thread::sleep(Duration::from_millis(200));

    let callbacks = facts.callbacks.load(Ordering::Relaxed);
    out.string("outcome", if callbacks > 0 { "the stream asked for samples" } else { "the stream never asked for samples" });
    out.i64("threads_with_the_stream_running", sys::thread_count());
    write_list(&mut out, "thread_names_with_the_stream_running", &sys::thread_names());
    write_list(&mut out, "audio_modules_with_the_stream_running", &sys::loaded_audio_modules());
    out.u64("callbacks", callbacks);
    out.u64("frames_asked", facts.frames.load(Ordering::Relaxed));
    out.u64("fewest_frames_in_a_callback", if callbacks > 0 { facts.fewest_frames.load(Ordering::Relaxed) } else { 0 });
    out.u64("most_frames_in_a_callback", facts.most_frames.load(Ordering::Relaxed));
    out.decimal("playback_lead_ms_at_the_last_callback", facts.playback_lead_ns.load(Ordering::Relaxed) as f64 / 1e6);
    out.i64("callback_thread_scheduling_policy", facts.policy.load(Ordering::Relaxed));
    out.i64("callback_thread_priority", facts.priority.load(Ordering::Relaxed));
    out.u64("streamed_sound_underruns_in_frames", facts.underruns.load(Ordering::Relaxed));
    out.u64("commands_dropped", facts.commands_dropped.load(Ordering::Relaxed));
    out.u64("allocations_in_the_mixer", mixer_allocations());
    out.bool("the_device_form_gives_the_reference_hashes", hashes == reference.tick_hashes);
    write_list(&mut out, "errors_reported_by_the_library", &errors.lock().map(|errors| errors.clone()).unwrap_or_default());

    drop(stream);
    thread::sleep(SETTLE);
    out.i64("threads_after_the_stream_is_dropped", sys::thread_count());
    drop(device);
    drop(host);
    thread::sleep(SETTLE);
    out.i64("threads_after_the_host_is_dropped", sys::thread_count());
    write_list(&mut out, "thread_names_after_the_host_is_dropped", &sys::thread_names());
    stop_decoder(&stop, decoder);
    out.object_end();
    out.finish()
}

fn drive(ticks: u32, rate: u32, mut producer: Producer<Command>, facts: &StreamFacts) -> Vec<u64> {
    let p = parameters(ticks);
    let mut state = State::initialize(&p);
    let mut grid = Grid::new();
    let mut timing = Timing::new(&SEQUENTIAL_PHASES);
    let mut hashes = vec![state.hash()];
    let mut previous: Option<[Emitter; EMITTERS]> = None;
    let lead = u64::from(rate) / 20;
    let over = rate / 60;
    for t in 1..=ticks {
        tick(&mut state, &mut grid, t, false, &mut timing);
        hashes.push(state.hash());
        let at = facts.position.load(Ordering::Relaxed) + lead;
        let current = extract(&state);
        let mut push = |command| producer.push(command);
        commands_between(previous.as_ref(), &current, Wave::Triangle, at, over, &mut push);
        scripted(t, Wave::Triangle, at, &mut push);
        previous = Some(current);
        thread::sleep(Duration::from_millis(8));
    }
    hashes
}

/// Starts a child of this executable and writes its report, or what became
/// of it.
fn child(out: &mut Json, name: &str, argument: &str) -> bool {
    out.key(name);
    out.object_start();
    let exe = env::current_exe().expect("the spike knows its own path");
    let started = Instant::now();
    let mut reported = false;
    match Process::new(exe).arg(argument).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Err(error) => out.string("process", &format!("not started: {error}")),
        Ok(mut process) => {
            let mut stdout = process.stdout.take().expect("the child's output is piped");
            let mut stderr = process.stderr.take().expect("the child's errors are piped");
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let mut text = String::new();
                let mut errors = String::new();
                let _ = stdout.read_to_string(&mut text);
                let _ = stderr.read_to_string(&mut errors);
                let _ = sender.send((text, errors));
            });
            match receiver.recv_timeout(PROBE_TIMEOUT) {
                Ok((text, errors)) => {
                    let status = process.wait().ok();
                    reported = status.is_some_and(|status| status.success()) && text.trim_start().starts_with('{');
                    out.string("process", if reported { "reported" } else { "ended without a report" });
                    out.string("exit_status", &status.map_or("unknown".to_string(), |status| status.to_string()));
                    if reported {
                        out.raw("report", &text);
                    }
                    let tail: String = errors.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
                    out.string("standard_error_tail", &tail);
                }
                Err(_) => {
                    let _ = process.kill();
                    out.string("process", "timed out");
                }
            }
        }
    }
    out.decimal("seconds", started.elapsed().as_secs_f64());
    out.object_end();
    reported
}

fn main() {
    let arguments: Vec<String> = env::args().collect();
    match arguments.get(1).map(String::as_str) {
        Some("--probe-null-form") => {
            println!("{}", null_form_probe());
            return;
        }
        Some("--probe-device-form") => {
            println!("{}", device_probe());
            return;
        }
        _ => {}
    }
    let cell = arguments.get(1).cloned().unwrap_or_else(|| "unnamed".to_string());
    let mut checks = Checks { every_check_holds: true };

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    out.string("profile", if cfg!(debug_assertions) { "dev" } else { "release" });
    write_platform_and_build(&mut out);
    out.string("output_library", "cpal 0.18.2");
    offline_form(&mut out, &mut checks);
    mixer_cost(&mut out);
    out.i64("threads_of_the_parent", sys::thread_count());
    write_list(&mut out, "audio_modules_loaded_by_the_parent_which_never_calls_the_library", &sys::loaded_audio_modules());
    let null_reported = child(&mut out, "null_form", "--probe-null-form");
    let device_reported = child(&mut out, "device_form", "--probe-device-form");
    checks.record(&mut out, "both_children_reported", null_reported && device_reported);
    out.bool("every_check_holds", checks.every_check_holds);
    out.object_end();
    println!("{}", out.finish());
}
