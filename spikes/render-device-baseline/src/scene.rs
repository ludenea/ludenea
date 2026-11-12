//! The inputs of every scene of the spike, the image type, its hashes and its
//! PNG form. Throwaway, never product code.
//!
//! Every input is built with integer arithmetic, or with the float operations
//! the standard requires correctly rounded (add, subtract, multiply, divide,
//! square root), so that the bytes handed to a device are the same on every
//! cell by construction. No transcendental function is called here.
//!
//! Conventions shared by the three graphics interfaces: clip space has y up
//! and a depth range of 0 to 1 (Vulkan's vertex shaders negate y); images are
//! tightly packed RGBA rows, top row first; colors in vertices are linear,
//! with alpha not multiplied in.

use reference_workload::{Fnv1a, fnv1a};

/// The filtering and blending scene's target.
pub const WIDTH: u32 = 256;
pub const HEIGHT: u32 = 256;
/// Samples per pixel of the filtering and blending scene.
pub const SAMPLES: u32 = 4;
/// The linear value the sRGB encoding stores as the byte 64: a clear color
/// that sits on a code value, far from a rounding boundary.
pub const CLEAR_LINEAR: f32 = 0.051_269_5;
pub const CLEAR_COLOR: [f32; 4] = [CLEAR_LINEAR, CLEAR_LINEAR, CLEAR_LINEAR, 1.0];
/// The bytes a cleared pixel reads back as.
pub const CLEAR_BYTES: [u8; 4] = [64, 64, 64, 255];
pub const CLEAR_DEPTH: f32 = 1.0;
/// The blocks of the block hashes: an 8 by 8 grid.
pub const BLOCKS_PER_SIDE: u32 = 8;

pub const SAMPLER_NEAREST: u32 = 0;
pub const SAMPLER_LINEAR: u32 = 1;
pub const SAMPLER_TRILINEAR: u32 = 2;

pub const TEXTURE_HASHED: u32 = 0;
pub const TEXTURE_GRADIENT: u32 = 1;
pub const TEXTURE_TINTED_LEVELS: u32 = 2;
pub const TEXTURE_SOFT_DISC: u32 = 3;
pub const TEXTURE_WHITE: u32 = 4;
pub const TEXTURE_COUNT: usize = 5;

/// The compute exercise: a square of invocations, one per element.
pub const COMPUTE_SIDE: u32 = 64;
pub const COMPUTE_ELEMENTS: usize = (COMPUTE_SIDE * COMPUTE_SIDE) as usize;
pub const COMPUTE_BUFFER_MULTIPLIER: u32 = 2_654_435_761;
pub const COMPUTE_TEXTURE_MASK: u32 = 0x9e37_79b9;

/// The pass-through exercise: a source of 32 by 32 texels, drawn texel for
/// pixel into each half of a 64 by 32 target.
pub const PASS_THROUGH_SIDE: u32 = 32;

/// The lit scene's target.
pub const LIT_SIZE: u32 = 512;
/// Cells per side of the lit height field.
const LIT_GRID: usize = 96;

/// One vertex of the filtering and blending scene: a clip-space position, a
/// texture coordinate and a linear color. 40 bytes, no padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 4],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// One vertex of the lit scene: a clip-space position, a world position, a
/// unit normal and a linear albedo. 52 bytes, no padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LitVertex {
    pub clip: [f32; 4],
    pub world: [f32; 3],
    pub normal: [f32; 3],
    pub albedo: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Stored sRGB-encoded, converted to linear when sampled.
    Srgb,
    /// Stored and sampled as is.
    Unorm,
}

/// A texture of 8-bit RGBA texels with every level authored here: a device
/// uploads the levels as given and generates none.
pub struct Texture {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub encoding: Encoding,
    pub levels: Vec<Vec<u8>>,
}

/// One draw of the filtering and blending scene, in the scene's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    pub name: &'static str,
    pub first_vertex: u32,
    pub vertex_count: u32,
    /// Index into the bound array of textures.
    pub texture: u32,
    /// One of the three samplers.
    pub sampler: u32,
    /// Source-alpha blending with no depth write; otherwise opaque with depth write.
    pub blended: bool,
    /// Issued through the indirect arguments buffer.
    pub indirect: bool,
}

/// A named rectangle of the target, in pixels: left, top, right, bottom.
pub struct Region {
    pub name: &'static str,
    pub bounds: [u32; 4],
}

