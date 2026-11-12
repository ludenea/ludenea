// A surface function whose material moves vertices: each vertex is pushed
// along its normal by an amount of the material. It reads nothing of the
// view.
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
    result.emission = float3(0.0, 0.0, 0.0);
    return result;
}

public float3 surface_offset(float3 position, float3 normal)
{
    return normal * material.sway.x * sin(position.x * material.sway.y);
}
