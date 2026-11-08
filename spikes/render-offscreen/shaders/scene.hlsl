// Direct3D 11 shaders of the offscreen scene, compiled at run time by the
// system's D3DCompile. Clip space has y up; colors are flat per triangle.
struct VertexInput {
    float2 position : POSITION;
    float3 color : COLOR;
};

struct PixelInput {
    float4 position : SV_Position;
    nointerpolation float3 color : COLOR;
};

PixelInput vs_main(VertexInput input) {
    PixelInput output;
    output.position = float4(input.position, 0.0, 1.0);
    output.color = input.color;
    return output;
}

float4 ps_main(PixelInput input) : SV_Target {
    return float4(input.color, 1.0);
}