pub const REGIONS: [Region; 6] = [
    Region { name: "depth_tested_triangles", bounds: [4, 4, 124, 124] },
    Region { name: "nearest_hashed_texels", bounds: NEAREST_BOUNDS },
    Region { name: "linear_gradient", bounds: [4, 136, 92, 188] },
    Region { name: "trilinear_floor", bounds: [8, 196, 248, 248] },
    Region { name: "blended_discs", bounds: [140, 116, 236, 188] },
    Region { name: "sliver_fan", bounds: [100, 100, 156, 156] },
];
/// The quad sampled with the nearest filter: 12 by 12 texels at 8 pixels each.
const NEAREST_BOUNDS: [u32; 4] = [152, 4, 248, 100];
const NEAREST_PIXELS_PER_TEXEL: u32 = 8;

pub struct Scene {
    /// The width and the height of the filtering and blending scene's target,
    /// `WIDTH` by `HEIGHT` as built. The vertices are in clip space, so a later
    /// spike that sets another size renders the same scene stretched to it; the
    /// regions and the nearest-filter check hold at the built size only.
    pub target: [u32; 2],
    pub vertices: Vec<Vertex>,
    pub textures: Vec<Texture>,
    pub draws: Vec<Draw>,
    /// The draws split into four consecutive chunks, one command buffer each
    /// in the chunked recordings.
    pub chunks: [std::ops::Range<usize>; 4],
    /// Vertex count, instance count, first vertex, first instance: the layout
    /// the three interfaces share for a non-indexed indirect draw.
    pub indirect_arguments: [u32; 4],
    pub compute_input: Vec<u32>,
    /// 32 by 32 RGBA texels in which every byte value occurs in every channel.
    pub pass_through_source: Vec<u8>,
    pub lit_vertices: Vec<LitVertex>,
    /// Camera position, ambient color, four light positions, four light
    /// colors with the intensity in alpha: ten vectors of four floats.
    pub lit_constants: [[f32; 4]; 10],
}

/// The finalizer of SplitMix64: a fixed integer hash.
fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A point given in pixels of the 256-pixel target, at depth `z` and with the
/// homogeneous coordinate `w`. Divisions by 128 are exact.
fn at(x: f32, y: f32, z: f32, w: f32) -> [f32; 4] {
    [(x / 128.0 - 1.0) * w, (1.0 - y / 128.0) * w, z * w, w]
}

fn triangle(vertices: &mut Vec<Vertex>, points: [[f32; 2]; 3], z: f32, color: [f32; 4]) {
    for point in points {
        vertices.push(Vertex { position: at(point[0], point[1], z, 1.0), uv: [0.0, 0.0], color });
    }
}

/// A rectangle as two triangles; `uv` gives the coordinates of the left, top,
/// right and bottom edges, `w` the homogeneous coordinate of the top and of
/// the bottom edge.
fn quad(vertices: &mut Vec<Vertex>, bounds: [f32; 4], z: f32, uv: [f32; 4], w: [f32; 2], color: [f32; 4]) {
    let [left, top, right, bottom] = bounds;
    let corner = |x: f32, y: f32, u: f32, v: f32, w: f32| Vertex { position: at(x, y, z, w), uv: [u, v], color };
    let top_left = corner(left, top, uv[0], uv[1], w[0]);
    let top_right = corner(right, top, uv[2], uv[1], w[0]);
    let bottom_left = corner(left, bottom, uv[0], uv[3], w[1]);
    let bottom_right = corner(right, bottom, uv[2], uv[3], w[1]);
    vertices.extend_from_slice(&[top_left, bottom_left, top_right, top_right, bottom_left, bottom_right]);
}

fn textures() -> Vec<Texture> {
    let hashed: Vec<u8> = (0..64 * 64_u64)
        .flat_map(|texel| {
            let hash = mix(texel);
            [hash as u8, (hash >> 8) as u8, (hash >> 16) as u8, 255]
        })
        .collect();
    let gradient: Vec<u8> = (0..64_u32)
        .flat_map(|y| (0..64_u32).flat_map(move |x| [(128 + x * 2) as u8, (y * 4) as u8, ((x ^ y) * 4) as u8, 255]))
        .collect();
    // Each level carries its own tint, so that the level a device selects and
    // the weights it blends two levels with show in the image.
    const TINTS: [[u8; 3]; 9] = [
        [255, 64, 64],
        [64, 255, 64],
        [64, 64, 255],
        [255, 255, 64],
        [255, 64, 255],
        [64, 255, 255],
        [255, 160, 64],
        [160, 64, 255],
        [255, 255, 255],
    ];
    let tinted_levels: Vec<Vec<u8>> = TINTS
        .iter()
        .enumerate()
        .map(|(level, tint)| {
            let side = 256_u32 >> level;
            (0..side)
                .flat_map(|y| {
                    (0..side).flat_map(move |x| {
                        let shift = (x ^ y) & 1;
                        [tint[0] >> shift, tint[1] >> shift, tint[2] >> shift, 255]
                    })
                })
                .collect()
        })
        .collect();
    let soft_disc: Vec<u8> = (0..32_u32)
        .flat_map(|y| {
            (0..32_u32).flat_map(move |x| {
                let (dx, dy) = (2 * x as i32 - 31, 2 * y as i32 - 31);
                let distance = (dx * dx + dy * dy) as u32;
                let alpha = if distance >= 961 { 0 } else { 255 - distance * 255 / 961 };
                [255, (128 + x * 3) as u8, (64 + y * 4) as u8, alpha as u8]
            })
        })
        .collect();
    vec![
        Texture { name: "hashed", width: 64, height: 64, encoding: Encoding::Srgb, levels: vec![hashed] },
        Texture { name: "gradient", width: 64, height: 64, encoding: Encoding::Srgb, levels: vec![gradient] },
        Texture { name: "tinted_levels", width: 256, height: 256, encoding: Encoding::Unorm, levels: tinted_levels },
        Texture { name: "soft_disc", width: 32, height: 32, encoding: Encoding::Srgb, levels: vec![soft_disc] },
        Texture { name: "white", width: 1, height: 1, encoding: Encoding::Unorm, levels: vec![vec![255; 4]] },
    ]
}

