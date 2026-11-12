// The engine's side of a lit shader, composed by text with a surface
// function: the engine's coordinate type and its sampling function, the
// properties a surface returns, the frame's constants and the lighting.
//
// A coordinate travels with its differentials as one value, and a surface
// samples through `engine_sample` only: in a raster pass the function is
// plain sampling, since the hardware supplies the differentials there.

struct Coordinate {
    vec2 value;
    vec2 ddx;
    vec2 ddy;
};

Coordinate coordinate_scaled(Coordinate c, vec2 scale) {
    return Coordinate(c.value * scale, c.ddx * scale, c.ddy * scale);
}

Coordinate coordinate_rotated(Coordinate c, float cosine, float sine) {
    mat2 turn = mat2(cosine, sine, -sine, cosine);
    return Coordinate(turn * c.value, turn * c.ddx, turn * c.ddy);
}

vec4 engine_sample(texture2D image, sampler filtering, Coordinate c) {
    return texture(sampler2D(image, filtering), c.value);
}

struct Surface {
    vec3 base_color;
    float roughness;
    vec3 normal;
    vec3 emission;
};

struct SurfaceInput {
    Coordinate uv;
    vec3 position;
    vec3 normal;
};

layout(set = 0, binding = 0) uniform View {
    vec4 camera;
} view;

layout(set = 0, binding = 1) uniform Lights {
    vec4 ambient;
    vec4 positions[4];
    vec4 colors[4];
} lights;

const float SPECULAR_STRENGTH = 0.25;

vec3 light_surface(Surface surface, vec3 position) {
    vec3 to_camera = normalize(view.camera.xyz - position);
    vec3 color = surface.base_color * lights.ambient.rgb + surface.emission;
    float power = mix(64.0, 4.0, surface.roughness);
    for (int index = 0; index < 4; index++) {
        vec3 to_light = lights.positions[index].xyz - position;
        float distance_squared = dot(to_light, to_light);
        to_light = to_light / sqrt(distance_squared);
        float attenuation = lights.colors[index].a / (1.0 + distance_squared);
        float diffuse = max(dot(surface.normal, to_light), 0.0);
        vec3 halfway = normalize(to_light + to_camera);
        float specular = pow(max(dot(surface.normal, halfway), 0.0), power) * SPECULAR_STRENGTH;
        color += (surface.base_color * diffuse + specular) * lights.colors[index].rgb * attenuation;
    }
    return color;
}
