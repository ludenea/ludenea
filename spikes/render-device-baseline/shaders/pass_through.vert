#version 450
// One triangle covering the viewport, with uv 0 to 1 across it and v = 0 on
// the top row. The y of the y-up clip space is negated for Vulkan.
layout(location = 0) out vec2 fragment_uv;

void main() {
    vec2 corner = vec2(float((gl_VertexIndex & 1) * 4 - 1), float((gl_VertexIndex >> 1) * 4 - 1));
    fragment_uv = vec2((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    gl_Position = vec4(corner.x, -corner.y, 0.0, 1.0);
}
