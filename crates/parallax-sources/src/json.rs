//! The shared JSON converter: documents to an Arrow batch of declared columns.
//!
//! JSON columns in SQL Server, MongoDB documents and REST responses all go through
//! here, so they follow one set of rules:
//!
//! - Each column names a path into the document and declares its type.
//! - Numbers are parsed from their text straight into the declared type.
//!   Decimals are exact; a decimal with more places than the column holds is an
//!   error, never rounded.
//! - A missing path is null. A value of the wrong type is an error naming the
//!   row, the path and the value.
//! - Nested objects are reached by dotted paths. Arrays are refused for now: the
//!   engine has no list columns yet.
//! - Wall-clock times without an offset are read in the column's zone,
//!   `America/New_York` unless it says otherwise. A time in a daylight-saving
//!   gap is an error, and one in the repeated hour is an error unless the column
//!   says to take the earlier or later instant.
//! - Bare `NaN` and `Infinity` tokens, which Python's `json` module writes, are
//!   read as numbers; the table load turns NaN into null and counts it.

use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, Date32Builder, Decimal128Builder, Float64Builder, Int64Builder, StringBuilder,
    TimestampMicrosecondBuilder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use jiff::tz::{AmbiguousOffset, TimeZone};
use parallax_protocol::layout::parse_exact_decimal;
use serde::Deserialize;
use serde_json::Value;

/// The zone wall-clock times are read in unless a column names another.
pub const DEFAULT_SOURCE_ZONE: &str = "America/New_York";

/// What to do with a wall-clock time in the hour that repeats when clocks go back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ambiguous {
    /// Refuse the load.
    #[default]
    Error,
    /// Take the first of the two instants.
    Earlier,
    /// Take the second.
    Later,
}

/// A column's declared type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    /// Text.
    Text,
    /// `true` or `false`.
    Boolean,
    /// A 64-bit integer.
    Integer,
    /// A double.
    Float,
    /// An exact decimal with this precision and scale.
    Decimal(u8, i8),
    /// A calendar date, `YYYY-MM-DD`.
    Date,
    /// An instant. Text with an offset is exact; text without one is read in the
    /// column's zone.
    Timestamp,
    /// A wall-clock time kept without a zone.
    WallClock,
}

impl FieldType {
    /// The Arrow type the column loads as.
    #[must_use]
    pub fn arrow(self) -> DataType {
        match self {
            Self::Text => DataType::Utf8,
            Self::Boolean => DataType::Boolean,
            Self::Integer => DataType::Int64,
            Self::Float => DataType::Float64,
            Self::Decimal(p, s) => DataType::Decimal128(p, s),
            Self::Date => DataType::Date32,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            Self::WallClock => DataType::Timestamp(TimeUnit::Microsecond, None),
        }
    }

    fn describe(self) -> String {
        match self {
            Self::Text => "text".into(),
            Self::Boolean => "a boolean".into(),
            Self::Integer => "an integer".into(),
            Self::Float => "a number".into(),
            Self::Decimal(p, s) => format!("a decimal({p}, {s})"),
            Self::Date => "a date".into(),
            Self::Timestamp => "a timestamp".into(),
            Self::WallClock => "a wall-clock time".into(),
        }
    }
}

impl TryFrom<String> for FieldType {
    type Error = String;

    /// Reads `text`, `boolean`, `integer`, `float`, `decimal(18, 2)`, `date`,
    /// `timestamp` or `wall_clock`.
    fn try_from(text: String) -> Result<Self, String> {
        let t = text.trim().to_ascii_lowercase();
        Ok(match t.as_str() {
            "text" => Self::Text,
            "boolean" => Self::Boolean,
            "integer" => Self::Integer,
            "float" => Self::Float,
            "date" => Self::Date,
            "timestamp" => Self::Timestamp,
            "wall_clock" => Self::WallClock,
            _ => {
                let inner = t
                    .strip_prefix("decimal(")
                    .and_then(|r| r.strip_suffix(')'))
                    .ok_or_else(|| format!("unknown type {text:?}"))?;
                let (p, s) = inner
                    .split_once(',')
                    .ok_or_else(|| format!("{text:?} needs a precision and a scale"))?;
                let precision: u8 = p
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad precision in {text:?}"))?;
                let scale: i8 = s
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad scale in {text:?}"))?;
                if !(1..=38).contains(&precision)
                    || scale < 0
                    || i16::from(scale) > i16::from(precision)
                {
                    return Err(format!(
                        "{text:?}: precision must be 1 to 38 and scale 0 to the precision"
                    ));
                }
                Self::Decimal(precision, scale)
            }
        })
    }
}