/// The height of the lit ground at a world position: hashed values on a
/// lattice of 2 units, joined by a cubic.
fn height(x: f32, z: f32) -> f32 {
    let (gx, gz) = ((x + 26.0) / 2.0, (z + 26.0) / 2.0);
    let (ix, iz) = (gx.floor(), gz.floor());
    let (fx, fz) = (gx - ix, gz - iz);
    let (sx, sz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let lattice = |i: f32, j: f32| (mix((i as u64) * 31 + (j as u64) * 1_009 + 7) & 0xff) as f32 / 256.0;
    let near = lattice(ix, iz) + (lattice(ix + 1.0, iz) - lattice(ix, iz)) * sx;
    let far = lattice(ix, iz + 1.0) + (lattice(ix + 1.0, iz + 1.0) - lattice(ix, iz + 1.0)) * sx;
    3.0 * (near + (far - near) * sz)
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / length, v[1] / length, v[2] / length]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

const LIT_CAMERA: [f32; 3] = [0.0, 14.0, -14.0];
/// The point the camera looks at: low and near, so that the ground fills the target.
const LIT_TARGET: [f32; 3] = [0.0, 0.0, -2.0];
/// The side of one cell of the height field, which spans 48 units.
const LIT_CELL: f32 = 0.5;

/// A height field of 96 by 96 cells seen from above, projected on
/// the processor so that the three interfaces receive clip-space positions.
fn lit_vertices() -> Vec<LitVertex> {
    let forward = normalized([LIT_TARGET[0] - LIT_CAMERA[0], LIT_TARGET[1] - LIT_CAMERA[1], LIT_TARGET[2] - LIT_CAMERA[2]]);
    let right = normalized(cross([0.0, 1.0, 0.0], forward));
    let up = cross(forward, right);
    let (focal, near, far) = (1.5_f32, 0.5_f32, 64.0_f32);
    let (depth_scale, depth_offset) = (far / (far - near), -near * far / (far - near));
    let vertex = |x: f32, z: f32, shade: f32| {
        let y = height(x, z);
        let offset = 0.125;
        let normal = normalized([height(x - offset, z) - height(x + offset, z), 2.0 * offset, height(x, z - offset) - height(x, z + offset)]);
        let band = if y < 0.75 {
            [0.125, 0.25, 0.75]
        } else if y < 1.5 {
            [0.25, 0.75, 0.25]
        } else if y < 2.25 {
            [0.75, 0.75, 0.25]
        } else {
            [1.0, 1.0, 1.0]
        };
        let relative = [x - LIT_CAMERA[0], y - LIT_CAMERA[1], z - LIT_CAMERA[2]];
        let (view_x, view_y, view_z) = (dot(relative, right), dot(relative, up), dot(relative, forward));
        LitVertex {
            clip: [focal * view_x, focal * view_y, depth_scale * view_z + depth_offset, view_z],
            world: [x, y, z],
            normal,
            albedo: [band[0] * shade, band[1] * shade, band[2] * shade],
        }
    };
    let mut vertices = Vec::with_capacity(LIT_GRID * LIT_GRID * 6);
    for j in 0..LIT_GRID {
        for i in 0..LIT_GRID {
            let (x0, z0) = (i as f32 * LIT_CELL - 24.0, j as f32 * LIT_CELL - 24.0);
            let (x1, z1) = (x0 + LIT_CELL, z0 + LIT_CELL);
            let shade = if (i + j) & 1 == 0 { 1.0 } else { 0.75 };
            vertices.extend_from_slice(&[vertex(x0, z0, shade), vertex(x0, z1, shade), vertex(x1, z0, shade)]);
            vertices.extend_from_slice(&[vertex(x1, z0, shade), vertex(x0, z1, shade), vertex(x1, z1, shade)]);
        }
    }
    vertices
}

fn bytes_of<T>(values: &[T]) -> &[u8] {
    // SAFETY: called on `repr(C)` structures of `f32` without padding and on
    // integers; the slice covers exactly the elements.
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values)) }
}

