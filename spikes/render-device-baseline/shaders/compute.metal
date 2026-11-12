// Metal kernel of the compute exercise, compiled at run time by the system's
// Metal compiler.
//
// One thread per element of a 64 by 64 square, dispatched as 8 by 8
// threadgroups of 8 by 8 threads: it reads its input value from a buffer,
// writes the value times 2654435761 plus its index (32-bit wrapping) to a
// second buffer, and the value with the bits of 0x9e3779b9 flipped to its
// texel of a texture of one 32-bit unsigned channel opened for writing.
#include <metal_stdlib>
using namespace metal;

kernel void compute_kernel(uint2 cell [[thread_position_in_grid]],
                           const device uint* input_values [[buffer(0)]],
                           device uint* output_values [[buffer(1)]],
                           texture2d<uint, access::write> output_texture [[texture(0)]]) {
    uint index = cell.y * 64u + cell.x;
    uint value = input_values[index];
    output_values[index] = value * 2654435761u + index;
    output_texture.write(uint4(value ^ 0x9e3779b9u, 0u, 0u, 0u), cell);
}
