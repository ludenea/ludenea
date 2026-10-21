// One instanced quad per sprite or tile, drawn as a 4-vertex triangle strip.
//
// Instances are in world space and hold both ticks: the centre (the pivot) at the previous
// tick and at the current one, the rotation at the previous tick, and the arc to the current
// rotation along the shorter way. The view's `alpha` blends them: the centre moves along the
// line, the rotation turns by `arc * alpha`, so a draw between two ticks needs no new
// instances. A local point `p` (world units from the pivot, +y up) then lands at
// `center + axis_x * p.x + axis_y * p.y`, the axes being the blended rotation. A flip negates
// `offset` and `size` on its axis, which mirrors the quad around the pivot. The layer's view,
// the camera seen through the layer's parallax, then puts the world point in clip space.

struct Instance {
    @location(0) previous_center: vec2<f32>,
    @location(1) current_center: vec2<f32>,
    // Cosine then sine.
    @location(2) rotation: vec2<f32>,
    @location(3) arc: f32,
    // The quad's bottom-left corner and size in world units, from the pivot.
    @location(4) offset: vec2<f32>,
    @location(5) size: vec2<f32>,
    // The region in the page, from its top-left texel, in normalized coordinates.
    @location(6) uv_min: vec2<f32>,
    @location(7) uv_size: vec2<f32>,
    @location(8) tint: vec4<f32>,
}

// World to clip space: `world * scale + translate`; `alpha` is how far from the previous tick
// to the current one, 0 to 1. The padding makes the view 32 bytes, as WebGL2 lays it out.
struct View {
    scale: vec2<f32>,
    translate: vec2<f32>,
    alpha: f32,
    padding: f32,
    padding_end: vec2<f32>,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
}

@group(0) @binding(0) var page: texture_2d<f32>;
@group(0) @binding(1) var page_sampler: sampler;
@group(1) @binding(0) var<uniform> view: View;

@vertex
fn vs(@builtin(vertex_index) vertex: u32, instance: Instance) -> Varyings {
    let center = mix(instance.previous_center, instance.current_center, view.alpha);
    let turn = instance.arc * view.alpha;
    let step = vec2<f32>(cos(turn), sin(turn));
    let r = instance.rotation;
    let turned = vec2<f32>(r.x * step.x - r.y * step.y, r.x * step.y + r.y * step.x);
    // Without an arc the rotation is the instance's, exactly.
    let axis_x = select(turned, r, instance.arc == 0.0);
    let axis_y = vec2<f32>(-axis_x.y, axis_x.x);
    let corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let local = instance.offset + corner * instance.size;
    let world = center + axis_x * local.x + axis_y * local.y;
    let position = world * view.scale + view.translate;
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
