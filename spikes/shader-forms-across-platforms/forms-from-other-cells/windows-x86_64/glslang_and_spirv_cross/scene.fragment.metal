#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct DrawIndexes
{
    uint texture_index;
    uint sampler_index;
};

struct main0_out
{
    float4 out_color [[color(0)]];
};

struct main0_in
{
    float2 fragment_uv [[user(locn0)]];
    float4 fragment_color [[user(locn1)]];
};

fragment main0_out main0(main0_in in [[stage_in]], constant DrawIndexes& draw [[buffer(0)]], array<texture2d<float>, 5> textures [[texture(0)]], array<sampler, 3> samplers [[sampler(0)]])
{
    main0_out out = {};
    out.out_color = textures[draw.texture_index].sample(samplers[draw.sampler_index], in.fragment_uv) * in.fragment_color;
    return out;
}