impl<'de> Deserialize<'de> for FieldType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::try_from(text).map_err(serde::de::Error::custom)
    }
}

/// A column read from JSON documents.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonField {
    /// The column's name.
    pub name: String,
    /// The dotted path to the value, such as `risk.beta`; the name if absent.
    #[serde(default)]
    pub path: Option<String>,
    /// The declared type.
    #[serde(rename = "type")]
    pub field_type: FieldType,
    /// The zone wall-clock text is read in, for timestamps.
    #[serde(default)]
    pub zone: Option<String>,
    /// What to do with a time in the repeated hour, for timestamps.
    #[serde(default)]
    pub ambiguous: Ambiguous,
}

impl JsonField {
    /// A column at its own name's path.
    #[must_use]
    pub fn new(name: &str, field_type: FieldType) -> Self {
        Self {
            name: name.to_owned(),
            path: None,
            field_type,
            zone: None,
            ambiguous: Ambiguous::Error,
        }
    }

    fn path(&self) -> &str {
        self.path.as_deref().unwrap_or(&self.name)
    }
}

/// Marks the strings [`quote_bare_numbers`] makes: a control character no JSON
/// text can hold unescaped.
const BARE_MARKER: &str = "\u{1}bare:";

/// A non-finite double as a value only number columns accept, for sources that
/// hold real doubles, such as MongoDB.
#[must_use]
pub fn non_finite(value: f64) -> Value {
    let token = if value.is_nan() {
        "NaN"
    } else if value > 0.0 {
        "Infinity"
    } else {
        "-Infinity"
    };
    Value::String(format!("{BARE_MARKER}{token}"))
}

/// Rewrites bare `NaN`, `Infinity` and `-Infinity` tokens outside strings as
/// marked strings, so that a standard parser reads Python's output and only
/// number columns accept them.
#[must_use]
pub fn quote_bare_numbers(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("NaN") && !text.contains("Infinity") {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut in_string = false;
    let mut escaped = false;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            rest = &rest[c.len_utf8()..];
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            rest = &rest[1..];
            continue;
        }
        let token = ["-Infinity", "Infinity", "NaN"]
            .into_iter()
            .find(|t| rest.starts_with(t));
        if let Some(token) = token {
            // A marker no real string can hold, so text columns refuse it as a
            // number rather than taking "NaN" as a name.
            out.push('"');
            // The marker written as a JSON escape, which parses into BARE_MARKER.
            out.push_str("\\u0001bare:");
            out.push_str(token);
            out.push('"');
            rest = &rest[token.len()..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Parses JSON text into its records: the array at the dotted `records` path, or
/// the top-level array.
///
/// # Errors
///
/// Returns a message for malformed JSON or when no array is found.
pub fn parse_records(text: &str, records: Option<&str>) -> Result<Vec<Value>, String> {
    let mut value: Value = serde_json::from_str(&quote_bare_numbers(text))
        .map_err(|e| format!("malformed JSON: {e}"))?;
    let found = match records {
        Some(path) => {
            let pointer = format!(
                "/{}",
                path.replace('~', "~0").replace('/', "~1").replace('.', "/")
            );
            value
                .pointer_mut(&pointer)
                .ok_or_else(|| format!("no {path} in the response"))?
        }
        None => &mut value,
    };
    // The records move out of the parsed document rather than being copied.
    match found.take() {
        Value::Array(items) => Ok(items),
        _ => Err(format!(
            "expected an array of records at {}",
            records.unwrap_or("the top level")
        )),
    }
}

/// The value at a dotted path, or `None` if any step is missing.
fn lookup<'v>(value: &'v Value, path: &str) -> Option<&'v Value> {
    path.split('.')
        .try_fold(value, |v, step| v.as_object()?.get(step))
}

/// A column being built.
enum Builder {
    Text(StringBuilder),
    Boolean(BooleanBuilder),
    Integer(Int64Builder),
    Float(Float64Builder),
    Decimal(Decimal128Builder),
    Date(Date32Builder),
    Timestamp(TimestampMicrosecondBuilder),
}

impl Builder {
    fn new(field_type: FieldType, rows: usize) -> Self {
        match field_type {
            FieldType::Text => Self::Text(StringBuilder::with_capacity(rows, rows * 8)),
            FieldType::Boolean => Self::Boolean(BooleanBuilder::with_capacity(rows)),
            FieldType::Integer => Self::Integer(Int64Builder::with_capacity(rows)),
            FieldType::Float => Self::Float(Float64Builder::with_capacity(rows)),
            FieldType::Decimal(..) => Self::Decimal(Decimal128Builder::with_capacity(rows)),
            FieldType::Date => Self::Date(Date32Builder::with_capacity(rows)),
            FieldType::Timestamp | FieldType::WallClock => {
                Self::Timestamp(TimestampMicrosecondBuilder::with_capacity(rows))
            }
        }
    }

    fn null(&mut self) {
        match self {
            Self::Text(b) => b.append_null(),
            Self::Boolean(b) => b.append_null(),
            Self::Integer(b) => b.append_null(),
            Self::Float(b) => b.append_null(),
            Self::Decimal(b) => b.append_null(),
            Self::Date(b) => b.append_null(),
            Self::Timestamp(b) => b.append_null(),
        }
    }

    fn finish(self, field_type: FieldType) -> Result<ArrayRef, String> {
        Ok(match (self, field_type) {
            (Self::Text(mut b), _) => Arc::new(b.finish()),
            (Self::Boolean(mut b), _) => Arc::new(b.finish()),
            (Self::Integer(mut b), _) => Arc::new(b.finish()),
            (Self::Float(mut b), _) => Arc::new(b.finish()),
            (Self::Decimal(mut b), FieldType::Decimal(p, s)) => Arc::new(
                b.finish()
                    .with_precision_and_scale(p, s)
                    .map_err(|e| e.to_string())?,
            ),
            (Self::Decimal(mut b), _) => Arc::new(b.finish()),
            (Self::Date(mut b), _) => Arc::new(b.finish()),
            (Self::Timestamp(mut b), FieldType::Timestamp) => {
                Arc::new(b.finish().with_timezone("UTC"))
            }
            (Self::Timestamp(mut b), _) => Arc::new(b.finish()),
        })
    }
}

/// The text of a JSON number as written, or of a string.
fn number_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.trim().to_owned()),
        _ => None,
    }
}

