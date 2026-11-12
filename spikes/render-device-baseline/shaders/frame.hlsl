// Direct3D 12 shaders of the frame device, compiled at run time by the
// system's D3DCompile to the 5.1 targets, with TINT_VARIANT defined as 0 or 1.
//
// The draw's constants are 22 root constants visible to both stages: the
// transform stored column by column, the tint, and the indexes of the texture
// and of the sampler. The five textures of the pass arrive as a table of five
// shader resource views and the samplers as a table of three samplers.
//
// Clip space has y up and a depth range of 0 to 1 in Direct3D, as the frames
// are authored, so the vertex shader writes the product of the transform and
// the position unchanged, and interpolates the texture coordinate and the
// color with perspective.

#ifndef TINT_VARIANT
#define TINT_VARIANT 0
#endif

cbuffer DrawConstants : register(b0)
{
    // Column by column: the element of column c and row r at index c * 4 + r.
    column_major float4x4 transform;
    float4 tint;
    uint texture_index;
    uint sampler_index;
};

Texture2D frame_textures[5] : register(t0);
SamplerState frame_samplers[3] : register(s0);

struct VertexInput
{
    float4 position : POSITION;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
};

struct PixelInput
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
};

PixelInput vs_main(VertexInput input)
{
    PixelInput output;
    output.position = mul(transform, input.position);
    output.uv = input.uv;
    output.color = input.color;
    return output;
}

float4 ps_main(PixelInput input) : SV_Target
{
    float4 color = frame_textures[texture_index].Sample(frame_samplers[sampler_index], input.uv) * input.color * tint;
#if TINT_VARIANT == 1
    color.rgb *= float3(0.5, 1.0, 0.25);
#endif
    return color;
}
