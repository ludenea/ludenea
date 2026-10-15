//! Golden hash: a fixed floating point computation from `ludenea_math`,
//! hashed canonically. Every target, build profile and thread count must
//! produce the committed reference hash.
//!
//! The computation only uses correctly rounded IEEE 754 operations, which
//! every target must reproduce bit for bit, so today the hash catches a
//! compiler or target breaking that guarantee, such as a contraction into a
//! fused multiply-add. It catches platform rounding differences once
//! `ludenea_math` provides the transcendental functions and they join it.

use std::thread;

use ludenea_math::lerp;

/// Number of values the golden computation produces.
pub const SAMPLES: u16 = 4096;

/// Thread counts every target checks; WASI has no threads.
#[cfg(not(target_os = "wasi"))]
pub const THREAD_COUNTS: &[usize] = &[1, 2, 3, 8];
#[cfg(target_os = "wasi")]
pub const THREAD_COUNTS: &[usize] = &[1];

/// Canonical bits of the quiet NaN every NaN is hashed as.
const CANONICAL_NAN: u32 = 0x7fc0_0000;

/// Computes the golden values split across `threads` threads (zero counts
/// as one) and hashes them in index order.
///
/// # Panics
///
/// Panics when a worker thread panics, which the computation never does.
#[must_use]
pub fn hash(threads: usize) -> u64 {
    let indices: Vec<u16> = (0..SAMPLES).collect();
    let chunk_length = indices.len().div_ceil(threads.max(1));
    let values: Vec<f32> = if threads <= 1 {
        indices.iter().copied().map(sample).collect()
    } else {
        thread::scope(|scope| {
            let workers: Vec<_> = indices
                .chunks(chunk_length)
                .map(|chunk| scope.spawn(|| chunk.iter().copied().map(sample).collect::<Vec<_>>()))
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().expect("a golden worker never panics"))
                .collect()
        })
    };
    canonical_hash(&values)
}

/// FNV-1a 64 over the little-endian bytes of each value, with every NaN
/// replaced by the canonical quiet NaN. Signed zeros stay distinct.
#[must_use]
pub fn canonical_hash(values: &[f32]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    values
        .iter()
        .map(|value| {
            if value.is_nan() {
                CANONICAL_NAN
            } else {
                value.to_bits()
            }
        })
        .flat_map(u32::to_le_bytes)
        .fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
}

/// Inputs of sample `index`, chosen so the products are inexact and a fused
/// multiply-add rounds differently.
fn inputs(index: u16) -> (f32, f32, f32) {
    let start = f32::from(index) * 0.1 - 200.0;
    let end = start * -3.7 + 11.3;
    let t = f32::from(index % 97) / 97.0;
    (start, end, t)
}

fn sample(index: u16) -> f32 {
    let (start, end, t) = inputs(index);
    lerp(start, end, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_pins_the_algorithm_and_the_byte_order() {
        // FNV-1a 64 of the little-endian bytes of 1.0f32: 00 00 80 3f.
        assert_eq!(canonical_hash(&[1.0]), 0x4b72_477f_9c5c_2f98);
    }

    #[test]
    fn every_nan_hashes_like_the_canonical_nan() {
        let payload_nan = f32::from_bits(0x7fc0_0001);

        assert_eq!(canonical_hash(&[payload_nan]), canonical_hash(&[f32::NAN]));
        assert_eq!(canonical_hash(&[-f32::NAN]), canonical_hash(&[f32::NAN]));
    }

    #[test]
    fn signed_zeros_hash_differently() {
        assert_ne!(canonical_hash(&[0.0]), canonical_hash(&[-0.0]));
    }

    #[test]
    fn the_hash_depends_on_the_order_of_values() {
        assert_ne!(canonical_hash(&[1.0, 2.0]), canonical_hash(&[2.0, 1.0]));
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore = "WASI has no threads")]
    fn every_thread_count_gives_the_same_hash() {
        for &threads in THREAD_COUNTS {
            assert_eq!(hash(threads), hash(1), "with {threads} threads");
        }
    }

    #[test]
    fn the_computation_detects_fused_multiply_add() {
        let fused = (0..SAMPLES).any(|index| {
            let (start, end, t) = inputs(index);
            (end - start).mul_add(t, start).to_bits() != sample(index).to_bits()
        });

        assert!(
            fused,
            "no sample rounds differently with a fused multiply-add"
        );
    }

    #[test]
    fn the_hash_matches_the_committed_reference() {
        let reference = include_str!("../golden-hash.txt").trim();

        assert_eq!(format!("{:016x}", hash(1)), reference);
    }
}
