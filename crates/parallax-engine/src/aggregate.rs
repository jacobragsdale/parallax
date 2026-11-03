//! Measures and the state that accumulates them.
//!
//! Every state is exact and supports removal, so a view updated by changes holds the
//! same state as a view built from scratch. Sums, counts, means, variances and
//! weighted means are kept at every node of a pivot. Minimum, maximum, median,
//! percentiles, distinct counts, first and last are kept at the leaves only and merged
//! upward when a value is needed.

use std::collections::BTreeMap;

use arrow_buffer::i256;
use rustc_hash::FxHashMap;

use crate::bignum::{BigUint, big_ratio_to_f64, ratio_to_f64};
use crate::exact::ExactSum;
use crate::format::Format;
use crate::types::{ColumnType, Key};

/// How a measure combines the values of a group.
#[derive(Clone, Debug, PartialEq)]
pub enum Aggregate {
    /// Rows with a non-null value.
    Count,
    /// All rows, whatever the column holds.
    CountAll,
    /// The exact sum.
    Sum,
    /// The exact sum divided by the count, rounded once.
    Mean,
    /// The sum of value × weight divided by the sum of weights.
    WeightedMean,
    /// The smallest value.
    Min,
    /// The largest value.
    Max,
    /// The non-null value of the earliest inserted row.
    First,
    /// The non-null value of the latest inserted row.
    Last,
    /// The number of distinct non-null values.
    DistinctCount,
    /// The sample variance, dividing by n - 1.
    Variance,
    /// The sample standard deviation.
    StdDev,
    /// The median, interpolated as for `Percentile(0.5)`.
    Median,
    /// A percentile in `[0, 1]`, interpolated linearly between ranks like Excel's
    /// `PERCENTILE.INC` and DuckDB's `quantile_cont`.
    Percentile(f64),
}

impl Aggregate {
    /// The name used in column labels.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Self::Count | Self::CountAll => "count".into(),
            Self::Sum => "sum".into(),
            Self::Mean => "mean".into(),
            Self::WeightedMean => "wmean".into(),
            Self::Min => "min".into(),
            Self::Max => "max".into(),
            Self::First => "first".into(),
            Self::Last => "last".into(),
            Self::DistinctCount => "distinct".into(),
            Self::Variance => "var".into(),
            Self::StdDev => "stddev".into(),
            Self::Median => "median".into(),
            Self::Percentile(p) => format!("p{}", p * 100.0),
        }
    }

    /// Returns `true` for aggregates kept only at leaves.
    pub(crate) fn is_lazy(&self) -> bool {
        matches!(
            self,
            Self::Min
                | Self::Max
                | Self::First
                | Self::Last
                | Self::DistinctCount
                | Self::Median
                | Self::Percentile(_)
        )
    }

    pub(crate) fn needs_numeric(&self) -> bool {
        matches!(
            self,
            Self::Sum
                | Self::Mean
                | Self::WeightedMean
                | Self::Variance
                | Self::StdDev
                | Self::Median
                | Self::Percentile(_)
        )
    }
}

/// How a measure's value is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShowAs {
    /// The value itself.
    #[default]
    Value,
    /// The value as a fraction of the grand total.
    PercentOfGrandTotal,
    /// The value as a fraction of its column's total: the grand total for the same
    /// column-pivot key.
    PercentOfColumnTotal,
    /// The value as a fraction of its row's total across column-pivot keys.
    PercentOfRowTotal,
    /// The value as a fraction of the parent row's value in the same column.
    PercentOfParent,
}

/// One measure of a grouped view.
#[derive(Clone, Debug, PartialEq)]
pub struct Measure {
    /// The column aggregated. `None` only for [`Aggregate::CountAll`].
    pub column: Option<String>,
    /// How values combine.
    pub aggregate: Aggregate,
    /// The weight column, for [`Aggregate::WeightedMean`].
    pub weight: Option<String>,
    /// A column naming each row's unit, such as `currency`. A group mixing units
    /// shows as mixed instead of combining them.
    pub unit_column: Option<String>,
    /// How the value is shown.
    pub show_as: ShowAs,
    /// The display format; `None` derives one from the column's.
    pub format: Option<Format>,
}

