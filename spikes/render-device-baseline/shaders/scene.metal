// Metal shaders of the filtering and blending scene, compiled at run time by
// the system's Metal compiler.
//
// Clip space has y up and a depth range of 0 to 1 in Metal, as the scene is
// authored, so the vertex function passes the position through unchanged and
// interpolates the texture coordinate and the color with perspective. The
// vertex buffer is read by vertex index, with no vertex descriptor. The
// fragment function samples the texture and the sampler the draw names: their
// indexes arrive as the fragment bytes of the draw, the textures as one array
// of five textures and the samplers as one array of three samplers.
#include <metal_stdlib>
using namespace metal;

// 40 bytes without padding: the layout of the scene's vertices.
struct Vertex {
    packed_float4 position;
    packed_float2 uv;
    packed_float4 color;
};

struct Varyings {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

struct DrawConstants {
    uint texture_index;
    uint sampler_index;
};

vertex Varyings scene_vertex(uint vertex_index [[vertex_id]], const device Vertex* vertices [[buffer(0)]]) {
    Varyings out;
    out.position = float4(vertices[vertex_index].position);
    out.uv = float2(vertices[vertex_index].uv);
    out.color = float4(vertices[vertex_index].color);
    return out;
}

fragment float4 scene_fragment(Varyings in [[stage_in]],
                               array<texture2d<float>, 5> scene_textures [[texture(0)]],
                               array<sampler, 3> scene_samplers [[sampler(0)]],
                               constant DrawConstants& draw [[buffer(0)]]) {
    return scene_textures[draw.texture_index].sample(scene_samplers[draw.sampler_index], in.uv) * in.color;
}
