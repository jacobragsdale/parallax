//! Technical-analysis indicators, computed over every row before a chart is
//! reduced to the screen: a 200-day average of thinned points would be wrong.
//!
//! Each function takes a series without gaps, in x order, and returns one value
//! per input, NaN where the indicator has no value yet or none at all. The
//! definitions are TA-Lib's, including where conventions differ between
//! libraries, such as an EMA seeded with the SMA of its first values, and the
//! tests compare every indicator with TA-Lib. Two cases differ on purpose,
//! because TA-Lib reports zero where there is no answer: a stochastic over a
//! range with no movement, and a VWAP before any volume has traded, have no
//! value here.

use std::collections::VecDeque;

use crate::error::ConfigError;

/// The longest period an indicator accepts, as in TA-Lib.
pub const MAX_PERIOD: u32 = 100_000;

/// An indicator a chart computes over every row, for each series.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Indicator {
    /// The simple moving average of the price.
    Sma {
        /// Rows averaged.
        period: u32,
    },
    /// The exponential moving average of the price.
    Ema {
        /// Rows in the smoothing, `2 / (period + 1)`.
        period: u32,
    },
    /// The weighted moving average of the price.
    Wma {
        /// Rows averaged.
        period: u32,
    },
    /// Bollinger Bands around the price's simple average.
    Bollinger {
        /// Rows averaged.
        period: u32,
        /// Standard deviations above and below.
        deviations: f64,
    },
    /// The relative strength index of the price, from 0 to 100.
    Rsi {
        /// Rows in Wilder's smoothing.
        period: u32,
    },
    /// MACD of the price.
    Macd {
        /// Rows of the fast average.
        fast: u32,
        /// Rows of the slow average, more than the fast one's.
        slow: u32,
        /// Rows of the signal line's average.
        signal: u32,
    },
    /// The average true range, from the high, low and close.
    Atr {
        /// Rows in Wilder's smoothing.
        period: u32,
    },
    /// The slow stochastic oscillator, from the high, low and close.
    Stochastic {
        /// Rows whose range the close is placed in.
        k: u32,
        /// Rows %K is averaged over.
        k_smooth: u32,
        /// Rows %D averages %K over.
        d: u32,
    },
    /// The volume-weighted average price, from each trading day's first bar for
    /// instants and from the first row for dates.
    Vwap,
    /// Volume, summed over each candle's bucket.
    Volume,
}

/// What an indicator reads from each row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inputs {
    /// The price: a candle's close, or a line's first plotted column.
    Price,
    /// The high, low and close.
    Range,
    /// The high, low, close and volume.
    RangeAndVolume,
    /// Volume alone, summed per candle rather than computed per row.
    Volume,
}

