#include <metal_stdlib>
#include <metal_math>
#include <metal_texture>
using namespace metal;

#line 20 "/home/runner/work/ludenea/ludenea/spikes/shader-forms-across-platforms/shaders/slang/scene.slang"
struct scene_vertex_Result_0
{
    float4 position_0 [[position]];
    float2 uv_0 [[user(TEXCOORD)]];
    float4 color_0 [[user(COLOR)]];
};


#line 20
struct vertexInput_0
{
    float4 position_1 [[attribute(0)]];
    float2 uv_1 [[attribute(1)]];
    float4 color_1 [[attribute(2)]];
};

struct SceneVaryings_0
{
    float4 position_2;
    float2 uv_2;
    float4 color_2;
};


#line 27
[[vertex]] scene_vertex_Result_0 scene_vertex(vertexInput_0 _S1 [[stage_in]])
{

#line 37
    thread SceneVaryings_0 output_0;
    (&output_0)->position_2 = _S1.position_1;
    (&output_0)->uv_2 = _S1.uv_1;
    (&output_0)->color_2 = _S1.color_1;

#line 40
    thread scene_vertex_Result_0 _S2;

#line 40
    (&_S2)->position_0 = output_0.position_2;

#line 40
    (&_S2)->uv_0 = output_0.uv_2;

#line 40
    (&_S2)->color_0 = output_0.color_2;

#line 40
    return _S2;
}