/// Reads a float from a number or numeric text, including NaN and infinities.
fn read_float(value: &Value) -> Option<f64> {
    let text = number_text(value)?;
    let text = text.strip_prefix(BARE_MARKER).unwrap_or(&text);
    match text.to_ascii_lowercase().as_str() {
        "nan" => Some(f64::NAN),
        "infinity" | "inf" | "+infinity" | "+inf" => Some(f64::INFINITY),
        "-infinity" | "-inf" => Some(f64::NEG_INFINITY),
        _ => text.parse().ok(),
    }
}

/// Reads an instant from text: exactly when it has an offset, and in `zone` when
/// it is wall-clock time, refusing gaps and, unless told otherwise, folds.
fn read_instant(text: &str, zone: &TimeZone, ambiguous: Ambiguous) -> Result<i64, String> {
    if let Ok(t) = text.parse::<jiff::Timestamp>() {
        return micros_of(t);
    }
    let civil: jiff::civil::DateTime = text.parse().map_err(|_| "is not a timestamp".to_owned())?;
    resolve_civil(civil, zone, ambiguous)
}

/// The instant a wall-clock time names in `zone`, refusing the spring gap and,
/// unless told otherwise, the autumn fold.
fn resolve_civil(
    civil: jiff::civil::DateTime,
    zone: &TimeZone,
    ambiguous: Ambiguous,
) -> Result<i64, String> {
    let candidates = zone.to_ambiguous_zoned(civil);
    let instant = match (candidates.offset(), ambiguous) {
        (AmbiguousOffset::Unambiguous { .. }, _) => candidates.compatible(),
        (AmbiguousOffset::Gap { .. }, _) => {
            return Err("falls in the hour skipped when clocks went forward".to_owned());
        }
        (AmbiguousOffset::Fold { .. }, Ambiguous::Error) => {
            return Err("falls in the hour that repeats when clocks go back; the column must say whether to take the earlier or the later instant".to_owned());
        }
        (AmbiguousOffset::Fold { .. }, Ambiguous::Earlier) => candidates.earlier(),
        (AmbiguousOffset::Fold { .. }, Ambiguous::Later) => candidates.later(),
    }
    .map_err(|e| e.to_string())?;
    micros_of(instant.timestamp())
}

