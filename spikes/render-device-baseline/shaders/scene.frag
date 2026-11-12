#version 450
// Fragment shader of the filtering and blending scene: the texture and the
// sampler the draw names, both chosen from arrays by push constants, times
// the interpolated color.
layout(set = 0, binding = 0) uniform texture2D textures[5];
layout(set = 0, binding = 1) uniform sampler samplers[3];
layout(push_constant) uniform DrawIndexes {
    uint texture_index;
    uint sampler_index;
} draw;
layout(location = 0) in vec2 fragment_uv;
layout(location = 1) in vec4 fragment_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(sampler2D(textures[draw.texture_index], samplers[draw.sampler_index]), fragment_uv) * fragment_color;
}
