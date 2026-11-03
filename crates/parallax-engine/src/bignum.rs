//! Just enough unsigned big-integer arithmetic to round exact quotients once.

/// An unsigned integer of any width, little-endian 64-bit words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BigUint(pub(crate) Vec<u64>);

impl BigUint {
    pub(crate) fn from_u128(value: u128) -> Self {
        Self(vec![value as u64, (value >> 64) as u64])
    }

    pub(crate) fn bits(&self) -> usize {
        match self.0.iter().rposition(|&w| w != 0) {
            Some(top) => top * 64 + 64 - self.0[top].leading_zeros() as usize,
            None => 0,
        }
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }

    fn bit(&self, i: usize) -> bool {
        self.0.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1)
    }

    /// Shifts left by `n` bits.
    pub(crate) fn shl(&self, n: usize) -> Self {
        let words = n / 64;
        let bits = n % 64;
        let mut out = vec![0u64; self.0.len() + words + 1];
        for (i, &w) in self.0.iter().enumerate() {
            out[i + words] |= w << bits;
            if bits > 0 {
                out[i + words + 1] |= w >> (64 - bits);
            }
        }
        Self(out)
    }

    /// Divides by `d`, returning the quotient and whether the remainder is non-zero.
    pub(crate) fn div_rem_u128(&self, d: u128) -> (Self, bool) {
        assert!(d > 0, "division by zero");
        if d <= u128::from(u64::MAX) {
            let d = d as u64;
            let mut quotient = vec![0u64; self.0.len()];
            let mut remainder: u128 = 0;
            for i in (0..self.0.len()).rev() {
                let current = remainder << 64 | u128::from(self.0[i]);
                quotient[i] = (current / u128::from(d)) as u64;
                remainder = current % u128::from(d);
            }
            return (Self(quotient), remainder != 0);
        }
        // Bit-by-bit long division for divisors wider than a word.
        let mut quotient = vec![0u64; self.0.len()];
        let mut remainder = BigUint(vec![0, 0, 0]);
        let divisor = BigUint::from_u128(d);
        for i in (0..self.bits()).rev() {
            remainder = remainder.shl(1);
            if self.bit(i) {
                remainder.0[0] |= 1;
            }
            if remainder.ge(&divisor) {
                remainder.sub_assign(&divisor);
                quotient[i / 64] |= 1 << (i % 64);
            }
        }
        (Self(quotient), !remainder.is_zero())
    }

    /// Multiplies two big integers.
    pub(crate) fn mul(&self, other: &Self) -> Self {
        let mut out = vec![0u64; self.0.len() + other.0.len()];
        for (i, &a) in self.0.iter().enumerate() {
            if a == 0 {
                continue;
            }
            let mut carry: u128 = 0;
            for (j, &b) in other.0.iter().enumerate() {
                let t = u128::from(a) * u128::from(b) + u128::from(out[i + j]) + carry;
                out[i + j] = t as u64;
                carry = t >> 64;
            }
            let mut k = i + other.0.len();
            while carry > 0 {
                let t = u128::from(out[k]) + carry;
                out[k] = t as u64;
                carry = t >> 64;
                k += 1;
            }
        }
        Self(out)
    }

    /// Returns `self - other`, or `None` if `other` is larger.
    pub(crate) fn checked_sub(&self, other: &Self) -> Option<Self> {
        if !self.ge(other) {
            return None;
        }
        let mut out = self.clone();
        out.sub_assign(other);
        Some(out)
    }

    fn ge(&self, other: &Self) -> bool {
        let n = self.0.len().max(other.0.len());
        for i in (0..n).rev() {
            let a = self.0.get(i).copied().unwrap_or(0);
            let b = other.0.get(i).copied().unwrap_or(0);
            if a != b {
                return a > b;
            }
        }
        true
    }

    fn sub_assign(&mut self, other: &Self) {
        let mut borrow = 0u64;
        for i in 0..self.0.len() {
            let b = other.0.get(i).copied().unwrap_or(0);
            let (d, b1) = self.0[i].overflowing_sub(b);
            let (d, b2) = d.overflowing_sub(borrow);
            self.0[i] = d;
            borrow = u64::from(b1) + u64::from(b2);
        }
    }

    /// Returns `self * 2^exponent`, plus an infinitesimal if `sticky`, correctly
    /// rounded to the nearest double with ties to even.
    pub(crate) fn to_f64(&self, exponent: i64, sticky: bool, negative: bool) -> f64 {
        let n = self.bits();
        if n == 0 {
            return 0.0;
        }
        let top = n - 1;
        // The value's leading bit is worth 2^(top + exponent). A normal double keeps
        // 53 bits; the lowest representable bit is 2^-1074.
        let leading_exponent = top as i64 + exponent;
        if leading_exponent > 1023 {
            return if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
        }
        let lowest_kept = (leading_exponent - 52).max(-1074);
        // Bits of self at index >= cut are kept.
        let cut = lowest_kept - exponent;
        let mut mantissa: u64 = 0;
        let mut round = false;
        let mut rest = sticky;
        if cut <= 0 {
            for i in (0..=top).rev() {
                mantissa = mantissa << 1 | u64::from(self.bit(i));
            }
            mantissa <<= (-cut) as u32;
        } else {
            let cut = cut as usize;
            for i in (cut..=top).rev() {
                mantissa = mantissa << 1 | u64::from(self.bit(i));
            }
            round = self.bit(cut - 1);
            rest |= (0..cut - 1).any(|i| self.bit(i));
        }
        let mut scale = lowest_kept; // value = mantissa * 2^scale
        if round && (rest || mantissa & 1 == 1) {
            mantissa += 1;
            if mantissa == 1 << 53 {
                mantissa >>= 1;
                scale += 1;
            }
        }
        let magnitude = if mantissa < 1 << 52 {
            debug_assert_eq!(scale, -1074);
            f64::from_bits(mantissa)
        } else {
            let biased = scale + 1075;
            if biased >= 0x7ff {
                f64::INFINITY
            } else {
                f64::from_bits((biased as u64) << 52 | (mantissa & ((1 << 52) - 1)))
            }
        };
        if negative { -magnitude } else { magnitude }
    }
}

