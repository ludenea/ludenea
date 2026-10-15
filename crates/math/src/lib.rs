//! Deterministic math for the Ludenea engine.

/// Linearly interpolates between `start` and `end`, where `t = 0` gives
/// `start` and `t = 1` gives `end`.
#[must_use]
pub fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_halfway_returns_the_midpoint() {
        assert_eq!(lerp(2.0, 6.0, 0.5), 4.0);
    }
}