impl Measure {
    /// A measure of `aggregate` over `column`.
    #[must_use]
    pub fn new(aggregate: Aggregate, column: &str) -> Self {
        Self {
            column: Some(column.to_owned()),
            aggregate,
            weight: None,
            unit_column: None,
            show_as: ShowAs::Value,
            format: None,
        }
    }

    /// A row count.
    #[must_use]
    pub fn count_all() -> Self {
        Self {
            column: None,
            aggregate: Aggregate::CountAll,
            weight: None,
            unit_column: None,
            show_as: ShowAs::Value,
            format: None,
        }
    }

    /// A weighted mean of `column` by `weight`.
    #[must_use]
    pub fn weighted_mean(column: &str, weight: &str) -> Self {
        Self {
            weight: Some(weight.to_owned()),
            ..Self::new(Aggregate::WeightedMean, column)
        }
    }

    /// Sets the unit column.
    #[must_use]
    pub fn with_unit_column(mut self, column: &str) -> Self {
        self.unit_column = Some(column.to_owned());
        self
    }

    /// Sets how the value is shown.
    #[must_use]
    pub fn show_as(mut self, show_as: ShowAs) -> Self {
        self.show_as = show_as;
        self
    }

    /// The column label, such as `sum(market_value_usd)`.
    #[must_use]
    pub fn label(&self) -> String {
        let base = match (&self.aggregate, &self.column, &self.weight) {
            (Aggregate::CountAll, _, _) | (_, None, _) => "count(*)".to_owned(),
            (Aggregate::WeightedMean, Some(c), Some(w)) => format!("wmean({c} by {w})"),
            (a, Some(c), _) => format!("{}({c})", a.name()),
        };
        match self.show_as {
            ShowAs::Value => base,
            ShowAs::PercentOfGrandTotal => format!("{base} % of total"),
            ShowAs::PercentOfColumnTotal => format!("{base} % of column"),
            ShowAs::PercentOfRowTotal => format!("{base} % of row"),
            ShowAs::PercentOfParent => format!("{base} % of parent"),
        }
    }
}

/// A row's input to one measure.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Input {
    /// The value as a key, for lazy measures and null checks.
    pub key: Key,
    /// The value as a double, for floating-point columns.
    pub float: Option<f64>,
    /// The unscaled value, for integer and decimal columns.
    pub exact: Option<i128>,
    /// The weight, as a double and as an unscaled value.
    pub weight_float: Option<f64>,
    pub weight_exact: Option<i128>,
    /// The row's unit, if the measure has a unit column.
    pub unit: Option<Key>,
    /// The row's insertion sequence number.
    pub seq: u64,
}

/// Accumulated state for one measure in one cell.
#[derive(Clone, Debug)]
pub(crate) enum State {
    Count(i64),
    // Exact accumulators are hundreds of bytes and a pivot holds a state per cell
    // and measure, so they are boxed: every state is as large as the largest
    // variant, which would otherwise make a count as large as two accumulators.
    Float {
        sum: Box<ExactSum>,
        /// The exact sum of squares, kept only for variances.
        squares: Option<Box<ExactSum>>,
        n: i64,
    },
    Exact {
        sum: i256,
        squares: i256,
        n: i64,
        /// Values too large to square without risk of overflow, for variances.
        huge: i64,
    },
    WeightedFloat {
        weighted: Box<ExactSum>,
        weights: Box<ExactSum>,
        n: i64,
    },
    WeightedExact {
        weighted: i256,
        weights: i256,
        n: i64,
        /// Rows whose value or weight is too large to multiply without overflow.
        huge: i64,
    },
    Values(FxHashMap<Key, u32>),
    Sequence(BTreeMap<u64, Key>),
    /// A lazy measure at a node above the leaves.
    Lazy,
}

