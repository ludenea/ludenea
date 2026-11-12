// The filtering and blending scene for Metal beneath wgpu, compiled ahead to
// a Metal library by the Metal compiler of Xcode, and handed as text to the
// platform's compiler at load: both go through wgpu's path for shaders it
// does not translate.
//
// wgpu assigns the slots of a stage from the pipeline layout, and a shader
// compiled ahead has to be written against that assignment, which wgpu does
// not document as an interface: the immediate data is the buffer at index 0
// of every stage, then each texture and each sampler takes the next index
// of its kind in the order of the layout's entries. An array of textures
// would be an argument buffer of wgpu's own design, so this source binds the
// five textures and the three samplers separately and chooses among them by
// the indexes of the draw. Vertex attributes arrive through the vertex
// descriptor wgpu builds, by their location.
#include <metal_stdlib>
using namespace metal;

struct VertexInput {
    float4 position [[attribute(0)]];
    float2 uv [[attribute(1)]];
    float4 color [[attribute(2)]];
};

struct Varyings {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

struct DrawIndexes {
    uint texture_index;
    uint sampler_index;
};

vertex Varyings scene_vertex(VertexInput in [[stage_in]]) {
    Varyings out;
    out.position = in.position;
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

static float4 sample_with(texture2d<float> chosen, uint sampler_index, sampler sampler_0, sampler sampler_1, sampler sampler_2, float2 uv) {
    switch (sampler_index) {
        case 0: return chosen.sample(sampler_0, uv);
        case 1: return chosen.sample(sampler_1, uv);
        default: return chosen.sample(sampler_2, uv);
    }
}

fragment float4 scene_fragment(Varyings in [[stage_in]],
                               constant DrawIndexes& draw [[buffer(0)]],
                               texture2d<float> texture_0 [[texture(0)]],
                               texture2d<float> texture_1 [[texture(1)]],
                               texture2d<float> texture_2 [[texture(2)]],
                               texture2d<float> texture_3 [[texture(3)]],
                               texture2d<float> texture_4 [[texture(4)]],
                               sampler sampler_0 [[sampler(0)]],
                               sampler sampler_1 [[sampler(1)]],
                               sampler sampler_2 [[sampler(2)]]) {
    float4 texel;
    switch (draw.texture_index) {
        case 0: texel = sample_with(texture_0, draw.sampler_index, sampler_0, sampler_1, sampler_2, in.uv); break;
        case 1: texel = sample_with(texture_1, draw.sampler_index, sampler_0, sampler_1, sampler_2, in.uv); break;
        case 2: texel = sample_with(texture_2, draw.sampler_index, sampler_0, sampler_1, sampler_2, in.uv); break;
        case 3: texel = sample_with(texture_3, draw.sampler_index, sampler_0, sampler_1, sampler_2, in.uv); break;
        default: texel = sample_with(texture_4, draw.sampler_index, sampler_0, sampler_1, sampler_2, in.uv); break;
    }
    return texel * in.color;
}
