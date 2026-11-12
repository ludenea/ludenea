//! The filtering and blending scene on Metal: 4-sample color and depth
//! targets resolved into a single-sample texture by the pass's store action,
//! an array of five textures and three samplers chosen per draw by fragment
//! bytes, one indirect draw, the pass recorded by one encoder or by the four
//! sub-encoders of a parallel render command encoder, two timestamps around
//! it where the device samples them, and a read-back whose completion is
//! polled. Throwaway, never product code.

use std::ffi::c_void;
use std::sync::Barrier;
use std::time::Instant;

use crate::backend::{Recording, SceneRender, SceneShaders, ShaderLoad};
use crate::scene::{self, Draw, Encoding, Image, Scene};

use super::gpu::{self, ColorAttachment, ColorTarget, DepthAttachment, Gpu, PipelineShape, TextureShape};
use super::objc::{Id, Owned, Pool, present};
use super::timestamps::StageTimestamps;
use super::values;

const FILE: &str = "scene.metal";
const WIDTH: usize = scene::WIDTH as usize;
const HEIGHT: usize = scene::HEIGHT as usize;
const SAMPLES: usize = scene::SAMPLES as usize;
const BYTES_PER_PIXEL: usize = 4;
const SAMPLER_COUNT: usize = 3;

/// Everything the pass needs, created before the recording starts.
struct Resources {
    opaque_pipeline: Owned,
    blended_pipeline: Owned,
    depth_written: Owned,
    depth_tested_only: Owned,
    samplers: Vec<Owned>,
    textures: Vec<Owned>,
    vertices: Owned,
    indirect_arguments: Owned,
    color: Owned,
    depth: Owned,
    resolved: Owned,
    read_back: Owned,
    timestamps: Option<StageTimestamps>,
}

/// What an encoder binds, as plain object pointers a recording thread can carry.
#[derive(Clone, Copy)]
struct Bindings {
    opaque_pipeline: Id,
    blended_pipeline: Id,
    depth_written: Id,
    depth_tested_only: Id,
    samplers: [Id; SAMPLER_COUNT],
    textures: [Id; scene::TEXTURE_COUNT],
    vertices: Id,
    indirect_arguments: Id,
}

/// One chunk of the pass for one recording thread.
struct ChunkJob<'a> {
    encoder: Id,
    bindings: Bindings,
    draws: &'a [Draw],
}

// SAFETY: the sub-encoder is used by the one thread that receives the job, as
// Metal requires of an encoder; the pipelines, depth states, samplers,
// textures and buffers are objects Metal lets any thread bind, alive until
// every recording thread has been joined.
unsafe impl Send for ChunkJob<'_> {}

impl ChunkJob<'_> {
    /// Records the chunk once the four threads are all ready to record.
    fn record(self, all_ready: &Barrier) -> Result<(), String> {
        let _pool = Pool::push();
        all_ready.wait();
        encode(self.encoder, &self.bindings, self.draws)
    }
}