/// The value a measure produces, before formatting.
#[derive(Clone, Debug, PartialEq)]
pub enum MeasureValue {
    /// No value: no rows, or only nulls.
    Null,
    /// An integer count.
    Count(i64),
    /// A double.
    Float(f64),
    /// An unscaled decimal at the measure's output scale.
    Decimal(i128),
    /// A key of the source column, for minimum, maximum, first and last.
    Key(KeyValue),
    /// The exact result does not fit 38 decimal digits.
    Overflow,
    /// The group mixes units.
    Mixed,
}

/// A source-column value carried out of the engine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyValue(pub(crate) Key);

/// What a compiled measure reads from each row.
#[derive(Clone, Debug)]
pub(crate) struct CompiledMeasure {
    pub aggregate: Aggregate,
    pub column: Option<usize>,
    pub column_type: Option<ColumnType>,
    pub weight: Option<usize>,
    pub weight_type: Option<ColumnType>,
    pub unit: Option<usize>,
    pub show_as: ShowAs,
}

impl CompiledMeasure {
    fn exact_input(&self) -> bool {
        matches!(
            self.column_type,
            Some(ColumnType::Int64 | ColumnType::Decimal { .. })
        )
    }

    fn exact_weight(&self) -> bool {
        matches!(
            self.weight_type,
            Some(ColumnType::Int64 | ColumnType::Decimal { .. })
        )
    }

    pub fn scale(&self) -> i8 {
        match self.column_type {
            Some(ColumnType::Decimal { scale, .. }) => scale,
            _ => 0,
        }
    }

    fn weight_scale(&self) -> i8 {
        match self.weight_type {
            Some(ColumnType::Decimal { scale, .. }) => scale,
            _ => 0,
        }
    }

    /// Returns an empty state for a cell at a leaf (`leaf`) or above.
    pub fn empty(&self, leaf: bool) -> State {
        match &self.aggregate {
            Aggregate::Count | Aggregate::CountAll => State::Count(0),
            Aggregate::Sum | Aggregate::Mean | Aggregate::Variance | Aggregate::StdDev => {
                if self.exact_input() {
                    State::Exact {
                        sum: i256::ZERO,
                        squares: i256::ZERO,
                        n: 0,
                        huge: 0,
                    }
                } else {
                    State::Float {
                        sum: Box::new(ExactSum::new()),
                        squares: matches!(self.aggregate, Aggregate::Variance | Aggregate::StdDev)
                            .then(|| Box::new(ExactSum::new())),
                        n: 0,
                    }
                }
            }
            Aggregate::WeightedMean => {
                if self.exact_input() && self.exact_weight() {
                    State::WeightedExact {
                        weighted: i256::ZERO,
                        weights: i256::ZERO,
                        n: 0,
                        huge: 0,
                    }
                } else {
                    State::WeightedFloat {
                        weighted: Box::new(ExactSum::new()),
                        weights: Box::new(ExactSum::new()),
                        n: 0,
                    }
                }
            }
            a if a.is_lazy() && !leaf => State::Lazy,
            Aggregate::First | Aggregate::Last => State::Sequence(BTreeMap::new()),
            _ => State::Values(FxHashMap::default()),
        }
    }
}

/// Adds the exact square of `x` to `sum`, using an error-free product.
fn add_square(sum: &mut ExactSum, x: f64, sign: i64) {
    let hi = x * x;
    let lo = libm::fma(x, x, -hi);
    if sign > 0 {
        sum.add(hi);
        sum.add(lo);
    } else {
        sum.remove(hi);
        sum.remove(lo);
    }
}

/// Adds the exact product `x * w` to `sum`.
fn add_product(sum: &mut ExactSum, x: f64, w: f64, sign: i64) {
    let hi = x * w;
    let lo = libm::fma(x, w, -hi);
    if sign > 0 {
        sum.add(hi);
        sum.add(lo);
    } else {
        sum.remove(hi);
        sum.remove(lo);
    }
}

