#version 450
// Fragment stage of the frame shader: the texture and the sampler the draw
// names, both chosen from the pass's arrays by the draw's constants, times
// the interpolated color and the draw's tint. Compiled with TINT_VARIANT
// defined to 1, it then scales the red, the green and the blue.
#ifndef TINT_VARIANT
#define TINT_VARIANT 0
#endif
layout(set = 0, binding = 0) uniform texture2D textures[5];
layout(set = 0, binding = 1) uniform sampler samplers[3];
layout(push_constant) uniform DrawConstants {
    mat4 transform;
    vec4 tint;
    uint texture_index;
    uint sampler_index;
} draw;
layout(location = 0) in vec2 fragment_uv;
layout(location = 1) in vec4 fragment_color;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(textures[draw.texture_index], samplers[draw.sampler_index]), fragment_uv) * fragment_color * draw.tint;
#if TINT_VARIANT == 1
    color.rgb *= vec3(0.5, 1.0, 0.25);
#endif
    out_color = color;
}
