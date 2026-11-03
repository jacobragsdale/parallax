//! Display text for values, following Excel's display rules.
//!
//! The screen and the Excel export show the same digits because both use this
//! formatter. Every number is first turned into an exact decimal:
//!
//! - A double is rounded to 15 significant digits, half away from zero, as Excel
//!   does. 2.675, whose double is 2.67499999999999982..., becomes 2.67500000000000.
//! - Decimals and integers are already exact and are not rounded at this step. A
//!   decimal of 15 significant digits or fewer survives Excel's conversion to a
//!   double unchanged, so the two agree; for longer ones the screen keeps every
//!   digit.
//!
//! Percent and K/M/B scaling then move the decimal point, which is exact, and the
//! result is rounded once to the shown decimals, half away from zero. A value that
//! rounds to zero never shows a minus sign.
//!
//! Instants are shown in a display zone, `America/New_York` unless the format names
//! another, using the time-zone database bundled into the build, so the server and
//! the browser worker agree. Sub-second digits beyond the shown precision are cut,
//! never rounded, so a time is never shown later than it happened.

use std::fmt::Write as _;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal128Type, Float64Type, Int64Type, TimestampMicrosecondType,
};
use arrow_array::{Array, ArrayRef};
use arrow_schema::{DataType, TimeUnit};
use jiff::tz::TimeZone;

use crate::types::{ColumnType, Scalar};

/// The display zone when a format names none.
pub const DEFAULT_ZONE: &str = "America/New_York";

/// The most decimals a format may show, Excel's limit.
pub const MAX_DECIMALS: u8 = 30;

/// Text for a null value.
pub const NULL_TEXT: &str = "";
/// Text for a result beyond 38 digits.
pub const OVERFLOW_TEXT: &str = "overflow";
/// Text for a group that mixes units.
pub const MIXED_TEXT: &str = "mixed";
/// Text for an undefined result, such as a percentage of zero.
pub const UNDEFINED_TEXT: &str = "—";
/// Text for a value outside the range a date can show.
pub const OUT_OF_RANGE_TEXT: &str = "out of range";

/// A fixed K, M or B scaling. The suffix is always shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abbreviation {
    /// Thousands, `K`.
    Thousands,
    /// Millions, `M`.
    Millions,
    /// Billions, `B`.
    Billions,
}

impl Abbreviation {
    const fn shift(self) -> i32 {
        match self {
            Self::Thousands => 3,
            Self::Millions => 6,
            Self::Billions => 9,
        }
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::Thousands => "K",
            Self::Millions => "M",
            Self::Billions => "B",
        }
    }
}

/// How negative numbers are written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Negative {
    /// `-1,234.50`
    #[default]
    Minus,
    /// `(1,234.50)`
    Parentheses,
}

/// The sub-second digits an instant shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimePrecision {
    /// Whole seconds.
    #[default]
    Seconds,
    /// Milliseconds.
    Milliseconds,
    /// Microseconds, every digit stored.
    Microseconds,
}

impl TimePrecision {
    const fn digits(self) -> usize {
        match self {
            Self::Seconds => 0,
            Self::Milliseconds => 3,
            Self::Microseconds => 6,
        }
    }
}

/// How a column's values are displayed. Unset fields take the column type's
/// default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Format {
    /// Digits after the decimal point. The default is the decimal's scale, 0 for
    /// integers, 1 when abbreviating, and 2 otherwise.
    pub decimals: Option<u8>,
    /// Group thousands with commas; the default is yes.
    pub thousands: Option<bool>,
    /// Multiply by 100 and append `%`.
    pub percent: bool,
    /// Divide by a thousand, million or billion and append the suffix.
    pub abbreviate: Option<Abbreviation>,
    /// How negative numbers are written.
    pub negative: Negative,
    /// The zone instants are shown in.
    pub zone: Option<String>,
    /// The sub-second digits instants show.
    pub precision: TimePrecision,
}

