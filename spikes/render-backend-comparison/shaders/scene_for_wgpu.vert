#version 450
// Vertex shader of the filtering and blending scene for Vulkan beneath
// wgpu, compiled ahead to SPIR-V. It differs from the device baseline's own
// by one sign: wgpu renders through a viewport of negative height, so a
// shader compiled ahead keeps y up as the scene is authored, where the
// baseline's shader negates y for a viewport of positive height. The
// fragment shader is the baseline's, unchanged.
layout(location = 0) in vec4 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in vec4 color;
layout(location = 0) out vec2 fragment_uv;
layout(location = 1) out vec4 fragment_color;

void main() {
    gl_Position = position;
    fragment_uv = uv;
    fragment_color = color;
}
