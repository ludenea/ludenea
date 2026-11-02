//! Platform facts probe for the cross-platform compatibility research.
//!
//! Throwaway spike, never product code. It prints one JSON object describing
//! the machine, the build configuration and the bit-exact results of a fixed
//! set of floating-point probes. The probes are grouped by the property they
//! test, and each group is hashed, so that the cells of the research CI matrix
//! can be compared at a glance:
//!
//! - `exact`: operations IEEE 754 defines exactly (basic arithmetic, square
//!   root, fused multiply-add, conversions). Expected identical everywhere.
//! - `codegen`: patterns a compiler could contract into fused multiply-adds or
//!   reassociate. Identical results mean the compiler did neither.
//! - `libm`: transcendental functions served by the platform math library.
//!   Differences across cells are expected and are the fact being measured.
//! - `nan`: the bit patterns of NaNs produced by invalid operations, which
//!   IEEE 754 leaves to the hardware.
//!
//! Values are printed as hexadecimal bit patterns; `f32` values are their
//! 32-bit pattern, zero-extended. Timings are informational (shared runners).

use std::hint::black_box;
use std::time::Instant;

const SCHEMA_VERSION: u64 = 1;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const TIMING_ITERATIONS: u32 = 1_000_000;
const TIMING_REPETITIONS: usize = 5;

struct Probe {
    name: &'static str,
    bits: u64,
}

fn main() {
    let cell = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "unspecified".to_string());
    let groups: [(&str, Vec<Probe>); 4] = [
        ("exact", exact_probes()),
        ("codegen", codegen_probes()),
        ("libm", libm_probes()),
        ("nan", nan_probes()),
    ];

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("cell", &cell);
    write_platform(&mut out);
    write_build(&mut out);
    out.key("hashes");
    out.object_start();
    for (group, probes) in &groups {
        out.hex(group, group_hash(probes));
    }
    out.object_end();
    out.key("probes");
    out.object_start();
    for (group, probes) in &groups {
        out.key(group);
        out.object_start();
        for probe in probes {
            out.hex(probe.name, probe.bits);
        }
        out.object_end();
    }
    out.object_end();
    write_timing(&mut out);
    out.object_end();
    println!("{}", out.finish());
}

fn write_platform(out: &mut Json) {
    out.key("platform");
    out.object_start();
    out.string("os", std::env::consts::OS);
    out.string("family", std::env::consts::FAMILY);
    out.string("arch", std::env::consts::ARCH);
    out.u64("pointer_width_bits", u64::from(usize::BITS));
    out.string(
        "endian",
        if cfg!(target_endian = "little") { "little" } else { "big" },
    );
    out.u64(
        "available_parallelism",
        std::thread::available_parallelism().map_or(0, |n| n.get() as u64),
    );
    out.key("runtime_cpu_features");
    out.object_start();
    write_runtime_cpu_features(out);
    out.object_end();
    out.object_end();
}

#[cfg(target_arch = "x86_64")]
fn write_runtime_cpu_features(out: &mut Json) {
    out.bool("sse2", std::arch::is_x86_feature_detected!("sse2"));
    out.bool("sse4.2", std::arch::is_x86_feature_detected!("sse4.2"));
    out.bool("avx", std::arch::is_x86_feature_detected!("avx"));
    out.bool("avx2", std::arch::is_x86_feature_detected!("avx2"));
    out.bool("fma", std::arch::is_x86_feature_detected!("fma"));
    out.bool("avx512f", std::arch::is_x86_feature_detected!("avx512f"));
}

#[cfg(target_arch = "aarch64")]
fn write_runtime_cpu_features(out: &mut Json) {
    out.bool("neon", std::arch::is_aarch64_feature_detected!("neon"));
    out.bool("fp16", std::arch::is_aarch64_feature_detected!("fp16"));
    out.bool("sve", std::arch::is_aarch64_feature_detected!("sve"));
    out.bool("dotprod", std::arch::is_aarch64_feature_detected!("dotprod"));
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn write_runtime_cpu_features(_out: &mut Json) {}

fn write_build(out: &mut Json) {
    out.key("build");
    out.object_start();
    out.string("package_version", env!("CARGO_PKG_VERSION"));
    out.bool("debug_assertions", cfg!(debug_assertions));
    out.string("target_env", target_env());
    out.key("compile_time_target_features");
    out.object_start();
    out.bool("sse2", cfg!(target_feature = "sse2"));
    out.bool("sse4.2", cfg!(target_feature = "sse4.2"));
    out.bool("avx", cfg!(target_feature = "avx"));
    out.bool("avx2", cfg!(target_feature = "avx2"));
    out.bool("fma", cfg!(target_feature = "fma"));
    out.bool("neon", cfg!(target_feature = "neon"));
    out.object_end();
    out.object_end();
}

fn target_env() -> &'static str {
    if cfg!(target_env = "msvc") {
        "msvc"
    } else if cfg!(target_env = "gnu") {
        "gnu"
    } else if cfg!(target_env = "musl") {
        "musl"
    } else {
        ""
    }
}

