// Metal shaders of the frames, compiled at run time by the system's Metal
// compiler when a pair of pipelines is built.
//
// Clip space has y up and a depth range of 0 to 1 in Metal, as the frames
// are authored, so the vertex function multiplies the position by the draw's
// transform and changes nothing else; the texture coordinate and the color
// are interpolated with perspective. The vertex buffer is read by vertex
// index, with no vertex descriptor. The fragment function samples the texture
// and the sampler the draw names, among the pass's five textures and the
// device's three samplers, and multiplies the texel by the interpolated color
// and by the draw's tint.
//
// The draw's constants arrive as bytes set for each stage: at the buffer
// index 1 of the vertex function, whose index 0 is the vertex buffer, and at
// the buffer index 0 of the fragment function.
//
// TINT_VARIANT is defined to 0 or 1 by a line the adapter puts before this
// text. With 1 the fragment function multiplies the red, the green and the
// blue of its result by (0.5, 1.0, 0.25).
#include <metal_stdlib>
using namespace metal;

#ifndef TINT_VARIANT
#define TINT_VARIANT 0
#endif

// 40 bytes without padding: the layout of the frames' vertices.
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

// The 88 bytes of a draw's constants: the transform column by column at 0,
// the tint at 64, the texture's index at 80 and the sampler's at 84. The
// matrix aligns the structure on 16 bytes, which makes it 96 bytes long: the
// adapter sets 96 bytes, the last 8 being zero.
struct DrawConstants {
    float4x4 transform;
    float4 tint;
    uint texture_index;
    uint sampler_index;
};

vertex Varyings frame_vertex(uint vertex_index [[vertex_id]],
                             const device Vertex* vertices [[buffer(0)]],
                             constant DrawConstants& draw [[buffer(1)]]) {
    Varyings out;
    out.position = draw.transform * float4(vertices[vertex_index].position);
    out.uv = float2(vertices[vertex_index].uv);
    out.color = float4(vertices[vertex_index].color);
    return out;
}

fragment float4 frame_fragment(Varyings in [[stage_in]],
                               array<texture2d<float>, 5> frame_textures [[texture(0)]],
                               array<sampler, 3> frame_samplers [[sampler(0)]],
                               constant DrawConstants& draw [[buffer(0)]]) {
    float4 color = frame_textures[draw.texture_index].sample(frame_samplers[draw.sampler_index], in.uv) * in.color * draw.tint;
#if TINT_VARIANT == 1
    color.rgb *= float3(0.5, 1.0, 0.25);
#endif
    return color;
}
