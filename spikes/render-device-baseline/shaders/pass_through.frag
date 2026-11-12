#version 450
// The source texel for the pixel, through the nearest filter: the views bound
// decide whether the texel is converted from sRGB on read and to sRGB on write.
layout(set = 0, binding = 0) uniform sampler2D source_texture;
layout(location = 0) in vec2 fragment_uv;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(source_texture, fragment_uv);
}
