#version 450
// The geometry buffer fill of the lit scene, no lighting: albedo, encoded
// normal, material (specular strength and power over 255), world position,
// and with FIFTH_ATTACHMENT defined a fifth image holding half the albedo.
layout(location = 0) in vec3 fragment_world;
layout(location = 1) in vec3 fragment_normal;
layout(location = 2) in vec3 fragment_albedo;
layout(location = 0) out vec4 out_albedo;
layout(location = 1) out vec4 out_normal;
layout(location = 2) out vec4 out_material;
layout(location = 3) out vec4 out_position;
#ifdef FIFTH_ATTACHMENT
layout(location = 4) out vec4 out_half_albedo;
#endif

void main() {
    vec3 normal = normalize(fragment_normal);
    out_albedo = vec4(fragment_albedo, 1.0);
    out_normal = vec4(normal * 0.5 + 0.5, 1.0);
    out_material = vec4(0.25, 32.0 / 255.0, 0.0, 1.0);
    out_position = vec4(fragment_world, 1.0);
#ifdef FIFTH_ATTACHMENT
    out_half_albedo = vec4(fragment_albedo * 0.5, 1.0);
#endif
}
