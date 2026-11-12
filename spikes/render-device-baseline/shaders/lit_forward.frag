#version 450
// The forward variant of the lit scene: the ambient term and four point
// lights with a diffuse and a specular term, written to one image.
layout(std140, set = 0, binding = 0) uniform Lighting {
    vec4 camera;
    vec4 ambient;
    vec4 light_positions[4];
    vec4 light_colors[4];
} lighting;
layout(location = 0) in vec3 fragment_world;
layout(location = 1) in vec3 fragment_normal;
layout(location = 2) in vec3 fragment_albedo;
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
    out_color = vec4(shade(fragment_world, normalize(fragment_normal), fragment_albedo, 0.25, 32.0), 1.0);
}
