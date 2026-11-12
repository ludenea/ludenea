// The filtering and blending scene of the device baseline spike in WGSL, for
// wgpu's safe level, which translates it for each interface.
//
// Clip space has y up and a depth range of 0 to 1 in wgpu, as the scene is
// authored, so the vertex function passes the position through unchanged and
// interpolates the texture coordinate and the color with perspective. The
// fragment function samples the texture and the sampler the draw names: their
// indexes arrive as immediate data, the textures as one array of five
// textures and the samplers as one array of three samplers.

enable wgpu_binding_array;

struct DrawIndexes {
    texture_index: u32,
    sampler_index: u32,
}

var<immediate> draw: DrawIndexes;

@group(0) @binding(0) var scene_textures: binding_array<texture_2d<f32>, 5>;
@group(0) @binding(1) var scene_samplers: binding_array<sampler, 3>;

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
    return textureSample(scene_textures[draw.texture_index], scene_samplers[draw.sampler_index], in.uv) * in.color;
}
