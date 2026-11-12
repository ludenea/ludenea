//! The arithmetic of prepare in two forms: a math layer built from the
//! operations IEEE 754 specifies exactly, and the platform's math library.
//! Throwaway, never product code.
//!
//! The math layer stands in for the engine-owned layer the determinism
//! contract describes, of which no function exists yet: its functions here
//! are written for the spike, from additions, subtractions, multiplications,
//! divisions, roundings, comparisons and conversions alone, evaluated in
//! double precision and rounded once to single precision. They are the same
//! on every machine by construction; their accuracy is not measured here and
//! is not the layer's.
//!
//! The two forms differ by the transcendental functions only: every other
//! operation of prepare (sums, products, divisions, square roots) is exact
//! under both and is written once.

/// The transcendental functions prepare calls.
pub trait Arithmetic: Copy + Send + Sync + 'static {
    fn sin_cos(angle: f32) -> (f32, f32);
    fn tan(angle: f32) -> f32;
    fn atan2(y: f32, x: f32) -> f32;
    fn log2(value: f32) -> f32;
}

/// The platform's math library, through the standard library.
#[derive(Clone, Copy)]
pub struct Platform;

impl Arithmetic for Platform {
    fn sin_cos(angle: f32) -> (f32, f32) {
        angle.sin_cos()
    }

    fn tan(angle: f32) -> f32 {
        angle.tan()
    }

    fn atan2(y: f32, x: f32) -> f32 {
        y.atan2(x)
    }

    fn log2(value: f32) -> f32 {
        value.log2()
    }
}

/// The spike's math layer.
#[derive(Clone, Copy)]
pub struct MathLayer;

const HALF_PI_HIGH: f64 = 1.570_796_326_794_896_558;
const HALF_PI_LOW: f64 = 6.123_233_995_736_766_036e-17;
const TWO_OVER_PI: f64 = 0.636_619_772_367_581_382_4;
const PI: f64 = 3.141_592_653_589_793_116;
const LN_2: f64 = 0.693_147_180_559_945_286_2;
const SQRT_2: f64 = 1.414_213_562_373_095_145_5;
/// The tangent of a sixteenth of a turn: above it the arc tangent is reduced.
const TAN_PI_8: f64 = 0.414_213_562_373_095_034_5;

/// The sine and the cosine of an angle of moderate size, in double precision:
/// the angle less a multiple of a quarter turn, then two polynomials.
fn sin_cos_f64(angle: f64) -> (f64, f64) {
    let quarter_turns = (angle * TWO_OVER_PI).round();
    let reduced = (angle - quarter_turns * HALF_PI_HIGH) - quarter_turns * HALF_PI_LOW;
    let square = reduced * reduced;
    // Taylor polynomials on an eighth of a turn, evaluated by Horner's rule.
    let sine = reduced
        * (1.0 + square * (-1.0 / 6.0 + square * (1.0 / 120.0 + square * (-1.0 / 5_040.0 + square * (1.0 / 362_880.0 + square * (-1.0 / 39_916_800.0 + square * (1.0 / 6_227_020_800.0)))))));
    let cosine =
        1.0 + square * (-0.5 + square * (1.0 / 24.0 + square * (-1.0 / 720.0 + square * (1.0 / 40_320.0 + square * (-1.0 / 3_628_800.0 + square * (1.0 / 479_001_600.0 + square * (-1.0 / 87_178_291_200.0)))))));
    // The quarter turn the angle fell in, 0 to 3; the remainder of a small
    // integer held in a double is exact.
    let quadrant = quarter_turns - 4.0 * (quarter_turns / 4.0).floor();
    if quadrant == 0.0 {
        (sine, cosine)
    } else if quadrant == 1.0 {
        (cosine, -sine)
    } else if quadrant == 2.0 {
        (-sine, -cosine)
    } else {
        (-cosine, sine)
    }
}

/// The arc tangent of a value between 0 and 1.
fn atan_unit(value: f64) -> f64 {
    let (offset, reduced) = if value > TAN_PI_8 { (PI / 4.0, (value - 1.0) / (value + 1.0)) } else { (0.0, value) };
    let square = reduced * reduced;
    // The odd terms from the 25th power down, by Horner's rule: the term of
    // the power `2 n + 1` has the sign of `(-1)^n`.
    let mut series = 0.0;
    let mut term = 12_i32;
    while term >= 0 {
        let sign = if term % 2 == 0 { 1.0 } else { -1.0 };
        series = sign / f64::from(2 * term + 1) + square * series;
        term -= 1;
    }
    offset + reduced * series
}

