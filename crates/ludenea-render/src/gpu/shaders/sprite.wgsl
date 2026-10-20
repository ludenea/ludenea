// One instanced quad per sprite, drawn as a 4-vertex triangle strip.
//
// The CPU resolves the camera: `center` is the pivot in clip space, and `axis_x` and `axis_y`
// carry the rotation and the clip-per-world-unit scale, so a local point `p` (world units from
// the pivot, +y up) lands at `center + axis_x * p.x + axis_y * p.y`. A flip negates `offset`
// and `size` on its axis, which mirrors the quad around the pivot.

struct Instance {
    @location(0) center: vec2<f32>,
    @location(1) axis_x: vec2<f32>,
    @location(2) axis_y: vec2<f32>,
    // The quad's bottom-left corner and size in world units, from the pivot.
    @location(3) offset: vec2<f32>,
    @location(4) size: vec2<f32>,
    // The region in the page, from its top-left texel, in normalized coordinates.
    @location(5) uv_min: vec2<f32>,
    @location(6) uv_size: vec2<f32>,
    @location(7) tint: vec4<f32>,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
}

@group(0) @binding(0) var page: texture_2d<f32>;
@group(0) @binding(1) var page_sampler: sampler;

@vertex
fn vs(@builtin(vertex_index) vertex: u32, instance: Instance) -> Varyings {
    let corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let local = instance.offset + corner * instance.size;
    let position = instance.center + instance.axis_x * local.x + instance.axis_y * local.y;
    // Local +y is up, the page's v grows down.
    let uv = instance.uv_min + vec2<f32>(corner.x, 1.0 - corner.y) * instance.uv_size;
    return Varyings(vec4<f32>(position, 0.0, 1.0), uv, instance.tint);
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    let color = textureSample(page, page_sampler, in.uv) * in.tint;
    // Premultiplied for the blend state.
    return vec4<f32>(color.rgb * color.a, color.a);
}
