// A surface function that reads the view: its emission grows where the
// surface turns away from the camera. It moves no vertex.
module surface;

import lighting;

[[vk::binding(0, 1)]]
public Texture2D base_color_image : register(t0, space1);
[[vk::binding(1, 1)]]
public SamplerState base_color_filtering : register(s0, space1);
[[vk::binding(2, 1)]]
public ConstantBuffer<Material> material : register(b0, space1);

public Surface surface(SurfaceInput inputs)
{
    Surface result;
    result.base_color = engine_sample(base_color_image, base_color_filtering, inputs.uv).rgb * material.tint.rgb;
    result.roughness = 0.5;
    result.normal = normalize(inputs.normal);
    float3 to_camera = normalize(view.camera.xyz - inputs.position);
    result.emission = material.tint.rgb * pow(1.0 - max(dot(result.normal, to_camera), 0.0), 3.0);
    return result;
}

public float3 surface_offset(float3 position, float3 normal)
{
    return float3(0.0, 0.0, 0.0);
}
