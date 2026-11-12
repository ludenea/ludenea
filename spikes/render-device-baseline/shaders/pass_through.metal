// Metal shaders of the pass-through exercise, compiled at run time by the
// system's Metal compiler.
//
// One triangle covers the viewport (one half of the target); the texture
// coordinate runs from 0 at the viewport's top-left corner to 1 at its
// bottom-right corner, so that every pixel center falls on the center of the
// texel of the same position. The fragment function returns the texel sampled
// with the nearest filter: through a plain view and written through a plain
// view, its bytes pass unchanged; through the sRGB views, they are converted
// to linear on read and back on write.
#include <metal_stdlib>
using namespace metal;

struct Varyings {
    float4 position [[position]];
    float2 uv;
};

vertex Varyings pass_through_vertex(uint vertex_index [[vertex_id]]) {
    // The corners (0, 0), (2, 0) and (0, 2) of the texture space.
    float2 corner = float2(float((vertex_index << 1u) & 2u), float(vertex_index & 2u));
    Varyings out;
    out.position = float4(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    out.uv = corner;
    return out;
}

fragment float4 pass_through_fragment(Varyings in [[stage_in]],
                                      texture2d<float> source_texture [[texture(0)]],
                                      sampler nearest_sampler [[sampler(0)]]) {
    return source_texture.sample(nearest_sampler, in.uv);
}