fn count_key<K: std::hash::Hash + Eq + Copy>(map: &mut FxHashMap<K, u32>, key: K, sign: i64) {
    if sign > 0 {
        *map.entry(key).or_insert(0) += 1;
    } else if let Some(count) = map.get_mut(&key) {
        *count -= 1;
        if *count == 0 {
            map.remove(&key);
        }
    }
}

impl State {
    /// Adds (`sign` = 1) or removes (`sign` = -1) one row's input.
    pub fn apply(&mut self, measure: &CompiledMeasure, input: &Input, sign: i64) {
        let present = input.key != Key::Null;
        match self {
            Self::Count(c) => {
                if measure.aggregate == Aggregate::CountAll || present {
                    *c += sign;
                }
            }
            Self::Float { sum, squares, n } => {
                if let Some(x) = input.float {
                    if sign > 0 {
                        sum.add(x);
                    } else {
                        sum.remove(x);
                    }
                    if let Some(squares) = squares.as_deref_mut()
                        && x.is_finite()
                    {
                        add_square(squares, x, sign);
                    }
                    *n += sign;
                }
            }
            Self::Exact {
                sum,
                squares,
                n,
                huge,
            } => {
                if let Some(x) = input.exact {
                    let wide = i256::from_i128(x);
                    *sum = if sign > 0 {
                        sum.wrapping_add(wide)
                    } else {
                        sum.wrapping_sub(wide)
                    };
                    if matches!(measure.aggregate, Aggregate::Variance | Aggregate::StdDev) {
                        // Below 2^100, at most 2^54 squares fit an i256 exactly.
                        if x.unsigned_abs() > HUGE {
                            *huge += sign;
                        } else {
                            let square = wide.wrapping_mul(wide);
                            *squares = if sign > 0 {
                                squares.wrapping_add(square)
                            } else {
                                squares.wrapping_sub(square)
                            };
                        }
                    }
                    *n += sign;
                }
            }
            Self::WeightedFloat {
                weighted,
                weights,
                n,
            } => {
                let x = input.float.or(input
                    .exact
                    .map(|e| ratio_to_f64(e, &[pow10(measure.scale())])));
                let w = input.weight_float.or(input
                    .weight_exact
                    .map(|e| ratio_to_f64(e, &[pow10(measure.weight_scale())])));
                if let (Some(x), Some(w)) = (x, w) {
                    add_product(weighted, x, w, sign);
                    if sign > 0 {
                        weights.add(w);
                    } else {
                        weights.remove(w);
                    }
                    *n += sign;
                }
            }
            Self::WeightedExact {
                weighted,
                weights,
                n,
                huge,
            } => {
                if let (Some(x), Some(w)) = (input.exact, input.weight_exact) {
                    if x.unsigned_abs() > HUGE || w.unsigned_abs() > HUGE {
                        *huge += sign;
                    } else {
                        let product = i256::from_i128(x).wrapping_mul(i256::from_i128(w));
                        let w = i256::from_i128(w);
                        if sign > 0 {
                            *weighted = weighted.wrapping_add(product);
                            *weights = weights.wrapping_add(w);
                        } else {
                            *weighted = weighted.wrapping_sub(product);
                            *weights = weights.wrapping_sub(w);
                        }
                    }
                    *n += sign;
                }
            }
            Self::Values(map) => {
                if present {
                    count_key(map, input.key, sign);
                }
            }
            Self::Sequence(map) => {
                if present {
                    if sign > 0 {
                        map.insert(input.seq, input.key);
                    } else {
                        map.remove(&input.seq);
                    }
                }
            }
            Self::Lazy => {}
        }
    }

