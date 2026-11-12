#version 450
// Vertex shader of the lit scene, shared by the forward and the geometry
// buffer fills: the clip position passes through (y negated for Vulkan), the
// world position, the normal and the albedo are interpolated.
layout(location = 0) in vec4 clip_position;
layout(location = 1) in vec3 world_position;
layout(location = 2) in vec3 normal;
layout(location = 3) in vec3 albedo;
layout(location = 0) out vec3 fragment_world;
layout(location = 1) out vec3 fragment_normal;
layout(location = 2) out vec3 fragment_albedo;

void main() {
    gl_Position = vec4(clip_position.x, -clip_position.y, clip_position.z, clip_position.w);
    fragment_world = world_position;
    fragment_normal = normal;
    fragment_albedo = albedo;
}
