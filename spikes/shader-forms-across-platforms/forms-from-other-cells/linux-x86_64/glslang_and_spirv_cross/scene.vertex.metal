#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct main0_out
{
    float2 fragment_uv [[user(locn0)]];
    float4 fragment_color [[user(locn1)]];
    float4 gl_Position [[position]];
};

struct main0_in
{
    float4 position [[attribute(0)]];
    float2 uv [[attribute(1)]];
    float4 color [[attribute(2)]];
};

vertex main0_out main0(main0_in in [[stage_in]])
{
    main0_out out = {};
    out.gl_Position = float4(in.position.x, -in.position.y, in.position.z, in.position.w);
    out.fragment_uv = in.uv;
    out.fragment_color = in.color;
    out.gl_Position.y = -(out.gl_Position.y);    // Invert Y-axis for Metal
    return out;
}