    /// Merges `other` into `self`. Lazy states merge only at leaves.
    pub fn merge(&mut self, other: &Self) {
        match (self, other) {
            (Self::Count(a), Self::Count(b)) => *a += b,
            (
                Self::Float { sum, squares, n },
                Self::Float {
                    sum: s2,
                    squares: q2,
                    n: n2,
                },
            ) => {
                sum.merge(s2);
                match (squares.as_deref_mut(), q2.as_deref()) {
                    (Some(a), Some(b)) => a.merge(b),
                    (None, None) => {}
                    _ => unreachable!("states of one measure keep squares alike"),
                }
                *n += n2;
            }
            (
                Self::Exact {
                    sum,
                    squares,
                    n,
                    huge,
                },
                Self::Exact {
                    sum: s2,
                    squares: q2,
                    n: n2,
                    huge: h2,
                },
            ) => {
                *sum = sum.wrapping_add(*s2);
                *squares = squares.wrapping_add(*q2);
                *n += n2;
                *huge += h2;
            }
            (
                Self::WeightedFloat {
                    weighted,
                    weights,
                    n,
                },
                Self::WeightedFloat {
                    weighted: x2,
                    weights: w2,
                    n: n2,
                },
            ) => {
                weighted.merge(x2);
                weights.merge(w2);
                *n += n2;
            }
            (
                Self::WeightedExact {
                    weighted,
                    weights,
                    n,
                    huge,
                },
                Self::WeightedExact {
                    weighted: x2,
                    weights: w2,
                    n: n2,
                    huge: h2,
                },
            ) => {
                *weighted = weighted.wrapping_add(*x2);
                *weights = weights.wrapping_add(*w2);
                *n += n2;
                *huge += h2;
            }
            (Self::Values(a), Self::Values(b)) => {
                for (&k, &c) in b {
                    *a.entry(k).or_insert(0) += c;
                }
            }
            (Self::Sequence(a), Self::Sequence(b)) => {
                a.extend(b.iter().map(|(&s, &k)| (s, k)));
            }
            (Self::Lazy, _) | (_, Self::Lazy) => {}
            _ => unreachable!("states of one measure share a kind"),
        }
    }
}

// A state per cell and measure: keep it small.
const _: () = assert!(std::mem::size_of::<State>() <= 96);

/// Returns `10^scale` for a non-negative decimal scale. Negative scales are
/// normalized to zero on load.
pub(crate) fn pow10(scale: i8) -> u128 {
    debug_assert!(scale >= 0, "negative scales are normalized on load");
    10u128.pow(u32::from(scale.unsigned_abs()))
}

/// Unscaled values above this are too large to square or multiply exactly in an
/// i256 accumulator across 2^54 rows.
const HUGE: u128 = 1 << 100;

/// The largest unscaled value of a 38-digit decimal.
const MAX_DECIMAL_38: i128 = 99_999_999_999_999_999_999_999_999_999_999_999_999;

/// Splits an i256 into a sign and a magnitude.
fn i256_parts(value: i256) -> (bool, BigUint) {
    let negative = value < i256::ZERO;
    let magnitude = if negative {
        value.wrapping_neg()
    } else {
        value
    };
    let (low, high) = magnitude.to_parts();
    let high = high as u128;
    (
        negative,
        BigUint(vec![
            low as u64,
            (low >> 64) as u64,
            high as u64,
            (high >> 64) as u64,
        ]),
    )
}

fn i256_to_f64(value: i256, divisors: &[u128]) -> f64 {
    let (negative, magnitude) = i256_parts(value);
    big_ratio_to_f64(negative, magnitude, divisors)
}

