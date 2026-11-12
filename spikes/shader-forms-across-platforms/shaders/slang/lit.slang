// A lit shader: the engine's template around a surface function. The module
// `surface` it imports is found on the module search path of the compile,
// which is how one template is composed with each surface.
import lighting;
import surface;

struct LitVertex
{
    float4 position : POSITION;
    float3 normal : NORMAL;
    float2 uv : TEXCOORD0;
};

struct LitVaryings
{
    float4 clip_position : SV_Position;
    float3 world_position : POSITION0;
    float3 world_normal : NORMAL0;
    float2 surface_uv : TEXCOORD0;
};

[shader("vertex")]
LitVaryings lit_vertex(LitVertex input)
{
    float3 moved = input.position.xyz + surface_offset(input.position.xyz, input.normal);
    LitVaryings output;
    output.clip_position = float4(moved, input.position.w);
    output.world_position = moved;
    output.world_normal = input.normal;
    output.surface_uv = input.uv;
    return output;
}

[shader("fragment")]
float4 lit_fragment(LitVaryings input) : SV_Target
{
    SurfaceInput inputs;
    inputs.uv.value = input.surface_uv;
    inputs.uv.ddx = ddx(input.surface_uv);
    inputs.uv.ddy = ddy(input.surface_uv);
    inputs.position = input.world_position;
    inputs.normal = input.world_normal;
    return float4(light_surface(surface(inputs), input.world_position), 1.0);
}
