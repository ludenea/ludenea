// A surface function: a textured surface sampled at its interpolated
// coordinate. It reads nothing of the view and moves no vertex.

layout(set = 1, binding = 0) uniform texture2D base_color_image;
layout(set = 1, binding = 1) uniform sampler base_color_filtering;
layout(set = 1, binding = 2) uniform Material {
    vec4 tint;
    vec4 sway;
} material;

Surface surface(SurfaceInput inputs) {
    Surface result;
    result.base_color = engine_sample(base_color_image, base_color_filtering, inputs.uv).rgb * material.tint.rgb;
    result.roughness = 0.5;
    result.normal = normalize(inputs.normal);
    result.emission = vec3(0.0);
    return result;
}

vec3 surface_offset(vec3 position, vec3 normal) {
    return vec3(0.0);
}
