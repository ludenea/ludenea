//! The visual model at the spike's size: the assets a frame names (sprites,
//! meshes with their levels of detail, a tile set), the geometry of each, and
//! the geometry of a tile block. Throwaway, never product code.
//!
//! Every asset is built with integer arithmetic or with operations IEEE 754
//! specifies exactly, so that the parts of the presentation payloads that
//! feed content (bounds, levels, texture coordinates) are the same bytes on
//! every machine, as one producer would distribute them.

use render_device_baseline::scene::{SAMPLER_LINEAR, SAMPLER_NEAREST, SAMPLER_TRILINEAR, TEXTURE_GRADIENT, TEXTURE_HASHED, TEXTURE_SOFT_DISC, TEXTURE_TINTED_LEVELS, TEXTURE_WHITE, Vertex};

use crate::slices::{TILE_EMPTY, mix};

/// A range of the shared geometry buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub first_vertex: u32,
    pub vertex_count: u32,
}

/// A sprite asset: its texture, its sampler and whether it is blended.
#[derive(Clone, Copy)]
pub struct Sprite {
    pub texture: u32,
    pub sampler: u32,
    pub blended: bool,
}

pub const SPRITES: [Sprite; 3] = [
    Sprite { texture: TEXTURE_GRADIENT, sampler: SAMPLER_LINEAR, blended: false },
    Sprite { texture: TEXTURE_TINTED_LEVELS, sampler: SAMPLER_TRILINEAR, blended: false },
    Sprite { texture: TEXTURE_SOFT_DISC, sampler: SAMPLER_LINEAR, blended: true },
];

pub const MESH_COUNT: usize = 2;
pub const MESH_LEVELS: usize = 3;
pub const MESH_TEXTURE: u32 = TEXTURE_WHITE;
pub const MESH_SAMPLER: u32 = SAMPLER_NEAREST;
pub const TILE_TEXTURE: u32 = TEXTURE_HASHED;
pub const TILE_SAMPLER: u32 = SAMPLER_NEAREST;

/// The size in world units of an entity of each sprite asset (the asset
/// follows the mass), and of a mesh.
pub const SPRITE_SIZES: [f32; 3] = [6.0, 8.0, 12.0];
pub const MESH_SIZES: [f32; MESH_COUNT] = [10.0, 14.0];
/// The radius of the sphere that bounds a mesh or a sprite of size one.
pub const BOUND_RADIUS: f32 = 1.25;

/// The geometry every world shares, in one vertex buffer: the sprite's quad
/// and each mesh at each level of detail.
pub struct SharedGeometry {
    pub vertices: Vec<Vertex>,
    pub sprite: Range,
    pub meshes: [[Range; MESH_LEVELS]; MESH_COUNT],
}

fn vertex(position: [f32; 3], uv: [f32; 2], color: [f32; 3]) -> Vertex {
    Vertex { position: [position[0], position[1], position[2], 1.0], uv, color: [color[0], color[1], color[2], 1.0] }
}

/// A quad as two triangles from four corners given in order around it.
fn quad(vertices: &mut Vec<Vertex>, corners: [[f32; 3]; 4], color: [f32; 3]) {
    let uvs = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
    for index in [0, 1, 3, 3, 1, 2] {
        vertices.push(vertex(corners[index], uvs[index], color));
    }
}

fn triangle(vertices: &mut Vec<Vertex>, corners: [[f32; 3]; 3], color: [f32; 3]) {
    let uvs = [[0.0, 1.0], [1.0, 1.0], [0.5, 0.0]];
    for index in 0..3 {
        vertices.push(vertex(corners[index], uvs[index], color));
    }
}

const SIDE_COLORS: [[f32; 3]; 4] = [[0.75, 0.25, 0.25], [0.25, 0.75, 0.25], [0.25, 0.25, 0.75], [0.75, 0.75, 0.25]];
const TOP_COLOR: [f32; 3] = [0.875, 0.875, 0.875];
const BASE_COLOR: [f32; 3] = [0.125, 0.125, 0.125];
/// The corners of the unit square around the origin, in order around it.
const SQUARE: [[f32; 2]; 4] = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];

/// A box on the ground: every face, then the sides and the top, then the top alone.
fn box_level(vertices: &mut Vec<Vertex>, level: usize) {
    let corner = |index: usize, z: f32| [SQUARE[index % 4][0], SQUARE[index % 4][1], z];
    quad(vertices, [corner(3, 1.0), corner(0, 1.0), corner(1, 1.0), corner(2, 1.0)], TOP_COLOR);
    if level < 2 {
        for side in 0..4 {
            quad(vertices, [corner(side, 1.0), corner(side, 0.0), corner(side + 1, 0.0), corner(side + 1, 1.0)], SIDE_COLORS[side]);
        }
    }
    if level < 1 {
        quad(vertices, [corner(0, 0.0), corner(3, 0.0), corner(2, 0.0), corner(1, 0.0)], BASE_COLOR);
    }
}

