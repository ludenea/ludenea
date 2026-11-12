//! Frames on the native path: resources that outlive a frame, passes
//! recorded in parts by any thread, and submissions that are polled. It is
//! what the frame content spike's render device port stands on, one thin
//! adapter per graphics interface (`vulkan/frames.rs`, `d3d12/frames.rs`,
//! `metal/frames.rs`), each reached through [`crate::backend::Api::open_frames`].
//! Throwaway, never product code.
//!
//! # What an adapter implements
//!
//! Every resource is named by an identifier the adapter gives when it creates
//! it, never reused within one device. Nothing is deferred and nothing is
//! checked beyond what the interface itself checks: a resource is destroyed
//! when [`FrameDevice::destroy`] is called, and ordering that call after the
//! frames that name the resource is the caller's affair (the port above).
//!
//! - **A texture** is a [`crate::scene::Texture`]: 8-bit RGBA in its
//!   encoding, every level uploaded as given, none generated.
//! - **A vertex buffer** holds [`crate::scene::Vertex`] records (40 bytes:
//!   position 4 floats, uv 2 floats, color 4 floats) and is never written
//!   again after its creation.
//! - **A target** is one color attachment of 8-bit RGBA in the sRGB encoding
//!   and one 32-bit float depth attachment, both at 4 samples, resolved to a
//!   single-sample image of the same format: the target's image. A later pass
//!   of the same or of another submission may sample that image, through a
//!   view in the sRGB encoding, and a submission may read it back.
//! - **A pair of pipelines** is built from a [`ShaderPayload`]: the opaque
//!   one (blending off, depth test less-or-equal, depth write on) and the
//!   blended one (depth test less-or-equal, depth write off, source alpha and
//!   one minus source alpha for color, one and one minus source alpha for
//!   alpha, operation add). No face culling. All four samples are written.
//! - **The samplers** are the device's own, the three of the filtering scene
//!   in its order: nearest (clamp to edge), linear (clamp to edge), trilinear
//!   (repeat).
//!
//! # The frame shader
//!
//! One source per interface under `shaders/` (`frame.vert` and `frame.frag`,
//! `frame.hlsl`, `frame.metal`), compiled by [`FrameDevice::import_shader`]
//! with the interface's own compiler, as the other shaders of the spike are.
//!
//! The vertex stage computes `clip = transform * position`, `transform`
//! being the 4 by 4 matrix of the draw's constants, stored column by column
//! (the element of column `c` and row `r` at index `c * 4 + r`), and
//! `position` the vertex's four floats. Clip space has y up and depth 0 to 1;
//! Vulkan's vertex stage negates y after the product. The uv and the color
//! are interpolated with perspective.
//!
//! The fragment stage writes `sample(textures[texture], samplers[sampler],
//! uv) * color * tint`, the five textures being the pass's, in its order.
//! With the variant 1 it then multiplies the red, green and blue of the
//! result by (0.5, 1.0, 0.25): the change of a shader's payload the frame
//! content spike times.
//!
//! The draw's constants are the 88 bytes of [`DrawConstants`], visible to
//! both stages (push constants on Vulkan, root constants on Direct3D 12,
//! bytes set for both stages on Metal).
//!
//! # A pass and its parts
//!
//! A pass renders to one target with one pair of pipelines and five texture
//! sources. It clears the color to [`PassDescription::clear`] (linear values)
//! and the depth to 1, draws its parts in their order, and resolves. The
//! viewport and the scissor cover the target. A pass of zero parts clears and
//! resolves and draws nothing.
//!
//! Each part is one command buffer of the interface (a secondary command
//! buffer on Vulkan, a command list on Direct3D 12, an encoder of a parallel
//! render encoder on Metal), written by one call of
//! [`FrameDevice::record_part`] on whichever thread makes it. The parts of
//! one pass, and those of different passes, may be recorded at once by
//! different threads; the image depends on the order of the parts and never
//! on who recorded them or when. Inside a part the draws are issued in their
//! order, the pipeline bound when it changes, the vertex buffer bound when
//! it changes.
//!
//! A pass may not sample the image of its own target. A target sampled by a
//! pass holds what the last pass that rendered to it resolved, in submission
//! order, passes in the order given to [`FrameDevice::submit`].
//!
//! # Submissions
//!
//! [`FrameDevice::submit`] hands the recorded passes to the device's queue
//! in the order given, followed by a copy of each target named for read-back
//! to a buffer the processor can read, and returns without waiting.
//! [`FrameDevice::finished`] polls without blocking;
//! [`FrameDevice::take_images`] reads the copies once the submission is
//! finished and frees what the submission owned (command buffers, descriptor
//! tables, read-back buffers). Several submissions may be in flight at once.
//!
//! # Threads
//!
//! Every method takes `&self` and the device is `Send + Sync`. Resources are
//! created and destroyed from any thread. `open_pass`, `record_part` and
//! `close_pass` are called from any thread, one thread at a time for one part
//! and for the opening and closing of one pass. `submit`, `finished`,
//! `take_images` and `wait_idle` are called by one thread at a time, which
//! need not be a thread that recorded.

