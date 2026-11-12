#include <metal_stdlib>
#include <metal_math>
#include <metal_texture>
using namespace metal;

#line 90 "core"
struct pixelOutput_0
{
    float4 output_0 [[color(0)]];
};


#line 90
struct pixelInput_0
{
    float2 uv_0 [[user(TEXCOORD)]];
    float4 color_0 [[user(COLOR)]];
};


#line 7 "D:/a/ludenea/ludenea/spikes/shader-forms-across-platforms/shaders/slang/scene.slang"
struct DrawIndexes_0
{
    uint texture_index_0;
    uint sampler_index_0;
};


#line 18
struct KernelContext_0
{
    DrawIndexes_0 constant* draw_0;
    array<texture2d<float, access::sample>, int(5)> scene_textures_0;
    array<sampler, int(3)> scene_samplers_0;
};


#line 45
[[fragment]] pixelOutput_0 scene_fragment(pixelInput_0 _S1 [[stage_in]], float4 position_0 [[position]], DrawIndexes_0 constant* draw_1 [[buffer(0)]], array<texture2d<float, access::sample>, int(5)> scene_textures_1 [[texture(0)]], array<sampler, int(3)> scene_samplers_1 [[sampler(0)]])
{

#line 45
    thread KernelContext_0 kernelContext_0;

#line 45
    (&kernelContext_0)->draw_0 = draw_1;

#line 45
    (&kernelContext_0)->scene_textures_0 = scene_textures_1;

#line 45
    (&kernelContext_0)->scene_samplers_0 = scene_samplers_1;

#line 45
    pixelOutput_0 _S2 = { ((scene_textures_1[draw_1->texture_index_0]).sample((scene_samplers_1[draw_1->sampler_index_0]), (_S1.uv_0))) * _S1.color_0 };

    return _S2;
}

