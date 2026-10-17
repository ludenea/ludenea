//! Broad phase: candidate pairs from overlapping bounding boxes.

use crate::components::BodyKind;
use ludenea_math::Aabb;

/// A collider's bounding box, fattened by the speculative distance, tagged with the
/// index of its body in the step's body array.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Proxy {
    pub aabb: Aabb,
    pub body: u32,
    pub kind: BodyKind,
}

/// Pairs of overlapping proxies with at least one dynamic body, as `(lower, higher)`
/// body indices sorted ascending. Each body has at most one proxy.
// ponytail: sort and sweep rebuilt every tick; dynamic tree if body counts grow past a few thousand
pub fn find_pairs(proxies: &[Proxy]) -> Vec<(u32, u32)> {
    let mut order: Vec<&Proxy> = proxies.iter().collect();
    // Body indices are unique, so the key has no equal elements.
    order.sort_unstable_by(|a, b| {
        a.aabb
            .min
            .x
            .total_cmp(&b.aabb.min.x)
            .then(a.body.cmp(&b.body))
    });
    let mut pairs = Vec::new();
    for (i, a) in order.iter().enumerate() {
        for b in &order[i + 1..] {
            if b.aabb.min.x > a.aabb.max.x {
                break;
            }
            let any_dynamic = a.kind == BodyKind::Dynamic || b.kind == BodyKind::Dynamic;
            if any_dynamic && a.aabb.overlaps(b.aabb) {
                pairs.push((a.body.min(b.body), a.body.max(b.body)));
            }
        }
    }
    pairs.sort_unstable();
    pairs
}
