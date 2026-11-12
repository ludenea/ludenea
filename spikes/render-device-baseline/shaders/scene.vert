#version 450
// Vertex shader of the filtering and blending scene. Clip space in this spike
// has y up and Vulkan's has y down, so the shader negates y and the three
// interfaces share one orientation. The uv and the color are interpolated
// with perspective.
layout(location = 0) in vec4 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in vec4 color;
layout(location = 0) out vec2 fragment_uv;
layout(location = 1) out vec4 fragment_color;

void main() {
    gl_Position = vec4(position.x, -position.y, position.z, position.w);
    fragment_uv = uv;
    fragment_color = color;
}