/// Computes a measure's value from its state. `values` supplies the merged leaf
/// state for lazy measures above the leaves; `cmp` orders source-column keys.
pub(crate) fn finish(
    measure: &CompiledMeasure,
    state: &State,
    cmp: &dyn Fn(Key, Key) -> std::cmp::Ordering,
) -> MeasureValue {
    let scale = measure.scale();
    match state {
        State::Count(c) => MeasureValue::Count(*c),
        State::Float { sum, squares, n } => {
            if *n == 0 {
                return MeasureValue::Null;
            }
            match measure.aggregate {
                Aggregate::Sum => MeasureValue::Float(sum.value()),
                Aggregate::Mean => MeasureValue::Float(sum.mean(*n as u64)),
                Aggregate::Variance | Aggregate::StdDev => {
                    if *n < 2 {
                        return MeasureValue::Null;
                    }
                    if !sum.is_finite() {
                        return MeasureValue::Float(f64::NAN);
                    }
                    let Some(squares) = squares.as_deref() else {
                        unreachable!("variances keep their squares")
                    };
                    // A square beyond the double range cannot be held exactly.
                    if !squares.is_finite() {
                        return MeasureValue::Overflow;
                    }
                    let var = exact_float_variance(sum, squares, *n as u64);
                    if measure.aggregate == Aggregate::Variance {
                        MeasureValue::Float(var)
                    } else {
                        MeasureValue::Float(libm::sqrt(var))
                    }
                }
                _ => unreachable!("float state for {:?}", measure.aggregate),
            }
        }
        State::Exact {
            sum,
            squares,
            n,
            huge,
        } => {
            if *n == 0 {
                return MeasureValue::Null;
            }
            match measure.aggregate {
                Aggregate::Sum => match sum.to_i128() {
                    Some(v) if v.abs() <= MAX_DECIMAL_38 => MeasureValue::Decimal(v),
                    _ => MeasureValue::Overflow,
                },
                Aggregate::Mean => {
                    MeasureValue::Float(i256_to_f64(*sum, &[*n as u128, pow10(scale)]))
                }
                Aggregate::Variance | Aggregate::StdDev => {
                    if *n < 2 {
                        return MeasureValue::Null;
                    }
                    if *huge > 0 {
                        return MeasureValue::Overflow;
                    }
                    // (n * sum(x^2) - sum(x)^2) / (n (n - 1)), exactly, then rounded once.
                    let (_, s) = i256_parts(*sum);
                    let (_, q) = i256_parts(*squares);
                    let nq = q.mul(&BigUint::from_u128(*n as u128));
                    let var = match nq.checked_sub(&s.mul(&s)) {
                        Some(numerator) => big_ratio_to_f64(
                            false,
                            numerator,
                            &[*n as u128 * (*n as u128 - 1), pow10(scale), pow10(scale)],
                        ),
                        None => 0.0,
                    };
                    if measure.aggregate == Aggregate::Variance {
                        MeasureValue::Float(var)
                    } else {
                        MeasureValue::Float(libm::sqrt(var))
                    }
                }
                _ => unreachable!("exact state for {:?}", measure.aggregate),
            }
        }
        State::WeightedFloat {
            weighted,
            weights,
            n,
        } => {
            if *n == 0 {
                return MeasureValue::Null;
            }
            let w = weights.value();
            if w == 0.0 {
                return MeasureValue::Null;
            }
            MeasureValue::Float(weighted.value() / w)
        }
        State::WeightedExact {
            weighted,
            weights,
            n,
            huge,
        } => {
            if *huge > 0 {
                return MeasureValue::Overflow;
            }
            if *n == 0 || *weights == i256::ZERO {
                return MeasureValue::Null;
            }
            // value = (sum(x w) / 10^(sx + sw)) / (sum(w) / 10^sw) = sum(x w) / (sum(w) 10^sx)
            let (weights_negative, weight_magnitude) = i256_parts(*weights);
            let (weighted_negative, weighted_magnitude) = i256_parts(*weighted);
            let negative = weights_negative != weighted_negative;
            match i256_to_u128_or_zero(&weight_magnitude) {
                w if w > 0 => MeasureValue::Float(big_ratio_to_f64(
                    negative,
                    weighted_magnitude,
                    &[w, pow10(scale)],
                )),
                _ => MeasureValue::Float(
                    i256_to_f64(*weighted, &[pow10(scale)]) / i256_to_f64(*weights, &[1]),
                ),
            }
        }
        State::Values(map) => finish_values(measure, map, cmp),
        State::Sequence(map) => {
            let entry = match measure.aggregate {
                Aggregate::First => map.iter().next(),
                _ => map.iter().next_back(),
            };
            entry.map_or(MeasureValue::Null, |(_, &k)| MeasureValue::Key(KeyValue(k)))
        }
        State::Lazy => unreachable!("lazy states are finished from merged leaves"),
    }
}