use std::sync::Mutex;

use crate::backend::DeviceInfo;
use crate::scene::{CLEAR_BYTES, CLEAR_COLOR, Image, SAMPLER_LINEAR, SAMPLER_NEAREST, SAMPLER_TRILINEAR, Scene, TEXTURE_WHITE, Texture, Vertex};

/// The constants of one draw: 88 bytes, no padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawConstants {
    /// Column by column: the element of column `c` and row `r` at `c * 4 + r`.
    pub transform: [f32; 16],
    /// Linear, multiplied into the sampled color.
    pub tint: [f32; 4],
    /// Index into the pass's five texture sources.
    pub texture: u32,
    /// One of the three samplers.
    pub sampler: u32,
}

pub const DRAW_CONSTANT_BYTES: usize = 88;
const _: () = assert!(std::mem::size_of::<DrawConstants>() == DRAW_CONSTANT_BYTES);

impl DrawConstants {
    pub fn bytes(&self) -> [u8; DRAW_CONSTANT_BYTES] {
        let mut bytes = [0_u8; DRAW_CONSTANT_BYTES];
        for (index, value) in self.transform.iter().chain(&self.tint).enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[80..84].copy_from_slice(&self.texture.to_le_bytes());
        bytes[84..88].copy_from_slice(&self.sampler.to_le_bytes());
        bytes
    }
}

/// One draw of a part: a range of a vertex buffer, as a list of triangles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameDraw {
    pub vertex_buffer: u64,
    pub first_vertex: u32,
    pub vertex_count: u32,
    /// Drawn with the blended pipeline; otherwise with the opaque one.
    pub blended: bool,
    pub constants: DrawConstants,
}

/// What a pass samples at one index of its five textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureSource {
    Texture(u64),
    /// The image of a target, as its last pass resolved it.
    Target(u64),
}

#[derive(Clone, Debug)]
pub struct PassDescription {
    pub target: u64,
    /// Linear values; the depth is cleared to 1.
    pub clear: [f32; 4],
    pub pipelines: u64,
    pub textures: [TextureSource; 5],
    /// How many parts the pass is recorded in; zero for a pass that only clears.
    pub parts: usize,
}

/// The two stages of the frame shader in the form the interface loads:
/// SPIR-V on Vulkan, the bytecode its compiler makes on Direct3D 12, Metal
/// text on Metal (the platform compiles it when the pipelines are built).
#[derive(Clone, Debug)]
pub struct ShaderPayload {
    pub vertex: Vec<u8>,
    pub fragment: Vec<u8>,
    /// The form, in words, for the record.
    pub form: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Texture(u64),
    VertexBuffer(u64),
    Target(u64),
    Pipelines(u64),
}

