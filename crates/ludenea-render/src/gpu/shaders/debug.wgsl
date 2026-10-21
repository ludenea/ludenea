// The debug draw: triangles in world space, one colour per vertex, put in clip space by the
// view (the camera with parallax 1).

// The quad shader's view; the debug draw is the current tick's, so it ignores `alpha`.
struct View {
    scale: vec2<f32>,
    translate: vec2<f32>,
    alpha: f32,
    padding: f32,
    padding_end: vec2<f32>,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;

@vertex
fn vs(@location(0) world: vec2<f32>, @location(1) color: vec4<f32>) -> Varyings {
    return Varyings(vec4<f32>(world * view.scale + view.translate, 0.0, 1.0), color);
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}
