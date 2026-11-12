// A surface function that reads the view: its emission grows where the
// surface turns away from the camera. It moves no vertex.

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
    vec3 to_camera = normalize(view.camera.xyz - inputs.position);
    result.emission = material.tint.rgb * pow(1.0 - max(dot(result.normal, to_camera), 0.0), 3.0);
    return result;
}

vec3 surface_offset(vec3 position, vec3 normal) {
    return vec3(0.0);
}