/// One device open for frames. See the module's documentation for what each
/// call must do.
pub trait FrameDevice: Send + Sync {
    fn info(&self) -> DeviceInfo;
    /// What configures the device beyond its name and version, for the reference key.
    fn settings(&self) -> String;
    /// Compiles the frame shader from the sources under `shaders/`, with the
    /// fragment stage's variant 0 or 1.
    fn import_shader(&self, variant: u32) -> Result<ShaderPayload, String>;
    fn create_pipelines(&self, payload: &ShaderPayload) -> Result<u64, String>;
    fn create_texture(&self, texture: &Texture) -> Result<u64, String>;
    fn create_vertex_buffer(&self, bytes: &[u8]) -> Result<u64, String>;
    fn create_target(&self, width: u32, height: u32) -> Result<u64, String>;
    /// Destroys the resource now.
    fn destroy(&self, resource: Resource);
    /// Starts the recording of a pass and returns its identifier.
    fn open_pass(&self, pass: &PassDescription) -> Result<u64, String>;
    /// Records one part of an open pass, start to end, on the calling thread.
    fn record_part(&self, pass: u64, part: usize, draws: &[FrameDraw]) -> Result<(), String>;
    /// Ends the recording of a pass whose parts are all recorded.
    fn close_pass(&self, pass: u64) -> Result<(), String>;
    /// Frees a pass that was opened, closed or not, and will not be submitted.
    fn discard_pass(&self, pass: u64);
    /// Hands the closed passes to the queue in this order, then the copies of
    /// the targets to read back, and returns the submission's identifier
    /// without waiting.
    fn submit(&self, passes: &[u64], read_back: &[u64]) -> Result<u64, String>;
    /// Whether the device has finished the submission; never blocks.
    fn finished(&self, submission: u64) -> Result<bool, String>;
    /// The images read back, in the order asked, and the release of what the
    /// submission owned. An error if the submission is not finished.
    fn take_images(&self, submission: u64) -> Result<Vec<Image>, String>;
    /// Blocks until the device has finished every submission.
    fn wait_idle(&self) -> Result<(), String>;
    /// Asks the interface to remove the device for real, where it offers a
    /// call that does, and says what was called. Every later call may fail;
    /// dropping the device must still return.
    fn remove_device(&self) -> Result<String, String> {
        Err("the interface offers no call that removes a device".to_string())
    }
    /// The reason the interface gives for a removed device, or nothing while
    /// the device is not removed or where the interface cannot tell.
    fn removed_reason(&self) -> Option<String> {
        None
    }
    /// How the validation layer's messages reach the program, in words.
    fn validation_delivery(&self) -> String;
    /// The messages delivered since the last call; empty with validation off.
    fn validation_messages(&self) -> Vec<String>;
}

/// The identity matrix scaled by (`sx`, `sy`) and moved by (`tx`, `ty`).
pub fn scale_translate(sx: f32, sy: f32, tx: f32, ty: f32) -> [f32; 16] {
    [sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, tx, ty, 0.0, 1.0]
}

/// A square covering clip space at depth 0.5, uv (0, 0) at the top left,
/// white: two triangles.
pub fn unit_quad() -> [Vertex; 6] {
    let corner = |x: f32, y: f32| Vertex { position: [x, y, 0.5, 1.0], uv: [(x + 1.0) / 2.0, (1.0 - y) / 2.0], color: [1.0; 4] };
    let (top_left, top_right, bottom_left, bottom_right) = (corner(-1.0, 1.0), corner(1.0, 1.0), corner(-1.0, -1.0), corner(1.0, -1.0));
    [top_left, bottom_left, top_right, top_right, bottom_left, bottom_right]
}

pub fn vertex_bytes(vertices: &[Vertex]) -> &[u8] {
    // SAFETY: `Vertex` is `repr(C)`, made of `f32` only, with no padding.
    unsafe { std::slice::from_raw_parts(vertices.as_ptr() as *const u8, std::mem::size_of_val(vertices)) }
}

