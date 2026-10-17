use ludenea_ecs::Resource;
use serde::{Deserialize, Serialize};

const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// PCG32 (XSH RR), the seeded random number generator of the simulation. Its state is
/// part of the world, so snapshots and hashes cover it.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[resource(name = "ludenea::Rng")]
pub struct Rng {
    state: u64,
    increment: u64,
}

impl Rng {
    /// Seeds like the reference `pcg32_srandom_r(seed, stream)`.
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut rng = Self {
            state: 0,
            increment: (stream << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.increment);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        xorshifted.rotate_right((old >> 59) as u32)
    }

    /// A uniform value in `[low, high)`, by Lemire's multiply-and-reject method.
    ///
    /// # Panics
    /// If `high <= low`.
    pub fn range_u32(&mut self, low: u32, high: u32) -> u32 {
        assert!(
            high > low,
            "range_u32 needs low < high, got [{low}, {high})"
        );
        let range = high - low;
        let mut product = u64::from(self.next_u32()) * u64::from(range);
        if (product as u32) < range {
            let threshold = range.wrapping_neg() % range;
            while (product as u32) < threshold {
                product = u64::from(self.next_u32()) * u64::from(range);
            }
        }
        low + (product >> 32) as u32
    }

    /// A value in `[low, high)` from the top 24 bits of `next_u32`.
    pub fn range_f32(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * ((self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0))
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn pcg32_matches_reference_sequence() {
        let mut rng = Rng::new(42, 54);
        let values: Vec<u32> = (0..6).map(|_| rng.next_u32()).collect();
        assert_eq!(
            values,
            [
                0xa15c02b7, 0x7b47f409, 0xba1d3330, 0x83d2f293, 0xbfa4784b, 0xcbed606e
            ]
        );
    }

    #[test]
    fn range_u32_stays_in_bounds() {
        let mut rng = Rng::new(1, 2);
        let mut seen = [false; 7];
        for _ in 0..10_000 {
            let value = rng.range_u32(3, 10);
            assert!((3..10).contains(&value));
            seen[(value - 3) as usize] = true;
        }
        assert!(seen.iter().all(|&hit| hit));
        assert_eq!(rng.range_u32(5, 6), 5);
        let _ = rng.range_u32(0, u32::MAX);
    }

    #[test]
    fn range_f32_stays_in_bounds() {
        let mut rng = Rng::new(9, 9);
        for _ in 0..10_000 {
            let value = rng.range_f32(-2.0, 3.0);
            assert!((-2.0..3.0).contains(&value));
        }
    }

    #[test]
    #[should_panic(expected = "range_u32 needs low < high")]
    fn range_u32_rejects_empty_range() {
        Rng::new(0, 0).range_u32(4, 4);
    }
}