/// The sample variance from exact sums of values and of squares, rounded once:
/// (n * sum(x^2) - sum(x)^2) / (n (n - 1)), computed with big integers. The sum is in
/// units of 2^-1074, so its square is in units of 2^-2148, and the sum of squares is
/// scaled to match.
fn exact_float_variance(sum: &ExactSum, squares: &ExactSum, n: u64) -> f64 {
    let (_, a) = sum.signed_magnitude();
    let (negative_squares, b) = squares.signed_magnitude();
    if negative_squares {
        return 0.0;
    }
    let nb = b.mul(&BigUint::from_u128(u128::from(n))).shl(1074);
    match nb.checked_sub(&a.mul(&a)) {
        Some(numerator) => {
            let (q, sticky) = numerator.div_rem_u128(u128::from(n) * u128::from(n - 1));
            q.to_f64(-2148, sticky, false)
        }
        // Underflow in the tiniest squares can leave the exact identity a hair short.
        None => 0.0,
    }
}

/// Returns the magnitude if it fits a u128, or zero.
fn i256_to_u128_or_zero(value: &BigUint) -> u128 {
    if value.bits() > 128 {
        0
    } else {
        u128::from(value.0[0]) | u128::from(value.0[1]) << 64
    }
}

fn key_to_f64(key: Key, scale: i8) -> f64 {
    match key {
        Key::Float(bits) => f64::from_bits(bits),
        Key::Int(i) => i as f64,
        Key::Decimal(d) => ratio_to_f64(d, &[pow10(scale)]),
        _ => f64::NAN,
    }
}

fn finish_values(
    measure: &CompiledMeasure,
    map: &FxHashMap<Key, u32>,
    cmp: &dyn Fn(Key, Key) -> std::cmp::Ordering,
) -> MeasureValue {
    if map.is_empty() {
        return if measure.aggregate == Aggregate::DistinctCount {
            MeasureValue::Count(0)
        } else {
            MeasureValue::Null
        };
    }
    match measure.aggregate {
        Aggregate::DistinctCount => MeasureValue::Count(map.len() as i64),
        Aggregate::Min => {
            let k = map
                .keys()
                .copied()
                .min_by(|&a, &b| cmp(a, b))
                .expect("non-empty");
            MeasureValue::Key(KeyValue(k))
        }
        Aggregate::Max => {
            let k = map
                .keys()
                .copied()
                .max_by(|&a, &b| cmp(a, b))
                .expect("non-empty");
            MeasureValue::Key(KeyValue(k))
        }
        Aggregate::Median | Aggregate::Percentile(_) => {
            let p = match measure.aggregate {
                Aggregate::Percentile(p) => p,
                _ => 0.5,
            };
            let mut keys: Vec<(Key, u32)> = map.iter().map(|(&k, &c)| (k, c)).collect();
            keys.sort_by(|a, b| cmp(a.0, b.0));
            let n: u64 = keys.iter().map(|(_, c)| u64::from(*c)).sum();
            let position = (n - 1) as f64 * p;
            let lo = position.floor();
            let at = |rank: u64| {
                let mut seen = 0u64;
                for &(k, c) in &keys {
                    seen += u64::from(c);
                    if rank < seen {
                        return k;
                    }
                }
                keys.last().expect("non-empty").0
            };
            let scale = measure.scale();
            let lo_value = key_to_f64(at(lo as u64), scale);
            let hi_value = key_to_f64(at(position.ceil() as u64), scale);
            let fraction = position - lo;
            // Interpolating between equal neighbours, infinities included, must
            // give that value rather than inf - inf = NaN.
            if fraction == 0.0 || lo_value == hi_value {
                MeasureValue::Float(lo_value)
            } else {
                MeasureValue::Float(lo_value + fraction * (hi_value - lo_value))
            }
        }
        _ => unreachable!("values state for {:?}", measure.aggregate),
    }
}