/// One line of the check: what was checked and what was found.
pub struct CheckLine {
    pub name: &'static str,
    pub outcome: Result<String, String>,
}

/// Submits the passes, waits by polling and returns the images read back.
pub fn render(device: &dyn FrameDevice, passes: &[u64], read_back: &[u64]) -> Result<Vec<Image>, String> {
    let submission = device.submit(passes, read_back)?;
    while !device.finished(submission)? {
        std::thread::yield_now();
    }
    device.take_images(submission)
}

struct CheckScene {
    textures: [u64; 5],
    quad: u64,
    plain: u64,
    varied: u64,
    left: u64,
    right: u64,
    wide: u64,
}

const CHECK_SIDE: u32 = 64;

impl CheckScene {
    fn create(device: &dyn FrameDevice) -> Result<CheckScene, String> {
        let scene = Scene::build();
        let mut textures = [0_u64; 5];
        for (slot, texture) in textures.iter_mut().zip(&scene.textures) {
            *slot = device.create_texture(texture)?;
        }
        Ok(CheckScene {
            textures,
            quad: device.create_vertex_buffer(vertex_bytes(&unit_quad()))?,
            plain: device.create_pipelines(&device.import_shader(0)?)?,
            varied: device.create_pipelines(&device.import_shader(1)?)?,
            left: device.create_target(CHECK_SIDE, CHECK_SIDE)?,
            right: device.create_target(CHECK_SIDE, CHECK_SIDE)?,
            wide: device.create_target(CHECK_SIDE * 2, CHECK_SIDE)?,
        })
    }

    fn destroy(&self, device: &dyn FrameDevice) {
        for texture in self.textures {
            device.destroy(Resource::Texture(texture));
        }
        device.destroy(Resource::VertexBuffer(self.quad));
        device.destroy(Resource::Pipelines(self.plain));
        device.destroy(Resource::Pipelines(self.varied));
        for target in [self.left, self.right, self.wide] {
            device.destroy(Resource::Target(target));
        }
    }

    fn sources(&self) -> [TextureSource; 5] {
        self.textures.map(TextureSource::Texture)
    }

    fn draw(&self, transform: [f32; 16], tint: [f32; 4], texture: u32, sampler: u32, blended: bool) -> FrameDraw {
        FrameDraw { vertex_buffer: self.quad, first_vertex: 0, vertex_count: 6, blended, constants: DrawConstants { transform, tint, texture, sampler } }
    }

    /// A pass of one part: a white quad over the middle of the target.
    fn one_quad(&self, device: &dyn FrameDevice, target: u64, pipelines: u64, tint: [f32; 4]) -> Result<u64, String> {
        let pass = device.open_pass(&PassDescription { target, clear: CLEAR_COLOR, pipelines, textures: self.sources(), parts: 1 })?;
        device.record_part(pass, 0, &[self.draw(scale_translate(0.5, 0.5, 0.0, 0.0), tint, TEXTURE_WHITE, SAMPLER_NEAREST, false)])?;
        device.close_pass(pass)?;
        Ok(pass)
    }

    /// The draws of one of four parts: eight quads on a row, opaque and
    /// blended in turn, over every texture and sampler.
    fn row(&self, part: usize) -> Vec<FrameDraw> {
        (0..8_usize)
            .map(|column| {
                let index = part * 8 + column;
                let tint = [0.25 + 0.25 * (index % 4) as f32, 1.0 - 0.125 * column as f32, 0.5 + 0.125 * part as f32, 0.5 + 0.0625 * column as f32];
                let transform = scale_translate(0.1875, 0.3125, -0.875 + 0.25 * column as f32 + 0.0625 * part as f32, 0.75 - 0.4375 * part as f32);
                self.draw(transform, tint, (index % 5) as u32, [SAMPLER_NEAREST, SAMPLER_LINEAR, SAMPLER_TRILINEAR][index % 3], index % 2 == 1)
            })
            .collect()
    }

