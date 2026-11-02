//! Exact arithmetic on scaled decimals stored as `i128`, as in Arrow `Decimal128`.
//!
//! A value `v` with scale `s` means `v / 10^s`. Rounding is half away from zero, the
//! rule Excel uses for display, applied exactly once per derived value.

/// Returns `10^exp`.
///
/// # Panics
///
/// Panics if the power overflows `i128` (`exp > 38`).
#[must_use]
pub const fn pow10(exp: u32) -> i128 {
    10i128.pow(exp)
}

/// Divides, rounding half away from zero.
///
/// # Panics
///
/// Panics if `denominator` is not positive.
#[must_use]
pub fn div_round_half_away(numerator: i128, denominator: i128) -> i128 {
    assert!(denominator > 0, "denominator must be positive");
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder.unsigned_abs() * 2 >= denominator.unsigned_abs() {
        quotient + numerator.signum()
    } else {
        quotient
    }
}

/// Changes the scale of `value` from `from` to `to`, rounding if the scale shrinks.
#[must_use]
pub fn rescale(value: i128, from: u32, to: u32) -> i128 {
    if to >= from {
        value * pow10(to - from)
    } else {
        div_round_half_away(value, pow10(from - to))
    }
}

/// Multiplies scaled decimals and rounds the exact product once, to `out_scale`.
///
/// # Panics
///
/// Panics if the exact product overflows `i128`.
#[must_use]
pub fn mul_round(factors: &[(i128, u32)], out_scale: u32) -> i128 {
    let mut product: i128 = 1;
    let mut scale = 0;
    for &(value, value_scale) in factors {
        product = product
            .checked_mul(value)
            .expect("exact decimal product overflows i128");
        scale += value_scale;
    }
    rescale(product, scale, out_scale)
}

/// Quantizes an `f64` to a scaled decimal, rounding half away from zero.
///
/// # Panics
///
/// Panics if `scale` exceeds 18 or the scaled value is not finite.
#[must_use]
pub fn from_f64(value: f64, scale: u32) -> i128 {
    const POW10: [f64; 19] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18,
    ];
    let scaled = value * POW10[scale as usize];
    assert!(scaled.is_finite(), "cannot quantize {value}");
    libm::round(scaled) as i128
}

/// Converts a scaled decimal to the nearest `f64`, for derived floating-point columns.
#[must_use]
pub fn to_f64(value: i128, scale: u32) -> f64 {
    value as f64 / pow10(scale) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_half_away_from_zero() {
        assert_eq!(div_round_half_away(25, 10), 3);
        assert_eq!(div_round_half_away(-25, 10), -3);
        assert_eq!(div_round_half_away(24, 10), 2);
        assert_eq!(div_round_half_away(-24, 10), -2);
        assert_eq!(div_round_half_away(0, 10), 0);
    }

    #[test]
    fn multiplies_exactly_then_rounds_once() {
        // 3 shares at 2.675 with a multiplier of 1: exactly 8.025, rounds to 8.03.
        let mv = mul_round(&[(30_000, 4), (2_675_000, 6), (100, 2)], 2);
        assert_eq!(mv, 803);
        // Rounding once and rounding each factor first disagree here:
        // 1.005 * 1.005 = 1.010025 -> 1.01, but 1.01 * 1.01 = 1.0201 -> 1.02.
        assert_eq!(mul_round(&[(1_005, 3), (1_005, 3)], 2), 101);
    }

    #[test]
    fn rescales_both_ways() {
        assert_eq!(rescale(12_345, 2, 4), 1_234_500);
        assert_eq!(rescale(1_234_550, 4, 2), 12_346);
        assert_eq!(rescale(-1_234_550, 4, 2), -12_346);
    }

    #[test]
    fn quantizes_floats() {
        assert_eq!(from_f64(1.005, 2), 100); // 1.005 is 1.00499999999999989... in binary.
        assert_eq!(from_f64(-2.5, 0), -3);
        assert_eq!(from_f64(123.456_789, 6), 123_456_789);
    }
}
