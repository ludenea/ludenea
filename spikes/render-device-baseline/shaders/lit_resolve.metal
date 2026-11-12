// Metal shaders of the lighting pass that reads the geometry buffer of the
// lit scene, compiled at run time by the system's Metal compiler.
//
// One triangle covers the target. For each pixel the fragment function loads
// the four attachments by integer coordinates and lights with the normal
// unpacked from 0..1, the specular strength and the power times 255 read from
// the third attachment; a pixel whose albedo has an alpha of zero was not
// covered by the fill and is written black. The lighting is the one of the
// forward shaders, repeated here since each file is compiled alone.
#include <metal_stdlib>
using namespace metal;

// Ten vectors of four floats: the layout of the lit scene's constants.
struct LitConstants {
    float4 camera_position;
    float4 ambient_color;
    float4 light_positions[4];
    float4 light_colors[4];
};

struct Varyings {
    float4 position [[position]];
};

static float3 light_surface(constant LitConstants& constants, float3 albedo, float3 normal, float3 world, float strength, float power) {
    float3 view = normalize(constants.camera_position.xyz - world);
    float3 color = albedo * constants.ambient_color.rgb;
    for (uint light = 0u; light < 4u; light++) {
        float3 towards = constants.light_positions[light].xyz - world;
        float distance_squared = dot(towards, towards);
        towards = towards / sqrt(distance_squared);
        float attenuation = constants.light_colors[light].a / (1.0 + distance_squared);
        float diffuse = max(dot(normal, towards), 0.0);
        float3 halfway = normalize(towards + view);
        float specular = pow(max(dot(normal, halfway), 0.0), power) * strength;
        color += (albedo * diffuse + specular) * constants.light_colors[light].rgb * attenuation;
    }
    return color;
}

vertex Varyings lit_resolve_vertex(uint vertex_index [[vertex_id]]) {
    // The corners (0, 0), (2, 0) and (0, 2) of the target, in halves of it.
    float2 corner = float2(float((vertex_index << 1u) & 2u), float(vertex_index & 2u));
    Varyings out;
    out.position = float4(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    return out;
}

fragment float4 lit_resolve_fragment(Varyings in [[stage_in]],
                                     texture2d<float> albedo_texture [[texture(0)]],
                                     texture2d<float> normal_texture [[texture(1)]],
                                     texture2d<float> material_texture [[texture(2)]],
                                     texture2d<float> position_texture [[texture(3)]],
                                     constant LitConstants& constants [[buffer(0)]]) {
    uint2 texel = uint2(in.position.xy);
    float4 albedo = albedo_texture.read(texel);
    if (albedo.a == 0.0) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    float3 normal = normalize(normal_texture.read(texel).xyz * 2.0 - 1.0);
    float4 material = material_texture.read(texel);
    float3 world = position_texture.read(texel).xyz;
    return float4(light_surface(constants, albedo.rgb, normal, world, material.r, material.g * 255.0), 1.0);
}
