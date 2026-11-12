// Direct3D 12 shader of the compute exercise, compiled at run time by the
// system's D3DCompile to the 5.1 target.
//
// One invocation per element of a 64 by 64 square: it reads its input value
// from a structured buffer, writes the value times 2654435761 plus its index
// (32-bit wrapping) to a second structured buffer, and the value with the
// bits of 0x9e3779b9 flipped to its texel of a storage texture of one 32-bit
// unsigned channel.

StructuredBuffer<uint> input_values : register(t0);
RWStructuredBuffer<uint> output_values : register(u0);
RWTexture2D<uint> output_texture : register(u1);

[numthreads(8, 8, 1)]
void cs_main(uint3 invocation : SV_DispatchThreadID)
{
    uint index = invocation.y * 64 + invocation.x;
    uint value = input_values[index];
    output_values[index] = value * 2654435761u + index;
    output_texture[invocation.xy] = value ^ 0x9e3779b9u;
}