impl Scene {
    pub fn build() -> Scene {
        let mut vertices = Vec::new();
        let mut draws = Vec::new();
        let mut draw = |vertices: &[Vertex], first: usize, name: &'static str, texture: u32, sampler: u32, blended: bool| {
            draws.push(Draw { name, first_vertex: first as u32, vertex_count: (vertices.len() - first) as u32, texture, sampler, blended, indirect: false });
        };

        // Three overlapping triangles drawn nearest, farthest, middle: the
        // depth test, and edges at arbitrary slopes for the coverage samples.
        let first = vertices.len();
        triangle(&mut vertices, [[8.0, 8.0], [120.0, 24.0], [40.0, 120.0]], 0.25, [1.0, 0.25, 0.25, 1.0]);
        draw(&vertices, first, "triangle_near", TEXTURE_WHITE, SAMPLER_NEAREST, false);
        let first = vertices.len();
        triangle(&mut vertices, [[120.0, 8.0], [104.0, 120.0], [8.0, 72.0]], 0.75, [0.25, 1.0, 0.25, 1.0]);
        draw(&vertices, first, "triangle_far", TEXTURE_WHITE, SAMPLER_NEAREST, false);
        let first = vertices.len();
        triangle(&mut vertices, [[64.0, 4.0], [124.0, 100.0], [4.0, 100.0]], 0.5, [0.25, 0.25, 1.0, 1.0]);
        draw(&vertices, first, "triangle_middle", TEXTURE_WHITE, SAMPLER_NEAREST, false);

        // 12 by 12 texels at 8 pixels each through the nearest filter: every
        // pixel center falls well inside a texel.
        let first = vertices.len();
        let [left, top, right, bottom] = NEAREST_BOUNDS.map(|edge| edge as f32);
        quad(&mut vertices, [left, top, right, bottom], 0.9, [0.0, 0.0, 12.0 / 64.0, 12.0 / 64.0], [1.0, 1.0], [1.0; 4]);
        draw(&vertices, first, "nearest_hashed_texels", TEXTURE_HASHED, SAMPLER_NEAREST, false);

        // A magnification that is no whole number, through the linear filter.
        let first = vertices.len();
        quad(&mut vertices, [4.0, 136.0, 92.0, 188.0], 0.9, [0.0, 0.0, 0.4, 0.25], [1.0, 1.0], [1.0; 4]);
        draw(&vertices, first, "linear_gradient", TEXTURE_GRADIENT, SAMPLER_LINEAR, false);

        // A floor receding from the bottom edge (w of 1) to the top edge (w of
        // 8): the level of detail varies across it under the trilinear filter.
        let first = vertices.len();
        quad(&mut vertices, [8.0, 196.0, 248.0, 248.0], 0.9, [0.0, 16.0, 4.0, 0.0], [8.0, 1.0], [1.0; 4]);
        draw(&vertices, first, "trilinear_floor", TEXTURE_TINTED_LEVELS, SAMPLER_TRILINEAR, false);

        // Three translucent discs, blended over what is there; the third lies
        // between the depths of the triangles.
        let first = vertices.len();
        quad(&mut vertices, [140.0, 116.0, 204.0, 180.0], 0.6, [0.0, 0.0, 1.0, 1.0], [1.0, 1.0], [1.0, 1.0, 1.0, 0.75]);
        draw(&vertices, first, "disc_white", TEXTURE_SOFT_DISC, SAMPLER_LINEAR, true);
        let first = vertices.len();
        quad(&mut vertices, [172.0, 124.0, 236.0, 188.0], 0.6, [0.0, 0.0, 1.0, 1.0], [1.0, 1.0], [1.0, 0.25, 0.25, 0.75]);
        draw(&vertices, first, "disc_red", TEXTURE_SOFT_DISC, SAMPLER_LINEAR, true);
        let first = vertices.len();
        quad(&mut vertices, [72.0, 72.0, 136.0, 136.0], 0.6, [0.0, 0.0, 1.0, 1.0], [1.0, 1.0], [0.25, 0.75, 1.0, 1.0]);
        draw(&vertices, first, "disc_between_triangles", TEXTURE_SOFT_DISC, SAMPLER_LINEAR, true);

