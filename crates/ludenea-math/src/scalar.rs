//! Deterministic scalar math: `f32` helpers whose results do not depend on the target.

pub const PI: f32 = std::f32::consts::PI;
const TWO_PI: f32 = 2.0 * PI;
const CANONICAL_NAN: u32 = 0x7fc0_0000;

/// Smaller of two values; `-0.0` is smaller than `+0.0`; NaN input gives canonical NaN.
pub fn min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::from_bits(CANONICAL_NAN)
    } else if a < b {
        a
    } else if b < a {
        b
    } else if a.is_sign_negative() {
        a
    } else {
        b
    }
}

/// Larger of two values; `+0.0` is larger than `-0.0`; NaN input gives canonical NaN.
pub fn max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::from_bits(CANONICAL_NAN)
    } else if a > b {
        a
    } else if b > a || a.is_sign_negative() {
        b
    } else {
        a
    }
}

pub fn clamp(x: f32, low: f32, high: f32) -> f32 {
    max(low, min(x, high))
}

/// Wraps an angle to `[-PI, PI]`, whatever the number of turns.
pub fn unwind_angle(angle: f32) -> f32 {
    if angle < -PI {
        angle - TWO_PI * ((angle - PI) / TWO_PI).trunc()
    } else if angle > PI {
        angle - TWO_PI * ((angle + PI) / TWO_PI).trunc()
    } else {
        angle
    }
}

/// Returns `(sin, cos)` using the Box2D v3 rational approximation, normalized. Gives
/// `(0, 1)`, the identity rotation, when the angle is not finite or so large that the
/// approximation overflows (1e30, for example). The reduction to `[-PI, PI]` loses precision
/// as the angle grows: from 2^26 (about 6.7e7) on, where consecutive `f32` values are more
/// than a turn apart, the result is a unit rotation that does not follow the angle, such as
/// `(0.7071, 0.7071)` for 1e12.
pub fn sin_cos(angle: f32) -> (f32, f32) {
    let x = unwind_angle(angle);
    let pi2 = PI * PI;

    let cosine = |y: f32| (pi2 - 4.0 * y * y) / (pi2 + y * y);
    let c = if x < -0.5 * PI {
        -cosine(x + PI)
    } else if x > 0.5 * PI {
        -cosine(x - PI)
    } else {
        cosine(x)
    };

    let sine = |y: f32| 16.0 * y * (PI - y) / (5.0 * pi2 - 4.0 * y * (PI - y));
    let s = if x < 0.0 { -sine(x + PI) } else { sine(x) };

    let magnitude = (s * s + c * c).sqrt();
    if magnitude > 0.0 && magnitude.is_finite() {
        (s / magnitude, c / magnitude)
    } else {
        (0.0, 1.0)
    }
}

pub fn sin(angle: f32) -> f32 {
    sin_cos(angle).0
}

pub fn cos(angle: f32) -> f32 {
    sin_cos(angle).1
}

/// Box2D v3 polynomial `atan2`; `atan2(0, 0) == 0`.
#[expect(
    clippy::excessive_precision,
    reason = "the constants are kept exactly as Box2D publishes them"
)]
pub fn atan2(y: f32, x: f32) -> f32 {
    let (ax, ay) = (x.abs(), y.abs());
    let a = min(ay, ax) / (max(ay, ax) + f32::MIN_POSITIVE);
    let s = a * a;
    let c = s * a;
    let q = s * s;
    let mut r = ((0.024840285 * q + 0.18681418) * s + (-0.094097948 * q - 0.33213072)) * c + a;
    if ay > ax {
        r = 1.57079637 - r;
    }
    if x < 0.0 {
        r = 3.14159274 - r;
    }
    if y < 0.0 {
        r = -r;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORDED_SIN_1: f32 = f32::from_bits(0x3f57_5b9b);
    const RECORDED_COS_1: f32 = f32::from_bits(0x3f0a_68a8);
    const RECORDED_ATAN2_1_2: f32 = f32::from_bits(0x3eed_6190);

    #[test]
    fn min_max_are_order_independent_for_signed_zero() {
        assert_eq!(min(-0.0, 0.0).to_bits(), (-0.0f32).to_bits());
        assert_eq!(min(0.0, -0.0).to_bits(), (-0.0f32).to_bits());
        assert_eq!(max(-0.0, 0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(max(0.0, -0.0).to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn min_max_return_canonical_nan() {
        let negative_nan = f32::from_bits(0xffc0_0000);
        assert_eq!(min(negative_nan, 1.0).to_bits(), 0x7fc0_0000);
        assert_eq!(max(1.0, f32::NAN).to_bits(), 0x7fc0_0000);
    }

    #[test]
    fn clamp_limits_to_range() {
        assert_eq!(clamp(5.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp(-5.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp(0.5, 0.0, 1.0), 0.5);
    }

    #[test]
    fn unwind_angle_wraps_any_number_of_turns() {
        for k in -5..=5 {
            let angle = 0.5 + k as f32 * 2.0 * PI;
            assert!((unwind_angle(angle) - 0.5).abs() < 1e-4, "k = {k}");
        }
        assert_eq!(unwind_angle(1.0), 1.0);
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the platform `f64` functions are the reference the approximation is measured against"
    )]
    fn sin_cos_error_is_below_two_thousandths() {
        let mut worst = 0.0f64;
        for i in 0..100_000 {
            let angle = -10.0 + 20.0 * (i as f32) / 100_000.0;
            let (s, c) = sin_cos(angle);
            worst = worst.max((s as f64 - (angle as f64).sin()).abs());
            worst = worst.max((c as f64 - (angle as f64).cos()).abs());
        }
        assert!(worst <= 0.002, "worst error {worst}");
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the platform `f64` functions are the reference the approximation is measured against"
    )]
    fn atan2_error_is_below_one_ten_thousandth_radian() {
        let mut worst = 0.0f64;
        for magnitude in [0.01f32, 1.0, 1000.0] {
            for i in 0..360 * 50 {
                let theta =
                    (i as f64) * std::f64::consts::TAU / (360.0 * 50.0) - std::f64::consts::PI;
                let (y, x) = (
                    theta.sin() as f32 * magnitude,
                    theta.cos() as f32 * magnitude,
                );
                let expected = (y as f64).atan2(x as f64);
                let mut diff = (atan2(y, x) as f64 - expected).abs();
                if diff > std::f64::consts::PI {
                    diff = std::f64::consts::TAU - diff;
                }
                worst = worst.max(diff);
            }
        }
        assert!(worst <= 1e-4, "worst error {worst}");
    }

    #[test]
    fn atan2_of_origin_is_zero() {
        assert_eq!(atan2(0.0, 0.0), 0.0);
    }

    #[test]
    fn sin_cos_gives_zero_and_one_for_an_angle_it_cannot_reduce() {
        for angle in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30] {
            assert_eq!(sin_cos(angle), (0.0, 1.0), "{angle}");
        }
    }

    #[test]
    fn known_bit_patterns_are_stable() {
        assert_eq!(sin_cos(1.0), (RECORDED_SIN_1, RECORDED_COS_1));
        assert_eq!(atan2(1.0, 2.0), RECORDED_ATAN2_1_2);
    }
}
