//! Exact, order-independent accumulators.
//!
//! [`ExactSum`] holds the exact sum of any number of `f64` values as a fixed-point
//! integer wide enough for every finite double: a Kulisch accumulator. Adding is
//! exact, removing a value is exactly the inverse of adding it, two accumulators merge
//! exactly, and the result is rounded once, to nearest with ties to even. The same
//! values therefore give the same bits in any order, incrementally or from scratch.

use std::cmp::Ordering;

use crate::bignum::BigUint;

/// Bit 0 of the accumulator is worth 2^-1074, the smallest subnormal double. The
/// largest finite double needs bit 2097. Two spare words leave room for more than
/// 2^60 additions of the largest values before the sign word could overflow.
const WORDS: usize = 35;

/// The exact sum of finite doubles, plus counts of infinities and NaNs.
#[derive(Clone, PartialEq, Eq)]
pub struct ExactSum {
    /// Two's-complement little-endian words.
    words: [u64; WORDS],
    positive_infinities: u64,
    negative_infinities: u64,
    nans: u64,
}

impl Default for ExactSum {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ExactSum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExactSum")
            .field("value", &self.value())
            .field("positive_infinities", &self.positive_infinities)
            .field("negative_infinities", &self.negative_infinities)
            .field("nans", &self.nans)
            .finish()
    }
}

