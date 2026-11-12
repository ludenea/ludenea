// The compute exercise of the device baseline spike in Slang: for the
// invocation at (x, y), with i = y * 64 + x and v the input at i, writes
// v * 2654435761 + i (wrapping) to the output buffer and v ^ 0x9e3779b9 to
// the texel (x, y) of the storage texture.
[[vk::binding(0, 0)]]
StructuredBuffer<uint> source_values : register(t0);
[[vk::binding(1, 0)]]
RWStructuredBuffer<uint> results : register(u0);
[[vk::binding(2, 0)]]
RWTexture2D<uint> result_texture : register(u1);

[shader("compute")]
[numthreads(8, 8, 1)]
void compute_main(uint3 invocation : SV_DispatchThreadID)
{
    uint index = invocation.y * 64u + invocation.x;
    uint value = source_values[index];
    results[index] = value * 2654435761u + index;
    result_texture[invocation.xy] = value ^ 0x9e3779b9u;
}
