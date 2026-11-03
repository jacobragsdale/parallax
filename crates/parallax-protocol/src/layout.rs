//! Layout documents: the JSON description of a view.
//!
//! A layout is the contract between the UI, saved views, shared URLs, exports and
//! tests. It is versioned: [`Layout::from_json`] migrates older versions forward,
//! and [`schema`] returns the JSON Schema generated from these types.
//!
//! Values in filters and paths are plain JSON. They are interpreted against the
//! table's column types when the view opens, and never rounded: a decimal filter
//! value with more digits than the column's scale is an error.

use std::collections::BTreeMap;

use parallax_engine::aggregate::{Aggregate, Measure, ShowAs};
use parallax_engine::chart::{ChartConfig, ChartKind, OhlcColumns, XAxis};
use parallax_engine::filter::{Filter, FilterOp};
use parallax_engine::format::{Abbreviation, Format, Negative, TimePrecision, exact_decimal};
use parallax_engine::view::{Comparison, GroupOrder, GroupSort, Having, Sort, TopN, ViewConfig};
use parallax_engine::{ColumnType, Scalar, Table};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The current layout version.
pub const LAYOUT_VERSION: u32 = 1;

/// A view, as saved and sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    /// The layout version; [`LAYOUT_VERSION`] for documents this crate writes.
    pub version: u32,
    /// The dataset the view reads. Empty in layouts sent with an open-view
    /// request, which names the dataset itself; when both are given they must
    /// agree.
    pub dataset: String,
    /// The dataset's parameters, such as an as-of date.
    pub parameters: BTreeMap<String, String>,
    /// Columns of a flat view, in order. Empty means every column.
    pub columns: Vec<String>,
    /// Row pivots, outermost first.
    pub group_by: Vec<String>,
    /// Column pivots.
    pub split_by: Vec<String>,
    /// Measures of a grouped view.
    pub measures: Vec<LayoutMeasure>,
    /// Row filters.
    pub filters: Vec<LayoutFilter>,
    /// Sorts of a flat view.
    pub sort: Vec<LayoutSort>,
    /// Ordering of groups among their siblings.
    pub group_sort: LayoutGroupSort,
    /// Aggregate filters.
    pub having: Vec<LayoutHaving>,
    /// Top-N per set of siblings.
    pub top_n: Option<LayoutTopN>,
    /// Levels expanded by default.
    pub expand_depth: usize,
    /// Groups expanded or collapsed by the user, by value.
    pub expanded: Vec<LayoutExpanded>,
    /// Display formats by source column. Measures take their column's format
    /// unless they set their own.
    pub formats: BTreeMap<String, LayoutFormat>,
    /// Column widths in CSS pixels, chosen by the user. Keys are source column
    /// names for flat views, `"__measure:0"`, `"__measure:1"` and so on for the
    /// data columns of grouped views, and `"__tree"` for the group column. The
    /// engine ignores them.
    pub column_widths: BTreeMap<String, u32>,
    /// Draws the view as a chart instead of a grid.
    pub chart: Option<LayoutChart>,
}

impl Default for Layout {
    /// An empty layout at the current version: a flat view of every column.
    fn default() -> Self {
        Self {
            version: LAYOUT_VERSION,
            dataset: String::new(),
            parameters: BTreeMap::new(),
            columns: Vec::new(),
            group_by: Vec::new(),
            split_by: Vec::new(),
            measures: Vec::new(),
            filters: Vec::new(),
            sort: Vec::new(),
            group_sort: LayoutGroupSort::default(),
            having: Vec::new(),
            top_n: None,
            expand_depth: 0,
            expanded: Vec::new(),
            formats: BTreeMap::new(),
            column_widths: BTreeMap::new(),
            chart: None,
        }
    }
}

/// What a chart draws, in a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChartKindName {
    /// Lines through each series.
    Line,
    /// Lines filled down to zero.
    Area,
    /// Bars of the layout's grouped view: one per group, one per measure or
    /// column pivot within it.
    Bar,
    /// Points, drawn as a density.
    Scatter,
    /// Cells of the layout's grouped view, coloured by its first measure: groups
    /// across, column pivots up.
    Heatmap,
    /// Counts of the x values in bins.
    Histogram,
    /// Candles of each bucket's open, high, low and close.
    Candlestick,
    /// Bars of each bucket's open, high, low and close.
    Ohlc,
}

impl ChartKindName {
    /// The engine's reduction for this kind, or `None` for kinds drawn from the
    /// layout's grouped view.
    #[must_use]
    pub fn reduced(self) -> Option<ChartKind> {
        Some(match self {
            Self::Line => ChartKind::Line,
            Self::Area => ChartKind::Area,
            Self::Scatter => ChartKind::Scatter,
            Self::Histogram => ChartKind::Histogram,
            Self::Candlestick => ChartKind::Candlestick,
            Self::Ohlc => ChartKind::Ohlc,
            Self::Bar | Self::Heatmap => return None,
        })
    }
}

/// How a chart places x values, in a layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum XAxisName {
    /// By value.
    #[default]
    Value,
    /// By trading time on the NYSE calendar.
    Trading,
}