impl ExactSum {
    /// Returns an accumulator holding zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            words: [0; WORDS],
            positive_infinities: 0,
            negative_infinities: 0,
            nans: 0,
        }
    }

    /// Adds `value` exactly.
    pub fn add(&mut self, value: f64) {
        self.apply(value, false);
    }

    /// Removes a value added earlier, exactly undoing [`ExactSum::add`].
    pub fn remove(&mut self, value: f64) {
        self.apply(value, true);
    }

    /// Adds every value held by `other`.
    pub fn merge(&mut self, other: &Self) {
        let mut carry = 0u64;
        for (a, &b) in self.words.iter_mut().zip(&other.words) {
            let (sum, c1) = a.overflowing_add(b);
            let (sum, c2) = sum.overflowing_add(carry);
            *a = sum;
            carry = u64::from(c1) + u64::from(c2);
        }
        self.positive_infinities += other.positive_infinities;
        self.negative_infinities += other.negative_infinities;
        self.nans += other.nans;
    }

    fn apply(&mut self, value: f64, negate: bool) {
        let step = |n: &mut u64| {
            if negate {
                *n -= 1;
            } else {
                *n += 1;
            }
        };
        if value.is_nan() {
            step(&mut self.nans);
            return;
        }
        if value == f64::INFINITY {
            step(&mut self.positive_infinities);
            return;
        }
        if value == f64::NEG_INFINITY {
            step(&mut self.negative_infinities);
            return;
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as usize;
        let fraction = bits & ((1 << 52) - 1);
        let (mantissa, shift) = if exponent == 0 {
            (fraction, 0)
        } else {
            (fraction | (1 << 52), exponent - 1)
        };
        if mantissa == 0 {
            return;
        }
        let subtract = (bits >> 63 == 1) != negate;
        let word = shift / 64;
        let offset = shift % 64;
        let wide = u128::from(mantissa) << offset;
        let parts = [wide as u64, (wide >> 64) as u64];
        if subtract {
            self.sub_at(word, parts);
        } else {
            self.add_at(word, parts);
        }
    }

    fn add_at(&mut self, word: usize, parts: [u64; 2]) {
        let mut carry = 0u64;
        for (i, w) in self.words.iter_mut().enumerate().skip(word) {
            let part = parts.get(i - word).copied().unwrap_or(0);
            if part == 0 && carry == 0 && i > word + 1 {
                break;
            }
            let (sum, c1) = w.overflowing_add(part);
            let (sum, c2) = sum.overflowing_add(carry);
            *w = sum;
            carry = u64::from(c1) + u64::from(c2);
        }
    }

    fn sub_at(&mut self, word: usize, parts: [u64; 2]) {
        let mut borrow = 0u64;
        for (i, w) in self.words.iter_mut().enumerate().skip(word) {
            let part = parts.get(i - word).copied().unwrap_or(0);
            if part == 0 && borrow == 0 && i > word + 1 {
                break;
            }
            let (diff, b1) = w.overflowing_sub(part);
            let (diff, b2) = diff.overflowing_sub(borrow);
            *w = diff;
            borrow = u64::from(b1) + u64::from(b2);
        }
    }

    fn is_negative(&self) -> bool {
        self.words[WORDS - 1] >> 63 == 1
    }

    fn magnitude(&self) -> [u64; WORDS] {
        if !self.is_negative() {
            return self.words;
        }
        let mut out = [0u64; WORDS];
        let mut carry = 1u64;
        for (o, &w) in out.iter_mut().zip(&self.words) {
            let (v, c) = (!w).overflowing_add(carry);
            *o = v;
            carry = u64::from(c);
        }
        out
    }

    /// Returns `true` if no non-zero finite value, infinity or NaN is held.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
            && self.positive_infinities == 0
            && self.negative_infinities == 0
            && self.nans == 0
    }

    /// Returns the sum, correctly rounded to nearest with ties to even.
    ///
    /// Infinities of one sign give that infinity. Infinities of both signs, or any
    /// NaN, give NaN, as IEEE 754 addition would.
    #[must_use]
    pub fn value(&self) -> f64 {
        if self.nans > 0 || (self.positive_infinities > 0 && self.negative_infinities > 0) {
            return f64::NAN;
        }
        if self.positive_infinities > 0 {
            return f64::INFINITY;
        }
        if self.negative_infinities > 0 {
            return f64::NEG_INFINITY;
        }
        BigUint(self.magnitude().to_vec()).to_f64(-1074, false, self.is_negative())
    }

    /// Returns the sum divided by `n`, correctly rounded once, or NaN if `n` is zero.
    /// Infinities and NaNs behave as in [`ExactSum::value`].
    #[must_use]
    pub fn mean(&self, n: u64) -> f64 {
        if n == 0 {
            return f64::NAN;
        }
        if self.nans > 0 || self.positive_infinities > 0 || self.negative_infinities > 0 {
            return self.value();
        }
        let shifted = BigUint(self.magnitude().to_vec()).shl(64);
        let (quotient, sticky) = shifted.div_rem_u128(u128::from(n));
        quotient.to_f64(-1074 - 64, sticky, self.is_negative())
    }

    /// Returns `true` if only finite values were added.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.nans == 0 && self.positive_infinities == 0 && self.negative_infinities == 0
    }

    /// Returns the exact held value as a sign and a magnitude in units of 2^-1074.
    pub(crate) fn signed_magnitude(&self) -> (bool, BigUint) {
        (self.is_negative(), BigUint(self.magnitude().to_vec()))
    }

    /// Compares the exact held values of two accumulators that hold only finite
    /// values.
    #[must_use]
    pub fn cmp_finite(&self, other: &Self) -> Ordering {
        match (self.is_negative(), other.is_negative()) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        for (a, b) in self.words.iter().rev().zip(other.words.iter().rev()) {
            match a.cmp(b) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
        }
        Ordering::Equal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(values: &[f64]) -> f64 {
        let mut s = ExactSum::new();
        for &v in values {
            s.add(v);
        }
        s.value()
    }

    #[test]
    fn small_sums_are_correctly_rounded() {
        assert_eq!(sum(&[0.1, 0.2, 0.3]).to_bits(), 0.6f64.to_bits());
        assert_eq!(sum(&[0.3, 0.2, 0.1]).to_bits(), 0.6f64.to_bits());
        assert_eq!(sum(&[1.0, 2.0, 3.5]), 6.5);
        assert_eq!(sum(&[]), 0.0);
        assert_eq!(sum(&[-1.5]), -1.5);
    }

    #[test]
    fn cancellation_is_exact() {
        assert_eq!(sum(&[1e308, 1e308, -1e308]), 1e308);
        assert_eq!(sum(&[1e100, 1.0, -1e100]), 1.0);
        assert_eq!(sum(&[f64::MAX, f64::MAX, -f64::MAX, -f64::MAX, 3.0]), 3.0);
        assert_eq!(sum(&[f64::MAX, f64::MAX]), f64::INFINITY);
    }

    #[test]
    fn subnormals_survive() {
        let tiny = f64::from_bits(1);
        assert_eq!(sum(&[tiny, tiny, tiny]), f64::from_bits(3));
        assert_eq!(
            sum(&[f64::MIN_POSITIVE, -tiny]).to_bits(),
            (f64::MIN_POSITIVE.to_bits() - 1)
        );
    }

    #[test]
    fn ties_round_to_even() {
        // 1 + 2^-53 is exactly halfway between 1 and the next double; even is 1.
        assert_eq!(sum(&[1.0, 2f64.powi(-53)]), 1.0);
        // 1 + 3 * 2^-53 is halfway between 1 + 2^-52 and 1 + 2^-51; even is the latter.
        assert_eq!(sum(&[1.0, 3.0 * 2f64.powi(-53)]), 1.0 + 2f64.powi(-51));
        // Slightly above halfway rounds up.
        assert_eq!(
            sum(&[1.0, 2f64.powi(-53), 2f64.powi(-80)]),
            1.0 + 2f64.powi(-52)
        );
    }

    #[test]
    fn remove_and_merge_are_exact() {
        let mut a = ExactSum::new();
        for v in [1e20, 3.25, -7.5e-300, 0.1] {
            a.add(v);
        }
        a.remove(1e20);
        a.remove(0.1);
        assert_eq!(a.value(), sum(&[3.25, -7.5e-300]));
        let mut b = ExactSum::new();
        b.add(-3.25);
        a.merge(&b);
        assert_eq!(a.value(), -7.5e-300);
        a.remove(-7.5e-300);
        assert!(a.is_zero() || a.value() == 0.0);
    }

    #[test]
    fn means_round_once() {
        let mut s = ExactSum::new();
        for v in [0.1, 0.2, 0.3] {
            s.add(v);
        }
        // The exact sum is 0.6000000000000000055511151231257827, and a third of it
        // is nearest to the double 0.2.
        assert_eq!(s.mean(3), 0.2);
        let mut t = ExactSum::new();
        t.add(1.0);
        t.add(2f64.powi(-60));
        assert_eq!(t.mean(1), 1.0);
        // (1 + 2^-60) / 3 is still nearest to the double closest to 1/3.
        assert_eq!(t.mean(3), 1.0 / 3.0);
        assert!(ExactSum::new().mean(0).is_nan());
    }

    #[test]
    fn infinities_and_nan() {
        assert_eq!(sum(&[1.0, f64::INFINITY]), f64::INFINITY);
        assert_eq!(sum(&[f64::NEG_INFINITY, 5.0]), f64::NEG_INFINITY);
        assert!(sum(&[f64::INFINITY, f64::NEG_INFINITY]).is_nan());
        assert!(sum(&[f64::NAN, 1.0]).is_nan());
        let mut s = ExactSum::new();
        s.add(f64::INFINITY);
        s.add(2.0);
        s.remove(f64::INFINITY);
        assert_eq!(s.value(), 2.0);
    }

    /// Shewchuk's exactly rounded summation, ported from CPython's `math.fsum`, as an
    /// independent reference. The `xsum` crate (0.1.6) was tried first and returned
    /// a neighbouring double for some single-value inputs.
    fn reference_fsum(values: &[f64]) -> f64 {
        let mut partials: Vec<f64> = Vec::new();
        for &value in values {
            let mut x = value;
            let mut i = 0;
            for j in 0..partials.len() {
                let mut y = partials[j];
                if x.abs() < y.abs() {
                    std::mem::swap(&mut x, &mut y);
                }
                let hi = x + y;
                let lo = y - (hi - x);
                if lo != 0.0 {
                    partials[i] = lo;
                    i += 1;
                }
                x = hi;
            }
            partials.truncate(i);
            partials.push(x);
        }
        let mut n = partials.len();
        if n == 0 {
            return 0.0;
        }
        n -= 1;
        let mut hi = partials[n];
        let mut lo = 0.0;
        while n > 0 {
            let x = hi;
            n -= 1;
            let y = partials[n];
            hi = x + y;
            let yr = hi - x;
            lo = y - yr;
            if lo != 0.0 {
                break;
            }
        }
        if n > 0 && ((lo < 0.0 && partials[n - 1] < 0.0) || (lo > 0.0 && partials[n - 1] > 0.0)) {
            let y = lo * 2.0;
            let x = hi + y;
            let yr = x - hi;
            if y == yr {
                hi = x;
            }
        }
        hi
    }

    #[test]
    fn a_single_value_sums_to_itself() {
        let v = f64::from_bits(13_024_853_264_395_063_744);
        assert_eq!(sum(&[v]).to_bits(), v.to_bits());
        assert_eq!(reference_fsum(&[v]).to_bits(), v.to_bits());
    }

    #[test]
    fn matches_an_independent_exact_summation() {
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for round in 0..5_000 {
            let n = 1 + (next() % 200) as usize;
            let values: Vec<f64> = (0..n)
                .map(|_| {
                    // Random finite bit patterns cover every exponent, subnormals included.
                    loop {
                        let v = f64::from_bits(next());
                        if v.is_finite() && v.abs() < 1e300 {
                            break v;
                        }
                    }
                })
                .collect();
            let expected = reference_fsum(&values);
            let mut forward = ExactSum::new();
            values.iter().for_each(|&v| forward.add(v));
            let mut backward = ExactSum::new();
            values.iter().rev().for_each(|&v| backward.add(v));
            assert_eq!(
                forward.value().to_bits(),
                expected.to_bits(),
                "round {round}"
            );
            assert_eq!(
                backward.value().to_bits(),
                expected.to_bits(),
                "round {round}"
            );
        }
    }
}
