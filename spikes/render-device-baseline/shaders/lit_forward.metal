// Metal shaders of the lit scene written to one image, compiled at run time
// by the system's Metal compiler.
//
// The vertex function passes the clip position through (Metal's clip space
// has y up, as the scene is authored) with the world position, the normal and
// the albedo; the vertex buffer is read by vertex index. The fragment
// function lights the surface with an ambient term and four point lights, a
// diffuse term and a specular term of strength 0.25 and power 32, as the
// spike's contract states.
#include <metal_stdlib>
using namespace metal;

// 52 bytes without padding: the layout of the lit scene's vertices.
struct LitVertex {
    packed_float4 clip;
    packed_float3 world;
    packed_float3 normal;
    packed_float3 albedo;
};

struct Varyings {
    float4 position [[position]];
    float3 world;
    float3 normal;
    float3 albedo;
};

// Ten vectors of four floats: the layout of the lit scene's constants.
struct LitConstants {
    float4 camera_position;
    float4 ambient_color;
    float4 light_positions[4];
    float4 light_colors[4];
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

vertex Varyings lit_forward_vertex(uint vertex_index [[vertex_id]], const device LitVertex* vertices [[buffer(0)]]) {
    Varyings out;
    out.position = float4(vertices[vertex_index].clip);
    out.world = float3(vertices[vertex_index].world);
    out.normal = float3(vertices[vertex_index].normal);
    out.albedo = float3(vertices[vertex_index].albedo);
    return out;
}

fragment float4 lit_forward_fragment(Varyings in [[stage_in]], constant LitConstants& constants [[buffer(0)]]) {
    return float4(light_surface(constants, in.albedo, normalize(in.normal), in.world, 0.25, 32.0), 1.0);
}