/// Returns `numerator / denominator` correctly rounded, for exact integer inputs.
/// Several divisors divide in turn, which equals dividing by their product.
pub(crate) fn ratio_to_f64(numerator: i128, divisors: &[u128]) -> f64 {
    big_ratio_to_f64(
        numerator < 0,
        BigUint::from_u128(numerator.unsigned_abs()),
        divisors,
    )
}

/// Returns `±magnitude / product(divisors)` correctly rounded, ties to even.
pub(crate) fn big_ratio_to_f64(negative: bool, magnitude: BigUint, divisors: &[u128]) -> f64 {
    if magnitude.is_zero() {
        return 0.0;
    }
    // Shift so that the quotient keeps at least 64 bits beyond a double's 53.
    let divisor_bits: usize = divisors
        .iter()
        .map(|d| 128 - d.leading_zeros() as usize)
        .sum();
    let shift = (divisor_bits + 64).saturating_sub(magnitude.bits()) + 64;
    let mut quotient = magnitude.shl(shift);
    let mut sticky = false;
    for &d in divisors {
        let (q, r) = quotient.div_rem_u128(d);
        quotient = q;
        sticky |= r;
    }
    quotient.to_f64(-(shift as i64), sticky, negative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_ratios_round_once() {
        assert_eq!(ratio_to_f64(1, &[3]), 1.0 / 3.0);
        assert_eq!(ratio_to_f64(-2, &[3]), -2.0 / 3.0);
        assert_eq!(ratio_to_f64(10, &[4]), 2.5);
        assert_eq!(ratio_to_f64(0, &[7]), 0.0);
        // 2.675 is 2675 / 1000 exactly; the nearest double is below 2.675.
        assert_eq!(ratio_to_f64(2675, &[1000]), 2.675);
        // A divisor wider than 64 bits.
        assert_eq!(ratio_to_f64(1, &[1u128 << 100]), 2f64.powi(-100));
        assert_eq!(ratio_to_f64(i128::MAX, &[1]), i128::MAX as f64);
        // Two divisors equal one combined divisor.
        assert_eq!(
            ratio_to_f64(1_000_001, &[7, 11]),
            ratio_to_f64(1_000_001, &[77])
        );
    }

    #[test]
    fn multiplies_and_subtracts() {
        let a = BigUint::from_u128(u128::MAX);
        let b = BigUint::from_u128(3);
        let p = a.mul(&b);
        // (2^128 - 1) * 3 = 2^129 + 2^128 - 3
        assert_eq!(p.0[..3], [u64::MAX - 2, u64::MAX, 2]);
        let trim = |x: BigUint| {
            let mut w = x.0;
            while w.last() == Some(&0) {
                w.pop();
            }
            w
        };
        assert_eq!(trim(p.checked_sub(&a).unwrap()), trim(a.shl(1)));
        assert!(b.checked_sub(&a).is_none());
    }

    #[test]
    fn halfway_cases_round_to_even() {
        // (2^53 + 1) / 1 is halfway between two doubles; even is 2^53.
        assert_eq!(ratio_to_f64((1 << 53) + 1, &[1]), 2f64.powi(53));
        assert_eq!(ratio_to_f64((1 << 53) + 3, &[1]), 2f64.powi(53) + 4.0);
    }
}