impl Indicator {
    /// The names of its lines, in the order [`Indicator::compute`] returns them.
    #[must_use]
    pub fn lines(&self) -> &'static [&'static str] {
        match self {
            Self::Sma { .. } => &["sma"],
            Self::Ema { .. } => &["ema"],
            Self::Wma { .. } => &["wma"],
            Self::Bollinger { .. } => &["upper", "middle", "lower"],
            Self::Rsi { .. } => &["rsi"],
            Self::Macd { .. } => &["macd", "signal", "histogram"],
            Self::Atr { .. } => &["atr"],
            Self::Stochastic { .. } => &["k", "d"],
            Self::Vwap => &["vwap"],
            Self::Volume => &["volume"],
        }
    }

    /// Its name in messages.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Sma { .. } => "an SMA",
            Self::Ema { .. } => "an EMA",
            Self::Wma { .. } => "a WMA",
            Self::Bollinger { .. } => "Bollinger Bands",
            Self::Rsi { .. } => "an RSI",
            Self::Macd { .. } => "MACD",
            Self::Atr { .. } => "an ATR",
            Self::Stochastic { .. } => "a stochastic",
            Self::Vwap => "a VWAP",
            Self::Volume => "volume",
        }
    }

    /// What it reads.
    #[must_use]
    pub fn inputs(&self) -> Inputs {
        match self {
            Self::Sma { .. }
            | Self::Ema { .. }
            | Self::Wma { .. }
            | Self::Bollinger { .. }
            | Self::Rsi { .. }
            | Self::Macd { .. } => Inputs::Price,
            Self::Atr { .. } | Self::Stochastic { .. } => Inputs::Range,
            Self::Vwap => Inputs::RangeAndVolume,
            Self::Volume => Inputs::Volume,
        }
    }

    /// Whether its values are in the price's units, rather than an oscillator's
    /// 0 to 100 or volume.
    #[must_use]
    pub fn in_price_units(&self) -> bool {
        !matches!(
            self,
            Self::Rsi { .. } | Self::Stochastic { .. } | Self::Volume
        )
    }

    /// Checks its parameters.
    ///
    /// # Errors
    ///
    /// Returns an error for a period of zero or above [`MAX_PERIOD`], a MACD whose
    /// fast average is not shorter than its slow one, or deviations that are not
    /// a positive finite number.
    pub fn check(&self) -> Result<(), ConfigError> {
        let periods: &[(&str, u32)] = match self {
            Self::Sma { period }
            | Self::Ema { period }
            | Self::Wma { period }
            | Self::Rsi { period }
            | Self::Atr { period } => &[("period", *period)],
            Self::Bollinger { period, deviations } => {
                if !(deviations.is_finite() && *deviations > 0.0) {
                    return Err(ConfigError::Chart(format!(
                        "Bollinger Bands need a positive number of deviations, not {deviations}"
                    )));
                }
                &[("period", *period)]
            }
            Self::Macd { fast, slow, signal } => {
                if fast >= slow {
                    return Err(ConfigError::Chart(format!(
                        "MACD's fast average must be shorter than its slow one, not {fast} against {slow}"
                    )));
                }
                &[("fast", *fast), ("slow", *slow), ("signal", *signal)]
            }
            Self::Stochastic { k, k_smooth, d } => &[("k", *k), ("k_smooth", *k_smooth), ("d", *d)],
            Self::Vwap | Self::Volume => &[],
        };
        for &(name, value) in periods {
            if value == 0 || value > MAX_PERIOD {
                return Err(ConfigError::Chart(format!(
                    "an indicator's {name} must be from 1 to {MAX_PERIOD}, not {value}"
                )));
            }
        }
        Ok(())
    }

    /// Computes its lines over one series without gaps: `price` for the price
    /// indicators, `high`, `low`, `close` and `volume` for the others, and
    /// `sessions`, the rows where a VWAP starts again. Volume is summed per candle
    /// instead, and gives no lines here.
    #[must_use]
    pub fn compute(&self, inputs: &Series<'_>) -> Vec<Vec<f64>> {
        let p = inputs.price;
        let n = |v: u32| v as usize;
        match *self {
            Self::Sma { period } => vec![sma(p, n(period))],
            Self::Ema { period } => vec![ema(p, n(period))],
            Self::Wma { period } => vec![wma(p, n(period))],
            Self::Bollinger { period, deviations } => {
                let b = bollinger(p, n(period), deviations);
                vec![b.upper, b.middle, b.lower]
            }
            Self::Rsi { period } => vec![rsi(p, n(period))],
            Self::Macd { fast, slow, signal } => {
                let m = macd(p, n(fast), n(slow), n(signal));
                vec![m.macd, m.signal, m.histogram]
            }
            Self::Atr { period } => vec![atr(inputs.high, inputs.low, inputs.close, n(period))],
            Self::Stochastic { k, k_smooth, d } => {
                let s = stochastic(
                    inputs.high,
                    inputs.low,
                    inputs.close,
                    n(k),
                    n(k_smooth),
                    n(d),
                );
                vec![s.k, s.d]
            }
            Self::Vwap => vec![vwap(
                inputs.high,
                inputs.low,
                inputs.close,
                inputs.volume,
                inputs.sessions,
            )],
            Self::Volume => Vec::new(),
        }
    }
}

