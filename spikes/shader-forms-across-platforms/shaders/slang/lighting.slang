// The engine's side of a lit shader as a Slang module: the engine's
// coordinate type and its sampling function, the properties a surface
// returns, the frame's constants and the lighting. A surface is a module
// named `surface` that imports this one; the lit shader imports both, and
// which surface it is composed with is the module search path of the
// compile.
//
// A coordinate travels with its differentials as one value, and a surface
// samples through `engine_sample` only: in a raster pass the function is
// plain sampling, since the hardware supplies the differentials there.
module lighting;

public struct Coordinate
{
    public float2 value;
    public float2 ddx;
    public float2 ddy;
};

public Coordinate coordinate_scaled(Coordinate c, float2 scale)
{
    Coordinate result;
    result.value = c.value * scale;
    result.ddx = c.ddx * scale;
    result.ddy = c.ddy * scale;
    return result;
}

float2 turned(float2 v, float cosine, float sine)
{
    return float2(cosine * v.x - sine * v.y, sine * v.x + cosine * v.y);
}

public Coordinate coordinate_rotated(Coordinate c, float cosine, float sine)
{
    Coordinate result;
    result.value = turned(c.value, cosine, sine);
    result.ddx = turned(c.ddx, cosine, sine);
    result.ddy = turned(c.ddy, cosine, sine);
    return result;
}

public float4 engine_sample(Texture2D image, SamplerState filtering, Coordinate c)
{
    return image.Sample(filtering, c.value);
}

public struct Surface
{
    public float3 base_color;
    public float roughness;
    public float3 normal;
    public float3 emission;
};

public struct SurfaceInput
{
    public Coordinate uv;
    public float3 position;
    public float3 normal;
};

public struct View
{
    public float4 camera;
};

public struct Lights
{
    public float4 ambient;
    public float4 positions[4];
    public float4 colors[4];
};

public struct Material
{
    public float4 tint;
    public float4 sway;
};

[[vk::binding(0, 0)]]
public ConstantBuffer<View> view : register(b0, space0);
[[vk::binding(1, 0)]]
public ConstantBuffer<Lights> lights : register(b1, space0);

static const float SPECULAR_STRENGTH = 0.25;

public float3 light_surface(Surface surface, float3 position)
{
    float3 to_camera = normalize(view.camera.xyz - position);
    float3 color = surface.base_color * lights.ambient.rgb + surface.emission;
    float power = lerp(64.0, 4.0, surface.roughness);
    for (int index = 0; index < 4; index++)
    {
        float3 to_light = lights.positions[index].xyz - position;
        float distance_squared = dot(to_light, to_light);
        to_light = to_light / sqrt(distance_squared);
        float attenuation = lights.colors[index].a / (1.0 + distance_squared);
        float diffuse = max(dot(surface.normal, to_light), 0.0);
        float3 halfway = normalize(to_light + to_camera);
        float specular = pow(max(dot(surface.normal, halfway), 0.0), power) * SPECULAR_STRENGTH;
        color += (surface.base_color * diffuse + specular) * lights.colors[index].rgb * attenuation;
    }
    return color;
}