/// A format that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// The time zone is not in the bundled database.
    #[error("unknown time zone {0}")]
    UnknownZone(String),
    /// More decimals than Excel can show.
    #[error("{0} decimals is more than the {MAX_DECIMALS} Excel can show")]
    TooManyDecimals(u8),
    /// Percent and K/M/B scaling together.
    #[error("a format cannot both show a percentage and abbreviate")]
    PercentAndAbbreviation,
}

/// A checked [`Format`], ready to format values.
#[derive(Clone, Debug)]
pub struct Formatter {
    decimals: Option<u8>,
    thousands: bool,
    percent: bool,
    abbreviate: Option<Abbreviation>,
    negative: Negative,
    zone: TimeZone,
    zone_name: String,
    precision: TimePrecision,
}

impl Default for Formatter {
    fn default() -> Self {
        Self::new(&Format::default()).expect("the default format is valid")
    }
}

impl Formatter {
    /// Checks `format`.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown zone, more than [`MAX_DECIMALS`] decimals,
    /// or percent combined with an abbreviation.
    pub fn new(format: &Format) -> Result<Self, FormatError> {
        if let Some(d) = format.decimals
            && d > MAX_DECIMALS
        {
            return Err(FormatError::TooManyDecimals(d));
        }
        if format.percent && format.abbreviate.is_some() {
            return Err(FormatError::PercentAndAbbreviation);
        }
        let zone_name = format.zone.as_deref().unwrap_or(DEFAULT_ZONE);
        let zone =
            TimeZone::get(zone_name).map_err(|_| FormatError::UnknownZone(zone_name.to_owned()))?;
        Ok(Self {
            decimals: format.decimals,
            thousands: format.thousands.unwrap_or(true),
            percent: format.percent,
            abbreviate: format.abbreviate,
            negative: format.negative,
            zone,
            zone_name: zone_name.to_owned(),
            precision: format.precision,
        })
    }

    /// The zone instants are shown in.
    #[must_use]
    pub fn zone_name(&self) -> &str {
        &self.zone_name
    }

    fn decimals_or(&self, natural: u8) -> u32 {
        u32::from(self.decimals.unwrap_or(if self.percent {
            2
        } else if self.abbreviate.is_some() {
            1
        } else {
            natural
        }))
    }

    /// Formats a double: 15 significant digits, then the shown decimals.
    #[must_use]
    pub fn float(&self, value: f64) -> String {
        if value.is_nan() {
            return UNDEFINED_TEXT.to_owned();
        }
        let negative = value.is_sign_negative();
        if value.is_infinite() {
            return self.signed(negative, "∞".to_owned());
        }
        let (digits, exp) = if value == 0.0 {
            ("0".to_owned(), 0)
        } else {
            fifteen_digits(value.abs())
        };
        self.number(negative, &digits, exp, self.decimals_or(2))
    }

    /// Formats an exact decimal, `unscaled × 10^-scale`.
    #[must_use]
    pub fn decimal(&self, unscaled: i128, scale: i8) -> String {
        let natural = u8::try_from(scale.max(0)).unwrap_or(0).min(MAX_DECIMALS);
        self.number(
            unscaled < 0,
            &unscaled.unsigned_abs().to_string(),
            -i32::from(scale),
            self.decimals_or(natural),
        )
    }

    /// Formats an integer.
    #[must_use]
    pub fn integer(&self, value: i64) -> String {
        self.number(
            value < 0,
            &value.unsigned_abs().to_string(),
            0,
            self.decimals_or(0),
        )
    }

    /// Formats a date, as `YYYY-MM-DD`. Dates never pass through a zone.
    #[must_use]
    pub fn date(&self, days: i32) -> String {
        date_text(days)
    }

    /// Formats an instant (`zoned`) in the display zone, or a wall-clock time as
    /// stored, to the format's precision.
    #[must_use]
    pub fn timestamp(&self, micros: i64, zoned: bool) -> String {
        self.timestamp_to(micros, zoned, self.precision)
    }

    /// Formats a timestamp with every stored digit, for copying.
    #[must_use]
    pub fn timestamp_full(&self, micros: i64, zoned: bool) -> String {
        self.timestamp_to(micros, zoned, TimePrecision::Microseconds)
    }

