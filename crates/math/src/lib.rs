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
    use proptest::prelude::*;

    #[test]
    fn lerp_halfway_returns_the_midpoint() {
        assert_eq!(lerp(2.0, 6.0, 0.5), 4.0);
    }

    #[test]
    fn lerp_samples_across_the_range() {
        let samples: Vec<_> = (0..=4u8)
            .map(|step| {
                let t = f32::from(step) / 4.0;
                (t, lerp(-10.0, 30.0, t))
            })
            .collect();

        insta::assert_debug_snapshot!(samples);
    }

    proptest! {
        #[test]
        #[expect(clippy::float_cmp, reason = "the property is exact equality")]
        fn lerp_at_zero_returns_the_start_exactly(start in -1e6f32..1e6, end in -1e6f32..1e6) {
            prop_assert_eq!(lerp(start, end, 0.0), start);
        }
    }
}