impl Arithmetic for MathLayer {
    fn sin_cos(angle: f32) -> (f32, f32) {
        let (sine, cosine) = sin_cos_f64(f64::from(angle));
        (sine as f32, cosine as f32)
    }

    fn tan(angle: f32) -> f32 {
        let (sine, cosine) = sin_cos_f64(f64::from(angle));
        (sine / cosine) as f32
    }

    fn atan2(y: f32, x: f32) -> f32 {
        let (y, x) = (f64::from(y), f64::from(x));
        let steep = y.abs() > x.abs();
        let (high, low) = if steep { (y.abs(), x.abs()) } else { (x.abs(), y.abs()) };
        if high == 0.0 {
            return 0.0;
        }
        let mut angle = atan_unit(low / high);
        if steep {
            angle = PI / 2.0 - angle;
        }
        if x < 0.0 {
            angle = PI - angle;
        }
        if y < 0.0 {
            angle = -angle;
        }
        angle as f32
    }

    fn log2(value: f32) -> f32 {
        if value.is_nan() || value <= 0.0 || value.is_infinite() || value < f32::MIN_POSITIVE {
            // Prepare never asks for it; a fixed answer keeps the function total.
            return f32::MIN;
        }
        let bits = f64::from(value).to_bits();
        let mut exponent = ((bits >> 52) & 0x7ff) as i64 - 1_023;
        let mut mantissa = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
        if mantissa > SQRT_2 {
            mantissa *= 0.5;
            exponent += 1;
        }
        let ratio = (mantissa - 1.0) / (mantissa + 1.0);
        let square = ratio * ratio;
        let series = 1.0 + square * (1.0 / 3.0 + square * (1.0 / 5.0 + square * (1.0 / 7.0 + square * (1.0 / 9.0 + square * (1.0 / 11.0 + square * (1.0 / 13.0 + square * (1.0 / 15.0 + square * (1.0 / 17.0))))))));
        (exponent as f64 + 2.0 * ratio * series / LN_2) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distance between two values in units of the last place of single
    /// precision: a sanity bound for the spike, not the layer's accuracy rule.
    fn units_apart(a: f32, b: f32) -> u32 {
        (i64::from(a.to_bits()) - i64::from(b.to_bits())).unsigned_abs() as u32
    }

    #[test]
    fn the_math_layer_stays_close_to_the_platform_library() {
        let mut worst = 0;
        for step in -4_000..4_000 {
            let angle = step as f32 * 0.003_7;
            let (sine, cosine) = MathLayer::sin_cos(angle);
            let (platform_sine, platform_cosine) = Platform::sin_cos(angle);
            if platform_sine.abs() > 1e-3 {
                worst = worst.max(units_apart(sine, platform_sine));
            }
            if platform_cosine.abs() > 1e-3 {
                worst = worst.max(units_apart(cosine, platform_cosine));
                worst = worst.max(units_apart(MathLayer::tan(angle), Platform::tan(angle)).saturating_sub(1));
            }
            let (y, x) = (sine * 3.0, cosine * 7.0 + 0.125);
            if y.abs() > 1e-3 {
                worst = worst.max(units_apart(MathLayer::atan2(y, x), Platform::atan2(y, x)));
            }
            let value = (step + 4_001) as f32 * 0.731;
            if (value - 1.0).abs() > 1e-2 {
                worst = worst.max(units_apart(MathLayer::log2(value), Platform::log2(value)));
            }
        }
        assert!(worst <= 2, "the math layer is {worst} units of the last place from the platform's library");
    }

    #[test]
    fn the_arc_tangent_covers_the_four_quadrants_and_the_axes() {
        for (y, x) in [(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-1.0, 0.0), (1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (0.0, 0.0)] {
            let (found, expected) = (MathLayer::atan2(y, x), Platform::atan2(y, x));
            assert!((found - expected).abs() < 1e-6, "atan2({y}, {x}) gives {found}, the platform {expected}");
        }
    }
}
