// Metal shaders of the offscreen scene, compiled at run time by the system's
// Metal compiler. The vertex shader reads the vertex buffer by vertex index
// (no vertex descriptor); clip space has y up; colors are flat per triangle.
#include <metal_stdlib>
using namespace metal;

struct Vertex {
    float x;
    float y;
    float r;
    float g;
    float b;
};

struct Fragment {
    float4 position [[position]];
    float3 color [[flat]];
};

vertex Fragment vertex_main(uint vertex_id [[vertex_id]], const device Vertex* vertices [[buffer(0)]]) {
    Vertex v = vertices[vertex_id];
    Fragment out;
    out.position = float4(v.x, v.y, 0.0, 1.0);
    out.color = float3(v.r, v.g, v.b);
    return out;
}

fragment float4 fragment_main(Fragment in [[stage_in]]) {
    return float4(in.color, 1.0);
}
