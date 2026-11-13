//! The platform's math functions themselves, as a plain `f32` method reaches
//! them, against the pure Rust library the determinism feature routes the
//! other calls to.
//!
//! The reference is the same computation on every cell, so two cells return
//! the same bits over a sweep exactly when their lists of differences from
//! the reference are the same. Each sweep reports the hash of the platform's
//! results, the hash of the reference's, the count of inputs on which the
//! two differ and the hash of those differences, and writes the differences
//! to a file a later comparison of two cells reads.

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::Path;

use reference_workload::{Fnv1a, Json};

/// The most differences a sweep writes to its file; the count and the hash
/// cover all of them.
const WRITTEN_DIFFERENCES: usize = 20_000;

/// Every sixteenth `f32` between two positive bounds, by its bits.
fn spread(from: f32, to: f32) -> impl Iterator<Item = f32> {
    (from.to_bits()..to.to_bits()).step_by(16).map(f32::from_bits)
}

fn both_signs(values: impl Iterator<Item = f32>) -> impl Iterator<Item = f32> {
    values.flat_map(|value| [value, -value])
}

pub struct Sweep {
    pub name: &'static str,
    pub what: &'static str,
    pub domain: &'static str,
    inputs: fn() -> Box<dyn Iterator<Item = f32>>,
    /// The call as the libraries write it: a method on a plain `f32`.
    platform: fn(f32) -> [u32; 2],
    /// The same function of the pure Rust library.
    reference: fn(f32) -> [u32; 2],
    results: usize,
}

fn one(value: f32) -> [u32; 2] {
    [value.to_bits(), 0]
}

pub fn sweeps() -> Vec<Sweep> {
    vec![
        Sweep {
            name: "sin_cos",
            what: "the sine and cosine taken together, as an angular joint limit and a convex polyhedron's feature tolerance take them",
            domain: "every sixteenth value of [2^-6, 4), both signs",
            inputs: || Box::new(both_signs(spread(0.015_625, 4.0))),
            platform: |x| {
                let (sin, cos) = x.sin_cos();
                [sin.to_bits(), cos.to_bits()]
            },
            reference: |x| {
                let (sin, cos) = libm::sincosf(x);
                [sin.to_bits(), cos.to_bits()]
            },
            results: 2,
        },
        Sweep {
            name: "sin",
            what: "the sine alone, as a multibody joint's motor takes it of half its target angle",
            domain: "every sixteenth value of [2^-6, 4), both signs",
            inputs: || Box::new(both_signs(spread(0.015_625, 4.0))),
            platform: |x| one(x.sin()),
            reference: |x| one(libm::sinf(x)),
            results: 1,
        },
        Sweep {
            name: "cos",
            what: "the cosine alone, as the eigenvalues of a symmetric matrix take it",
            domain: "every sixteenth value of [2^-6, 4), both signs",
            inputs: || Box::new(both_signs(spread(0.015_625, 4.0))),
            platform: |x| one(x.cos()),
            reference: |x| one(libm::cosf(x)),
            results: 1,
        },
        Sweep {
            name: "acos",
            what: "the arc cosine, as the eigenvalues of a symmetric matrix and the angle between two vectors take it",
            domain: "every sixteenth value of [2^-8, 1), both signs",
            inputs: || Box::new(both_signs(spread(0.003_906_25, 1.0))),
            platform: |x| one(x.acos()),
            reference: |x| one(libm::acosf(x)),
            results: 1,
        },
        Sweep {
            name: "log2",
            what: "the base-2 logarithm of a count, as the incremental optimization of the broad phase's tree takes it",
            domain: "every whole number from 1 to 2^20",
            inputs: || Box::new((1..=1_u32 << 20).map(|count| count as f32)),
            platform: |x| one(x.log2()),
            reference: |x| one(libm::log2f(x)),
            results: 1,
        },
    ]
}

pub struct Swept {
    pub inputs: u64,
    pub platform_hash: u64,
    pub reference_hash: u64,
    pub differing_inputs: u64,
    pub differences_hash: u64,
    /// The largest distance between the two results, in units of the last place.
    pub largest_distance: u32,
    /// `input platform reference` per differing input, as bits in hexadecimal.
    pub differences: String,
}

fn distance(a: u32, b: u32) -> u32 {
    // Results of one sign, or zeros: the distance between their bits.
    if (a ^ b) >> 31 == 0 { a.abs_diff(b) } else { (a & 0x7fff_ffff) + (b & 0x7fff_ffff) }
}