/// One series' inputs, without gaps, in x order. Inputs an indicator does not
/// read may be empty.
#[derive(Clone, Copy, Debug, Default)]
pub struct Series<'a> {
    /// The price.
    pub price: &'a [f64],
    /// The highs.
    pub high: &'a [f64],
    /// The lows.
    pub low: &'a [f64],
    /// The closes.
    pub close: &'a [f64],
    /// The volumes.
    pub volume: &'a [f64],
    /// The rows where a VWAP starts again.
    pub sessions: &'a [usize],
}

/// A running sum kept as two doubles, the sum and the rounding error it has
/// shed (Knuth's two-sum), so that adding and removing millions of values does
/// not drift as a plain running sum would.
#[derive(Clone, Copy, Debug, Default)]
struct RunningSum {
    sum: f64,
    error: f64,
}

impl RunningSum {
    fn add(&mut self, value: f64) {
        let sum = self.sum + value;
        let shed = if self.sum.abs() >= value.abs() {
            (self.sum - sum) + value
        } else {
            (value - sum) + self.sum
        };
        self.sum = sum;
        self.error += shed;
    }

    fn value(self) -> f64 {
        self.sum + self.error
    }
}

/// The simple moving average of the last `period` values, from the
/// `period`-th value on. Each window's sum is carried with its rounding error,
/// so the average never drifts however long the series.
#[must_use]
pub fn sma(values: &[f64], period: usize) -> Vec<f64> {
    let period = period.max(1);
    let mut out = vec![f64::NAN; values.len()];
    let mut sum = RunningSum::default();
    for (i, &value) in values.iter().enumerate() {
        sum.add(value);
        if i >= period {
            sum.add(-values[i - period]);
        }
        if i + 1 >= period {
            out[i] = sum.value() / period as f64;
        }
    }
    out
}

/// The exponential moving average with smoothing `2 / (period + 1)`, seeded with
/// the simple average of the first `period` values, as TA-Lib does.
#[must_use]
pub fn ema(values: &[f64], period: usize) -> Vec<f64> {
    let period = period.max(1);
    let mut out = vec![f64::NAN; values.len()];
    if values.len() < period {
        return out;
    }
    ema_from(values, period, period - 1, &mut out);
    out
}

/// Writes the EMA of `values` into `out` from index `start`, seeded with the
/// simple average of the `period` values ending there.
fn ema_from(values: &[f64], period: usize, start: usize, out: &mut [f64]) {
    let k = 2.0 / (period as f64 + 1.0);
    let seed: f64 = values[start + 1 - period..=start].iter().sum();
    let mut previous = seed / period as f64;
    out[start] = previous;
    for i in start + 1..values.len() {
        previous = (values[i] - previous) * k + previous;
        out[i] = previous;
    }
}

/// The weighted moving average: the latest value weighs `period`, the one
/// before it `period - 1`, and so on.
#[must_use]
pub fn wma(values: &[f64], period: usize) -> Vec<f64> {
    let period = period.max(1);
    let divisor = (period * (period + 1) / 2) as f64;
    let mut out = vec![f64::NAN; values.len()];
    for i in period.saturating_sub(1)..values.len() {
        let window = &values[i + 1 - period..=i];
        let weighted: f64 = window
            .iter()
            .enumerate()
            .map(|(w, &v)| (w + 1) as f64 * v)
            .sum();
        out[i] = weighted / divisor;
    }
    out
}

/// Bollinger Bands: the simple average, and that plus and minus `deviations`
/// population standard deviations of the same window.
#[derive(Clone, Debug, PartialEq)]
pub struct Bands {
    /// The average plus the deviations.
    pub upper: Vec<f64>,
    /// The simple average.
    pub middle: Vec<f64>,
    /// The average minus the deviations.
    pub lower: Vec<f64>,
}