fn f64_probe(name: &'static str, value: f64) -> Probe {
    Probe { name, bits: value.to_bits() }
}

fn f32_probe(name: &'static str, value: f32) -> Probe {
    Probe { name, bits: u64::from(value.to_bits()) }
}

fn int_probe(name: &'static str, value: i64) -> Probe {
    Probe { name, bits: value as u64 }
}

/// Operations whose result IEEE 754 defines exactly: identical bits are
/// expected on every conforming platform.
fn exact_probes() -> Vec<Probe> {
    let one_plus = black_box(1.0_f64 + f64::EPSILON);
    let one_minus = black_box(1.0_f64 - f64::EPSILON);
    let one_plus_f32 = black_box(1.0_f32 + f32::EPSILON);
    let one_minus_f32 = black_box(1.0_f32 - f32::EPSILON);
    let big = black_box(1e16_f64);
    vec![
        f64_probe("f64_add", black_box(0.1_f64) + black_box(0.2_f64)),
        f64_probe("f64_sub", black_box(1.0_f64) - black_box(0.9_f64)),
        f64_probe("f64_mul", black_box(0.1_f64) * black_box(3.0_f64)),
        f64_probe("f64_div", black_box(1.0_f64) / black_box(3.0_f64)),
        f64_probe("f64_rem", black_box(10.0_f64) % black_box(3.0_f64)),
        f64_probe("f64_sqrt", black_box(2.0_f64).sqrt()),
        f64_probe("f64_mul_add", one_plus.mul_add(one_minus, black_box(-1.0))),
        f64_probe("f64_no_reassociation", (big + black_box(1.0)) - big),
        f64_probe("f64_subnormal", black_box(f64::MIN_POSITIVE) / black_box(1024.0)),
        f64_probe(
            "f64_subnormal_round_trip",
            (black_box(f64::MIN_POSITIVE) * black_box(0.5)) * black_box(2.0),
        ),
        f64_probe("f64_round_half_away", black_box(2.5_f64).round()),
        f64_probe("f64_round_ties_even", black_box(2.5_f64).round_ties_even()),
        f64_probe("f64_from_f32", f64::from(black_box(0.1_f32))),
        f32_probe("f32_add", black_box(0.1_f32) + black_box(0.2_f32)),
        f32_probe("f32_mul", black_box(0.1_f32) * black_box(3.0_f32)),
        f32_probe("f32_div", black_box(1.0_f32) / black_box(3.0_f32)),
        f32_probe("f32_sqrt", black_box(2.0_f32).sqrt()),
        f32_probe(
            "f32_mul_add",
            one_plus_f32.mul_add(one_minus_f32, black_box(-1.0_f32)),
        ),
        f32_probe("f32_subnormal", black_box(f32::MIN_POSITIVE) / black_box(1024.0_f32)),
        f32_probe("f32_from_f64", black_box(0.1_f64) as f32),
        int_probe("f64_to_i32_saturating", i64::from(black_box(1e20_f64) as i32)),
        int_probe("f64_nan_to_i32", i64::from(black_box(f64::NAN) as i32)),
        int_probe("f64_to_u8_negative", i64::from(black_box(-5.0_f64) as u8)),
    ]
}

/// Patterns a compiler may legally rewrite only when contraction or
/// reassociation is allowed. A zero result means the multiply and the add were
/// rounded separately; a fused result shows up as a tiny nonzero value.
fn codegen_probes() -> Vec<Probe> {
    let a = black_box(1.0_f64 + f64::EPSILON);
    let b = black_box(1.0_f64 - f64::EPSILON);
    let one = black_box(1.0_f64);
    let a32 = black_box(1.0_f32 + f32::EPSILON);
    let b32 = black_box(1.0_f32 - f32::EPSILON);
    let one32 = black_box(1.0_f32);
    let mut accumulated = 0.0_f64;
    for i in 0..black_box(1000_u32) {
        accumulated += f64::from(i) * black_box(0.1_f64);
    }
    vec![
        f64_probe("f64_mul_then_add", a * b - one),
        f64_probe("f64_mul_then_sub", one - a * b),
        f64_probe("f64_neg_mul_then_add", -(a * b) + one),
        f32_probe("f32_mul_then_add", a32 * b32 - one32),
        f32_probe("f32_mul_then_sub", one32 - a32 * b32),
        f64_probe("f64_loop_accumulate", accumulated),
    ]
}