/// The price columns of a candlestick or OHLC chart, in a layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutOhlc {
    /// The opening price.
    pub open: String,
    /// The highest price.
    pub high: String,
    /// The lowest price.
    pub low: String,
    /// The closing price.
    pub close: String,
}

/// An x range a chart was zoomed to, in axis positions.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutRange {
    /// The left edge.
    pub from: f64,
    /// The right edge.
    pub to: f64,
}

/// A chart, in a layout. Bar charts and heatmaps draw the layout's grouped view;
/// the other kinds are reduced by the engine to the plot's pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutChart {
    /// What it draws.
    pub kind: ChartKindName,
    /// The x column of reduced kinds; a histogram bins it.
    #[serde(default)]
    pub x: String,
    /// Columns drawn against the left y axis.
    #[serde(default)]
    pub y: Vec<String>,
    /// Columns drawn against a second y axis on the right.
    #[serde(default)]
    pub y_right: Vec<String>,
    /// A column whose values split each plotted column into series.
    #[serde(default)]
    pub series: Option<String>,
    /// The price columns of candlestick and OHLC charts.
    #[serde(default)]
    pub ohlc: Option<LayoutOhlc>,
    /// How x values are placed.
    #[serde(default)]
    pub axis: XAxisName,
    /// A logarithmic y axis.
    #[serde(default)]
    pub log_y: bool,
    /// Bars of one group stacked instead of side by side.
    #[serde(default)]
    pub stacked: bool,
    /// Bars running across instead of up.
    #[serde(default)]
    pub horizontal: bool,
    /// The x range zoomed to; absent shows everything.
    #[serde(default)]
    pub range: Option<LayoutRange>,
    /// Series hidden from the plot, by their legend labels.
    #[serde(default)]
    pub hidden: Vec<String>,
}

impl LayoutChart {
    /// The engine's chart for this layout chart, with the view's filters, or
    /// `None` for kinds drawn from the grouped view.
    #[must_use]
    pub fn to_engine(&self, filters: &[Filter]) -> Option<ChartConfig> {
        Some(ChartConfig {
            kind: self.kind.reduced()?,
            x: self.x.clone(),
            y: self.y.iter().chain(&self.y_right).cloned().collect(),
            series: self.series.clone(),
            ohlc: self.ohlc.as_ref().map(|o| OhlcColumns {
                open: o.open.clone(),
                high: o.high.clone(),
                low: o.low.clone(),
                close: o.close.clone(),
            }),
            filters: filters.to_vec(),
            axis: match self.axis {
                XAxisName::Value => XAxis::Value,
                XAxisName::Trading => XAxis::Trading,
            },
            indicators: Vec::new(),
            volume: None,
        })
    }
}

/// How a column's values are displayed. Unset fields take the type's default:
/// a decimal's scale, no decimals for integers, one when abbreviating, and two
/// otherwise; thousands grouped; instants in `America/New_York` to the second.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutFormat {
    /// Digits after the decimal point, at most 30.
    #[schemars(range(max = 30))]
    pub decimals: Option<u8>,
    /// Group thousands with commas.
    pub thousands: Option<bool>,
    /// Multiply by 100 and append `%`.
    pub percent: bool,
    /// Divide by a thousand, million or billion and append `K`, `M` or `B`.
    pub abbreviate: Option<AbbreviationName>,
    /// How negative numbers are written.
    pub negative: NegativeName,
    /// The IANA zone instants are shown in.
    pub zone: Option<String>,
    /// The sub-second digits instants show.
    pub precision: PrecisionName,
}

/// A K, M or B scaling in a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AbbreviationName {
    /// Thousands, `K`.
    Thousands,
    /// Millions, `M`.
    Millions,
    /// Billions, `B`.
    Billions,
}

/// How negative numbers are written, in a layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NegativeName {
    /// `-1,234.50`
    #[default]
    Minus,
    /// `(1,234.50)`
    Parentheses,
}

/// Sub-second digits in a layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrecisionName {
    /// Whole seconds.
    #[default]
    Seconds,
    /// Milliseconds.
    Milliseconds,
    /// Microseconds.
    Microseconds,
}

impl LayoutFormat {
    /// The engine's format.
    #[must_use]
    pub fn to_engine(&self) -> Format {
        Format {
            decimals: self.decimals,
            thousands: self.thousands,
            percent: self.percent,
            abbreviate: self.abbreviate.map(|a| match a {
                AbbreviationName::Thousands => Abbreviation::Thousands,
                AbbreviationName::Millions => Abbreviation::Millions,
                AbbreviationName::Billions => Abbreviation::Billions,
            }),
            negative: match self.negative {
                NegativeName::Minus => Negative::Minus,
                NegativeName::Parentheses => Negative::Parentheses,
            },
            zone: self.zone.clone(),
            precision: match self.precision {
                PrecisionName::Seconds => TimePrecision::Seconds,
                PrecisionName::Milliseconds => TimePrecision::Milliseconds,
                PrecisionName::Microseconds => TimePrecision::Microseconds,
            },
        }
    }

