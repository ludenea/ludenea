//! The fixed scene, the frame, its hashes and the PNG writer. Throwaway, never
//! product code.
//!
//! The scene is what an extraction of the reference workload hands a renderer:
//! one triangle per alive entity of the small population after 100 sequential
//! ticks, placed at the entity's position and colored by its mass. The state
//! hash of that tick is a reference value, so the vertex buffer is identical on
//! every cell by construction, and the three platform renderers draw the same
//! bytes.

use std::time::Instant;

use reference_workload::{Grid, Parameters, SEQUENTIAL_PHASES, Size, State, Timing, fnv1a, run_sequential, tick};

pub const WIDTH: u32 = 256;
pub const HEIGHT: u32 = 256;
pub const SCENE_TICKS: u32 = 100;
pub const SCENE_SIZE: Size = Size::Small;
/// Six pixels of a 128-pixel half-width: exact in binary.
pub const TRIANGLE_RADIUS: f32 = 0.046_875;
/// The blocks of the block hashes: an 8 by 8 grid of 32-pixel squares.
pub const BLOCKS_PER_SIDE: u32 = 8;

/// One vertex, as the three renderers read it: a clip-space position with y
/// up, then a flat color.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

pub const VERTEX_STRIDE: u32 = std::mem::size_of::<Vertex>() as u32;

pub struct Scene {
    pub vertices: Vec<Vertex>,
    pub entities: usize,
    pub alive: usize,
    pub tick: u32,
    pub state_hash: u64,
    pub matches_sequential_reference: bool,
    pub build_us: f64,
}

impl Scene {
    /// The small population stepped 100 ticks by the sequential systems of the
    /// reference workload, then one triangle per alive entity.
    pub fn build() -> Scene {
        let started = Instant::now();
        let parameters = Parameters::new(SCENE_SIZE, SCENE_TICKS, false);
        let mut state = State::initialize(&parameters);
        let mut grid = Grid::new();
        let mut timing = Timing::new(&SEQUENTIAL_PHASES);
        for t in 1..=SCENE_TICKS {
            tick(&mut state, &mut grid, t, false, &mut timing);
        }
        let state_hash = state.hash();
        let reference = run_sequential(&parameters);
        let mut vertices = Vec::with_capacity(state.len() * 3);
        let mut alive = 0;
        for slot in 0..state.len() {
            if !state.alive[slot] {
                continue;
            }
            alive += 1;
            // [0, 1024) to [-1, 1): a division by a power of two, then one subtraction.
            let cx = state.x[slot] / 512.0 - 1.0;
            let cy = state.y[slot] / 512.0 - 1.0;
            let mass = state.mass[slot];
            let (r, g, b) = if mass == 1.0 {
                (1.0, 0.0, 0.0)
            } else if mass == 2.0 {
                (0.0, 1.0, 0.0)
            } else {
                (0.0, 0.0, 1.0)
            };
            let radius = TRIANGLE_RADIUS;
            vertices.push(Vertex { x: cx, y: cy + radius, r, g, b });
            vertices.push(Vertex { x: cx - 0.875 * radius, y: cy - 0.5 * radius, r, g, b });
            vertices.push(Vertex { x: cx + 0.875 * radius, y: cy - 0.5 * radius, r, g, b });
        }
        Scene {
            vertices,
            entities: state.len(),
            alive,
            tick: SCENE_TICKS,
            state_hash,
            matches_sequential_reference: state_hash == reference.final_hash(),
            build_us: started.elapsed().as_secs_f64() * 1_000_000.0,
        }
    }

    pub fn vertex_bytes(&self) -> &[u8] {
        // SAFETY: `Vertex` is `repr(C)` of five `f32` with no padding; the slice
        // covers exactly the vector's elements.
        unsafe { std::slice::from_raw_parts(self.vertices.as_ptr() as *const u8, self.vertices.len() * VERTEX_STRIDE as usize) }
    }

    pub fn triangles(&self) -> usize {
        self.vertices.len() / 3
    }
}