        // Sixteen slivers from the center to a square ring, each under two
        // pixels wide at its far end: coverage and its resolve.
        let first = vertices.len();
        let center = [128.25, 127.75];
        for sliver in 0..16 {
            let along = 7.0 + 14.0 * (sliver % 4) as f32;
            let (start, end) = match sliver / 4 {
                0 => ([100.0 + along, 100.0], [101.75 + along, 100.0]),
                1 => ([156.0, 100.0 + along], [156.0, 101.75 + along]),
                2 => ([156.0 - along, 156.0], [154.25 - along, 156.0]),
                _ => ([100.0, 156.0 - along], [100.0, 154.25 - along]),
            };
            let color = if sliver % 2 == 0 { [1.0, 1.0, 1.0, 1.0] } else { [1.0, 0.75, 0.125, 1.0] };
            triangle(&mut vertices, [center, start, end], 0.125, color);
        }
        draw(&vertices, first, "sliver_fan", TEXTURE_WHITE, SAMPLER_NEAREST, false);
        let fan = draws.last_mut().expect("the fan");
        fan.indirect = true;
        let indirect_arguments = [fan.vertex_count, 1, fan.first_vertex, 0];

        let compute_input = (0..COMPUTE_ELEMENTS as u64).map(|index| mix(index ^ 0x00c0_ffee) as u32).collect();
        let pass_through_source = (0..PASS_THROUGH_SIDE * PASS_THROUGH_SIDE)
            .flat_map(|texel| [texel as u8, (texel * 5 + 3) as u8, (255 - (texel & 255)) as u8, (texel * 3 + 1) as u8])
            .collect();
        let lit_constants = [
            [LIT_CAMERA[0], LIT_CAMERA[1], LIT_CAMERA[2], 1.0],
            [0.031_25, 0.031_25, 0.031_25, 0.0],
            [-10.0, 6.0, -10.0, 1.0],
            [10.0, 6.0, -10.0, 1.0],
            [-10.0, 6.0, 10.0, 1.0],
            [10.0, 6.0, 10.0, 1.0],
            [1.0, 0.75, 0.5, 80.0],
            [0.5, 0.75, 1.0, 80.0],
            [0.75, 1.0, 0.5, 80.0],
            [1.0, 0.5, 0.75, 80.0],
        ];
        Scene {
            target: [WIDTH, HEIGHT],
            vertices,
            textures: textures(),
            draws,
            chunks: [0..3, 3..5, 5..7, 7..10],
            indirect_arguments,
            compute_input,
            pass_through_source,
            lit_vertices: lit_vertices(),
            lit_constants,
        }
    }

    pub fn vertex_bytes(&self) -> &[u8] {
        bytes_of(&self.vertices)
    }

    pub fn lit_vertex_bytes(&self) -> &[u8] {
        bytes_of(&self.lit_vertices)
    }

    pub fn lit_constant_bytes(&self) -> &[u8] {
        bytes_of(&self.lit_constants)
    }

    pub fn compute_input_bytes(&self) -> &[u8] {
        bytes_of(&self.compute_input)
    }

    pub fn indirect_argument_bytes(&self) -> &[u8] {
        bytes_of(&self.indirect_arguments)
    }

    /// One hash over every byte a device receives.
    pub fn input_hash(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        hasher.write(self.vertex_bytes());
        for texture in &self.textures {
            hasher.write(&[texture.width as u8, texture.height as u8, texture.encoding as u8]);
            for level in &texture.levels {
                hasher.write(level);
            }
        }
        for draw in &self.draws {
            hasher.write(bytes_of(&[draw.first_vertex, draw.vertex_count, draw.texture, draw.sampler, u32::from(draw.blended), u32::from(draw.indirect)]));
        }
        hasher.write(self.indirect_argument_bytes());
        hasher.write(self.compute_input_bytes());
        hasher.write(&self.pass_through_source);
        hasher.write(self.lit_vertex_bytes());
        hasher.write(self.lit_constant_bytes());
        hasher.finish()
    }

    /// What the compute exercise must write to its storage buffer.
    pub fn compute_expected_buffer(&self) -> Vec<u32> {
        self.compute_input.iter().enumerate().map(|(index, value)| value.wrapping_mul(COMPUTE_BUFFER_MULTIPLIER).wrapping_add(index as u32)).collect()
    }

    /// What the compute exercise must write to its storage texture, row by row.
    pub fn compute_expected_texture(&self) -> Vec<u32> {
        self.compute_input.iter().map(|value| value ^ COMPUTE_TEXTURE_MASK).collect()
    }

    /// The pixels of the nearest-filtered quad that do not carry the bytes of
    /// their texel: sampling, the conversion to linear and back, and the
    /// resolve of a fully covered pixel are exact when this is zero.
    pub fn nearest_mismatches(&self, image: &Image) -> u64 {
        let texels = &self.textures[TEXTURE_HASHED as usize].levels[0];
        let [left, top, right, bottom] = NEAREST_BOUNDS;
        let mut mismatches = 0;
        for y in top..bottom {
            for x in left..right {
                let texel = (((y - top) / NEAREST_PIXELS_PER_TEXEL) * 64 + (x - left) / NEAREST_PIXELS_PER_TEXEL) as usize * 4;
                if *image.pixel(x, y) != texels[texel..texel + 4] {
                    mismatches += 1;
                }
            }
        }
        mismatches
    }
}