    /// The layout form of an engine format.
    #[must_use]
    pub fn from_engine(format: &Format) -> Self {
        Self {
            decimals: format.decimals,
            thousands: format.thousands,
            percent: format.percent,
            abbreviate: format.abbreviate.map(|a| match a {
                Abbreviation::Thousands => AbbreviationName::Thousands,
                Abbreviation::Millions => AbbreviationName::Millions,
                Abbreviation::Billions => AbbreviationName::Billions,
            }),
            negative: match format.negative {
                Negative::Minus => NegativeName::Minus,
                Negative::Parentheses => NegativeName::Parentheses,
            },
            zone: format.zone.clone(),
            precision: match format.precision {
                TimePrecision::Seconds => PrecisionName::Seconds,
                TimePrecision::Milliseconds => PrecisionName::Milliseconds,
                TimePrecision::Microseconds => PrecisionName::Microseconds,
            },
        }
    }
}

/// An aggregate's name in a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AggregateName {
    /// Rows with a value.
    Count,
    /// All rows.
    CountAll,
    /// Exact sum.
    Sum,
    /// Mean.
    Mean,
    /// Weighted mean; needs `weight`.
    WeightedMean,
    /// Minimum.
    Min,
    /// Maximum.
    Max,
    /// Value of the earliest inserted row.
    First,
    /// Value of the latest inserted row.
    Last,
    /// Distinct values.
    DistinctCount,
    /// Sample variance.
    Variance,
    /// Sample standard deviation.
    StdDev,
    /// Median.
    Median,
    /// Percentile; needs `percentile`.
    Percentile,
}

/// How a measure is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShowAsName {
    /// The value.
    #[default]
    Value,
    /// Fraction of the grand total.
    PercentOfGrandTotal,
    /// Fraction of the column's total.
    PercentOfColumnTotal,
    /// Fraction of the row's total.
    PercentOfRowTotal,
    /// Fraction of the parent row's value.
    PercentOfParent,
}

/// A measure in a layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutMeasure {
    /// The column, absent for `count_all`.
    #[serde(default)]
    pub column: Option<String>,
    /// The aggregate.
    pub aggregate: AggregateName,
    /// The percentile in `[0, 1]`, for `percentile`.
    #[serde(default)]
    pub percentile: Option<f64>,
    /// The weight column, for `weighted_mean`.
    #[serde(default)]
    pub weight: Option<String>,
    /// A column naming each row's unit.
    #[serde(default)]
    pub unit_column: Option<String>,
    /// How the value is shown.
    #[serde(default)]
    pub show_as: ShowAsName,
    /// The display format, overriding the one derived from the column's.
    #[serde(default)]
    pub format: Option<LayoutFormat>,
}

/// A filter operator in a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterOpName {
    /// Equal to `value`.
    Eq,
    /// Not equal to `value`.
    Ne,
    /// Less than `value`.
    Lt,
    /// At most `value`.
    Le,
    /// Greater than `value`.
    Gt,
    /// At least `value`.
    Ge,
    /// One of `values`.
    In,
    /// None of `values`.
    NotIn,
    /// Null.
    IsNull,
    /// Not null.
    IsNotNull,
    /// Text containing `value`, ignoring case.
    Contains,
    /// Text starting with `value`, ignoring case.
    StartsWith,
}

/// A filter in a layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutFilter {
    /// The column.
    pub column: String,
    /// The operator.
    pub op: FilterOpName,
    /// The operand of single-value operators.
    #[serde(default)]
    pub value: Option<Value>,
    /// The operands of `in` and `not_in`.
    #[serde(default)]
    pub values: Vec<Value>,
}

/// A sort in a layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutSort {
    /// The column.
    pub column: String,
    /// Largest first.
    #[serde(default)]
    pub descending: bool,
}

/// What orders groups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupOrderName {
    /// The group's own value.
    #[default]
    Key,
    /// A measure's value, by index.
    Measure(usize),
}

/// How groups are ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutGroupSort {
    /// What to order by.
    pub by: GroupOrderName,
    /// Largest first.
    pub descending: bool,
}

/// A comparison for aggregate filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompareName {
    /// Greater than.
    Gt,
    /// At least.
    Ge,
    /// Less than.
    Lt,
    /// At most.
    Le,
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
}

/// An aggregate filter in a layout.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutHaving {
    /// The measure, by index.
    pub measure: usize,
    /// The comparison.
    pub op: CompareName,
    /// The value compared with.
    pub value: f64,
}

/// Top-N in a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutTopN {
    /// Groups to keep.
    pub n: usize,
    /// The measure ranked, by index.
    pub measure: usize,
    /// Keep the largest.
    #[serde(default = "yes")]
    pub largest: bool,
    /// Show the rest as one Other row.
    #[serde(default = "yes")]
    pub other: bool,
}

fn yes() -> bool {
    true
}

/// A group the user expanded or collapsed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutExpanded {
    /// The group's values from the outermost level.
    pub path: Vec<Value>,
    /// Expanded or collapsed.
    pub expanded: bool,
}

/// A layout that cannot be read or does not fit the table.
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    /// The JSON is malformed or does not match the schema.
    #[error("layout JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The version is newer than this build understands.
    #[error("layout version {0} is newer than {LAYOUT_VERSION}")]
    Version(u32),
    /// A column is not in the table.
    #[error("column {0} is not in the table")]
    UnknownColumn(String),
    /// A value does not fit its column.
    #[error("column {column}: {reason}")]
    Value {
        /// The column.
        column: String,
        /// What is wrong.
        reason: String,
    },
    /// A measure is incomplete.
    #[error("measure {0}: {1}")]
    Measure(usize, String),
}

