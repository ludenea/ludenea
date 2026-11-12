// The filtering and blending scene of the device baseline spike in Slang,
// with the registers, sets and bindings its three native interfaces expect:
// the per-draw indexes as two root constants at b0 on Direct3D 12 and as push
// constants on Vulkan, the five textures as one array and the three samplers
// as another. Clip space has y up as the scene is authored; the SPIR-V is
// compiled with the option that negates y for Vulkan.
struct DrawIndexes
{
    uint texture_index;
    uint sampler_index;
};

[[vk::push_constant]]
ConstantBuffer<DrawIndexes> draw : register(b0);
[[vk::binding(0, 0)]]
Texture2D scene_textures[5] : register(t0);
[[vk::binding(1, 0)]]
SamplerState scene_samplers[3] : register(s0);

struct SceneVertex
{
    float4 position : POSITION;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
};

struct SceneVaryings
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
};

[shader("vertex")]
SceneVaryings scene_vertex(SceneVertex input)
{
    SceneVaryings output;
    output.position = input.position;
    output.uv = input.uv;
    output.color = input.color;
    return output;
}

[shader("fragment")]
float4 scene_fragment(SceneVaryings input) : SV_Target
{
    return scene_textures[draw.texture_index].Sample(scene_samplers[draw.sampler_index], input.uv) * input.color;
}