/// Microseconds of an instant, refusing finer digits rather than dropping them.
fn micros_of(t: jiff::Timestamp) -> Result<i64, String> {
    if t.subsec_nanosecond() % 1000 != 0 {
        return Err("has more than microsecond precision".to_owned());
    }
    Ok(t.as_microsecond())
}

fn read_wall_clock(text: &str) -> Result<i64, String> {
    let civil: jiff::civil::DateTime = text
        .parse()
        .map_err(|_| "is not a date and time".to_owned())?;
    let instant = civil.to_zoned(TimeZone::UTC).map_err(|e| e.to_string())?;
    micros_of(instant.timestamp())
}

fn read_date(text: &str) -> Result<i32, String> {
    let date: jiff::civil::Date = text.parse().map_err(|_| "is not a date".to_owned())?;
    let days = date
        .since(jiff::civil::date(1970, 1, 1))
        .map_err(|e| e.to_string())?
        .get_days();
    Ok(days)
}

/// Appends one value to a column, or says why it does not fit.
fn append(
    builder: &mut Builder,
    field: &JsonField,
    zone: &TimeZone,
    value: &Value,
) -> Result<(), String> {
    let mismatch = || format!("is not {}", field.field_type.describe());
    match (builder, value) {
        (b, Value::Null) => b.null(),
        (_, Value::Array(_)) => return Err("is an array; arrays are not supported yet".to_owned()),
        (_, Value::Object(_)) => {
            return Err(format!(
                "is an object, not {}; name a path inside it",
                field.field_type.describe()
            ));
        }
        (_, Value::String(s))
            if s.starts_with(BARE_MARKER) && !matches!(field.field_type, FieldType::Float) =>
        {
            return Err(format!(
                "is a bare {}, which only a number column accepts",
                &s[BARE_MARKER.len()..]
            ));
        }
        (Builder::Text(b), Value::String(s)) => b.append_value(s),
        (Builder::Boolean(b), Value::Bool(v)) => b.append_value(*v),
        (Builder::Integer(b), v) => {
            let text = number_text(v).ok_or_else(mismatch)?;
            b.append_value(text.parse().map_err(|_| mismatch())?);
        }
        (Builder::Float(b), v) => b.append_value(read_float(v).ok_or_else(mismatch)?),
        (Builder::Decimal(b), v) => {
            let FieldType::Decimal(precision, scale) = field.field_type else {
                return Err(mismatch());
            };
            let text = number_text(v).ok_or_else(mismatch)?;
            let unscaled = parse_exact_decimal(&text, scale)?;
            // The precision bounds the digits, which the array itself does not check.
            if unscaled.unsigned_abs() >= 10u128.pow(u32::from(precision)) {
                return Err(format!(
                    "has more digits than decimal({precision}, {scale}) holds"
                ));
            }
            b.append_value(unscaled);
        }
        (Builder::Date(b), Value::String(s)) => b.append_value(read_date(s)?),
        (Builder::Timestamp(b), Value::String(s)) => {
            b.append_value(if field.field_type == FieldType::WallClock {
                read_wall_clock(s)?
            } else {
                read_instant(s, zone, field.ambiguous)?
            })
        }
        _ => return Err(mismatch()),
    }
    Ok(())
}