/// Functions served by the platform math library (or by LLVM's lowering of
/// them): the Rust documentation states their precision is unspecified and
/// varies by platform. Differences across cells are the measurement.
fn libm_probes() -> Vec<Probe> {
    let x = black_box(1.0_f64);
    let half = black_box(0.5_f64);
    let ten = black_box(10.0_f64);
    let large = black_box(1e6_f64);
    let huge = black_box(1e22_f64);
    let x32 = black_box(1.0_f32);
    let ten32 = black_box(10.0_f32);
    let large32 = black_box(1e6_f32);
    vec![
        f64_probe("f64_sin", x.sin()),
        f64_probe("f64_cos", x.cos()),
        f64_probe("f64_tan", x.tan()),
        f64_probe("f64_asin", half.asin()),
        f64_probe("f64_acos", half.acos()),
        f64_probe("f64_atan", x.atan()),
        f64_probe("f64_atan2", x.atan2(black_box(2.0_f64))),
        f64_probe("f64_sinh", x.sinh()),
        f64_probe("f64_cosh", x.cosh()),
        f64_probe("f64_tanh", x.tanh()),
        f64_probe("f64_exp", x.exp()),
        f64_probe("f64_exp2", half.exp2()),
        f64_probe("f64_exp_m1", black_box(1e-5_f64).exp_m1()),
        f64_probe("f64_ln", ten.ln()),
        f64_probe("f64_log2", ten.log2()),
        f64_probe("f64_log10", black_box(3.0_f64).log10()),
        f64_probe("f64_ln_1p", black_box(1e-5_f64).ln_1p()),
        f64_probe("f64_powf", black_box(1.1_f64).powf(black_box(7.3_f64))),
        f64_probe("f64_powi", black_box(1.1_f64).powi(black_box(7))),
        f64_probe("f64_cbrt", black_box(2.0_f64).cbrt()),
        f64_probe("f64_hypot", black_box(3.0_f64).hypot(black_box(4.0_f64))),
        f64_probe("f64_sin_large_argument", large.sin()),
        f64_probe("f64_sin_huge_argument", huge.sin()),
        f32_probe("f32_sin", x32.sin()),
        f32_probe("f32_cos", x32.cos()),
        f32_probe("f32_tan", x32.tan()),
        f32_probe("f32_atan2", x32.atan2(black_box(2.0_f32))),
        f32_probe("f32_exp", x32.exp()),
        f32_probe("f32_ln", ten32.ln()),
        f32_probe("f32_powf", black_box(1.1_f32).powf(black_box(7.3_f32))),
        f32_probe("f32_sin_large_argument", large32.sin()),
    ]
}

/// NaN bit patterns produced by invalid operations. IEEE 754 leaves the sign
/// and payload of a generated NaN to the implementation; x86-64 and ARM64 are
/// known to differ.
#[allow(clippy::eq_op)] // `zero / zero` is the point of these probes.
fn nan_probes() -> Vec<Probe> {
    let zero = black_box(0.0_f64);
    let zero32 = black_box(0.0_f32);
    let payload = black_box(f64::from_bits(0x7ff0_0000_0000_1234));
    vec![
        f64_probe("f64_zero_div_zero", zero / zero),
        f64_probe("f64_sqrt_negative", black_box(-1.0_f64).sqrt()),
        f64_probe(
            "f64_inf_minus_inf",
            black_box(f64::INFINITY) - black_box(f64::INFINITY),
        ),
        f64_probe("f64_nan_payload_propagation", payload + black_box(1.0_f64)),
        f64_probe("f64_nan_times_negative_one", payload * black_box(-1.0_f64)),
        f32_probe("f32_zero_div_zero", zero32 / zero32),
        f32_probe("f32_from_f64_nan", (zero / zero) as f32),
    ]
}

