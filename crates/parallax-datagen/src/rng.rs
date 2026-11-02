//! Deterministic pseudo-random numbers.
//!
//! The generator's output must depend only on this crate's code. `rand` does not
//! promise that a seed produces the same values across its versions, so parallax
//! carries its own SplitMix64 and xoshiro256** (Blackman and Vigna, public domain).

const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64, used to expand seeds into xoshiro state.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Starts a SplitMix64 sequence at `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Returns the next value.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GOLDEN_GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Independent random streams. Each stream is keyed separately, so adding draws to
/// one stream never shifts the values of another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Stream {
    /// Securities, books and funds.
    Universe = 1,
    /// Ordinary position values, keyed by row.
    Positions = 2,
    /// Hostile-value decisions for positions, keyed by row.
    PositionsHostile = 3,
    /// Duplicate-key decisions, keyed by row.
    Keys = 4,
    /// Ordinary trade values, keyed by row.
    Trades = 5,
    /// Hostile-value decisions for trades, keyed by row.
    TradesHostile = 6,
    /// Price paths, keyed by security.
    Prices = 7,
    /// Hostile-value decisions for prices, keyed by row.
    PricesHostile = 8,
}

/// xoshiro256** 1.0.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Seeds the state from SplitMix64, as the reference implementation recommends.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        let mut expander = SplitMix64::new(seed);
        let s = [
            expander.next_u64(),
            expander.next_u64(),
            expander.next_u64(),
            expander.next_u64(),
        ];
        Self { s }
    }

    /// Returns the generator for one `(seed, stream, index)` key. Neighbouring
    /// indices give unrelated sequences, which is what makes any row reproducible
    /// on its own.
    #[must_use]
    pub fn for_key(seed: u64, stream: Stream, index: u64) -> Self {
        let mut mixer = SplitMix64::new(seed ^ (stream as u64).wrapping_mul(GOLDEN_GAMMA));
        let base = mixer.next_u64();
        let mut row_mixer = SplitMix64::new(base ^ index);
        Self::from_seed(row_mixer.next_u64())
    }

    /// Returns the next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Returns a uniform value in `[0, 1)` with 53 random bits.
    pub fn unit(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / (1u64 << 53) as f64;
        (self.next_u64() >> 11) as f64 * SCALE
    }

    /// Returns a uniform integer in `[0, n)` without bias (Lemire's method).
    ///
    /// # Panics
    ///
    /// Panics if `n` is zero.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0) has no valid result");
        let mut product = u128::from(self.next_u64()) * u128::from(n);
        let mut low = product as u64;
        if low < n {
            let threshold = n.wrapping_neg() % n;
            while low < threshold {
                product = u128::from(self.next_u64()) * u128::from(n);
                low = product as u64;
            }
        }
        (product >> 64) as u64
    }

    /// Returns a uniform index into a slice of length `len`.
    ///
    /// # Panics
    ///
    /// Panics if `len` is zero.
    pub fn index(&mut self, len: usize) -> usize {
        self.below(len as u64) as usize
    }

    /// Returns a uniform integer in `[low, high]`.
    ///
    /// # Panics
    ///
    /// Panics if `low > high`.
    pub fn range_i64(&mut self, low: i64, high: i64) -> i64 {
        assert!(low <= high, "empty range {low}..={high}");
        let span = high.abs_diff(low);
        if span == u64::MAX {
            return self.next_u64() as i64;
        }
        low.wrapping_add(self.below(span + 1) as i64)
    }

    /// Returns `true` with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// Returns a uniform value in `[low, high)`.
    pub fn uniform(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values come from the reference C code at https://prng.di.unimi.it/,
    // compiled with gcc and seeded the same way.
    #[test]
    fn splitmix_matches_reference() {
        let mut sm = SplitMix64::new(42);
        assert_eq!(sm.next_u64(), 13_679_457_532_755_275_413);
        assert_eq!(sm.next_u64(), 2_949_826_092_126_892_291);
        assert_eq!(sm.next_u64(), 5_139_283_748_462_763_858);
    }

    #[test]
    fn xoshiro_matches_reference() {
        let mut rng = Rng::from_seed(42);
        let expected = [
            1_546_998_764_402_558_742,
            6_990_951_692_964_543_102,
            12_544_586_762_248_559_009,
            17_057_574_109_182_124_193,
            18_295_552_978_065_317_476,
        ];
        for value in expected {
            assert_eq!(rng.next_u64(), value);
        }
    }

    #[test]
    fn keyed_streams_are_reproducible_and_distinct() {
        let a = Rng::for_key(7, Stream::Positions, 1000).next_u64();
        let b = Rng::for_key(7, Stream::Positions, 1000).next_u64();
        let c = Rng::for_key(7, Stream::Positions, 1001).next_u64();
        let d = Rng::for_key(7, Stream::Trades, 1000).next_u64();
        let e = Rng::for_key(8, Stream::Positions, 1000).next_u64();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_ne!(a, e);
    }

    #[test]
    fn below_is_in_range_and_unbiased() {
        let mut rng = Rng::from_seed(1);
        let mut counts = [0u32; 6];
        let draws = 600_000;
        for _ in 0..draws {
            counts[rng.below(6) as usize] += 1;
        }
        // Chi-square with 5 degrees of freedom; 20.5 is the 0.999 quantile.
        let expected = f64::from(draws) / 6.0;
        let chi2: f64 = counts
            .iter()
            .map(|&c| (f64::from(c) - expected).powi(2) / expected)
            .sum();
        assert!(chi2 < 20.5, "chi-square {chi2} with counts {counts:?}");
    }

    #[test]
    fn range_covers_both_ends() {
        let mut rng = Rng::from_seed(3);
        let mut seen = [false; 3];
        for _ in 0..1000 {
            let v = rng.range_i64(-1, 1);
            seen[(v + 1) as usize] = true;
        }
        assert_eq!(seen, [true; 3]);
    }

    #[test]
    fn unit_stays_below_one() {
        let mut rng = Rng::from_seed(9);
        assert!(
            (0..100_000)
                .map(|_| rng.unit())
                .all(|u| (0.0..1.0).contains(&u))
        );
    }
}