/// Returns the JSON Schema of layout documents.
#[must_use]
pub fn schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Layout)).expect("schemas serialize")
}

fn value_error(column: &str, reason: impl Into<String>) -> LayoutError {
    LayoutError::Value {
        column: column.to_owned(),
        reason: reason.into(),
    }
}

/// Parses decimal text, such as `-1234.5` or `1.2e3`, exactly into an unscaled
/// value at `scale`. More fractional digits than the scale holds are an error
/// unless they are zeros, so a value is never rounded.
///
/// # Errors
///
/// Returns a message for text that is not a decimal, does not fit 128 bits, or
/// has more decimal places than `scale`.
pub fn parse_exact_decimal(text: &str, scale: i8) -> Result<i128, String> {
    parse_decimal("value", text, scale).map_err(|e| match e {
        LayoutError::Value { reason, .. } => reason,
        other => other.to_string(),
    })
}

/// Parses decimal text exactly into an unscaled value at `scale`. More fractional
/// digits than the scale holds are an error unless they are zeros.
fn parse_decimal(column: &str, text: &str, scale: i8) -> Result<i128, LayoutError> {
    let bad = || value_error(column, format!("{text:?} is not a decimal"));
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(i) => (&text[..i], text[i + 1..].parse::<i64>().map_err(|_| bad())?),
        None => (text, 0),
    };
    let (negative, digits) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa.strip_prefix('+').unwrap_or(mantissa)),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    // The value is all_digits * 10^(exponent - fraction.len()); rescale to `scale`.
    let all_digits = format!("{whole}{fraction}");
    // Computed in i64 and bounded, so hostile exponents cannot overflow.
    let shift = i64::from(scale)
        .checked_add(exponent)
        .and_then(|s| s.checked_sub(i64::try_from(fraction.len()).ok()?))
        .filter(|s| s.abs() <= 76)
        .ok_or_else(|| value_error(column, format!("{text} is out of range")))?;
    let trimmed = all_digits.trim_start_matches('0');
    let mut unscaled: i128 = if trimmed.is_empty() {
        0
    } else {
        trimmed
            .parse()
            .map_err(|_| value_error(column, format!("{text} is too large")))?
    };
    if shift >= 0 {
        unscaled = 10i128
            .checked_pow(shift as u32)
            .and_then(|p| unscaled.checked_mul(p))
            .ok_or_else(|| value_error(column, format!("{text} is too large")))?;
    } else {
        let divisor = 10i128
            .checked_pow((-shift) as u32)
            .ok_or_else(|| value_error(column, format!("{text} has too many digits")))?;
        if unscaled % divisor != 0 {
            return Err(value_error(
                column,
                format!("{text} has more decimal places than the column's {scale}"),
            ));
        }
        unscaled /= divisor;
    }
    Ok(if negative { -unscaled } else { unscaled })
}

/// Converts a JSON value to a scalar of the column's type.
fn scalar(column: &str, ty: &ColumnType, value: &Value) -> Result<Scalar, LayoutError> {
    let mismatch = || value_error(column, format!("{value} does not fit type {ty:?}"));
    Ok(match (ty, value) {
        (_, Value::Null) => Scalar::Null,
        (ColumnType::Boolean, Value::Bool(b)) => Scalar::Boolean(*b),
        (ColumnType::Int64, Value::Number(n)) => {
            Scalar::Int64(n.as_str().parse().map_err(|_| mismatch())?)
        }
        // JavaScript numbers hold integers exactly only up to 2^53, so larger ones
        // travel as strings.
        (ColumnType::Int64, Value::String(s)) => {
            Scalar::Int64(s.trim().parse().map_err(|_| mismatch())?)
        }
        (ColumnType::Float64, Value::Number(n)) => {
            Scalar::Float64(n.as_str().parse().map_err(|_| mismatch())?)
        }
        // JSON has no infinity, so infinities travel as "inf" and "-inf".
        (ColumnType::Float64, Value::String(s)) => match s.as_str() {
            "inf" | "+inf" | "infinity" => Scalar::Float64(f64::INFINITY),
            "-inf" | "-infinity" => Scalar::Float64(f64::NEG_INFINITY),
            _ => return Err(mismatch()),
        },
        (ColumnType::Decimal { scale, .. }, Value::Number(n)) => {
            Scalar::Decimal(parse_decimal(column, n.as_str(), *scale)?)
        }
        (ColumnType::Decimal { scale, .. }, Value::String(s)) => {
            Scalar::Decimal(parse_decimal(column, s.trim(), *scale)?)
        }
        (ColumnType::Utf8, Value::String(s)) => Scalar::Utf8(s.clone()),
        (ColumnType::Date32, Value::String(s)) => {
            let date: jiff::civil::Date = s.parse().map_err(|_| mismatch())?;
            let days = date
                .since(jiff::civil::date(1970, 1, 1))
                .map_err(|_| mismatch())?
                .get_days();
            Scalar::Date32(days)
        }
        (ColumnType::Timestamp { timezone: Some(_) }, Value::String(s)) => {
            let t: jiff::Timestamp = s.parse().map_err(|_| {
                value_error(
                    column,
                    format!("{s:?} needs an offset, such as Z or -05:00"),
                )
            })?;
            Scalar::Timestamp(t.as_microsecond())
        }
        (ColumnType::Timestamp { timezone: None }, Value::String(s)) => {
            let dt: jiff::civil::DateTime = s.parse().map_err(|_| mismatch())?;
            let t = dt
                .to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|_| mismatch())?;
            Scalar::Timestamp(t.timestamp().as_microsecond())
        }
        _ => return Err(mismatch()),
    })
}

