#version 450
// Vulkan fragment shader of the offscreen scene: the triangle's flat color.
layout(location = 0) flat in vec3 fragment_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = vec4(fragment_color, 1.0);
}
