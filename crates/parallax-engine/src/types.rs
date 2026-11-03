//! Column types, scalar values, and how Arrow types map onto them.

use std::sync::Arc;

use arrow_schema::{DataType, TimeUnit};

/// The type of a table column. Arrow types are normalized to these on load, and
/// only widenings that cannot change a value are accepted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ColumnType {
    /// `true` or `false`.
    Boolean,
    /// A signed 64-bit integer. Narrower signed and unsigned integers widen to it.
    Int64,
    /// A double. `Float32` widens to it.
    Float64,
    /// An exact decimal, stored as an `i128` count of `10^-scale`.
    Decimal {
        /// Total significant digits, at most 38.
        precision: u8,
        /// Digits after the decimal point.
        scale: i8,
    },
    /// UTF-8 text, dictionary-encoded inside the engine.
    Utf8,
    /// Days since the Unix epoch.
    Date32,
    /// Microseconds since the Unix epoch. With a zone the value is an instant; without
    /// one it is a wall-clock time.
    Timestamp {
        /// The Arrow time-zone string, if any.
        timezone: Option<Arc<str>>,
    },
}

impl ColumnType {
    /// Maps an Arrow type to a column type, or `None` if the engine does not support
    /// it.
    #[must_use]
    pub fn from_arrow(data_type: &DataType) -> Option<Self> {
        Some(match data_type {
            DataType::Boolean => Self::Boolean,
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32 => Self::Int64,
            DataType::Float32 | DataType::Float64 => Self::Float64,
            // A negative scale such as Oracle's NUMBER(10, -2) becomes scale 0 with
            // more digits, which is exact; values are multiplied on load.
            DataType::Decimal128(precision, scale) if *scale < 0 => {
                let precision = u32::from(*precision) + scale.unsigned_abs() as u32;
                if precision > 38 {
                    return None;
                }
                Self::Decimal {
                    precision: precision as u8,
                    scale: 0,
                }
            }
            DataType::Decimal128(precision, scale) => Self::Decimal {
                precision: *precision,
                scale: *scale,
            },
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => Self::Utf8,
            DataType::Dictionary(_, value) if Self::from_arrow(value) == Some(Self::Utf8) => {
                Self::Utf8
            }
            DataType::Date32 => Self::Date32,
            DataType::Timestamp(_, timezone) => Self::Timestamp {
                timezone: timezone.clone(),
            },
            _ => return None,
        })
    }

    /// The Arrow type the engine produces for this column.
    #[must_use]
    pub fn to_arrow(&self) -> DataType {
        match self {
            Self::Boolean => DataType::Boolean,
            Self::Int64 => DataType::Int64,
            Self::Float64 => DataType::Float64,
            Self::Decimal { precision, scale } => DataType::Decimal128(*precision, *scale),
            Self::Utf8 => DataType::Utf8,
            Self::Date32 => DataType::Date32,
            Self::Timestamp { timezone } => {
                DataType::Timestamp(TimeUnit::Microsecond, timezone.clone())
            }
        }
    }

    /// Returns `true` for types that sums and means apply to.
    #[must_use]
    pub const fn is_numeric(&self) -> bool {
        matches!(self, Self::Int64 | Self::Float64 | Self::Decimal { .. })
    }
}

/// A single value, as used in filters and returned to callers.
#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    /// No value.
    Null,
    /// A boolean.
    Boolean(bool),
    /// An integer.
    Int64(i64),
    /// A double.
    Float64(f64),
    /// A decimal's unscaled value. Its scale is the column's.
    Decimal(i128),
    /// Text.
    Utf8(String),
    /// Days since the Unix epoch.
    Date32(i32),
    /// Microseconds since the Unix epoch.
    Timestamp(i64),
}

/// A compact, hashable value used for grouping, keys and change detection.
/// Strings are dictionary codes, so two keys from different dictionaries must not
/// be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Null,
    Bool(bool),
    Int(i64),
    /// Float bits with negative zero folded into positive zero.
    Float(u64),
    Decimal(i128),
    Str(u32),
    Date(i32),
    Ts(i64),
}

impl Key {
    pub(crate) fn float(value: f64) -> Self {
        Self::Float(if value == 0.0 { 0 } else { value.to_bits() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widens_only_losslessly() {
        assert_eq!(
            ColumnType::from_arrow(&DataType::Int32),
            Some(ColumnType::Int64)
        );
        assert_eq!(
            ColumnType::from_arrow(&DataType::UInt32),
            Some(ColumnType::Int64)
        );
        assert_eq!(ColumnType::from_arrow(&DataType::UInt64), None);
        assert_eq!(
            ColumnType::from_arrow(&DataType::Float32),
            Some(ColumnType::Float64)
        );
        assert_eq!(
            ColumnType::from_arrow(&DataType::Dictionary(
                Box::new(DataType::Int32),
                Box::new(DataType::Utf8)
            )),
            Some(ColumnType::Utf8)
        );
        assert_eq!(ColumnType::from_arrow(&DataType::Date64), None);
    }

    #[test]
    fn negative_decimal_scales_become_exact_integers() {
        assert_eq!(
            ColumnType::from_arrow(&DataType::Decimal128(10, -2)),
            Some(ColumnType::Decimal {
                precision: 12,
                scale: 0
            })
        );
        assert_eq!(ColumnType::from_arrow(&DataType::Decimal128(38, -1)), None);
    }

    #[test]
    fn negative_zero_keys_equal_zero() {
        assert_eq!(Key::float(-0.0), Key::float(0.0));
        assert_ne!(Key::float(1.0), Key::float(-1.0));
    }
}
