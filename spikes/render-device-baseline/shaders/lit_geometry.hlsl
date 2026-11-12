// Direct3D 12 shaders of the geometry buffer fill of the lit scene, compiled
// at run time by the system's D3DCompile to the 5.1 targets.
//
// No lighting here: the pixel shaders write the surface's attributes to four
// attachments (160 bits per pixel) or five (192 bits per pixel): the albedo,
// the normal mapped to 0..1, the specular strength and power over 255, the
// world position, and for the wider form the albedo at half intensity.

struct VertexInput
{
    float4 clip : POSITION;
    float3 world : TEXCOORD0;
    float3 normal : NORMAL;
    float3 albedo : COLOR0;
};

struct PixelInput
{
    float4 position : SV_Position;
    float3 world : TEXCOORD0;
    float3 normal : NORMAL;
    float3 albedo : COLOR0;
};

struct FourAttachments
{
    float4 albedo : SV_Target0;
    float4 normal : SV_Target1;
    float4 material : SV_Target2;
    float4 position : SV_Target3;
};

struct FiveAttachments
{
    float4 albedo : SV_Target0;
    float4 normal : SV_Target1;
    float4 material : SV_Target2;
    float4 position : SV_Target3;
    float4 half_albedo : SV_Target4;
};

PixelInput vs_main(VertexInput input)
{
    PixelInput output;
    output.position = input.clip;
    output.world = input.world;
    output.normal = input.normal;
    output.albedo = input.albedo;
    return output;
}

FourAttachments ps_fill_four(PixelInput input)
{
    float3 normal = normalize(input.normal);
    FourAttachments output;
    output.albedo = float4(input.albedo, 1.0);
    output.normal = float4(normal * 0.5 + 0.5, 1.0);
    output.material = float4(0.25, 32.0 / 255.0, 0.0, 1.0);
    output.position = float4(input.world, 1.0);
    return output;
}

FiveAttachments ps_fill_five(PixelInput input)
{
    float3 normal = normalize(input.normal);
    FiveAttachments output;
    output.albedo = float4(input.albedo, 1.0);
    output.normal = float4(normal * 0.5 + 0.5, 1.0);
    output.material = float4(0.25, 32.0 / 255.0, 0.0, 1.0);
    output.position = float4(input.world, 1.0);
    output.half_albedo = float4(input.albedo * 0.5, 1.0);
    return output;
}