/// Bollinger Bands over `period` values. The deviation is computed from each
/// window's own mean, in two passes, rather than from running sums of squares.
#[must_use]
pub fn bollinger(values: &[f64], period: usize, deviations: f64) -> Bands {
    let period = period.max(1);
    let middle = sma(values, period);
    let mut upper = vec![f64::NAN; values.len()];
    let mut lower = vec![f64::NAN; values.len()];
    for i in period.saturating_sub(1)..values.len() {
        let mean = middle[i];
        let squares: f64 = values[i + 1 - period..=i]
            .iter()
            .map(|&v| (v - mean) * (v - mean))
            .sum();
        let spread = deviations * (squares / period as f64).sqrt();
        upper[i] = mean + spread;
        lower[i] = mean - spread;
    }
    Bands {
        upper,
        middle,
        lower,
    }
}

/// The relative strength index with Wilder's smoothing, from the value after
/// the first `period` changes. A window without movement reads 50.
#[must_use]
pub fn rsi(values: &[f64], period: usize) -> Vec<f64> {
    let period = period.max(1);
    let mut out = vec![f64::NAN; values.len()];
    if values.len() <= period {
        return out;
    }
    let reading = |gain: f64, loss: f64| {
        let total = gain + loss;
        if total == 0.0 {
            50.0
        } else {
            100.0 * (gain / total)
        }
    };
    let (mut gain, mut loss) = (0.0, 0.0);
    for i in 1..=period {
        let change = values[i] - values[i - 1];
        if change < 0.0 {
            loss -= change;
        } else {
            gain += change;
        }
    }
    let n = period as f64;
    gain /= n;
    loss /= n;
    out[period] = reading(gain, loss);
    for i in period + 1..values.len() {
        let change = values[i] - values[i - 1];
        gain *= n - 1.0;
        loss *= n - 1.0;
        if change < 0.0 {
            loss -= change;
        } else {
            gain += change;
        }
        gain /= n;
        loss /= n;
        out[i] = reading(gain, loss);
    }
    out
}

/// MACD: the fast EMA less the slow one, its signal line, and their difference.
#[derive(Clone, Debug, PartialEq)]
pub struct Macd {
    /// The fast EMA less the slow EMA.
    pub macd: Vec<f64>,
    /// The EMA of the MACD line.
    pub signal: Vec<f64>,
    /// The MACD line less its signal line.
    pub histogram: Vec<f64>,
}

/// MACD with TA-Lib's alignment: both averages start where the slow one has its
/// first value, the fast one seeded with the average of the `fast` values ending
/// there, and all three lines start where the signal line has its first value.
/// `fast` must be shorter than `slow`.
#[must_use]
pub fn macd(values: &[f64], fast: usize, slow: usize, signal: usize) -> Macd {
    let (fast, slow, signal) = (fast.max(1), slow.max(2), signal.max(1));
    let n = values.len();
    let empty = || vec![f64::NAN; n];
    let first = slow + signal - 2;
    if n <= first {
        return Macd {
            macd: empty(),
            signal: empty(),
            histogram: empty(),
        };
    }
    let start = slow - 1;
    let (mut fast_ema, mut slow_ema) = (empty(), empty());
    ema_from(values, fast, start, &mut fast_ema);
    ema_from(values, slow, start, &mut slow_ema);
    let line: Vec<f64> = (start..n).map(|i| fast_ema[i] - slow_ema[i]).collect();
    let line_signal = ema(&line, signal);
    let (mut macd, mut signal_out, mut histogram) = (empty(), empty(), empty());
    for i in first..n {
        let (m, s) = (line[i - start], line_signal[i - start]);
        macd[i] = m;
        signal_out[i] = s;
        histogram[i] = m - s;
    }
    Macd {
        macd,
        signal: signal_out,
        histogram,
    }
}