    /// A pass of four parts, recorded by the calling thread or by four at once.
    fn four_parts(&self, device: &dyn FrameDevice, target: u64, at_once: bool) -> Result<u64, String> {
        let pass = device.open_pass(&PassDescription { target, clear: CLEAR_COLOR, pipelines: self.plain, textures: self.sources(), parts: 4 })?;
        if at_once {
            let failure = Mutex::new(None);
            std::thread::scope(|threads| {
                for part in 0..4 {
                    let failure = &failure;
                    threads.spawn(move || {
                        if let Err(reason) = device.record_part(pass, part, &self.row(part)) {
                            *failure.lock().unwrap() = Some(reason);
                        }
                    });
                }
            });
            if let Some(reason) = failure.into_inner().unwrap() {
                return Err(reason);
            }
        } else {
            for part in 0..4 {
                device.record_part(pass, part, &self.row(part))?;
            }
        }
        device.close_pass(pass)?;
        Ok(pass)
    }

    /// A pass that copies the images of the two square targets, texel for
    /// pixel, into the two halves of the wide one.
    fn composition(&self, device: &dyn FrameDevice) -> Result<u64, String> {
        let mut textures = self.sources();
        textures[0] = TextureSource::Target(self.left);
        textures[1] = TextureSource::Target(self.right);
        let pass = device.open_pass(&PassDescription { target: self.wide, clear: [0.0, 0.0, 0.0, 1.0], pipelines: self.plain, textures, parts: 1 })?;
        let halves = [self.draw(scale_translate(0.5, 1.0, -0.5, 0.0), [1.0; 4], 0, SAMPLER_NEAREST, false), self.draw(scale_translate(0.5, 1.0, 0.5, 0.0), [1.0; 4], 1, SAMPLER_NEAREST, false)];
        device.record_part(pass, 0, &halves)?;
        device.close_pass(pass)?;
        Ok(pass)
    }
}

fn expect(condition: bool, found: String) -> Result<String, String> {
    if condition { Ok(found) } else { Err(found) }
}

fn half_of(wide: &Image, right: bool) -> Image {
    let mut rgba = Vec::with_capacity((CHECK_SIDE * CHECK_SIDE * 4) as usize);
    for row in wide.rgba.chunks_exact((CHECK_SIDE * 2 * 4) as usize) {
        let start = if right { (CHECK_SIDE * 4) as usize } else { 0 };
        rgba.extend_from_slice(&row[start..start + (CHECK_SIDE * 4) as usize]);
    }
    Image { width: CHECK_SIDE, height: CHECK_SIDE, rgba }
}