fn column_type(table: &Table, column: &str) -> Result<ColumnType, LayoutError> {
    let index = table
        .column_index(column)
        .ok_or_else(|| LayoutError::UnknownColumn(column.to_owned()))?;
    Ok(table.column(index).column_type().clone())
}

/// A layout read against a table: the engine configuration and the expanded and
/// collapsed groups.
#[derive(Clone, Debug)]
pub struct ResolvedLayout {
    /// The view configuration.
    pub config: ViewConfig,
    /// The chart the engine reduces, for layouts with a reduced chart.
    pub chart: Option<ChartConfig>,
    /// Group paths and whether each is expanded.
    pub expanded: Vec<(Vec<Scalar>, bool)>,
}

impl Layout {
    /// Reads a layout document, migrating older versions.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, unknown fields, or a newer version.
    pub fn from_json(json: &str) -> Result<Self, LayoutError> {
        let mut value: Value = serde_json::from_str(json)?;
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or(u64::from(LAYOUT_VERSION)) as u32;
        if version > LAYOUT_VERSION {
            return Err(LayoutError::Version(version));
        }
        // Migrations from older versions go here, one step at a time.
        if let Some(object) = value.as_object_mut() {
            object.insert("version".into(), Value::from(LAYOUT_VERSION));
        }
        Ok(serde_json::from_value(value)?)
    }

    /// Writes the layout as JSON.
    ///
    /// # Panics
    ///
    /// Never: layouts always serialize.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("layouts serialize")
    }

    /// Interprets the layout against `table`.
    ///
    /// # Errors
    ///
    /// Returns an error for unknown columns, values that do not fit their columns,
    /// and incomplete measures.
    pub fn resolve(&self, table: &Table) -> Result<ResolvedLayout, LayoutError> {
        let measures = self
            .measures
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let aggregate = match m.aggregate {
                    AggregateName::Count => Aggregate::Count,
                    AggregateName::CountAll => Aggregate::CountAll,
                    AggregateName::Sum => Aggregate::Sum,
                    AggregateName::Mean => Aggregate::Mean,
                    AggregateName::WeightedMean => Aggregate::WeightedMean,
                    AggregateName::Min => Aggregate::Min,
                    AggregateName::Max => Aggregate::Max,
                    AggregateName::First => Aggregate::First,
                    AggregateName::Last => Aggregate::Last,
                    AggregateName::DistinctCount => Aggregate::DistinctCount,
                    AggregateName::Variance => Aggregate::Variance,
                    AggregateName::StdDev => Aggregate::StdDev,
                    AggregateName::Median => Aggregate::Median,
                    AggregateName::Percentile => {
                        Aggregate::Percentile(m.percentile.ok_or_else(|| {
                            LayoutError::Measure(i, "percentile is missing".into())
                        })?)
                    }
                };
                if aggregate == Aggregate::WeightedMean && m.weight.is_none() {
                    return Err(LayoutError::Measure(
                        i,
                        "weighted_mean needs a weight".into(),
                    ));
                }
                Ok(Measure {
                    column: m.column.clone(),
                    aggregate,
                    weight: m.weight.clone(),
                    unit_column: m.unit_column.clone(),
                    show_as: match m.show_as {
                        ShowAsName::Value => ShowAs::Value,
                        ShowAsName::PercentOfGrandTotal => ShowAs::PercentOfGrandTotal,
                        ShowAsName::PercentOfColumnTotal => ShowAs::PercentOfColumnTotal,
                        ShowAsName::PercentOfRowTotal => ShowAs::PercentOfRowTotal,
                        ShowAsName::PercentOfParent => ShowAs::PercentOfParent,
                    },
                    format: m.format.as_ref().map(LayoutFormat::to_engine),
                })
            })
            .collect::<Result<Vec<_>, LayoutError>>()?;
        let filters = self
            .filters
            .iter()
            .map(|f| {
                let ty = column_type(table, &f.column)?;
                let single = || -> Result<Scalar, LayoutError> {
                    let v = f
                        .value
                        .as_ref()
                        .ok_or_else(|| value_error(&f.column, "the filter needs a value"))?;
                    match scalar(&f.column, &ty, v)? {
                        Scalar::Null => Err(value_error(&f.column, "use is_null to match nulls")),
                        s => Ok(s),
                    }
                };
                let many = || -> Result<Vec<Scalar>, LayoutError> {
                    f.values.iter().map(|v| scalar(&f.column, &ty, v)).collect()
                };
                let text = || -> Result<String, LayoutError> {
                    match &f.value {
                        Some(Value::String(s)) => Ok(s.clone()),
                        _ => Err(value_error(&f.column, "the filter needs text")),
                    }
                };
                let op = match f.op {
                    FilterOpName::Eq => FilterOp::Eq(single()?),
                    FilterOpName::Ne => FilterOp::Ne(single()?),
                    FilterOpName::Lt => FilterOp::Lt(single()?),
                    FilterOpName::Le => FilterOp::Le(single()?),
                    FilterOpName::Gt => FilterOp::Gt(single()?),
                    FilterOpName::Ge => FilterOp::Ge(single()?),
                    FilterOpName::In => FilterOp::In(many()?),
                    FilterOpName::NotIn => FilterOp::NotIn(many()?),
                    FilterOpName::IsNull => FilterOp::IsNull,
                    FilterOpName::IsNotNull => FilterOp::IsNotNull,
                    FilterOpName::Contains => FilterOp::Contains(text()?),
                    FilterOpName::StartsWith => FilterOp::StartsWith(text()?),
                };
                Ok(Filter::new(&f.column, op))
            })
            .collect::<Result<Vec<_>, LayoutError>>()?;
        let expanded = self
            .expanded
            .iter()
            .map(|e| {
                let path = e
                    .path
                    .iter()
                    .zip(&self.group_by)
                    .map(|(v, column)| scalar(column, &column_type(table, column)?, v))
                    .collect::<Result<Vec<_>, LayoutError>>()?;
                Ok((path, e.expanded))
            })
            .collect::<Result<Vec<_>, LayoutError>>()?;
        let config = ViewConfig {
            columns: self.columns.clone(),
            group_by: self.group_by.clone(),
            split_by: self.split_by.clone(),
            measures,
            filters,
            sort: self
                .sort
                .iter()
                .map(|s| Sort {
                    column: s.column.clone(),
                    descending: s.descending,
                })
                .collect(),
            group_sort: GroupSort {
                by: match self.group_sort.by {
                    GroupOrderName::Key => GroupOrder::Key,
                    GroupOrderName::Measure(i) => GroupOrder::Measure(i),
                },
                descending: self.group_sort.descending,
            },
            having: self
                .having
                .iter()
                .map(|h| Having {
                    measure: h.measure,
                    op: match h.op {
                        CompareName::Gt => Comparison::Gt,
                        CompareName::Ge => Comparison::Ge,
                        CompareName::Lt => Comparison::Lt,
                        CompareName::Le => Comparison::Le,
                        CompareName::Eq => Comparison::Eq,
                        CompareName::Ne => Comparison::Ne,
                    },
                    value: h.value,
                })
                .collect(),
            top_n: self.top_n.map(|t| TopN {
                n: t.n,
                measure: t.measure,
                largest: t.largest,
                other: t.other,
            }),
            expand_depth: self.expand_depth,
            formats: self
                .formats
                .iter()
                .map(|(column, f)| {
                    column_type(table, column)?;
                    Ok((column.clone(), f.to_engine()))
                })
                .collect::<Result<_, LayoutError>>()?,
        };
        let chart = self
            .chart
            .as_ref()
            .and_then(|c| c.to_engine(&config.filters));
        Ok(ResolvedLayout {
            config,
            chart,
            expanded,
        })
    }
}