    fn timestamp_to(&self, micros: i64, zoned: bool, precision: TimePrecision) -> String {
        if zoned {
            civil_text(micros, &self.zone, precision)
        } else {
            civil_text(micros, &TimeZone::UTC, precision)
        }
    }

    /// Formats a boolean as Excel does.
    #[must_use]
    pub const fn boolean(value: bool) -> &'static str {
        if value { "TRUE" } else { "FALSE" }
    }

    /// Formats one value of a column of type `ty`.
    #[must_use]
    pub fn scalar(&self, ty: &ColumnType, value: &Scalar) -> String {
        match value {
            Scalar::Null => NULL_TEXT.to_owned(),
            Scalar::Boolean(b) => Self::boolean(*b).to_owned(),
            Scalar::Int64(i) => self.integer(*i),
            Scalar::Float64(f) => self.float(*f),
            Scalar::Decimal(d) => {
                let scale = match ty {
                    ColumnType::Decimal { scale, .. } => *scale,
                    _ => 0,
                };
                self.decimal(*d, scale)
            }
            Scalar::Utf8(s) => s.clone(),
            Scalar::Date32(d) => self.date(*d),
            Scalar::Timestamp(t) => self.timestamp(
                *t,
                matches!(ty, ColumnType::Timestamp { timezone: Some(_) }),
            ),
        }
    }

    /// Formats every value of an array, nulls as empty text.
    ///
    /// # Panics
    ///
    /// Panics for Arrow types the engine never produces.
    #[must_use]
    pub fn texts(&self, array: &ArrayRef) -> Vec<String> {
        match array.data_type() {
            DataType::Boolean => {
                let a = array.as_boolean();
                each(array, |i| Self::boolean(a.value(i)).to_owned())
            }
            DataType::Int64 => {
                let a = array.as_primitive::<Int64Type>();
                each(array, |i| self.integer(a.value(i)))
            }
            DataType::Float64 => {
                let a = array.as_primitive::<Float64Type>();
                each(array, |i| self.float(a.value(i)))
            }
            DataType::Decimal128(_, scale) => {
                let a = array.as_primitive::<Decimal128Type>();
                each(array, |i| self.decimal(a.value(i), *scale))
            }
            DataType::Utf8 => {
                let a = array.as_string::<i32>();
                each(array, |i| a.value(i).to_owned())
            }
            DataType::Date32 => {
                let a = array.as_primitive::<Date32Type>();
                each(array, |i| self.date(a.value(i)))
            }
            DataType::Timestamp(TimeUnit::Microsecond, zone) => {
                let a = array.as_primitive::<TimestampMicrosecondType>();
                let zoned = zone.is_some();
                each(array, |i| self.timestamp(a.value(i), zoned))
            }
            other => panic!("the engine does not produce {other} columns"),
        }
    }

    /// Formats every timestamp of an array with every stored digit.
    ///
    /// # Panics
    ///
    /// Panics unless `array` holds microsecond timestamps.
    #[must_use]
    pub fn timestamps_full(&self, array: &ArrayRef) -> Vec<String> {
        let DataType::Timestamp(TimeUnit::Microsecond, zone) = array.data_type() else {
            panic!("not a microsecond timestamp array");
        };
        let zoned = zone.is_some();
        let values = array.as_primitive::<TimestampMicrosecondType>();
        each(array, |i| self.timestamp_full(values.value(i), zoned))
    }

    fn signed(&self, negative: bool, body: String) -> String {
        match (negative, self.negative) {
            (false, _) => body,
            (true, Negative::Minus) => format!("-{body}"),
            (true, Negative::Parentheses) => format!("({body})"),
        }
    }

    /// Formats `digits × 10^exp`, rounded to `decimals` places.
    fn number(&self, negative: bool, digits: &str, exp: i32, decimals: u32) -> String {
        let mut exp = exp;
        if self.percent {
            exp += 2;
        }
        if let Some(a) = self.abbreviate {
            exp -= a.shift();
        }
        let rounded = round_half_away(digits, exp, decimals);
        let is_zero = rounded.bytes().all(|b| b == b'0');
        let decimals = decimals as usize;
        let padded = format!("{rounded:0>width$}", width = decimals + 1);
        let (whole, fraction) = padded.split_at(padded.len() - decimals);
        let mut body = if self.thousands {
            group_thousands(whole)
        } else {
            whole.to_owned()
        };
        if decimals > 0 {
            body.push('.');
            body.push_str(fraction);
        }
        if let Some(a) = self.abbreviate {
            body.push_str(a.suffix());
        }
        if self.percent {
            body.push('%');
        }
        self.signed(negative && !is_zero, body)
    }
}