impl Resources {
    fn create(gpu: &Gpu, stages: &Stages, scene: &Scene) -> Result<(Resources, ShaderLoad), String> {
        let started = Instant::now();
        // The spike's own source reads the vertex buffer by vertex index; a
        // shader a toolchain made takes its inputs through a vertex descriptor.
        let (vertex, fragment, supplied) = match stages {
            Stages::Source(source) => {
                let library = gpu.library(FILE, source)?;
                (gpu::function(&library, FILE, "scene_vertex")?, gpu::function(&library, FILE, "scene_fragment")?, false)
            }
            Stages::Supplied(shaders) => {
                let load = |bytes: &[u8], what: &str| {
                    if shaders.metal_text {
                        gpu.library(what, std::str::from_utf8(bytes).map_err(|error| format!("{what} is not text: {error}"))?)
                    } else {
                        gpu.library_from_bytes(what, bytes)
                    }
                };
                let (vertex_library, fragment_library) = (load(&shaders.vertex, "the supplied vertex stage")?, load(&shaders.fragment, "the supplied fragment stage")?);
                (
                    gpu::function(&vertex_library, "the supplied vertex stage", &shaders.vertex_entry)?,
                    gpu::function(&fragment_library, "the supplied fragment stage", &shaders.fragment_entry)?,
                    true,
                )
            }
        };
        let libraries_us = microseconds_since(started);
        let started = Instant::now();
        let pipeline = |what: &str, blended: bool| {
            let shape = PipelineShape { what, vertex: &vertex, fragment: &fragment, colors: &[ColorTarget { format: values::PIXEL_FORMAT_RGBA8_UNORM_SRGB, blended }], depth: true, samples: SAMPLES };
            if supplied { gpu.render_pipeline_with_scene_attributes(&shape) } else { gpu.render_pipeline(&shape) }
        };
        let (opaque_pipeline, blended_pipeline) = (pipeline("the scene's opaque pipeline", false)?, pipeline("the scene's blended pipeline", true)?);
        let load = ShaderLoad { libraries_us, pipelines_us: microseconds_since(started) };
        let target = |what: &str, format: usize, samples: usize| {
            gpu.texture(what, &TextureShape { format, width: WIDTH, height: HEIGHT, levels: 1, samples, usage: values::TEXTURE_USAGE_RENDER_TARGET, private: true })
        };
        let resources = Resources {
            opaque_pipeline,
            blended_pipeline,
            depth_written: gpu.depth_state(true)?,
            depth_tested_only: gpu.depth_state(false)?,
            samplers: samplers(gpu)?,
            textures: scene.textures.iter().map(|texture| sampled_texture(gpu, texture)).collect::<Result<_, _>>()?,
            vertices: gpu.buffer("the scene's vertices", scene.vertex_bytes())?,
            indirect_arguments: gpu.buffer("the scene's indirect arguments", scene.indirect_argument_bytes())?,
            color: target("the scene's 4-sample color target", values::PIXEL_FORMAT_RGBA8_UNORM_SRGB, SAMPLES)?,
            depth: target("the scene's 4-sample depth target", values::PIXEL_FORMAT_DEPTH32_FLOAT, SAMPLES)?,
            resolved: target("the scene's resolved target", values::PIXEL_FORMAT_RGBA8_UNORM_SRGB, 1)?,
            read_back: gpu.empty_buffer("the scene's read-back", WIDTH * HEIGHT * BYTES_PER_PIXEL)?,
            timestamps: StageTimestamps::create(gpu).ok(),
        };
        Ok((resources, load))
    }

    fn bindings(&self) -> Result<Bindings, String> {
        let ids = |objects: &[Owned]| objects.iter().map(Owned::id).collect::<Vec<Id>>();
        Ok(Bindings {
            opaque_pipeline: self.opaque_pipeline.id(),
            blended_pipeline: self.blended_pipeline.id(),
            depth_written: self.depth_written.id(),
            depth_tested_only: self.depth_tested_only.id(),
            samplers: ids(&self.samplers).try_into().map_err(|_| format!("the scene binds {SAMPLER_COUNT} samplers"))?,
            textures: ids(&self.textures).try_into().map_err(|_| format!("the scene binds {} textures, it has {}", scene::TEXTURE_COUNT, self.textures.len()))?,
            vertices: self.vertices.id(),
            indirect_arguments: self.indirect_arguments.id(),
        })
    }