/// Converts one engine value of `column` to JSON. Decimals become strings so that no
/// digit is lost, dates become `YYYY-MM-DD`, and instants become RFC 3339 text.
#[must_use]
pub fn scalar_to_json(table: &Table, column: &str, value: &Scalar) -> Value {
    match value {
        Scalar::Null => Value::Null,
        Scalar::Boolean(b) => Value::Bool(*b),
        Scalar::Int64(i) => Value::from(*i),
        Scalar::Float64(f) if f.is_infinite() => {
            Value::String(if *f > 0.0 { "inf" } else { "-inf" }.to_owned())
        }
        Scalar::Float64(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Scalar::Decimal(d) => {
            let scale = match column_type(table, column) {
                Ok(ColumnType::Decimal { scale, .. }) => i32::from(scale),
                _ => 0,
            };
            Value::String(exact_decimal(*d, scale))
        }
        Scalar::Utf8(t) => Value::String(t.clone()),
        Scalar::Date32(d) => Value::String(
            jiff::civil::date(1970, 1, 1)
                .checked_add(jiff::Span::new().days(i64::from(*d)))
                .map_or_else(|_| d.to_string(), |date| date.to_string()),
        ),
        Scalar::Timestamp(t) => match (
            jiff::Timestamp::from_microsecond(*t),
            column_type(table, column),
        ) {
            (Ok(ts), Ok(ColumnType::Timestamp { timezone: None })) => {
                Value::String(ts.to_zoned(jiff::tz::TimeZone::UTC).datetime().to_string())
            }
            (Ok(ts), _) => Value::String(ts.to_string()),
            (Err(_), _) => Value::from(*t),
        },
    }
}

/// Converts a group path from the engine to JSON values.
#[must_use]
pub fn path_to_json(table: &Table, group_by: &[String], path: &[Scalar]) -> Vec<Value> {
    path.iter()
        .zip(group_by)
        .map(|(s, column)| scalar_to_json(table, column, s))
        .collect()
}

impl Layout {
    /// Describes an engine configuration as a layout, for views the server creates
    /// itself, such as drill-throughs. Measures and aggregate filters are not
    /// carried; flat views have none.
    #[must_use]
    pub fn from_flat_config(config: &ViewConfig, table: &Table) -> Self {
        let filters = config
            .filters
            .iter()
            .map(|f| {
                let one = |v: &Scalar| Some(scalar_to_json(table, &f.column, v));
                let (op, value, values) = match &f.op {
                    FilterOp::Eq(v) => (FilterOpName::Eq, one(v), Vec::new()),
                    FilterOp::Ne(v) => (FilterOpName::Ne, one(v), Vec::new()),
                    FilterOp::Lt(v) => (FilterOpName::Lt, one(v), Vec::new()),
                    FilterOp::Le(v) => (FilterOpName::Le, one(v), Vec::new()),
                    FilterOp::Gt(v) => (FilterOpName::Gt, one(v), Vec::new()),
                    FilterOp::Ge(v) => (FilterOpName::Ge, one(v), Vec::new()),
                    FilterOp::In(vs) => (
                        FilterOpName::In,
                        None,
                        vs.iter()
                            .map(|v| scalar_to_json(table, &f.column, v))
                            .collect(),
                    ),
                    FilterOp::NotIn(vs) => (
                        FilterOpName::NotIn,
                        None,
                        vs.iter()
                            .map(|v| scalar_to_json(table, &f.column, v))
                            .collect(),
                    ),
                    FilterOp::IsNull => (FilterOpName::IsNull, None, Vec::new()),
                    FilterOp::IsNotNull => (FilterOpName::IsNotNull, None, Vec::new()),
                    FilterOp::Contains(t) => (
                        FilterOpName::Contains,
                        Some(Value::String(t.clone())),
                        Vec::new(),
                    ),
                    FilterOp::StartsWith(t) => (
                        FilterOpName::StartsWith,
                        Some(Value::String(t.clone())),
                        Vec::new(),
                    ),
                };
                LayoutFilter {
                    column: f.column.clone(),
                    op,
                    value,
                    values,
                }
            })
            .collect();
        Self {
            version: LAYOUT_VERSION,
            columns: config.columns.clone(),
            filters,
            sort: config
                .sort
                .iter()
                .map(|s| LayoutSort {
                    column: s.column.clone(),
                    descending: s.descending,
                })
                .collect(),
            formats: config
                .formats
                .iter()
                .map(|(c, f)| (c.clone(), LayoutFormat::from_engine(f)))
                .collect(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_parse_exactly_or_not_at_all() {
        assert_eq!(parse_decimal("x", "123.45", 2).unwrap(), 12_345);
        assert_eq!(parse_decimal("x", "-0.5", 2).unwrap(), -50);
        assert_eq!(parse_decimal("x", "1e3", 2).unwrap(), 100_000);
        assert_eq!(parse_decimal("x", "1.250", 2).unwrap(), 125);
        assert_eq!(parse_decimal("x", "7", 0).unwrap(), 7);
        assert!(parse_decimal("x", "1.234", 2).is_err(), "would round");
        assert!(parse_decimal("x", "abc", 2).is_err());
        assert!(parse_decimal("x", "", 2).is_err());
        assert!(parse_decimal("x", "1e40", 2).is_err());
        assert!(
            parse_decimal("x", "1e2147483647", 2).is_err(),
            "hostile exponent"
        );
        assert!(parse_decimal("x", "1e-9223372036854775808", 2).is_err());
    }

    #[test]
    fn large_integers_arrive_as_strings() {
        let ty = ColumnType::Int64;
        assert_eq!(
            scalar("id", &ty, &Value::String("1152921504606846976".into())).unwrap(),
            Scalar::Int64(1 << 60)
        );
        assert!(scalar("id", &ty, &Value::String("1.5".into())).is_err());
    }

    #[test]
    fn microsecond_timestamps_round_trip_through_text() {
        let zoned = ColumnType::Timestamp {
            timezone: Some("UTC".into()),
        };
        let naive = ColumnType::Timestamp { timezone: None };
        assert_eq!(
            scalar(
                "t",
                &zoned,
                &Value::String("2026-11-01T05:30:00.123456Z".into())
            )
            .unwrap(),
            Scalar::Timestamp(1_793_511_000_123_456)
        );
        assert_eq!(
            scalar(
                "t",
                &naive,
                &Value::String("2026-11-01T01:30:00.123456".into())
            )
            .unwrap(),
            Scalar::Timestamp(1_793_496_600_123_456)
        );
    }

    #[test]
    fn decimals_format_exactly() {
        assert_eq!(exact_decimal(12_345, 2), "123.45");
        assert_eq!(exact_decimal(5, -2), "500");
        assert_eq!(exact_decimal(-5, 3), "-0.005");
        assert_eq!(exact_decimal(42, 0), "42");
    }

    #[test]
    fn layouts_round_trip_and_reject_the_future() {
        let json = r#"{"version":1,"group_by":["fund"],"measures":[{"column":"x","aggregate":"sum"}],
            "group_sort":{"by":{"measure":0},"descending":true},"top_n":{"n":5,"measure":0}}"#;
        let layout = Layout::from_json(json).unwrap();
        assert_eq!(layout.group_sort.by, GroupOrderName::Measure(0));
        assert!(layout.top_n.unwrap().other);
        assert_eq!(Layout::from_json(&layout.to_json()).unwrap(), layout);
        assert!(matches!(
            Layout::from_json(r#"{"version":99}"#),
            Err(LayoutError::Version(99))
        ));
        assert!(
            Layout::from_json(r#"{"version":1,"grup_by":[]}"#).is_err(),
            "unknown fields"
        );
    }

    #[test]
    fn formats_round_trip_through_the_engine() {
        let mut formats = Vec::new();
        for decimals in [None, Some(0), Some(30)] {
            for thousands in [None, Some(false), Some(true)] {
                for (percent, abbreviate) in [
                    (false, None),
                    (true, None),
                    (false, Some(AbbreviationName::Thousands)),
                    (false, Some(AbbreviationName::Millions)),
                    (false, Some(AbbreviationName::Billions)),
                ] {
                    for negative in [NegativeName::Minus, NegativeName::Parentheses] {
                        for precision in [
                            PrecisionName::Seconds,
                            PrecisionName::Milliseconds,
                            PrecisionName::Microseconds,
                        ] {
                            for zone in [None, Some("Europe/London".to_owned())] {
                                formats.push(LayoutFormat {
                                    decimals,
                                    thousands,
                                    percent,
                                    abbreviate,
                                    negative,
                                    zone,
                                    precision,
                                });
                            }
                        }
                    }
                }
            }
        }
        for f in formats {
            assert_eq!(LayoutFormat::from_engine(&f.to_engine()), f);
        }
    }

    /// Layouts as this crate writes them: every default, and every field set. The
    /// web library's schema must read each one back unchanged.
    fn fixture_layouts() -> Vec<Value> {
        let full = r#"{
            "version": 1,
            "dataset": "positions",
            "parameters": {"as_of": "2026-11-02"},
            "columns": ["fund", "ticker"],
            "group_by": ["fund", "sector"],
            "split_by": ["asset_class"],
            "measures": [
                {"column": "market_value_usd", "aggregate": "sum", "unit_column": "currency",
                 "show_as": "percent_of_parent",
                 "format": {"decimals": 1, "thousands": false, "percent": true,
                            "negative": "parentheses", "zone": "Europe/London",
                            "precision": "milliseconds"}},
                {"column": "price_local", "aggregate": "percentile", "percentile": 0.9},
                {"column": "beta", "aggregate": "weighted_mean", "weight": "market_value_usd"},
                {"aggregate": "count_all"}
            ],
            "filters": [
                {"column": "fund", "op": "in", "values": ["Atlas", "Cobalt"]},
                {"column": "quantity", "op": "gt", "value": "1.5"},
                {"column": "sector", "op": "is_null"}
            ],
            "sort": [{"column": "fund", "descending": true}],
            "group_sort": {"by": {"measure": 0}, "descending": true},
            "having": [{"measure": 0, "op": "ge", "value": 1000.5}],
            "top_n": {"n": 10, "measure": 0, "largest": false, "other": true},
            "expand_depth": 2,
            "expanded": [{"path": ["Atlas", null, 1.5, true], "expanded": false}],
            "formats": {"quantity": {"abbreviate": "thousands"}, "trade_time": {"precision": "microseconds"}},
            "column_widths": {"fund": 180, "__tree": 300, "__measure:0": 120, "region": 0}
        }"#;
        let chart = r#"{
            "dataset": "prices",
            "chart": {"kind": "line", "x": "date", "y": ["close"], "y_right": ["volume"],
                      "series": "ticker", "axis": "trading", "log_y": true, "stacked": true,
                      "horizontal": true, "range": {"from": 1200.5, "to": 1450.0},
                      "hidden": ["volume AAPL"]}
        }"#;
        let candles = r#"{
            "dataset": "prices",
            "filters": [{"column": "ticker", "op": "eq", "value": "AAPL"}],
            "chart": {"kind": "candlestick", "x": "date",
                      "ohlc": {"open": "open", "high": "high", "low": "low", "close": "close"}}
        }"#;
        [
            Layout::default(),
            Layout::from_json(full).unwrap(),
            Layout::from_json(chart).unwrap(),
            Layout::from_json(candles).unwrap(),
            Layout::from_json(r#"{"chart": {"kind": "bar"}}"#).unwrap(),
        ]
        .iter()
        .map(|l| serde_json::to_value(l).unwrap())
        .collect()
    }

    /// Compares a committed file with what this build generates, or rewrites it
    /// when `PARALLAX_UPDATE_FIXTURES` is set.
    fn check_generated(relative: &str, contents: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
        if std::env::var_os("PARALLAX_UPDATE_FIXTURES").is_some() {
            std::fs::write(&path, contents).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == contents,
            "{} is out of date; rerun with PARALLAX_UPDATE_FIXTURES=1",
            path.display()
        );
    }

    #[test]
    fn committed_schema_and_fixtures_are_current() {
        let schema = serde_json::to_string_pretty(&schema()).unwrap() + "\n";
        check_generated("../../schema/layout.schema.json", &schema);
        let fixtures = serde_json::to_string_pretty(&fixture_layouts()).unwrap() + "\n";
        check_generated(
            "../../web/projects/parallax/src/lib/testing/layouts.json",
            &fixtures,
        );
    }
}