/// Applies `text` to every non-null position of `array`; nulls become empty text.
fn each(array: &ArrayRef, text: impl Fn(usize) -> String) -> Vec<String> {
    (0..array.len())
        .map(|i| {
            if array.is_null(i) {
                NULL_TEXT.to_owned()
            } else {
                text(i)
            }
        })
        .collect()
}

/// The digits and exponent of a positive finite double rounded to 15 significant
/// digits, half away from zero: the value is `digits × 10^exp`.
fn fifteen_digits(value: f64) -> (String, i32) {
    // Seventeen correctly rounded digits decide the rounding unless the two digits
    // past the fifteenth read exactly 50, which may hide a value just below or above
    // the midpoint. Then the exact expansion, at most 767 significant digits for any
    // double, decides.
    let (digits, exp) = scientific(&format!("{value:.16e}"));
    let up = match &digits[15..17] {
        "50" => {
            let (exact, _) = scientific(&format!("{value:.800e}"));
            exact.as_bytes()[15] >= b'5'
        }
        tail => tail.as_bytes()[0] >= b'5',
    };
    let kept = &digits[..15];
    let kept = if up { increment(kept) } else { kept.to_owned() };
    (kept, exp - 14)
}

/// Splits Rust's `d.ddde±x` text into its digits and decimal exponent.
fn scientific(text: &str) -> (String, i32) {
    let (mantissa, exp) = text.split_once('e').expect("scientific notation");
    (
        mantissa.replace('.', ""),
        exp.parse().expect("an integer exponent"),
    )
}

/// Adds one to a decimal digit string.
fn increment(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    for b in bytes.iter_mut().rev() {
        if *b == b'9' {
            *b = b'0';
        } else {
            *b += 1;
            return String::from_utf8(bytes).expect("ASCII digits");
        }
    }
    let mut out = String::with_capacity(bytes.len() + 1);
    out.push('1');
    out.push_str(std::str::from_utf8(&bytes).expect("ASCII digits"));
    out
}