/// The average true range with Wilder's smoothing, from the `period`-th true
/// range on. The first row has no true range, having no close before it.
#[must_use]
pub fn atr(high: &[f64], low: &[f64], close: &[f64], period: usize) -> Vec<f64> {
    let period = period.max(1);
    let n = close.len();
    let mut out = vec![f64::NAN; n];
    if n <= period {
        return out;
    }
    let true_range = |i: usize| {
        let previous = close[i - 1];
        (high[i] - low[i])
            .max((high[i] - previous).abs())
            .max((low[i] - previous).abs())
    };
    if period == 1 {
        for (i, value) in out.iter_mut().enumerate().skip(1) {
            *value = true_range(i);
        }
        return out;
    }
    let first: f64 = (1..=period).map(true_range).sum();
    let mut previous = first / period as f64;
    out[period] = previous;
    for (i, value) in out.iter_mut().enumerate().skip(period + 1) {
        previous *= (period - 1) as f64;
        previous += true_range(i);
        previous /= period as f64;
        *value = previous;
    }
    out
}

/// The slow stochastic oscillator's %K and %D lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Stochastic {
    /// %K: where the close sits in the range of the last `k` rows, from 0 at the
    /// lowest low to 100 at the highest high, averaged over `k_smooth` rows.
    pub k: Vec<f64>,
    /// %D: %K averaged over `d` rows.
    pub d: Vec<f64>,
}

/// The slow stochastic, with simple averages for both smoothings. A window
/// whose highest high equals its lowest low has no %K, so neither line has a
/// value for the windows that include it.
#[must_use]
pub fn stochastic(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    k: usize,
    k_smooth: usize,
    d: usize,
) -> Stochastic {
    let k = k.max(1);
    let fast = fast_k(high, low, close, k);
    let slow_k = average_defined(&fast, k_smooth.max(1));
    let slow_d = average_defined(&slow_k, d.max(1));
    // Both lines start where %D does, as in TA-Lib.
    let first = (k - 1) + (k_smooth.max(1) - 1) + (d.max(1) - 1);
    let mut k_out = slow_k;
    for value in k_out.iter_mut().take(first.min(close.len())) {
        *value = f64::NAN;
    }
    Stochastic {
        k: k_out,
        d: slow_d,
    }
}

/// The fast %K over windows of `k` rows, with running extremes kept in
/// monotonic queues.
fn fast_k(high: &[f64], low: &[f64], close: &[f64], k: usize) -> Vec<f64> {
    let n = close.len();
    let mut out = vec![f64::NAN; n];
    let (mut highs, mut lows): (VecDeque<usize>, VecDeque<usize>) =
        (VecDeque::new(), VecDeque::new());
    for i in 0..n {
        while highs.back().is_some_and(|&j| high[j] <= high[i]) {
            highs.pop_back();
        }
        highs.push_back(i);
        while lows.back().is_some_and(|&j| low[j] >= low[i]) {
            lows.pop_back();
        }
        lows.push_back(i);
        if i + 1 < k {
            continue;
        }
        let oldest = i + 1 - k;
        while highs.front().is_some_and(|&j| j < oldest) {
            highs.pop_front();
        }
        while lows.front().is_some_and(|&j| j < oldest) {
            lows.pop_front();
        }
        let (highest, lowest) = (high[highs[0]], low[lows[0]]);
        if highest > lowest {
            out[i] = (close[i] - lowest) / (highest - lowest) * 100.0;
        }
    }
    out
}

/// The simple average of the last `period` values, NaN where any of them is.
fn average_defined(values: &[f64], period: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; values.len()];
    for i in period.saturating_sub(1)..values.len() {
        let window = &values[i + 1 - period..=i];
        if window.iter().all(|v| !v.is_nan()) {
            out[i] = window.iter().sum::<f64>() / period as f64;
        }
    }
    out
}