/// How two images of one size differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Difference {
    pub differing_pixels: u64,
    pub differing_bytes: u64,
    /// The largest difference of each channel.
    pub largest: [u8; 4],
}

/// An image read back from a device: tightly packed RGBA8 rows, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Repacks rows of `row_pitch` bytes (what a device returns) into tight rows.
    pub fn from_rows(width: u32, height: u32, row_pitch: usize, data: &[u8]) -> Image {
        let tight = width as usize * 4;
        let mut rgba = Vec::with_capacity(tight * height as usize);
        for row in 0..height as usize {
            let start = row * row_pitch;
            rgba.extend_from_slice(&data[start..start + tight]);
        }
        Image { width, height, rgba }
    }

    /// An image of 16-bit float channels shown as 8 bits, values clamped to
    /// 0..1 with no curve applied: a picture for a person, not a measurement.
    pub fn from_half_floats(width: u32, height: u32, halves: &[u8]) -> Image {
        let rgba = halves.as_chunks::<2>().0.iter().map(|pair| (half_to_f32(u16::from_le_bytes(*pair)).clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
        Image { width, height, rgba }
    }

    pub fn hash(&self) -> u64 {
        fnv1a(&self.rgba)
    }

    pub fn pixel(&self, x: u32, y: u32) -> &[u8] {
        let start = (y as usize * self.width as usize + x as usize) * 4;
        &self.rgba[start..start + 4]
    }

    /// The pixels of a rectangle (left, top, right, bottom) that are not the clear color.
    pub fn drawn_pixels(&self, bounds: [u32; 4]) -> u64 {
        let mut drawn = 0;
        for y in bounds[1]..bounds[3] {
            for x in bounds[0]..bounds[2] {
                if self.pixel(x, y) != CLEAR_BYTES {
                    drawn += 1;
                }
            }
        }
        drawn
    }

    /// One hash per block of an 8 by 8 grid, row by row: a coarse map of where
    /// two images differ.
    pub fn block_hashes(&self) -> Vec<u64> {
        let block_w = (self.width / BLOCKS_PER_SIDE) as usize;
        let block_h = (self.height / BLOCKS_PER_SIDE) as usize;
        let row_bytes = self.width as usize * 4;
        let mut hashes = Vec::with_capacity((BLOCKS_PER_SIDE * BLOCKS_PER_SIDE) as usize);
        for by in 0..BLOCKS_PER_SIDE as usize {
            for bx in 0..BLOCKS_PER_SIDE as usize {
                let mut hasher = Fnv1a::new();
                for y in by * block_h..(by + 1) * block_h {
                    let start = y * row_bytes + bx * block_w * 4;
                    hasher.write(&self.rgba[start..start + block_w * 4]);
                }
                hashes.push(hasher.finish());
            }
        }
        hashes
    }

    /// The difference with an image of the same size, or nothing when the sizes differ.
    pub fn difference(&self, other: &Image) -> Option<Difference> {
        if self.width != other.width || self.height != other.height || self.rgba.len() != other.rgba.len() {
            return None;
        }
        let mut difference = Difference { differing_pixels: 0, differing_bytes: 0, largest: [0; 4] };
        for (a, b) in self.rgba.as_chunks::<4>().0.iter().zip(other.rgba.as_chunks::<4>().0) {
            if a == b {
                continue;
            }
            difference.differing_pixels += 1;
            for channel in 0..4 {
                let distance = a[channel].abs_diff(b[channel]);
                difference.differing_bytes += u64::from(distance != 0);
                difference.largest[channel] = difference.largest[channel].max(distance);
            }
        }
        Some(difference)
    }

    /// The image as a PNG file: RGBA8, one IDAT chunk holding a zlib stream of
    /// stored (uncompressed) deflate blocks, so that the writer needs no
    /// compressor and the bytes are a pure function of the pixels.
    pub fn png(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity((self.width as usize * 4 + 1) * self.height as usize);
        for row in self.rgba.chunks_exact(self.width as usize * 4) {
            raw.push(0); // filter type: none
            raw.extend_from_slice(row);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&PNG_SIGNATURE);
        let mut header = Vec::new();
        header.extend_from_slice(&self.width.to_be_bytes());
        header.extend_from_slice(&self.height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA, deflate, no filter, no interlace
        png_chunk(&mut out, b"IHDR", &header);
        png_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
        png_chunk(&mut out, b"IEND", &[]);
        out
    }

    /// Reads back a PNG file this writer produced: stored blocks, no filter.
    pub fn from_png(bytes: &[u8]) -> Result<Image, String> {
        if bytes.get(..8) != Some(&PNG_SIGNATURE[..]) {
            return Err("not a PNG file".to_string());
        }
        let (mut width, mut height, mut stream) = (0_u32, 0_u32, Vec::new());
        let mut at = 8;
        while at + 12 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("four bytes")) as usize;
            let data = bytes.get(at + 8..at + 8 + length).ok_or("a chunk runs past the end of the file")?;
            match &bytes[at + 4..at + 8] {
                b"IHDR" if length == 13 => {
                    width = u32::from_be_bytes(data[0..4].try_into().expect("four bytes"));
                    height = u32::from_be_bytes(data[4..8].try_into().expect("four bytes"));
                    if data[8..13] != [8, 6, 0, 0, 0] {
                        return Err("not an 8-bit RGBA image without interlace".to_string());
                    }
                }
                b"IDAT" => stream.extend_from_slice(data),
                _ => {}
            }
            at += 12 + length;
        }
        let mut raw = Vec::new();
        let mut at = 2;
        loop {
            let header = *stream.get(at).ok_or("the deflate stream ends early")?;
            if header & 0b110 != 0 {
                return Err("a compressed deflate block: not a file this spike wrote".to_string());
            }
            let length = u16::from_le_bytes(stream.get(at + 1..at + 3).ok_or("the deflate stream ends early")?.try_into().expect("two bytes")) as usize;
            raw.extend_from_slice(stream.get(at + 5..at + 5 + length).ok_or("a stored block runs past the stream")?);
            at += 5 + length;
            if header & 1 != 0 {
                break;
            }
        }
        let row = width as usize * 4 + 1;
        if width == 0 || raw.len() != row * height as usize {
            return Err(format!("{} bytes of pixels for a {width} by {height} image", raw.len()));
        }
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for line in raw.chunks_exact(row) {
            if line[0] != 0 {
                return Err("a filtered row: not a file this spike wrote".to_string());
            }
            rgba.extend_from_slice(&line[1..]);
        }
        Ok(Image { width, height, rgba })
    }
}