/// Rounds `digits × 10^exp` to `decimals` places, half away from zero, and returns
/// the result scaled by `10^decimals` as a digit string without leading zeros.
fn round_half_away(digits: &str, exp: i32, decimals: u32) -> String {
    let target = -(decimals as i64);
    let exp = i64::from(exp);
    let out = if exp >= target {
        let zeros = usize::try_from(exp - target).expect("non-negative");
        let mut s = String::with_capacity(digits.len() + zeros);
        s.push_str(digits);
        s.extend(std::iter::repeat_n('0', zeros));
        s
    } else {
        let shift = usize::try_from(target - exp).expect("positive");
        if shift > digits.len() {
            "0".to_owned()
        } else {
            let (kept, dropped) = digits.split_at(digits.len() - shift);
            let kept = if kept.is_empty() { "0" } else { kept };
            if dropped.as_bytes()[0] >= b'5' {
                increment(kept)
            } else {
                kept.to_owned()
            }
        }
    };
    let trimmed = out.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn group_thousands(whole: &str) -> String {
    let mut out = String::with_capacity(whole.len() + whole.len() / 3);
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn date_text(days: i32) -> String {
    let date = jiff::Span::new()
        .try_days(days)
        .ok()
        .and_then(|span| jiff::civil::date(1970, 1, 1).checked_add(span).ok());
    match date {
        Some(d) => d.strftime("%Y-%m-%d").to_string(),
        None => OUT_OF_RANGE_TEXT.to_owned(),
    }
}

/// The wall-clock time of an instant in `zone`, with sub-second digits cut to
/// `precision`.
fn civil_text(micros: i64, zone: &TimeZone, precision: TimePrecision) -> String {
    let Ok(instant) = jiff::Timestamp::from_microsecond(micros) else {
        return OUT_OF_RANGE_TEXT.to_owned();
    };
    let civil = zone.to_datetime(instant);
    let mut text = civil.strftime("%Y-%m-%d %H:%M:%S").to_string();
    let digits = precision.digits();
    if digits > 0 {
        let fraction = format!("{:09}", civil.subsec_nanosecond());
        text.push('.');
        text.push_str(&fraction[..digits]);
    }
    text
}

/// Canonical text for a value, used for column identities such as column-pivot
/// names. Unlike display text it never merges distinct non-null values of one
/// column: floats keep every digit, decimals their scale, and timestamps their
/// microseconds, in UTC with a `Z` for instants. Dates and times beyond what a
/// calendar can show fall back to their stored integer.
#[must_use]
pub fn canonical(ty: &ColumnType, value: &Scalar) -> String {
    let mut out = String::new();
    match value {
        Scalar::Null => out.push_str("null"),
        Scalar::Boolean(b) => out.push_str(Formatter::boolean(*b)),
        Scalar::Int64(i) => write!(out, "{i}").expect("writes to a string"),
        Scalar::Float64(f) => write!(out, "{f:?}").expect("writes to a string"),
        Scalar::Decimal(d) => {
            let scale = match ty {
                ColumnType::Decimal { scale, .. } => i32::from(*scale),
                _ => 0,
            };
            out.push_str(&exact_decimal(*d, scale));
        }
        Scalar::Utf8(s) => out.push_str(s),
        Scalar::Date32(d) => match date_text(*d) {
            t if t == OUT_OF_RANGE_TEXT => write!(out, "days:{d}").expect("writes to a string"),
            t => out.push_str(&t),
        },
        Scalar::Timestamp(t) => {
            match civil_text(*t, &TimeZone::UTC, TimePrecision::Microseconds) {
                text if text == OUT_OF_RANGE_TEXT => {
                    write!(out, "micros:{t}").expect("writes to a string");
                }
                text => out.push_str(&text),
            }
            if matches!(ty, ColumnType::Timestamp { timezone: Some(_) }) {
                out.push('Z');
            }
        }
    }
    out
}

/// An exact decimal, `unscaled × 10^-scale`, with no grouping, such as
/// `-1234.50`. A negative scale appends zeros.
#[must_use]
pub fn exact_decimal(unscaled: i128, scale: i32) -> String {
    let negative = unscaled < 0;
    let mut digits = unscaled.unsigned_abs().to_string();
    if scale < 0 && unscaled != 0 {
        digits.extend(std::iter::repeat_n('0', scale.unsigned_abs() as usize));
    }
    let scale = usize::try_from(scale.max(0)).expect("non-negative");
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (whole, fraction) = padded.split_at(padded.len() - scale);
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(whole);
    if scale > 0 {
        out.push('.');
        out.push_str(fraction);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(format: Format) -> Formatter {
        Formatter::new(&format).expect("valid format")
    }

    fn decimals(d: u8) -> Formatter {
        with(Format {
            decimals: Some(d),
            ..Format::default()
        })
    }

    #[test]
    fn doubles_round_at_fifteen_digits_then_half_away_from_zero() {
        let f = decimals(2);
        assert_eq!(f.float(2.675), "2.68");
        assert_eq!(f.float(-2.675), "-2.68");
        assert_eq!(f.float(1.005), "1.01");
        assert_eq!(f.float(0.125), "0.13");
        assert_eq!(f.float(0.115), "0.12");
        assert_eq!(decimals(0).float(0.5), "1");
        assert_eq!(decimals(0).float(1.5), "2");
        assert_eq!(decimals(0).float(2.5), "3");
        assert_eq!(decimals(0).float(-0.5), "-1");
    }

    #[test]
    fn digits_past_the_fifteenth_show_as_zeros() {
        let f = decimals(0);
        assert_eq!(
            f.float(12_345_678_901_234_567_890.0),
            "12,345,678,901,234,600,000"
        );
        assert_eq!(f.float(1e15), "1,000,000,000,000,000");
        // 1000000000000005 is a double exactly; the tie rounds away from zero.
        assert_eq!(f.float(1_000_000_000_000_005.0), "1,000,000,000,000,010");
        assert_eq!(decimals(17).float(0.1), "0.10000000000000000");
    }

    #[test]
    fn values_that_round_to_zero_have_no_sign() {
        assert_eq!(decimals(2).float(-0.001), "0.00");
        assert_eq!(decimals(2).float(-0.0), "0.00");
        assert_eq!(decimals(2).decimal(-4, 3), "0.00");
        assert_eq!(decimals(2).decimal(-5, 3), "-0.01");
        assert_eq!(
            with(Format {
                negative: Negative::Parentheses,
                ..Format::default()
            })
            .float(-0.004),
            "0.00"
        );
    }

    #[test]
    fn decimals_are_exact_at_any_length() {
        let f = Formatter::default();
        assert_eq!(f.decimal(12_345, 2), "123.45");
        assert_eq!(f.decimal(-5, 2), "-0.05");
        assert_eq!(
            f.decimal(12_345_678_901_234_567_890_123_456_789_012_345_678, 2),
            "123,456,789,012,345,678,901,234,567,890,123,456.78"
        );
        assert_eq!(
            f.decimal(i128::MIN, 0),
            "-170,141,183,460,469,231,731,687,303,715,884,105,728"
        );
        assert_eq!(decimals(0).decimal(25, 1), "3");
        assert_eq!(decimals(0).decimal(-25, 1), "-3");
        assert_eq!(decimals(4).decimal(25, 1), "2.5000");
    }

    #[test]
    fn integers_group_and_keep_every_digit() {
        let f = Formatter::default();
        assert_eq!(f.integer(0), "0");
        assert_eq!(f.integer(999), "999");
        assert_eq!(f.integer(1_000), "1,000");
        assert_eq!(f.integer(-1_234_567), "-1,234,567");
        assert_eq!(f.integer(i64::MIN), "-9,223,372,036,854,775,808");
        let plain = with(Format {
            thousands: Some(false),
            ..Format::default()
        });
        assert_eq!(plain.integer(9_007_199_254_740_993), "9007199254740993");
    }

    #[test]
    fn percent_and_abbreviations_move_the_point_exactly() {
        let percent = with(Format {
            percent: true,
            ..Format::default()
        });
        assert_eq!(percent.float(0.125), "12.50%");
        assert_eq!(percent.float(0.000_05), "0.01%");
        assert_eq!(percent.decimal(1, 4), "0.01%");
        let millions = with(Format {
            abbreviate: Some(Abbreviation::Millions),
            ..Format::default()
        });
        assert_eq!(millions.float(1_250_000.0), "1.3M");
        assert_eq!(millions.float(-1_249_999.0), "-1.2M");
        assert_eq!(millions.integer(999), "0.0M");
        let billions = with(Format {
            abbreviate: Some(Abbreviation::Billions),
            decimals: Some(2),
            ..Format::default()
        });
        assert_eq!(billions.decimal(1_234_567_890_123, 2), "12.35B");
    }

    #[test]
    fn parentheses_replace_the_minus_sign() {
        let f = with(Format {
            negative: Negative::Parentheses,
            ..Format::default()
        });
        assert_eq!(f.float(-1234.5), "(1,234.50)");
        assert_eq!(f.float(1234.5), "1,234.50");
        assert_eq!(f.float(f64::NEG_INFINITY), "(∞)");
    }

    #[test]
    fn infinities_and_nan() {
        let f = Formatter::default();
        assert_eq!(f.float(f64::INFINITY), "∞");
        assert_eq!(f.float(f64::NEG_INFINITY), "-∞");
        assert_eq!(f.float(f64::NAN), UNDEFINED_TEXT);
    }

    #[test]
    fn instants_show_in_new_york_by_default() {
        let f = Formatter::default();
        // 2026-03-08 06:59:59 UTC is 01:59:59 EST; one second later is 03:00:00 EDT.
        let before = 1_772_953_199_000_000;
        assert_eq!(f.timestamp(before, true), "2026-03-08 01:59:59");
        assert_eq!(f.timestamp(before + 1_000_000, true), "2026-03-08 03:00:00");
        assert_eq!(f.timestamp(before, false), "2026-03-08 06:59:59");
        let tokyo = with(Format {
            zone: Some("Asia/Tokyo".into()),
            ..Format::default()
        });
        assert_eq!(tokyo.timestamp(before, true), "2026-03-08 15:59:59");
    }

    #[test]
    fn sub_second_digits_are_cut_not_rounded() {
        let millis = with(Format {
            precision: TimePrecision::Milliseconds,
            ..Format::default()
        });
        // 23:59:59.999999 must not become the next day.
        let late = 1_767_243_599_999_999; // 2026-01-01 04:59:59.999999 UTC
        assert_eq!(
            Formatter::default().timestamp(late, true),
            "2025-12-31 23:59:59"
        );
        assert_eq!(millis.timestamp(late, true), "2025-12-31 23:59:59.999");
        assert_eq!(
            millis.timestamp_full(late, true),
            "2025-12-31 23:59:59.999999"
        );
        // Before 1970 the fraction still counts forward within the second.
        assert_eq!(millis.timestamp(-1, false), "1969-12-31 23:59:59.999");
    }

    #[test]
    fn dates_never_pass_through_a_zone() {
        let f = with(Format {
            zone: Some("Pacific/Kiritimati".into()),
            ..Format::default()
        });
        assert_eq!(f.date(0), "1970-01-01");
        assert_eq!(f.date(-1), "1969-12-31");
        assert_eq!(f.date(20_454), "2026-01-01");
        assert_eq!(f.date(i32::MAX), OUT_OF_RANGE_TEXT);
    }

    #[test]
    fn invalid_formats_are_refused() {
        assert_eq!(
            Formatter::new(&Format {
                zone: Some("Mars/Olympus".into()),
                ..Format::default()
            })
            .unwrap_err(),
            FormatError::UnknownZone("Mars/Olympus".into())
        );
        assert!(
            Formatter::new(&Format {
                decimals: Some(31),
                ..Format::default()
            })
            .is_err()
        );
        assert!(
            Formatter::new(&Format {
                percent: true,
                abbreviate: Some(Abbreviation::Thousands),
                ..Format::default()
            })
            .is_err()
        );
    }

    #[test]
    fn canonical_text_keeps_distinct_values_apart() {
        let float = ColumnType::Float64;
        assert_ne!(
            canonical(&float, &Scalar::Float64(1.001)),
            canonical(&float, &Scalar::Float64(1.004))
        );
        let dec = ColumnType::Decimal {
            precision: 10,
            scale: 2,
        };
        assert_eq!(canonical(&dec, &Scalar::Decimal(-5)), "-0.05");
        let hundreds = ColumnType::Decimal {
            precision: 10,
            scale: -2,
        };
        assert_eq!(canonical(&hundreds, &Scalar::Decimal(5)), "500");
        // Values beyond the calendar keep their stored integers, so they stay apart.
        assert_ne!(
            canonical(&ColumnType::Date32, &Scalar::Date32(i32::MAX)),
            canonical(&ColumnType::Date32, &Scalar::Date32(i32::MAX - 1))
        );
        let instant = ColumnType::Timestamp {
            timezone: Some("UTC".into()),
        };
        assert_eq!(
            canonical(&instant, &Scalar::Timestamp(i64::MAX)),
            format!("micros:{}Z", i64::MAX)
        );
        assert_eq!(
            canonical(&ColumnType::Date32, &Scalar::Date32(0)),
            "1970-01-01"
        );
    }
}