pub fn sweep(sweep: &Sweep) -> Swept {
    let (mut platform_hash, mut reference_hash, mut differences_hash) = (Fnv1a::new(), Fnv1a::new(), Fnv1a::new());
    let mut swept = Swept { inputs: 0, platform_hash: 0, reference_hash: 0, differing_inputs: 0, differences_hash: 0, largest_distance: 0, differences: String::new() };
    for input in (sweep.inputs)() {
        let input = black_box(input);
        let (platform, reference) = ((sweep.platform)(input), (sweep.reference)(input));
        swept.inputs += 1;
        for result in 0..sweep.results {
            platform_hash.write(&platform[result].to_le_bytes());
            reference_hash.write(&reference[result].to_le_bytes());
        }
        if platform != reference {
            swept.differing_inputs += 1;
            differences_hash.write(&input.to_bits().to_le_bytes());
            let mut line = format!("{:08x}", input.to_bits());
            for result in 0..sweep.results {
                differences_hash.write(&platform[result].to_le_bytes());
                swept.largest_distance = swept.largest_distance.max(distance(platform[result], reference[result]));
                let _ = write!(line, " {:08x} {:08x}", platform[result], reference[result]);
            }
            if swept.differing_inputs as usize <= WRITTEN_DIFFERENCES {
                swept.differences.push_str(&line);
                swept.differences.push('\n');
            }
        }
    }
    swept.platform_hash = platform_hash.finish();
    swept.reference_hash = reference_hash.finish();
    swept.differences_hash = differences_hash.finish();
    swept
}

/// The sine and cosine of the values the worlds hand to the platform by
/// construction, as the platform returns them: one line per value.
pub fn constants(out: &mut Json, name: &str, values: &[f32]) {
    let mut hash = Fnv1a::new();
    let mut lines = String::new();
    for &value in values {
        let (sin, cos) = black_box(value).sin_cos();
        let (reference_sin, reference_cos) = libm::sincosf(value);
        hash.write(&sin.to_bits().to_le_bytes());
        hash.write(&cos.to_bits().to_le_bytes());
        let _ = write!(lines, "{:08x}:{:08x},{:08x}", value.to_bits(), sin.to_bits(), cos.to_bits());
        if (sin.to_bits(), cos.to_bits()) != (reference_sin.to_bits(), reference_cos.to_bits()) {
            lines.push('*');
        }
        lines.push(' ');
    }
    out.key(name);
    out.object_start();
    out.u64("values", values.len() as u64);
    out.hex("platform_hash", hash.finish());
    out.u64("differing_from_the_reference", lines.matches('*').count() as u64);
    out.string("input_sine_cosine", lines.trim_end());
    out.object_end();
}

pub fn write(out: &mut Json, output: &Path) {
    let directory = output.join("functions");
    let _ = fs::create_dir_all(&directory);
    out.key("functions");
    out.array_start();
    for function in sweeps() {
        let swept = sweep(&function);
        let _ = fs::write(directory.join(format!("{}.txt", function.name)), &swept.differences);
        out.object_start();
        out.string("function", function.name);
        out.string("what", function.what);
        out.string("domain", function.domain);
        out.u64("inputs", swept.inputs);
        out.hex("platform_hash", swept.platform_hash);
        out.hex("reference_hash", swept.reference_hash);
        out.u64("inputs_differing_from_the_reference", swept.differing_inputs);
        out.hex("differences_hash", swept.differences_hash);
        out.u64("largest_distance_in_units_of_the_last_place", u64::from(swept.largest_distance));
        out.u64("differences_written", swept.differences.lines().count() as u64);
        out.object_end();
    }
    out.array_end();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spread_is_ordered_and_stays_inside_its_bounds() {
        let values: Vec<f32> = spread(1.0, 2.0).collect();
        assert_eq!(values.len(), (1 << 23) / 16);
        assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(values.iter().all(|value| (1.0..2.0).contains(value)));
    }

    #[test]
    fn the_distance_counts_the_floats_between_two_results() {
        assert_eq!(distance(1.0_f32.to_bits(), 1.0_f32.to_bits()), 0);
        assert_eq!(distance(1.0_f32.to_bits(), f32::from_bits(1.0_f32.to_bits() + 3).to_bits()), 3);
        assert_eq!(distance(0.0_f32.to_bits(), (-0.0_f32).to_bits()), 0);
        assert_eq!(distance(f32::from_bits(1).to_bits(), (-f32::from_bits(2)).to_bits()), 3);
    }

    #[test]
    fn a_sweep_of_the_reference_against_itself_finds_no_difference() {
        let function = Sweep { name: "self", what: "", domain: "", inputs: || Box::new(spread(1.0, 1.001)), platform: |x| one(libm::sinf(x)), reference: |x| one(libm::sinf(x)), results: 1 };
        let swept = sweep(&function);
        assert!(swept.inputs > 0);
        assert_eq!(swept.differing_inputs, 0);
        assert_eq!(swept.platform_hash, swept.reference_hash);
        assert!(swept.differences.is_empty());
    }

    #[test]
    fn a_sweep_reports_each_differing_input_once() {
        let function = Sweep { name: "off", what: "", domain: "", inputs: || Box::new([1.0_f32, 2.0, 3.0].into_iter()), platform: |x| one(x), reference: |x| one(if x == 2.0 { f32::from_bits(x.to_bits() + 2) } else { x }), results: 1 };
        let swept = sweep(&function);
        assert_eq!((swept.inputs, swept.differing_inputs, swept.largest_distance), (3, 1, 2));
        assert_eq!(swept.differences, "40000000 40000000 40000002\n");
    }
}