    /// The pass: both 4-sample targets cleared, the color resolved by the
    /// store action. The second value says whether the pass writes the two
    /// timestamps: a descriptor that refuses their attachment is replaced by
    /// one without them.
    fn pass(&self, timestamps: Option<&StageTimestamps>) -> Result<(Id, bool), String> {
        let [red, green, blue, alpha] = scene::CLEAR_COLOR.map(f64::from);
        let color = ColorAttachment { texture: self.color.id(), resolve: Some(self.resolved.id()), load: values::LOAD_ACTION_CLEAR, clear: values::ClearColor { red, green, blue, alpha } };
        let depth = DepthAttachment { texture: self.depth.id(), clear: f64::from(scene::CLEAR_DEPTH) };
        let describe = || gpu::render_pass(&[color], Some(&depth));
        let pass = describe()?;
        match timestamps {
            Some(timestamps) if timestamps.attach(pass).is_ok() => Ok((pass, true)),
            Some(_) => Ok((describe()?, false)),
            None => Ok((pass, false)),
        }
    }
}

/// Nearest, linear and trilinear, in the order of the scene's sampler indexes.
fn samplers(gpu: &Gpu) -> Result<Vec<Owned>, String> {
    Ok(vec![
        gpu.sampler(values::SAMPLER_MIN_MAG_FILTER_NEAREST, values::SAMPLER_MIP_FILTER_NEAREST, values::SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE)?,
        gpu.sampler(values::SAMPLER_MIN_MAG_FILTER_LINEAR, values::SAMPLER_MIP_FILTER_NEAREST, values::SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE)?,
        gpu.sampler(values::SAMPLER_MIN_MAG_FILTER_LINEAR, values::SAMPLER_MIP_FILTER_LINEAR, values::SAMPLER_ADDRESS_MODE_REPEAT)?,
    ])
}

/// One texture of the scene in the format of its encoding, every level uploaded as given.
fn sampled_texture(gpu: &Gpu, texture: &scene::Texture) -> Result<Owned, String> {
    let format = match texture.encoding {
        Encoding::Srgb => values::PIXEL_FORMAT_RGBA8_UNORM_SRGB,
        Encoding::Unorm => values::PIXEL_FORMAT_RGBA8_UNORM,
    };
    let (width, height) = (texture.width as usize, texture.height as usize);
    let what = format!("the scene's texture {}", texture.name);
    let created = gpu.texture(&what, &TextureShape { format, width, height, levels: texture.levels.len(), samples: 1, usage: values::TEXTURE_USAGE_SHADER_READ, private: false })?;
    for (level, texels) in texture.levels.iter().enumerate() {
        gpu::upload(&created, &what, level, (width >> level).max(1), (height >> level).max(1), texels)?;
    }
    Ok(created)
}

/// Binds what every draw shares, issues the draws in order and ends the encoder.
fn encode(encoder: Id, bindings: &Bindings, draws: &[Draw]) -> Result<(), String> {
    gpu::end_encoding(encoder, bind_and_draw(encoder, bindings, draws))
}

fn bind_and_draw(encoder: Id, bindings: &Bindings, draws: &[Draw]) -> Result<(), String> {
    send!((); encoder, "setViewport:", values::Viewport: values::Viewport::of(0, 0, WIDTH, HEIGHT))?;
    send!((); encoder, "setScissorRect:", values::ScissorRect: values::ScissorRect { x: 0, y: 0, width: WIDTH, height: HEIGHT })?;
    send!((); encoder, "setVertexBuffer:offset:atIndex:", Id: bindings.vertices, usize: 0, usize: 0)?;
    for (index, texture) in bindings.textures.iter().enumerate() {
        send!((); encoder, "setFragmentTexture:atIndex:", Id: *texture, usize: index)?;
    }
    for (index, sampler) in bindings.samplers.iter().enumerate() {
        send!((); encoder, "setFragmentSamplerState:atIndex:", Id: *sampler, usize: index)?;
    }
    // The pipeline and the depth state are set when they change only: the
    // validation layer reports a state set again to the same value.
    let mut bound = None;
    for draw in draws {
        if bound != Some(draw.blended) {
            let (pipeline, depth_state) = if draw.blended { (bindings.blended_pipeline, bindings.depth_tested_only) } else { (bindings.opaque_pipeline, bindings.depth_written) };
            send!((); encoder, "setRenderPipelineState:", Id: pipeline)?;
            send!((); encoder, "setDepthStencilState:", Id: depth_state)?;
            bound = Some(draw.blended);
        }
        // The fragment bytes of the draw: the texture's index, then the sampler's.
        let constants = [draw.texture, draw.sampler];
        send!((); encoder, "setFragmentBytes:length:atIndex:", *const c_void: constants.as_ptr().cast(), usize: size_of_val(&constants), usize: 0)?;
        if draw.indirect {
            send!((); encoder, "drawPrimitives:indirectBuffer:indirectBufferOffset:", usize: values::PRIMITIVE_TYPE_TRIANGLE, Id: bindings.indirect_arguments, usize: 0)?;
        } else {
            send!((); encoder, "drawPrimitives:vertexStart:vertexCount:", usize: values::PRIMITIVE_TYPE_TRIANGLE, usize: draw.first_vertex as usize, usize: draw.vertex_count as usize)?;
        }
    }
    Ok(())
}

