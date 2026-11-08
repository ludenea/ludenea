#version 450
// Vulkan vertex shader of the offscreen scene. Clip space in this spike has y
// up, as Direct3D and Metal have it; Vulkan's has y down, so the vertex shader
// flips it and the three images share one orientation.
layout(location = 0) in vec2 position;
layout(location = 1) in vec3 color;
layout(location = 0) flat out vec3 fragment_color;

void main() {
    gl_Position = vec4(position.x, -position.y, 0.0, 1.0);
    fragment_color = color;
}
