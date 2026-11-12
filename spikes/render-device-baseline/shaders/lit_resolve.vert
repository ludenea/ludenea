#version 450
// One triangle covering the target, for the lighting pass that reads the
// geometry buffer. The y of the y-up clip space is negated for Vulkan.
void main() {
    vec2 corner = vec2(float((gl_VertexIndex & 1) * 4 - 1), float((gl_VertexIndex >> 1) * 4 - 1));
    gl_Position = vec4(corner.x, -corner.y, 0.0, 1.0);
}
