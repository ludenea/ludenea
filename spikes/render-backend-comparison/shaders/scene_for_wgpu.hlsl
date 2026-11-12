// The filtering and blending scene for Direct3D 12 beneath wgpu, compiled
// ahead to DXIL by the DirectX shader compiler and handed to wgpu's path for
// shaders it does not translate.
//
// wgpu builds the root signature from the pipeline layout, and a shader
// compiled ahead has to be written against the registers it assigns, which
// wgpu does not document as an interface: the immediate data is a constant
// buffer at b0, the array of five textures sits at t0 to t4, and no sampler
// is bound where the layout names it. Every sampler of the device lives in
// one heap of 2 048 samplers at s0, and each bind group brings a buffer of
// indexes into that heap, here at t5: the three samplers of the scene are
// the first three entries of that buffer. Vertex attributes carry the
// semantic LOC with their location as its index.

cbuffer DrawIndexes : register(b0)
{
    uint texture_index;
    uint sampler_index;
};

Texture2D<float4> scene_textures[5] : register(t0);
SamplerState sampler_heap[2048] : register(s0, space0);
StructuredBuffer<uint> group_0_sampler_indexes : register(t5, space0);

struct VertexInput
{
    float4 position : LOC0;
    float2 uv : LOC1;
    float4 color : LOC2;
};

struct Varyings
{
    float2 uv : LOC0;
    float4 color : LOC1;
    float4 position : SV_Position;
};

Varyings scene_vertex(VertexInput input)
{
    Varyings output;
    output.position = input.position;
    output.uv = input.uv;
    output.color = input.color;
    return output;
}

float4 scene_fragment(Varyings input) : SV_Target0
{
    SamplerState chosen = sampler_heap[group_0_sampler_indexes[sampler_index]];
    return scene_textures[texture_index].Sample(chosen, input.uv) * input.color;
}
