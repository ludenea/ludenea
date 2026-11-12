// Direct3D 12 shaders of the lit scene written to one image, compiled at run
// time by the system's D3DCompile to the 5.1 targets.
//
// The vertex shader passes the clip position through (Direct3D's clip space
// has y up, as the scene is authored) with the world position, the normal and
// the albedo. The pixel shader lights the surface with an ambient term and
// four point lights, a diffuse term and a specular term of strength 0.25 and
// power 32, as the spike's contract states.

cbuffer LitConstants : register(b0)
{
    float4 camera_position;
    float4 ambient_color;
    float4 light_positions[4];
    float4 light_colors[4];
};

struct VertexInput
{
    float4 clip : POSITION;
    float3 world : TEXCOORD0;
    float3 normal : NORMAL;
    float3 albedo : COLOR0;
};

struct PixelInput
{
    float4 position : SV_Position;
    float3 world : TEXCOORD0;
    float3 normal : NORMAL;
    float3 albedo : COLOR0;
};

PixelInput vs_main(VertexInput input)
{
    PixelInput output;
    output.position = input.clip;
    output.world = input.world;
    output.normal = input.normal;
    output.albedo = input.albedo;
    return output;
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

float4 ps_main(PixelInput input) : SV_Target
{
    return float4(light_surface(input.albedo, normalize(input.normal), input.world, 0.25, 32.0), 1.0);
}