/// Builds a batch with one row per document and one column per field.
///
/// `key` names a field whose value identifies a row in error messages.
///
/// # Errors
///
/// Returns a message naming the row, path and value of the first value that does
/// not fit its column, or an unknown zone.
pub fn to_batch(
    fields: &[JsonField],
    documents: &[Value],
    key: Option<&str>,
) -> Result<RecordBatch, String> {
    let zones = fields
        .iter()
        .map(|f| {
            let name = f.zone.as_deref().unwrap_or(DEFAULT_SOURCE_ZONE);
            TimeZone::get(name).map_err(|_| format!("{}: unknown time zone {name}", f.name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut builders: Vec<Builder> = fields
        .iter()
        .map(|f| Builder::new(f.field_type, documents.len()))
        .collect();
    for (row, document) in documents.iter().enumerate() {
        if !document.is_object() {
            return Err(format!("row {row}: {document} is not an object"));
        }
        for ((field, builder), zone) in fields.iter().zip(&mut builders).zip(&zones) {
            match lookup(document, field.path()) {
                None => builder.null(),
                Some(value) => append(builder, field, zone, value).map_err(|why| {
                    let at = key.and_then(|k| lookup(document, k)).map_or_else(
                        || format!("row {row}"),
                        |v| format!("row with {} = {v}", key.unwrap_or_default()),
                    );
                    format!("{at}, {}: {value} {why}", field.path())
                })?,
            }
        }
    }
    let schema = Arc::new(Schema::new(
        fields
            .iter()
            .map(|f| Field::new(&f.name, f.field_type.arrow(), true))
            .collect::<Vec<_>>(),
    ));
    let arrays = builders
        .into_iter()
        .zip(fields)
        .map(|(b, f)| b.finish(f.field_type))
        .collect::<Result<Vec<_>, _>>()?;
    RecordBatch::try_new(schema, arrays).map_err(|e| e.to_string())
}

/// Converts text values, as a database returns them, into a column of the
/// field's declared type, with the same rules as JSON values. Used where a source
/// must fetch a column as text to keep it exact, such as Oracle's `NUMBER`
/// without a declared precision.
///
/// # Errors
///
/// Returns a message naming the row and the value that does not fit.
pub fn typed_from_text<'a>(
    field: &JsonField,
    texts: impl Iterator<Item = Option<&'a str>>,
) -> Result<ArrayRef, String> {
    let zone_name = field.zone.as_deref().unwrap_or(DEFAULT_SOURCE_ZONE);
    let zone = TimeZone::get(zone_name)
        .map_err(|_| format!("{}: unknown time zone {zone_name}", field.name))?;
    let (lower, upper) = texts.size_hint();
    let mut builder = Builder::new(field.field_type, upper.unwrap_or(lower));
    for (row, text) in texts.enumerate() {
        match text {
            None => builder.null(),
            Some(t) => {
                let value = match field.field_type {
                    FieldType::Boolean => match t.trim().to_ascii_lowercase().as_str() {
                        "1" | "true" => Value::Bool(true),
                        "0" | "false" => Value::Bool(false),
                        _ => Value::String(t.to_owned()),
                    },
                    // Databases put a space before an offset; only times need it gone.
                    FieldType::Timestamp => Value::String(normalize_offset(t)),
                    _ => Value::String(t.to_owned()),
                };
                append(&mut builder, field, &zone, &value)
                    .map_err(|why| format!("row {row}, {}: {t:?} {why}", field.name))?;
            }
        }
    }
    builder.finish(field.field_type)
}

/// Databases write offsets after a space, as in `2026-11-01 05:30:00.0000000 +00:00`;
/// RFC 3339 has none there.
fn normalize_offset(text: &str) -> String {
    let t = text.trim();
    match t.rfind([' ']) {
        Some(i) if t[i + 1..].starts_with(['+', '-']) && t[i + 1..].contains(':') => {
            format!("{}{}", &t[..i], &t[i + 1..])
        }
        _ => t.to_owned(),
    }
}

/// Turns wall-clock times, stored as microseconds read as UTC, into instants in
/// `zone`, refusing the spring gap and, unless told otherwise, the autumn fold.
///
/// # Errors
///
/// Returns a message naming the row of a time that does not resolve.
pub fn instants_from_wall_clock(
    column: &str,
    micros: impl Iterator<Item = Option<i64>>,
    zone: &str,
    ambiguous: Ambiguous,
) -> Result<ArrayRef, String> {
    let tz = TimeZone::get(zone).map_err(|_| format!("{column}: unknown time zone {zone}"))?;
    let mut builder = TimestampMicrosecondBuilder::new();
    for (row, value) in micros.enumerate() {
        match value {
            None => builder.append_null(),
            Some(us) => {
                let civil = jiff::Timestamp::from_microsecond(us)
                    .map_err(|e| format!("row {row}, {column}: {e}"))?
                    .to_zoned(TimeZone::UTC)
                    .datetime();
                builder.append_value(
                    resolve_civil(civil, &tz, ambiguous)
                        .map_err(|why| format!("row {row}, {column}: {civil} {why}"))?,
                );
            }
        }
    }
    Ok(Arc::new(builder.finish().with_timezone("UTC")))
}