/// The value of an IEEE 754 half-precision float.
pub fn half_to_f32(half: u16) -> f32 {
    let sign = if half & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((half >> 10) & 0x1f);
    let fraction = f32::from(half & 0x3ff);
    let magnitude = match exponent {
        0 => fraction / 1024.0 / 16_384.0,
        31 => {
            if fraction == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1.0 + fraction / 1024.0) * 2.0_f32.powi(exponent - 15),
    };
    sign * magnitude
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream whose deflate blocks are stored: a 2-byte header, blocks of
/// at most 65 535 bytes each with its length and complement, an Adler-32 trailer.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = data.chunks(65_535).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        out.push(u8::from(last));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hash of every byte a device receives, measured once: a cell whose
    /// inputs differ fails here before it renders anything.
    const INPUT_HASH: u64 = 0x6dbd_9f47_8900_acc5;

    #[test]
    fn the_inputs_are_the_same_bytes_everywhere() {
        let scene = Scene::build();
        assert_eq!(scene.input_hash(), Scene::build().input_hash());
        assert_eq!(scene.input_hash(), INPUT_HASH, "measured 0x{:016x}", scene.input_hash());
    }

    #[test]
    fn the_scene_has_the_shape_the_interfaces_are_written_against() {
        let scene = Scene::build();
        assert_eq!(std::mem::size_of::<Vertex>(), 40);
        assert_eq!(std::mem::size_of::<LitVertex>(), 52);
        assert_eq!(scene.textures.len(), TEXTURE_COUNT);
        assert_eq!(scene.textures[TEXTURE_TINTED_LEVELS as usize].levels.len(), 9);
        for texture in &scene.textures {
            for (level, texels) in texture.levels.iter().enumerate() {
                assert_eq!(texels.len(), ((texture.width >> level) * (texture.height >> level) * 4) as usize, "{} level {level}", texture.name);
            }
        }
        assert_eq!(scene.draws.len(), 10);
        let covered: Vec<usize> = scene.chunks.iter().flat_map(|chunk| chunk.clone()).collect();
        assert_eq!(covered, (0..scene.draws.len()).collect::<Vec<_>>(), "the chunks cover every draw once, in order");
        let mut next = 0;
        for draw in &scene.draws {
            assert_eq!(draw.first_vertex, next, "{}", draw.name);
            assert_eq!(draw.vertex_count % 3, 0);
            next += draw.vertex_count;
        }
        assert_eq!(next as usize, scene.vertices.len());
        assert_eq!(scene.draws.iter().filter(|draw| draw.indirect).count(), 1);
        assert_eq!(scene.indirect_arguments, [48, 1, scene.draws[9].first_vertex, 0]);
        assert_eq!(scene.compute_input.len(), COMPUTE_ELEMENTS);
        assert_eq!(scene.lit_vertices.len(), LIT_GRID * LIT_GRID * 6);
        for vertex in &scene.lit_vertices {
            let length = dot(vertex.normal, vertex.normal);
            assert!((length - 1.0).abs() < 1e-5, "a unit normal: {length}");
            assert!(vertex.clip[3] > 0.5, "in front of the near plane: {:?}", vertex.clip);
            assert!(vertex.world[1] >= 0.0 && vertex.world[1] <= 3.0);
        }
    }

    #[test]
    fn every_byte_value_occurs_in_every_channel_of_the_pass_through_source() {
        let scene = Scene::build();
        assert_eq!(scene.pass_through_source.len(), (PASS_THROUGH_SIDE * PASS_THROUGH_SIDE * 4) as usize);
        for channel in 0..4 {
            let mut seen = [false; 256];
            for texel in scene.pass_through_source.as_chunks::<4>().0 {
                seen[texel[channel] as usize] = true;
            }
            assert!(seen.iter().all(|&value| value), "channel {channel}");
        }
    }

    #[test]
    fn the_compute_expectations_follow_the_stated_arithmetic() {
        let scene = Scene::build();
        let buffer = scene.compute_expected_buffer();
        let texture = scene.compute_expected_texture();
        assert_eq!(buffer[5], scene.compute_input[5].wrapping_mul(COMPUTE_BUFFER_MULTIPLIER).wrapping_add(5));
        assert_eq!(texture[4095], scene.compute_input[4095] ^ COMPUTE_TEXTURE_MASK);
    }

    #[test]
    fn images_hash_by_block_compare_and_survive_the_png_round_trip() {
        let mut image = Image { width: 256, height: 256, rgba: CLEAR_BYTES.repeat(256 * 256) };
        assert_eq!(image.drawn_pixels([0, 0, 256, 256]), 0);
        let blank = image.clone();
        let pixel = (3 * 256 + 40) * 4;
        image.rgba[pixel..pixel + 4].copy_from_slice(&[255, 60, 64, 255]);
        assert_eq!(image.drawn_pixels([32, 0, 64, 32]), 1);
        assert_eq!(image.drawn_pixels([0, 0, 32, 32]), 0);
        let (lit, dark) = (image.block_hashes(), blank.block_hashes());
        assert_eq!(lit.iter().zip(&dark).filter(|(a, b)| a != b).count(), 1);
        assert_eq!(image.difference(&blank), Some(Difference { differing_pixels: 1, differing_bytes: 2, largest: [191, 4, 0, 0] }));
        assert_eq!(image.difference(&Image { width: 1, height: 1, rgba: vec![0; 4] }), None);
        assert_eq!(Image::from_png(&image.png()), Ok(image));
        let repacked = Image::from_rows(2, 2, 12, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9, 10, 11, 12, 13, 14, 15, 16, 17, 9, 9, 9, 9]);
        assert_eq!(repacked.rgba, vec![1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17]);
        assert!(Image::from_png(b"not a png").is_err());
    }

    #[test]
    fn the_png_writer_and_the_half_float_reader_are_well_formed() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(zlib_stored(&[1, 2, 3]), vec![0x78, 0x01, 1, 3, 0, 0xfc, 0xff, 1, 2, 3, 0x00, 0x0d, 0x00, 0x07]);
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0x0001), 1.0 / 16_777_216.0);
        assert_eq!(half_to_f32(0), 0.0);
        let shown = Image::from_half_floats(1, 1, &[0x00, 0x3c, 0x00, 0x38, 0x00, 0x00, 0x00, 0x40]);
        assert_eq!(shown.rgba, vec![255, 128, 0, 255]);
    }

    #[test]
    fn a_perfect_nearest_quad_has_no_mismatch() {
        let scene = Scene::build();
        let mut image = Image { width: WIDTH, height: HEIGHT, rgba: CLEAR_BYTES.repeat((WIDTH * HEIGHT) as usize) };
        let texels = &scene.textures[TEXTURE_HASHED as usize].levels[0];
        let [left, top, right, bottom] = NEAREST_BOUNDS;
        for y in top..bottom {
            for x in left..right {
                let texel = (((y - top) / 8) * 64 + (x - left) / 8) as usize * 4;
                let pixel = ((y * WIDTH + x) * 4) as usize;
                image.rgba[pixel..pixel + 4].copy_from_slice(&texels[texel..texel + 4]);
            }
        }
        assert_eq!(scene.nearest_mismatches(&image), 0);
        image.rgba[((top * WIDTH + left) * 4) as usize] ^= 1;
        assert_eq!(scene.nearest_mismatches(&image), 1);
    }
}
