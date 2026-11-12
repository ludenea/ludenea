#version 450
// Vertex stage of the frame shader: the clip position is the draw's transform
// times the vertex's position. Clip space in this spike has y up and Vulkan's
// has y down, so the shader negates y after the product and the three
// interfaces share one orientation. The uv and the color are interpolated
// with perspective.
layout(push_constant) uniform DrawConstants {
    mat4 transform;
    vec4 tint;
    uint texture_index;
    uint sampler_index;
} draw;
layout(location = 0) in vec4 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in vec4 color;
layout(location = 0) out vec2 fragment_uv;
layout(location = 1) out vec4 fragment_color;

void main() {
    vec4 clip = draw.transform * position;
    gl_Position = vec4(clip.x, -clip.y, clip.z, clip.w);
    fragment_uv = uv;
    fragment_color = color;
}
