// Direct3D 12 shaders of the lighting pass that reads the geometry buffer of
// the lit scene, compiled at run time by the system's D3DCompile to the 5.1
// targets.
//
// One triangle covers the target. For each pixel the shader loads the four
// attachments by integer coordinates and lights with the normal unpacked from
// 0..1, the specular strength and the power times 255 read from the third
// attachment; a pixel whose albedo has an alpha of zero was not covered by
// the fill and is written black.

cbuffer LitConstants : register(b0)
{
    float4 camera_position;
    float4 ambient_color;
    float4 light_positions[4];
    float4 light_colors[4];
};

Texture2D<float4> albedo_texture : register(t0);
Texture2D<float4> normal_texture : register(t1);
Texture2D<float4> material_texture : register(t2);
Texture2D<float4> position_texture : register(t3);

float4 vs_main(uint vertex_index : SV_VertexID) : SV_Position
{
    // The corners (0, 0), (2, 0) and (0, 2) of the target, in halves of it.
    float2 corner = float2((vertex_index << 1) & 2, vertex_index & 2);
    return float4(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
}

float3 light_surface(float3 albedo, float3 normal, float3 world, float strength, float power)
{
    float3 view = normalize(camera_position.xyz - world);
    float3 color = albedo * ambient_color.rgb;
    for (uint light = 0; light < 4; light++)
    {
        float3 towards = light_positions[light].xyz - world;
        float distance_squared = dot(towards, towards);
        towards = towards / sqrt(distance_squared);
        float attenuation = light_colors[light].a / (1.0 + distance_squared);
        float diffuse = max(dot(normal, towards), 0.0);
        float3 halfway = normalize(towards + view);
        float specular = pow(max(dot(normal, halfway), 0.0), power) * strength;
        color += (albedo * diffuse + specular) * light_colors[light].rgb * attenuation;
    }
    return color;
}

float4 ps_main(float4 position : SV_Position) : SV_Target
{
    int3 texel = int3(position.xy, 0);
    float4 albedo = albedo_texture.Load(texel);
    if (albedo.a == 0.0)
    {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    float3 normal = normalize(normal_texture.Load(texel).xyz * 2.0 - 1.0);
    float4 material = material_texture.Load(texel);
    float3 world = position_texture.Load(texel).xyz;
    return float4(light_surface(albedo.rgb, normal, world, material.r, material.g * 255.0), 1.0);
}