fn run_checks(device: &dyn FrameDevice, scene: &CheckScene, lines: &mut Vec<CheckLine>) -> Result<(), String> {
    let mut line = |name: &'static str, outcome: Result<String, String>| lines.push(CheckLine { name, outcome });

    let cleared = device.open_pass(&PassDescription { target: scene.left, clear: CLEAR_COLOR, pipelines: scene.plain, textures: scene.sources(), parts: 0 })?;
    device.close_pass(cleared)?;
    let image = render(device, &[cleared], &[scene.left])?.remove(0);
    let other = image.rgba.as_chunks::<4>().0.iter().filter(|pixel| **pixel != CLEAR_BYTES).count();
    line("a pass of no part clears its target", expect(other == 0, format!("{other} pixels are not the clear color")));

    let red = scene.one_quad(device, scene.left, scene.plain, [1.0, 0.0, 0.0, 1.0])?;
    let image = render(device, &[red], &[scene.left])?.remove(0);
    let (middle, corner) = (image.pixel(32, 32).to_vec(), image.pixel(2, 2).to_vec());
    line("an opaque quad is drawn with its tint", expect(middle == [255, 0, 0, 255] && corner == CLEAR_BYTES, format!("middle {middle:?}, corner {corner:?}")));

    let varied = scene.one_quad(device, scene.left, scene.varied, [1.0; 4])?;
    let image = render(device, &[varied], &[scene.left])?.remove(0);
    let middle = image.pixel(32, 32).to_vec();
    let near = middle.iter().zip([188_u8, 255, 137, 255]).all(|(found, expected)| found.abs_diff(expected) <= 1);
    line("the second variant of the shader changes the color", expect(near, format!("middle {middle:?}, expected about [188, 255, 137, 255]")));

    let serial = scene.four_parts(device, scene.left, false)?;
    let serial_image = render(device, &[serial], &[scene.left])?.remove(0);
    let at_once = scene.four_parts(device, scene.right, true)?;
    let at_once_image = render(device, &[at_once], &[scene.right])?.remove(0);
    let drawn = serial_image.rgba.as_chunks::<4>().0.iter().filter(|pixel| **pixel != CLEAR_BYTES).count();
    line(
        "four parts recorded at once give the bytes of four recorded in turn",
        expect(serial_image == at_once_image && drawn > 1_000, format!("in turn {:016x}, at once {:016x}, {drawn} pixels drawn", serial_image.hash(), at_once_image.hash())),
    );

    let left = scene.four_parts(device, scene.left, false)?;
    let right = scene.one_quad(device, scene.right, scene.plain, [0.0, 1.0, 0.0, 1.0])?;
    let composition = scene.composition(device)?;
    let images = render(device, &[left, right, composition], &[scene.left, scene.right, scene.wide])?;
    let same = half_of(&images[2], false) == images[0] && half_of(&images[2], true) == images[1] && images[0] == serial_image;
    line("a pass samples the images of two targets texel for pixel, in one submission", expect(same, format!("left {:016x}, right {:016x}, wide {:016x}", images[0].hash(), images[1].hash(), images[2].hash())));

    let first = scene.four_parts(device, scene.left, true)?;
    let second = scene.one_quad(device, scene.right, scene.plain, [0.0, 1.0, 0.0, 1.0])?;
    let first_submission = device.submit(&[first], &[scene.left])?;
    let second_submission = device.submit(&[second], &[scene.right])?;
    while !(device.finished(first_submission)? && device.finished(second_submission)?) {
        std::thread::yield_now();
    }
    let (first_image, second_image) = (device.take_images(first_submission)?.remove(0), device.take_images(second_submission)?.remove(0));
    line("two submissions in flight at once both finish with their images", expect(first_image == serial_image && second_image == images[1], format!("{:016x} and {:016x}", first_image.hash(), second_image.hash())));

    let discarded = scene.one_quad(device, scene.left, scene.plain, [1.0; 4])?;
    device.discard_pass(discarded);
    device.wait_idle()?;
    line("a recorded pass is discarded and the device comes to rest", Ok("done".to_string()));
    Ok(())
}

/// Exercises a device through every call of [`FrameDevice`], twice over with
/// every resource destroyed in between, and says what it found.
pub fn check(device: &dyn FrameDevice) -> Vec<CheckLine> {
    let mut lines = Vec::new();
    for round in ["first", "second"] {
        let before = lines.len();
        let outcome = CheckScene::create(device).and_then(|scene| {
            let outcome = run_checks(device, &scene, &mut lines);
            let idle = device.wait_idle();
            scene.destroy(device);
            outcome.and(idle)
        });
        if let Err(reason) = outcome {
            lines.push(CheckLine { name: "every call returns", outcome: Err(format!("{round} round, after {} checks: {reason}", lines.len() - before)) });
            return lines;
        }
    }
    let (first, second) = lines.split_at(lines.len() / 2);
    let repeated = first.iter().zip(second).all(|(a, b)| a.outcome == b.outcome);
    lines.push(CheckLine { name: "a second round on resources created anew finds the same", outcome: expect(repeated, format!("{} checks a round", first.len())) });
    lines
}
