// The filtering and blending scene of the device baseline spike in WGSL, for
// a device on which wgpu offers no array of textures: the five textures and
// the three samplers are eight bindings, and the fragment function chooses
// among them by the indexes of the draw, which arrive as immediate data. The
// choice is uniform control flow, so each branch samples with the implicit
// level of detail as the array form does.

struct DrawIndexes {
    texture_index: u32,
    sampler_index: u32,
}

var<immediate> draw: DrawIndexes;

@group(0) @binding(0) var texture_0: texture_2d<f32>;
@group(0) @binding(1) var texture_1: texture_2d<f32>;
@group(0) @binding(2) var texture_2: texture_2d<f32>;
@group(0) @binding(3) var texture_3: texture_2d<f32>;
@group(0) @binding(4) var texture_4: texture_2d<f32>;
@group(0) @binding(5) var sampler_0: sampler;
@group(0) @binding(6) var sampler_1: sampler;
@group(0) @binding(7) var sampler_2: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn scene_vertex(@location(0) position: vec4<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> Varyings {
    var out: Varyings;
    out.position = position;
    out.uv = uv;
    out.color = color;
    return out;
}

@fragment
fn scene_fragment(in: Varyings) -> @location(0) vec4<f32> {
    var texel: vec4<f32>;
    switch draw.texture_index * 3u + draw.sampler_index {
        case 0u { texel = textureSample(texture_0, sampler_0, in.uv); }
        case 1u { texel = textureSample(texture_0, sampler_1, in.uv); }
        case 2u { texel = textureSample(texture_0, sampler_2, in.uv); }
        case 3u { texel = textureSample(texture_1, sampler_0, in.uv); }
        case 4u { texel = textureSample(texture_1, sampler_1, in.uv); }
        case 5u { texel = textureSample(texture_1, sampler_2, in.uv); }
        case 6u { texel = textureSample(texture_2, sampler_0, in.uv); }
        case 7u { texel = textureSample(texture_2, sampler_1, in.uv); }
        case 8u { texel = textureSample(texture_2, sampler_2, in.uv); }
        case 9u { texel = textureSample(texture_3, sampler_0, in.uv); }
        case 10u { texel = textureSample(texture_3, sampler_1, in.uv); }
        case 11u { texel = textureSample(texture_3, sampler_2, in.uv); }
        case 12u { texel = textureSample(texture_4, sampler_0, in.uv); }
        case 13u { texel = textureSample(texture_4, sampler_1, in.uv); }
        default { texel = textureSample(texture_4, sampler_2, in.uv); }
    }
    return texel * in.color;
}
