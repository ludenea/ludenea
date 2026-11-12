// Metal shaders of the geometry buffer fill of the lit scene, compiled at run
// time by the system's Metal compiler.
//
// No lighting here: the fragment functions write the surface's attributes to
// four attachments (160 bits per pixel) or five (192 bits per pixel): the
// albedo, the normal mapped to 0..1, the specular strength and power over
// 255, the world position, and for the wider form the albedo at half
// intensity. The vertex buffer is read by vertex index.
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

struct FourAttachments {
    float4 albedo [[color(0)]];
    float4 normal [[color(1)]];
    float4 material [[color(2)]];
    float4 position [[color(3)]];
};

struct FiveAttachments {
    float4 albedo [[color(0)]];
    float4 normal [[color(1)]];
    float4 material [[color(2)]];
    float4 position [[color(3)]];
    float4 half_albedo [[color(4)]];
};

vertex Varyings lit_geometry_vertex(uint vertex_index [[vertex_id]], const device LitVertex* vertices [[buffer(0)]]) {
    Varyings out;
    out.position = float4(vertices[vertex_index].clip);
    out.world = float3(vertices[vertex_index].world);
    out.normal = float3(vertices[vertex_index].normal);
    out.albedo = float3(vertices[vertex_index].albedo);
    return out;
}

fragment FourAttachments lit_geometry_fragment_four(Varyings in [[stage_in]]) {
    float3 normal = normalize(in.normal);
    FourAttachments out;
    out.albedo = float4(in.albedo, 1.0);
    out.normal = float4(normal * 0.5 + 0.5, 1.0);
    out.material = float4(0.25, 32.0 / 255.0, 0.0, 1.0);
    out.position = float4(in.world, 1.0);
    return out;
}

fragment FiveAttachments lit_geometry_fragment_five(Varyings in [[stage_in]]) {
    float3 normal = normalize(in.normal);
    FiveAttachments out;
    out.albedo = float4(in.albedo, 1.0);
    out.normal = float4(normal * 0.5 + 0.5, 1.0);
    out.material = float4(0.25, 32.0 / 255.0, 0.0, 1.0);
    out.position = float4(in.world, 1.0);
    out.half_albedo = float4(in.albedo * 0.5, 1.0);
    return out;
}
