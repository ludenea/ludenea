#version 450
// The lighting pass of the lit scene: loads the four texels of the geometry
// buffer at the pixel by integer coordinates and lights them as the forward
// variant does, with the strength and the power read from the material; a
// pixel the fill did not cover is written black.
layout(set = 0, binding = 0) uniform sampler2D albedo_texture;
layout(set = 0, binding = 1) uniform sampler2D normal_texture;
layout(set = 0, binding = 2) uniform sampler2D material_texture;
layout(set = 0, binding = 3) uniform sampler2D position_texture;
layout(std140, set = 0, binding = 4) uniform Lighting {
    vec4 camera;
    vec4 ambient;
    vec4 light_positions[4];
    vec4 light_colors[4];
} lighting;
layout(location = 0) out vec4 out_color;

vec3 shade(vec3 world, vec3 normal, vec3 albedo, float strength, float power) {
    vec3 view = normalize(lighting.camera.xyz - world);
    vec3 color = albedo * lighting.ambient.rgb;
    for (int light = 0; light < 4; light++) {
        vec3 to_light = lighting.light_positions[light].xyz - world;
        float distance_squared = dot(to_light, to_light);
        to_light = to_light / sqrt(distance_squared);
        float attenuation = lighting.light_colors[light].a / (1.0 + distance_squared);
        float diffuse = max(dot(normal, to_light), 0.0);
        vec3 half_vector = normalize(to_light + view);
        float specular = pow(max(dot(normal, half_vector), 0.0), power) * strength;
        color += (albedo * diffuse + specular) * lighting.light_colors[light].rgb * attenuation;
    }
    return color;
}

void main() {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    vec4 albedo = texelFetch(albedo_texture, pixel, 0);
    if (albedo.a == 0.0) {
        out_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }
    vec3 normal = normalize(texelFetch(normal_texture, pixel, 0).xyz * 2.0 - 1.0);
    vec4 material = texelFetch(material_texture, pixel, 0);
    vec3 world = texelFetch(position_texture, pixel, 0).xyz;
    out_color = vec4(shade(world, normal, albedo.rgb, material.r, material.g * 255.0), 1.0);
}
