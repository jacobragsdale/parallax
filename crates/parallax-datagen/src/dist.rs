//! Probability distributions built on [`Rng`].
//!
//! Transcendental functions come from `libm`, which is pure Rust, so the values are
//! identical on Linux, Windows and WebAssembly. Platform maths libraries can differ in
//! the last bit.

use crate::rng::Rng;

/// Draws from the standard normal distribution (Marsaglia's polar method, which
/// discards its second value so that each call consumes a predictable stream).
pub fn standard_normal(rng: &mut Rng) -> f64 {
    loop {
        let u = 2.0 * rng.unit() - 1.0;
        let v = 2.0 * rng.unit() - 1.0;
        let s = u * u + v * v;
        if s > 0.0 && s < 1.0 {
            return u * libm::sqrt(-2.0 * libm::log(s) / s);
        }
    }
}

/// Draws from Student's t distribution with `dof` degrees of freedom, scaled to unit
/// variance. Small `dof` gives the fat tails of daily equity returns.
///
/// # Panics
///
/// Panics if `dof` is less than 3, where the variance is infinite.
pub fn student_t_unit_variance(rng: &mut Rng, dof: u32) -> f64 {
    assert!(dof >= 3, "Student's t needs dof >= 3 for finite variance");
    let z = standard_normal(rng);
    let chi2: f64 = (0..dof).map(|_| standard_normal(rng).powi(2)).sum();
    let t = z / libm::sqrt(chi2 / f64::from(dof));
    t * libm::sqrt(f64::from(dof - 2) / f64::from(dof))
}

/// Draws from a log-normal distribution with the given log-space mean and deviation.
pub fn log_normal(rng: &mut Rng, mu: f64, sigma: f64) -> f64 {
    libm::exp(mu + sigma * standard_normal(rng))
}

/// A discrete distribution over `0..n` with fixed weights, sampled by binary search
/// over cumulative weights.
#[derive(Clone, Debug)]
pub struct Weighted {
    cumulative: Vec<f64>,
}

impl Weighted {
    /// Builds the distribution.
    ///
    /// # Panics
    ///
    /// Panics if `weights` is empty, contains a negative or non-finite weight, or sums
    /// to zero.
    #[must_use]
    pub fn new(weights: &[f64]) -> Self {
        assert!(!weights.is_empty(), "no weights");
        let mut total = 0.0;
        let cumulative = weights
            .iter()
            .map(|&w| {
                assert!(w.is_finite() && w >= 0.0, "invalid weight {w}");
                total += w;
                total
            })
            .collect();
        assert!(total > 0.0, "weights sum to zero");
        Self { cumulative }
    }

    /// Zipf weights `1 / k^exponent` for `k` in `1..=n`: a few items are very common and
    /// most are rare, like ticker frequency in a portfolio.
    #[must_use]
    pub fn zipf(n: usize, exponent: f64) -> Self {
        let weights: Vec<f64> = (1..=n).map(|k| libm::pow(k as f64, -exponent)).collect();
        Self::new(&weights)
    }

    /// Returns the number of outcomes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cumulative.len()
    }

    /// Returns `true` if there are no outcomes, which `new` never allows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cumulative.is_empty()
    }

    /// Draws an outcome.
    pub fn sample(&self, rng: &mut Rng) -> usize {
        let total = *self.cumulative.last().expect("non-empty by construction");
        let x = rng.unit() * total;
        self.cumulative
            .partition_point(|&c| c <= x)
            .min(self.cumulative.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moments(samples: &[f64]) -> (f64, f64, f64) {
        let n = samples.len() as f64;
        let mean = samples.iter().sum::<f64>() / n;
        let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        let kurt = samples.iter().map(|x| (x - mean).powi(4)).sum::<f64>() / n / var.powi(2);
        (mean, var, kurt)
    }

    #[test]
    fn normal_has_unit_moments() {
        let mut rng = Rng::from_seed(11);
        let samples: Vec<f64> = (0..200_000).map(|_| standard_normal(&mut rng)).collect();
        let (mean, var, kurt) = moments(&samples);
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.02, "variance {var}");
        assert!((kurt - 3.0).abs() < 0.1, "kurtosis {kurt}");
    }

    #[test]
    fn student_t_has_unit_variance_and_fat_tails() {
        let mut rng = Rng::from_seed(12);
        let samples: Vec<f64> = (0..200_000)
            .map(|_| student_t_unit_variance(&mut rng, 5))
            .collect();
        let (_, var, _) = moments(&samples);
        assert!((var - 1.0).abs() < 0.05, "variance {var}");
        // A unit normal exceeds 4 deviations about 63 times per million draws. With five
        // degrees of freedom the standardized t does so roughly ten times as often.
        let beyond_four = samples.iter().filter(|x| x.abs() > 4.0).count();
        assert!(
            beyond_four > 100,
            "only {beyond_four} draws beyond 4 deviations"
        );
    }

    #[test]
    fn zipf_favours_low_ranks() {
        let zipf = Weighted::zipf(1000, 1.1);
        let mut rng = Rng::from_seed(13);
        let mut counts = vec![0u32; zipf.len()];
        for _ in 0..100_000 {
            counts[zipf.sample(&mut rng)] += 1;
        }
        assert!(counts[0] > counts[9] && counts[9] > counts[99] && counts[99] > counts[999]);
    }

    #[test]
    fn weighted_never_picks_zero_weight() {
        let w = Weighted::new(&[0.0, 1.0, 0.0, 3.0, 0.0]);
        let mut rng = Rng::from_seed(14);
        for _ in 0..10_000 {
            let i = w.sample(&mut rng);
            assert!(i == 1 || i == 3, "picked zero-weight outcome {i}");
        }
    }
}