fn group_hash(probes: &[Probe]) -> u64 {
    let mut bytes = Vec::with_capacity(probes.len() * 8);
    for probe in probes {
        bytes.extend_from_slice(&probe.bits.to_le_bytes());
    }
    fnv1a(&bytes)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Wall-clock cost of a fixed arithmetic loop, repeated so that the spread
/// between repetitions documents the noise of the machine.
fn write_timing(out: &mut Json) {
    let mut picoseconds_per_iteration = [0_u64; TIMING_REPETITIONS];
    for slot in &mut picoseconds_per_iteration {
        let start = Instant::now();
        let mut value = black_box(1.0_f64);
        for _ in 0..TIMING_ITERATIONS {
            value = (value * 1.000_001 + 0.5).sqrt();
        }
        black_box(value);
        let nanoseconds = start.elapsed().as_nanos() as u64;
        *slot = nanoseconds * 1000 / u64::from(TIMING_ITERATIONS);
    }
    picoseconds_per_iteration.sort_unstable();
    out.key("timing");
    out.object_start();
    out.u64("iterations", u64::from(TIMING_ITERATIONS));
    out.u64("repetitions", TIMING_REPETITIONS as u64);
    out.u64("picoseconds_per_iteration_min", picoseconds_per_iteration[0]);
    out.u64(
        "picoseconds_per_iteration_median",
        picoseconds_per_iteration[TIMING_REPETITIONS / 2],
    );
    out.u64(
        "picoseconds_per_iteration_max",
        picoseconds_per_iteration[TIMING_REPETITIONS - 1],
    );
    out.object_end();
}

/// Minimal JSON writer: flat objects of strings, integers, booleans and
/// nested objects, enough for this fact sheet without a dependency.
struct Json {
    buf: String,
    needs_comma: bool,
}

impl Json {
    fn new() -> Self {
        Json { buf: String::new(), needs_comma: false }
    }

    fn object_start(&mut self) {
        self.separator();
        self.buf.push('{');
        self.needs_comma = false;
    }

    fn object_end(&mut self) {
        self.buf.push('}');
        self.needs_comma = true;
    }

    fn key(&mut self, key: &str) {
        self.separator();
        self.quoted(key);
        self.buf.push(':');
        self.needs_comma = false;
    }

    fn string(&mut self, key: &str, value: &str) {
        self.key(key);
        self.quoted(value);
        self.needs_comma = true;
    }

    fn u64(&mut self, key: &str, value: u64) {
        self.key(key);
        self.buf.push_str(&value.to_string());
        self.needs_comma = true;
    }

    fn bool(&mut self, key: &str, value: bool) {
        self.key(key);
        self.buf.push_str(if value { "true" } else { "false" });
        self.needs_comma = true;
    }

    fn hex(&mut self, key: &str, bits: u64) {
        self.string(key, &format!("0x{bits:016x}"));
    }

    fn separator(&mut self) {
        if self.needs_comma {
            self.buf.push(',');
        }
    }

    fn quoted(&mut self, text: &str) {
        self.buf.push('"');
        for character in text.chars() {
            match character {
                '"' => self.buf.push_str("\\\""),
                '\\' => self.buf.push_str("\\\\"),
                control if (control as u32) < 0x20 => {
                    self.buf.push_str(&format!("\\u{:04x}", control as u32));
                }
                other => self.buf.push(other),
            }
        }
        self.buf.push('"');
    }

    fn finish(self) -> String {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_the_reference_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn fused_multiply_add_is_exactly_rounded() {
        let probes = exact_probes();
        let f64_fma = probes.iter().find(|p| p.name == "f64_mul_add").unwrap();
        let f32_fma = probes.iter().find(|p| p.name == "f32_mul_add").unwrap();
        // (1 + eps)(1 - eps) - 1 = -eps^2, representable only when fused.
        assert_eq!(f64_fma.bits, (-(f64::EPSILON * f64::EPSILON)).to_bits());
        assert_eq!(f64_fma.bits, 0xb970_0000_0000_0000);
        assert_eq!(f32_fma.bits, u64::from((-(f32::EPSILON * f32::EPSILON)).to_bits()));
        assert_eq!(f32_fma.bits, 0xa880_0000);
    }

    #[test]
    fn json_writer_escapes_and_separates() {
        let mut out = Json::new();
        out.object_start();
        out.string("a", "x\"y\\z");
        out.key("b");
        out.object_start();
        out.u64("c", 1);
        out.bool("d", false);
        out.object_end();
        out.hex("e", 0xff);
        out.object_end();
        assert_eq!(
            out.finish(),
            r#"{"a":"x\"y\\z","b":{"c":1,"d":false},"e":"0x00000000000000ff"}"#
        );
    }
}