/// The volume-weighted average of the typical price, `(high + low + close) / 3`,
/// from each session's first row: `starts` lists the rows where sessions begin,
/// in order, and the first row always begins one. Rows before any volume has
/// traded in their session have no value.
#[must_use]
pub fn vwap(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    volume: &[f64],
    starts: &[usize],
) -> Vec<f64> {
    let n = close.len();
    let mut out = vec![f64::NAN; n];
    let (mut weighted, mut traded) = (RunningSum::default(), RunningSum::default());
    let mut next_start = starts.iter().copied().peekable();
    for i in 0..n {
        if i == 0 || next_start.peek() == Some(&i) {
            weighted = RunningSum::default();
            traded = RunningSum::default();
        }
        while next_start.peek().is_some_and(|&s| s <= i) {
            next_start.next();
        }
        let typical = (high[i] + low[i] + close[i]) / 3.0;
        weighted.add(typical * volume[i]);
        traded.add(volume[i]);
        let total = traded.value();
        if total != 0.0 {
            out[i] = weighted.value() / total;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_series_have_no_values() {
        assert!(sma(&[1.0, 2.0], 3).iter().all(|v| v.is_nan()));
        assert!(ema(&[1.0, 2.0], 3).iter().all(|v| v.is_nan()));
        assert!(rsi(&[1.0, 2.0], 2).iter().all(|v| v.is_nan()));
        assert!(atr(&[1.0], &[1.0], &[1.0], 1).iter().all(|v| v.is_nan()));
        let m = macd(&[1.0; 10], 3, 5, 9);
        assert!(m.macd.iter().all(|v| v.is_nan()));
        assert!(vwap(&[], &[], &[], &[], &[]).is_empty());
    }

    #[test]
    fn a_flat_window_has_no_stochastic_and_no_volume_no_vwap() {
        let flat = [5.0; 6];
        let s = stochastic(&flat, &flat, &flat, 3, 1, 1);
        assert!(s.k.iter().all(|v| v.is_nan()));
        let w = vwap(&[2.0, 2.0], &[1.0, 1.0], &[1.5, 1.5], &[0.0, 10.0], &[0]);
        assert!(w[0].is_nan());
        assert_eq!(w[1], 1.5);
    }

    #[test]
    fn a_session_restarts_the_vwap() {
        let price = [1.0, 1.0, 4.0, 4.0];
        let w = vwap(&price, &price, &price, &[1.0, 1.0, 1.0, 1.0], &[0, 2]);
        assert_eq!(w, vec![1.0, 1.0, 4.0, 4.0]);
    }

    #[test]
    fn a_long_average_does_not_drift() {
        // A million prices around 10,000. After them, a plain running sum is a
        // dozen units in the last place off the exact mean; ours stays within one.
        let mut state: u64 = 12_345;
        let values: Vec<f64> = (0..1_000_000)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                1e4 + (state >> 11) as f64 / (1u64 << 53) as f64 * 100.0
            })
            .collect();
        let period = 20;
        let out = sma(&values, period);
        let mut plain = 0.0;
        let (mut ours_worst, mut plain_worst) = (0.0f64, 0.0f64);
        for (i, &v) in values.iter().enumerate() {
            plain += v;
            if i >= period {
                plain -= values[i - period];
            }
            if i + 1000 < values.len() {
                continue;
            }
            let mut exact = crate::exact::ExactSum::new();
            for &w in &values[i + 1 - period..=i] {
                exact.add(w);
            }
            let mean = exact.mean(period as u64);
            let ulp = f64::from_bits(mean.to_bits() + 1) - mean;
            ours_worst = ours_worst.max((out[i] - mean).abs() / ulp);
            plain_worst = plain_worst.max((plain / period as f64 - mean).abs() / ulp);
        }
        assert!(ours_worst <= 1.0, "{ours_worst} units in the last place");
        assert!(
            plain_worst > 4.0,
            "a plain running sum drifts here, so the test can tell: {plain_worst}"
        );
    }
}