fn record_with_one_encoder(command_buffer: Id, pass: Id, bindings: &Bindings, scene: &Scene) -> Result<(), String> {
    let encoder = present(send!(Id; command_buffer, "renderCommandEncoderWithDescriptor:", Id: pass)?, "renderCommandEncoderWithDescriptor: for the scene")?;
    encode(encoder, bindings, &scene.draws)
}

/// The pass as a parallel render command encoder: one sub-encoder per chunk,
/// created in the chunks' order by the calling thread, which is the order
/// their commands run in whoever records them and whenever.
fn record_with_sub_encoders(command_buffer: Id, pass: Id, bindings: &Bindings, scene: &Scene, on_threads: bool) -> Result<(), String> {
    let parallel = present(send!(Id; command_buffer, "parallelRenderCommandEncoderWithDescriptor:", Id: pass)?, "parallelRenderCommandEncoderWithDescriptor: for the scene")?;
    let mut encoders = Vec::new();
    let mut created = Ok(());
    for _ in &scene.chunks {
        match send!(Id; parallel, "renderCommandEncoder").and_then(|encoder| present(encoder, "renderCommandEncoder of the parallel encoder")) {
            Ok(encoder) => encoders.push(encoder),
            Err(reason) => {
                created = Err(reason);
                break;
            }
        }
    }
    let recorded = match created {
        Ok(()) if on_threads => record_on_threads(&encoders, bindings, scene),
        Ok(()) => record_in_turn(&encoders, bindings, scene),
        Err(reason) => {
            // Every sub-encoder is ended before its parent, recorded or not.
            for encoder in &encoders {
                let _ = send!((); *encoder, "endEncoding");
            }
            Err(reason)
        }
    };
    gpu::end_encoding(parallel, recorded)
}

/// Every chunk is recorded and its encoder ended even after a failure.
fn record_in_turn(encoders: &[Id], bindings: &Bindings, scene: &Scene) -> Result<(), String> {
    let mut outcome = Ok(());
    for (encoder, chunk) in encoders.iter().zip(&scene.chunks) {
        let recorded = encode(*encoder, bindings, &scene.draws[chunk.clone()]);
        outcome = outcome.and(recorded);
    }
    outcome
}

/// Each chunk recorded by its own thread, the four recording at once.
fn record_on_threads(encoders: &[Id], bindings: &Bindings, scene: &Scene) -> Result<(), String> {
    let all_ready = Barrier::new(encoders.len());
    let outcomes: Vec<Result<(), String>> = std::thread::scope(|threads| {
        let recordings: Vec<_> = encoders
            .iter()
            .zip(&scene.chunks)
            .map(|(encoder, chunk)| {
                let job = ChunkJob { encoder: *encoder, bindings: *bindings, draws: &scene.draws[chunk.clone()] };
                let all_ready = &all_ready;
                threads.spawn(move || job.record(all_ready))
            })
            .collect();
        recordings.into_iter().map(|recording| recording.join().unwrap_or_else(|_| Err("a recording thread of the scene panicked".to_string()))).collect()
    });
    outcomes.into_iter().collect()
}