/// A pyramid on the ground: four sides and the base, then the sides, then two sides.
fn pyramid_level(vertices: &mut Vec<Vertex>, level: usize) {
    let corner = |index: usize| [SQUARE[index % 4][0], SQUARE[index % 4][1], 0.0];
    let sides = if level < 2 { 4 } else { 2 };
    for side in 0..sides {
        triangle(vertices, [corner(side), corner(side + 1), [0.0, 0.0, 1.0]], SIDE_COLORS[side]);
    }
    if level < 1 {
        quad(vertices, [corner(0), corner(3), corner(2), corner(1)], BASE_COLOR);
    }
}

impl SharedGeometry {
    pub fn build() -> SharedGeometry {
        let mut vertices = Vec::new();
        let range = |vertices: &mut Vec<Vertex>, fill: &dyn Fn(&mut Vec<Vertex>)| {
            let first_vertex = vertices.len() as u32;
            fill(vertices);
            Range { first_vertex, vertex_count: vertices.len() as u32 - first_vertex }
        };
        let sprite = range(&mut vertices, &|vertices| quad(vertices, [[-0.5, 0.5, 0.0], [-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.5, 0.5, 0.0]], [1.0; 3]));
        let mut meshes = [[Range { first_vertex: 0, vertex_count: 0 }; MESH_LEVELS]; MESH_COUNT];
        for level in 0..MESH_LEVELS {
            meshes[0][level] = range(&mut vertices, &|vertices| box_level(vertices, level));
            meshes[1][level] = range(&mut vertices, &|vertices| pyramid_level(vertices, level));
        }
        SharedGeometry { vertices, sprite, meshes }
    }
}

/// The size of a tile layer's cell in world units: 128 cells cover the world's side.
pub const CELL_SIZE: f32 = 8.0;
/// The tile set: 16 tiles in a grid of 4 by 4 over the hashed texture.
const TILES_PER_SIDE: u32 = 4;

/// The geometry of one block of a tile layer, in the layer's space: two
/// triangles per cell that holds a tile, cell by cell, row by row. The
/// coordinates are sums and products of small integers and of powers of two.
pub fn block_geometry(cells: &[u16], block_side: u32, blocks_per_side: u32, coordinate: u32, layer: u32) -> Vec<Vertex> {
    let (block_x, block_y) = (coordinate % blocks_per_side, coordinate / blocks_per_side);
    let mut vertices = Vec::with_capacity(cells.len() * 6);
    for (index, &cell) in cells.iter().enumerate() {
        let tile = cell & 0x3fff;
        if tile == TILE_EMPTY {
            continue;
        }
        let (cell_x, cell_y) = (block_x * block_side + index as u32 % block_side, block_y * block_side + index as u32 / block_side);
        let (left, bottom) = (cell_x as f32 * CELL_SIZE, cell_y as f32 * CELL_SIZE);
        let (right, top) = (left + CELL_SIZE, bottom + CELL_SIZE);
        let tile_side = 1.0 / TILES_PER_SIDE as f32;
        let (mut u0, mut v0) = ((u32::from(tile) % TILES_PER_SIDE) as f32 * tile_side, (u32::from(tile) / TILES_PER_SIDE) as f32 * tile_side);
        let (mut u1, mut v1) = (u0 + tile_side, v0 + tile_side);
        if cell & 0x4000 != 0 {
            std::mem::swap(&mut u0, &mut u1);
        }
        if cell & 0x8000 != 0 {
            std::mem::swap(&mut v0, &mut v1);
        }
        // A shade per cell, so that a layer is not flat: one of four levels.
        let shade = 0.625 + 0.125 * (mix(u64::from(cell_y) << 32 | u64::from(cell_x) | u64::from(layer) << 60) % 4) as f32;
        let alpha = if layer == 0 { 1.0 } else { 0.75 };
        let corner = |x: f32, y: f32, u: f32, v: f32| Vertex { position: [x, y, 0.0, 1.0], uv: [u, v], color: [shade, shade, shade, alpha] };
        let (top_left, bottom_left, bottom_right, top_right) = (corner(left, top, u0, v0), corner(left, bottom, u0, v1), corner(right, bottom, u1, v1), corner(right, top, u1, v0));
        vertices.extend_from_slice(&[top_left, bottom_left, top_right, top_right, bottom_left, bottom_right]);
    }
    vertices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_geometry_holds_the_sprite_and_each_mesh_at_three_levels() {
        let geometry = SharedGeometry::build();
        assert_eq!(geometry.sprite.vertex_count, 6);
        assert_eq!(geometry.meshes[0].map(|range| range.vertex_count), [36, 30, 6]);
        assert_eq!(geometry.meshes[1].map(|range| range.vertex_count), [18, 12, 6]);
        assert_eq!(geometry.vertices.len(), 6 + 36 + 30 + 6 + 18 + 12 + 6);
    }

    #[test]
    fn a_block_has_two_triangles_for_each_cell_that_holds_a_tile() {
        let cells = [0, TILE_EMPTY, 5 | 0x4000, 15 | 0x8000];
        let vertices = block_geometry(&cells, 2, 4, 5, 0);
        assert_eq!(vertices.len(), 18);
        assert_eq!(vertices[0].position, [16.0, 24.0, 0.0, 1.0]);
    }
}
