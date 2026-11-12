// Direct3D 12 shaders of the filtering and blending scene, compiled at run
// time by the system's D3DCompile to the 5.1 targets.
//
// Clip space has y up and a depth range of 0 to 1 in Direct3D, as the scene
// is authored, so the vertex shader passes the position through unchanged and
// interpolates the texture coordinate and the color with perspective. The
// pixel shader samples the texture and the sampler the draw names: their
// indexes arrive as two root constants, the textures as a table of five
// shader resource views and the samplers as a table of three samplers.

cbuffer DrawConstants : register(b0)
{
    uint texture_index;
    uint sampler_index;
};

Texture2D scene_textures[5] : register(t0);
SamplerState scene_samplers[3] : register(s0);

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
    output.position = input.position;
    output.uv = input.uv;
    output.color = input.color;
    return output;
}

float4 ps_main(PixelInput input) : SV_Target
{
    return scene_textures[texture_index].Sample(scene_samplers[sampler_index], input.uv) * input.color;
}