/// A frame read back from a renderer: tightly packed RGBA8 rows, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// Repacks rows of `row_pitch` bytes (what a device returns) into tight rows.
    pub fn from_rows(width: u32, height: u32, row_pitch: usize, data: &[u8]) -> Frame {
        let tight = width as usize * 4;
        let mut rgba = Vec::with_capacity(tight * height as usize);
        for row in 0..height as usize {
            let start = row * row_pitch;
            rgba.extend_from_slice(&data[start..start + tight]);
        }
        Frame { width, height, rgba }
    }

    pub fn hash(&self) -> u64 {
        fnv1a(&self.rgba)
    }

    /// The pixels that are not the clear color (opaque black).
    pub fn lit_pixels(&self) -> usize {
        self.rgba.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0 || pixel[3] != 255).count()
    }

    /// One hash per 32-pixel block, row by row: a coarse map of where two
    /// frames differ.
    pub fn block_hashes(&self) -> Vec<u64> {
        let block_w = (self.width / BLOCKS_PER_SIDE) as usize;
        let block_h = (self.height / BLOCKS_PER_SIDE) as usize;
        let row_bytes = self.width as usize * 4;
        let mut hashes = Vec::with_capacity((BLOCKS_PER_SIDE * BLOCKS_PER_SIDE) as usize);
        for by in 0..BLOCKS_PER_SIDE as usize {
            for bx in 0..BLOCKS_PER_SIDE as usize {
                let mut hasher = reference_workload::Fnv1a::new();
                for y in by * block_h..(by + 1) * block_h {
                    let start = y * row_bytes + bx * block_w * 4;
                    hasher.write(&self.rgba[start..start + block_w * 4]);
                }
                hashes.push(hasher.finish());
            }
        }
        hashes
    }

    /// The frame as a PNG file: RGBA8, one IDAT chunk holding a zlib stream of
    /// stored (uncompressed) deflate blocks, so that the writer needs no
    /// compressor and the bytes are a pure function of the pixels.
    pub fn png(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity((self.width as usize * 4 + 1) * self.height as usize);
        for row in self.rgba.chunks_exact(self.width as usize * 4) {
            raw.push(0); // filter type: none
            raw.extend_from_slice(row);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let mut header = Vec::new();
        header.extend_from_slice(&self.width.to_be_bytes());
        header.extend_from_slice(&self.height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA, deflate, no filter, no interlace
        png_chunk(&mut out, b"IHDR", &header);
        png_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
        png_chunk(&mut out, b"IEND", &[]);
        out
    }
}

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

    #[test]
    fn the_scene_is_a_pure_function_of_the_reference_workload() {
        let a = Scene::build();
        let b = Scene::build();
        assert!(a.matches_sequential_reference);
        assert_eq!(a.vertices, b.vertices);
        assert_eq!(a.state_hash, b.state_hash);
        assert_eq!(a.vertices.len(), a.alive * 3);
        assert!(a.alive > 900 && a.alive <= a.entities, "{} alive of {}", a.alive, a.entities);
        assert_eq!(a.vertex_bytes().len(), a.vertices.len() * 20);
        for vertex in &a.vertices {
            assert!(vertex.x > -1.1 && vertex.x < 1.1 && vertex.y > -1.1 && vertex.y < 1.1, "{vertex:?}");
            assert_eq!(vertex.r + vertex.g + vertex.b, 1.0, "one primary color per mass");
        }
    }

    #[test]
    fn frames_hash_block_by_block_and_count_lit_pixels() {
        let mut frame = Frame { width: 256, height: 256, rgba: vec![0, 0, 0, 255].repeat(256 * 256) };
        assert_eq!(frame.lit_pixels(), 0);
        let blank = frame.block_hashes();
        assert_eq!(blank.len(), 64);
        assert!(blank.iter().all(|&h| h == blank[0]));
        // One red pixel in the second block of the first row of blocks.
        let pixel = (3 * 256 + 40) * 4;
        frame.rgba[pixel..pixel + 4].copy_from_slice(&[255, 0, 0, 255]);
        assert_eq!(frame.lit_pixels(), 1);
        let lit = frame.block_hashes();
        assert_ne!(lit[1], blank[1]);
        assert_eq!(lit.iter().zip(&blank).filter(|(a, b)| a != b).count(), 1);
        let repacked = Frame::from_rows(2, 2, 12, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9, 10, 11, 12, 13, 14, 15, 16, 17, 9, 9, 9, 9]);
        assert_eq!(repacked.rgba, vec![1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17]);
    }

    #[test]
    fn the_png_writer_produces_a_well_formed_file() {
        let frame = Frame { width: 3, height: 2, rgba: (0..24).collect() };
        let png = frame.png();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 3);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 2);
        // The IHDR chunk's CRC covers its type and its 13 data bytes.
        assert_eq!(u32::from_be_bytes(png[29..33].try_into().unwrap()), crc32(&png[12..29]));
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let stream = zlib_stored(&[1, 2, 3]);
        assert_eq!(stream, vec![0x78, 0x01, 1, 3, 0, 0xfc, 0xff, 1, 2, 3, 0x00, 0x0d, 0x00, 0x07]);
    }
}
