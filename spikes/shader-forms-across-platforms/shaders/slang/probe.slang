// A probe around a surface function alone, with no lighting: what an
// importer compiles to ask a chain which inputs the surface itself uses.
import lighting;
import surface;

struct ProbeVaryings
{
    float4 clip_position : SV_Position;
    float3 world_position : POSITION0;
    float3 world_normal : NORMAL0;
    float2 surface_uv : TEXCOORD0;
};

[shader("fragment")]
float4 probe_fragment(ProbeVaryings input) : SV_Target
{
    SurfaceInput inputs;
    inputs.uv.value = input.surface_uv;
    inputs.uv.ddx = ddx(input.surface_uv);
    inputs.uv.ddy = ddy(input.surface_uv);
    inputs.position = input.world_position;
    inputs.normal = input.world_normal;
    Surface result = surface(inputs);
    return float4(result.base_color + result.emission + result.normal, result.roughness);
}

[shader("vertex")]
float4 probe_vertex(float4 position : POSITION, float3 normal : NORMAL) : SV_Position
{
    return float4(position.xyz + surface_offset(position.xyz, normal), position.w);
}