/// The resolved target copied to a buffer: the copy is committed, its
/// completion polled, then the buffer read. Returns the bytes and the polls.
fn read_back(gpu: &Gpu, resources: &Resources) -> Result<(Vec<u8>, u64), String> {
    const WHAT: &str = "the scene's read-back";
    let command_buffer = gpu.command_buffer(WHAT)?;
    gpu::encode_copy_to_buffer(command_buffer, resources.resolved.id(), resources.read_back.id(), WIDTH, HEIGHT, BYTES_PER_PIXEL)?;
    send!((); command_buffer, "commit")?;
    let polls = gpu::poll_until_completed(command_buffer, WHAT)?;
    Ok((gpu::buffer_bytes(&resources.read_back, WIDTH * HEIGHT * BYTES_PER_PIXEL, WHAT)?, polls))
}

fn microseconds_since(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000_000.0
}

/// Records the pass, commits it and waits for the device. Returns the times
/// from the start of the recording to its end, to the commit and to the
/// completion and, where the pass wrote its two timestamps, the time between
/// them.
fn record_and_complete(gpu: &Gpu, resources: &Resources, bindings: &Bindings, scene: &Scene, recording: Recording, timestamps: Option<&StageTimestamps>) -> Result<([f64; 3], Option<u64>), String> {
    let started = Instant::now();
    let command_buffer = gpu.command_buffer("the scene")?;
    let (pass, timed) = resources.pass(timestamps)?;
    match recording {
        Recording::SerialOneBuffer => record_with_one_encoder(command_buffer, pass, bindings, scene)?,
        Recording::ChunkedOneThread => record_with_sub_encoders(command_buffer, pass, bindings, scene, false)?,
        Recording::ChunkedFourThreads => record_with_sub_encoders(command_buffer, pass, bindings, scene, true)?,
    }
    let recorded_us = microseconds_since(started);
    send!((); command_buffer, "commit")?;
    let submitted_us = microseconds_since(started);
    gpu::wait_until_completed(command_buffer, "the scene's pass")?;
    let render_us = microseconds_since(started);
    Ok(([recorded_us, submitted_us, render_us], timestamps.filter(|_| timed).and_then(|timestamps| timestamps.elapsed_nanoseconds(gpu))))
}

/// Where the scene's two functions come from: the spike's own source, or a
/// form a toolchain made.
pub enum Stages<'a> {
    Source(&'a str),
    Supplied(&'a SceneShaders),
}

pub fn render(gpu: &Gpu, stages: &Stages, scene: &Scene, recording: Recording) -> Result<(SceneRender, ShaderLoad), String> {
    let (resources, load) = Resources::create(gpu, stages, scene)?;
    let bindings = resources.bindings()?;

    // A device that offers the timestamps and then fails the pass that takes
    // them still renders the scene: the failure is said, the pass repeated.
    let ([recorded_us, submitted_us, render_us], timestamp_ns) = match resources.timestamps.as_ref().map(|timestamps| record_and_complete(gpu, &resources, &bindings, scene, recording, Some(timestamps))) {
        Some(Ok(rendered)) => rendered,
        Some(Err(reason)) => {
            gpu::log(&format!("the scene's pass failed with its two timestamps attached ({reason}); it is rendered again without them"));
            record_and_complete(gpu, &resources, &bindings, scene, recording, None)?
        }
        None => record_and_complete(gpu, &resources, &bindings, scene, recording, None)?,
    };

    let started = Instant::now();
    let (bytes, readback_polls) = read_back(gpu, &resources)?;
    let readback_us = microseconds_since(started);
    let image = Image::from_rows(scene::WIDTH, scene::HEIGHT, WIDTH * BYTES_PER_PIXEL, &bytes);
    Ok((SceneRender { image, recorded_us, submitted_us, render_us, readback_us, readback_polls, timestamp_ns }, load))
}
