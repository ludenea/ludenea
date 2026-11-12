// Direct3D 12 shaders of the pass-through exercise, compiled at run time by
// the system's D3DCompile to the 5.1 targets.
//
// One triangle covers the viewport (one half of the target); the texture
// coordinate runs from 0 at the viewport's top-left corner to 1 at its
// bottom-right corner, so that every pixel center falls on the center of the
// texel of the same position. The pixel shader returns the texel sampled with
// the nearest filter: through a plain view and written through a plain view,
// its bytes pass unchanged; through the sRGB views, they are converted to
// linear on read and back on write.

Texture2D source_texture : register(t0);
SamplerState nearest_sampler : register(s0);

struct PixelInput
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

PixelInput vs_main(uint vertex_index : SV_VertexID)
{
    // The corners (0, 0), (2, 0) and (0, 2) of the texture space.
    float2 corner = float2((vertex_index << 1) & 2, vertex_index & 2);
    PixelInput output;
    output.position = float4(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    output.uv = corner;
    return output;
}

float4 ps_main(PixelInput input) : SV_Target
{
    return source_texture.Sample(nearest_sampler, input.uv);
}
