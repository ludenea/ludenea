//! Graph coloring of contact constraints, so constraints of one color share no dynamic
//! body and can be solved in parallel.

/// Colors tried before a constraint goes to the overflow group.
pub const GRAPH_COLOR_COUNT: usize = 12;

/// Constraints or bodies per parallel task.
pub const SOLVER_CHUNK: usize = 64;

/// Greedy coloring of touching contacts given in pair order as body index pairs: each
/// constraint takes the lowest color holding neither of its dynamic bodies. Static and
/// kinematic bodies are never written by the solver, so they never block a color.
/// Returns `GRAPH_COLOR_COUNT` colors then an overflow group for constraints that fit no
/// color, each holding ascending constraint indices.
pub fn color_constraints(pairs: &[(u32, u32)], is_dynamic: &[bool]) -> Vec<Vec<u32>> {
    let mut groups = vec![Vec::new(); GRAPH_COLOR_COUNT + 1];
    let mut used = vec![[false; GRAPH_COLOR_COUNT]; is_dynamic.len()];
    for (index, &(a, b)) in pairs.iter().enumerate() {
        let bodies = [a as usize, b as usize].map(|body| is_dynamic[body].then_some(body));
        let color = (0..GRAPH_COLOR_COUNT)
            .find(|&color| bodies.iter().flatten().all(|&body| !used[body][color]))
            .unwrap_or(GRAPH_COLOR_COUNT);
        if color < GRAPH_COLOR_COUNT {
            for &body in bodies.iter().flatten() {
                used[body][color] = true;
            }
        }
        groups[color].push(index as u32);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dynamic_bodies(groups: &[Vec<u32>], pairs: &[(u32, u32)], is_dynamic: &[bool]) {
        for group in &groups[..GRAPH_COLOR_COUNT] {
            let mut seen = vec![false; is_dynamic.len()];
            for &index in group {
                let (a, b) = pairs[index as usize];
                for body in [a as usize, b as usize] {
                    if is_dynamic[body] {
                        assert!(!seen[body], "body {body} appears twice in one color");
                        seen[body] = true;
                    }
                }
            }
        }
    }

    #[test]
    fn no_color_shares_a_dynamic_body() {
        // A chain and a grid of dynamic bodies plus a static body at index 0.
        let mut pairs = Vec::new();
        for i in 1..40u32 {
            pairs.push((0, i));
            pairs.push((i, i + 1));
            if i + 7 <= 40 {
                pairs.push((i, i + 7));
            }
        }
        pairs.sort_unstable();
        let mut is_dynamic = vec![true; 41];
        is_dynamic[0] = false;
        let groups = color_constraints(&pairs, &is_dynamic);
        assert_eq!(groups.len(), GRAPH_COLOR_COUNT + 1);
        dynamic_bodies(&groups, &pairs, &is_dynamic);
        let mut all: Vec<u32> = groups.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, (0..pairs.len() as u32).collect::<Vec<_>>());
        for group in &groups {
            assert!(group.is_sorted(), "group not ascending: {group:?}");
        }
    }

    #[test]
    fn static_body_does_not_block_colors() {
        let pairs: Vec<(u32, u32)> = (1..=10).map(|i| (0, i)).collect();
        let mut is_dynamic = vec![true; 11];
        is_dynamic[0] = false;
        let groups = color_constraints(&pairs, &is_dynamic);
        assert_eq!(groups[0], (0..10).collect::<Vec<u32>>());
        assert!(groups[1..].iter().all(Vec::is_empty));
    }

    #[test]
    fn overflow_receives_constraints_beyond_twelve_colors() {
        let pairs: Vec<(u32, u32)> = (1..=14).map(|i| (0, i)).collect();
        let is_dynamic = vec![true; 15];
        let groups = color_constraints(&pairs, &is_dynamic);
        for (color, group) in groups[..GRAPH_COLOR_COUNT].iter().enumerate() {
            assert_eq!(group, &[color as u32]);
        }
        assert_eq!(groups[GRAPH_COLOR_COUNT], [12, 13]);
    }

    #[test]
    fn coloring_is_deterministic() {
        let pairs: Vec<(u32, u32)> = (0..200u32).map(|i| (i % 23, 23 + (i * 7) % 31)).collect();
        let is_dynamic: Vec<bool> = (0..54).map(|i| i % 5 != 0).collect();
        let first = color_constraints(&pairs, &is_dynamic);
        assert_eq!(first, color_constraints(&pairs, &is_dynamic));
        dynamic_bodies(&first, &pairs, &is_dynamic);
    }
}
