//! Typed, slot-addressed column storage.
//!
//! A column holds one value per table slot. Slots are stable: an update overwrites a
//! slot in place and a delete frees it for reuse. Text is stored as dictionary codes.

use std::cmp::Ordering;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal128Type, Float32Type, Float64Type, Int8Type, Int16Type, Int32Type,
    Int64Type, TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
    TimestampSecondType, UInt8Type, UInt16Type, UInt32Type,
};
use arrow_array::{
    Array, ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int64Array,
    StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, TimeUnit};

use crate::bitmap::Bitmap;
use crate::dictionary::Dictionary;
use crate::error::LoadError;
use crate::types::{ColumnType, Key, Scalar};

/// The values of a column, one per slot. Invalid (null) slots hold a placeholder.
#[derive(Clone, Debug)]
pub(crate) enum Values {
    Boolean(Vec<bool>),
    Int64(Vec<i64>),
    Float64(Vec<f64>),
    Decimal(Vec<i128>),
    Utf8 {
        codes: Vec<u32>,
        dictionary: Dictionary,
    },
    Date32(Vec<i32>),
    Timestamp(Vec<i64>),
}

/// One column of a table, or of a batch being loaded.
#[derive(Clone, Debug)]
pub struct Column {
    pub(crate) ty: ColumnType,
    pub(crate) values: Values,
    pub(crate) valid: Bitmap,
}

impl Column {
    /// Returns an empty column of type `ty`.
    #[must_use]
    pub fn new(ty: ColumnType) -> Self {
        let values = match ty {
            ColumnType::Boolean => Values::Boolean(Vec::new()),
            ColumnType::Int64 => Values::Int64(Vec::new()),
            ColumnType::Float64 => Values::Float64(Vec::new()),
            ColumnType::Decimal { .. } => Values::Decimal(Vec::new()),
            ColumnType::Utf8 => Values::Utf8 {
                codes: Vec::new(),
                dictionary: Dictionary::new(),
            },
            ColumnType::Date32 => Values::Date32(Vec::new()),
            ColumnType::Timestamp { .. } => Values::Timestamp(Vec::new()),
        };
        Self {
            ty,
            values,
            valid: Bitmap::new(),
        }
    }

    /// The column's type.
    #[must_use]
    pub fn column_type(&self) -> &ColumnType {
        &self.ty
    }

    /// The number of slots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.valid.len()
    }

    /// Returns `true` if the column has no slots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.valid.is_empty()
    }

    /// Returns `true` if `slot` holds a value rather than null.
    #[must_use]
    pub fn is_valid(&self, slot: usize) -> bool {
        self.valid.get(slot)
    }

    pub(crate) fn dictionary(&self) -> Option<&Dictionary> {
        match &self.values {
            Values::Utf8 { dictionary, .. } => Some(dictionary),
            _ => None,
        }
    }

    pub(crate) fn refresh_ranks(&mut self) {
        if let Values::Utf8 { dictionary, .. } = &mut self.values {
            dictionary.refresh_ranks();
        }
    }

    /// Returns the key of `slot`, for grouping and change detection.
    pub(crate) fn key(&self, slot: usize) -> Key {
        if !self.valid.get(slot) {
            return Key::Null;
        }
        match &self.values {
            Values::Boolean(v) => Key::Bool(v[slot]),
            Values::Int64(v) => Key::Int(v[slot]),
            Values::Float64(v) => Key::float(v[slot]),
            Values::Decimal(v) => Key::Decimal(v[slot]),
            Values::Utf8 { codes, .. } => Key::Str(codes[slot]),
            Values::Date32(v) => Key::Date(v[slot]),
            Values::Timestamp(v) => Key::Ts(v[slot]),
        }
    }

    /// Returns the value of `slot`.
    #[must_use]
    pub fn scalar(&self, slot: usize) -> Scalar {
        self.key_to_scalar(self.key(slot))
    }

    pub(crate) fn key_to_scalar(&self, key: Key) -> Scalar {
        match key {
            Key::Null => Scalar::Null,
            Key::Bool(b) => Scalar::Boolean(b),
            Key::Int(i) => Scalar::Int64(i),
            Key::Float(bits) => Scalar::Float64(f64::from_bits(bits)),
            Key::Decimal(d) => Scalar::Decimal(d),
            Key::Str(code) => Scalar::Utf8(
                self.dictionary()
                    .expect("string keys come from text columns")
                    .get(code)
                    .to_owned(),
            ),
            Key::Date(d) => Scalar::Date32(d),
            Key::Ts(t) => Scalar::Timestamp(t),
        }
    }

    /// Returns the string of a text key in this column.
    pub(crate) fn text(&self, key: Key) -> Option<&str> {
        match key {
            Key::Str(code) => self.dictionary().map(|d| d.get(code)),
            _ => None,
        }
    }

    /// Compares two keys of this column in display order. Nulls sort last.
    pub(crate) fn cmp_keys(&self, a: Key, b: Key) -> Ordering {
        match (a, b) {
            (Key::Null, Key::Null) => Ordering::Equal,
            (Key::Null, _) => Ordering::Greater,
            (_, Key::Null) => Ordering::Less,
            (Key::Bool(x), Key::Bool(y)) => x.cmp(&y),
            (Key::Int(x), Key::Int(y)) => x.cmp(&y),
            (Key::Float(x), Key::Float(y)) => f64::from_bits(x).total_cmp(&f64::from_bits(y)),
            (Key::Decimal(x), Key::Decimal(y)) => x.cmp(&y),
            (Key::Str(x), Key::Str(y)) => {
                let d = self.dictionary().expect("text column");
                d.rank(x).cmp(&d.rank(y))
            }
            (Key::Date(x), Key::Date(y)) => x.cmp(&y),
            (Key::Ts(x), Key::Ts(y)) => x.cmp(&y),
            _ => unreachable!("keys of one column share a type"),
        }
    }

    /// Returns `slot` as a double, for floating-point aggregates.
    pub(crate) fn f64(&self, slot: usize) -> Option<f64> {
        if !self.valid.get(slot) {
            return None;
        }
        match &self.values {
            Values::Float64(v) => Some(v[slot]),
            Values::Int64(v) => Some(v[slot] as f64),
            _ => None,
        }
    }

    /// Returns `slot` as an unscaled integer: the value of an integer column, or the
    /// unscaled value of a decimal column.
    pub(crate) fn exact(&self, slot: usize) -> Option<i128> {
        if !self.valid.get(slot) {
            return None;
        }
        match &self.values {
            Values::Int64(v) => Some(i128::from(v[slot])),
            Values::Decimal(v) => Some(v[slot]),
            _ => None,
        }
    }

    /// Appends a null.
    pub(crate) fn push_null(&mut self) {
        match &mut self.values {
            Values::Boolean(v) => v.push(false),
            Values::Int64(v) => v.push(0),
            Values::Float64(v) => v.push(0.0),
            Values::Decimal(v) => v.push(0),
            Values::Utf8 { codes, .. } => codes.push(0),
            Values::Date32(v) => v.push(0),
            Values::Timestamp(v) => v.push(0),
        }
        self.valid.push(false);
    }

    /// Copies `source[source_slot]` into `self[slot]`, re-interning text.
    pub(crate) fn set_from(&mut self, slot: usize, source: &Self, source_slot: usize) {
        let valid = source.valid.get(source_slot);
        self.valid.set(slot, valid);
        if !valid {
            return;
        }
        match (&mut self.values, &source.values) {
            (Values::Boolean(d), Values::Boolean(s)) => d[slot] = s[source_slot],
            (Values::Int64(d), Values::Int64(s)) => d[slot] = s[source_slot],
            (Values::Float64(d), Values::Float64(s)) => d[slot] = s[source_slot],
            (Values::Decimal(d), Values::Decimal(s)) => d[slot] = s[source_slot],
            (Values::Date32(d), Values::Date32(s)) => d[slot] = s[source_slot],
            (Values::Timestamp(d), Values::Timestamp(s)) => d[slot] = s[source_slot],
            (
                Values::Utf8 { codes, dictionary },
                Values::Utf8 {
                    codes: source_codes,
                    dictionary: source_dictionary,
                },
            ) => {
                codes[slot] = dictionary.intern(source_dictionary.get(source_codes[source_slot]));
            }
            _ => unreachable!("columns share a type"),
        }
    }

    /// Appends `source[source_slot]`.
    pub(crate) fn push_from(&mut self, source: &Self, source_slot: usize) {
        self.push_null();
        let slot = self.len() - 1;
        self.set_from(slot, source, source_slot);
    }

    /// Returns `true` if `self[slot]` and `other[other_slot]` hold the same value.
    /// Text is compared by content, so the two columns may have different
    /// dictionaries. Floats compare by bits, so a change in the sign of zero counts.
    pub(crate) fn same_value(&self, slot: usize, other: &Self, other_slot: usize) -> bool {
        let (a, b) = (self.valid.get(slot), other.valid.get(other_slot));
        if !a || !b {
            return a == b;
        }
        match (&self.values, &other.values) {
            (Values::Boolean(x), Values::Boolean(y)) => x[slot] == y[other_slot],
            (Values::Int64(x), Values::Int64(y)) => x[slot] == y[other_slot],
            (Values::Float64(x), Values::Float64(y)) => {
                x[slot].to_bits() == y[other_slot].to_bits()
            }
            (Values::Decimal(x), Values::Decimal(y)) => x[slot] == y[other_slot],
            (Values::Date32(x), Values::Date32(y)) => x[slot] == y[other_slot],
            (Values::Timestamp(x), Values::Timestamp(y)) => x[slot] == y[other_slot],
            (
                Values::Utf8 {
                    codes: x,
                    dictionary: dx,
                },
                Values::Utf8 {
                    codes: y,
                    dictionary: dy,
                },
            ) => dx.get(x[slot]) == dy.get(y[other_slot]),
            _ => false,
        }
    }

    /// Builds an Arrow array of the values at `slots`, in order.
    #[must_use]
    pub fn to_arrow(&self, slots: &[usize]) -> ArrayRef {
        let valid = |s: usize| self.valid.get(s);
        match &self.values {
            Values::Boolean(v) => Arc::new(
                slots
                    .iter()
                    .map(|&s| valid(s).then(|| v[s]))
                    .collect::<BooleanArray>(),
            ),
            Values::Int64(v) => Arc::new(
                slots
                    .iter()
                    .map(|&s| valid(s).then(|| v[s]))
                    .collect::<Int64Array>(),
            ),
            Values::Float64(v) => Arc::new(
                slots
                    .iter()
                    .map(|&s| valid(s).then(|| v[s]))
                    .collect::<Float64Array>(),
            ),
            Values::Decimal(v) => {
                let ColumnType::Decimal { precision, scale } = self.ty else {
                    unreachable!("decimal values have a decimal type")
                };
                Arc::new(
                    slots
                        .iter()
                        .map(|&s| valid(s).then(|| v[s]))
                        .collect::<Decimal128Array>()
                        .with_precision_and_scale(precision, scale)
                        .expect("valid decimal type"),
                )
            }
            Values::Utf8 { codes, dictionary } => Arc::new(
                slots
                    .iter()
                    .map(|&s| valid(s).then(|| dictionary.get(codes[s])))
                    .collect::<StringArray>(),
            ),
            Values::Date32(v) => Arc::new(
                slots
                    .iter()
                    .map(|&s| valid(s).then(|| v[s]))
                    .collect::<Date32Array>(),
            ),
            Values::Timestamp(v) => {
                let array: TimestampMicrosecondArray =
                    slots.iter().map(|&s| valid(s).then(|| v[s])).collect();
                let ColumnType::Timestamp { timezone } = &self.ty else {
                    unreachable!("timestamp values have a timestamp type")
                };
                Arc::new(array.with_timezone_opt(timezone.clone()))
            }
        }
    }

    /// Appends every value of an Arrow array, converting it to this column's type.
    /// Returns how many NaNs became nulls.
    ///
    /// # Errors
    ///
    /// Returns an error if the array's type does not map to this column's type, or a
    /// timestamp has sub-microsecond precision that would be lost.
    pub(crate) fn extend_from_arrow(
        &mut self,
        name: &str,
        array: &dyn Array,
    ) -> Result<usize, LoadError> {
        let found = ColumnType::from_arrow(array.data_type());
        if found.as_ref() != Some(&self.ty) {
            return Err(LoadError::TypeChanged {
                column: name.to_owned(),
                expected: self.ty.to_arrow(),
                found: array.data_type().clone(),
            });
        }
        let mut nans = 0;
        let n = array.len();
        macro_rules! ints {
            ($t:ty) => {{
                let a = array.as_primitive::<$t>();
                let Values::Int64(v) = &mut self.values else {
                    unreachable!()
                };
                for i in 0..n {
                    v.push(i64::from(a.value(i)));
                    self.valid.push(a.is_valid(i));
                }
            }};
        }
        match array.data_type() {
            DataType::Boolean => {
                let a = array.as_boolean();
                let Values::Boolean(v) = &mut self.values else {
                    unreachable!()
                };
                for i in 0..n {
                    v.push(a.value(i));
                    self.valid.push(a.is_valid(i));
                }
            }
            DataType::Int8 => ints!(Int8Type),
            DataType::Int16 => ints!(Int16Type),
            DataType::Int32 => ints!(Int32Type),
            DataType::Int64 => ints!(Int64Type),
            DataType::UInt8 => ints!(UInt8Type),
            DataType::UInt16 => ints!(UInt16Type),
            DataType::UInt32 => ints!(UInt32Type),
            DataType::Float32 | DataType::Float64 => {
                let Values::Float64(v) = &mut self.values else {
                    unreachable!()
                };
                for i in 0..n {
                    let value = if array.data_type() == &DataType::Float32 {
                        f64::from(array.as_primitive::<Float32Type>().value(i))
                    } else {
                        array.as_primitive::<Float64Type>().value(i)
                    };
                    let is_nan = array.is_valid(i) && value.is_nan();
                    nans += usize::from(is_nan);
                    v.push(if is_nan { 0.0 } else { value });
                    self.valid.push(array.is_valid(i) && !is_nan);
                }
            }
            DataType::Decimal128(_, source_scale) => {
                let factor = if *source_scale < 0 {
                    10i128.pow(u32::from(source_scale.unsigned_abs()))
                } else {
                    1
                };
                let a = array.as_primitive::<Decimal128Type>();
                let Values::Decimal(v) = &mut self.values else {
                    unreachable!()
                };
                for i in 0..n {
                    v.push(a.value(i) * factor);
                    self.valid.push(a.is_valid(i));
                }
            }
            DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Utf8View
            | DataType::Dictionary(_, _) => {
                let strings = arrow_cast_strings(array);
                let Values::Utf8 { codes, dictionary } = &mut self.values else {
                    unreachable!()
                };
                for value in strings {
                    match value {
                        Some(s) => {
                            codes.push(dictionary.intern(&s));
                            self.valid.push(true);
                        }
                        None => {
                            codes.push(0);
                            self.valid.push(false);
                        }
                    }
                }
            }
            DataType::Date32 => {
                let a = array.as_primitive::<Date32Type>();
                let Values::Date32(v) = &mut self.values else {
                    unreachable!()
                };
                for i in 0..n {
                    v.push(a.value(i));
                    self.valid.push(a.is_valid(i));
                }
            }
            DataType::Timestamp(unit, _) => {
                let Values::Timestamp(v) = &mut self.values else {
                    unreachable!()
                };
                for i in 0..n {
                    let valid = array.is_valid(i);
                    let micros = match unit {
                        TimeUnit::Second => array
                            .as_primitive::<TimestampSecondType>()
                            .value(i)
                            .checked_mul(1_000_000),
                        TimeUnit::Millisecond => array
                            .as_primitive::<TimestampMillisecondType>()
                            .value(i)
                            .checked_mul(1_000),
                        TimeUnit::Microsecond => {
                            Some(array.as_primitive::<TimestampMicrosecondType>().value(i))
                        }
                        TimeUnit::Nanosecond => {
                            let nanos = array.as_primitive::<TimestampNanosecondType>().value(i);
                            (!valid || nanos % 1_000 == 0).then_some(nanos / 1_000)
                        }
                    };
                    match micros {
                        Some(m) => v.push(m),
                        None if !valid => v.push(0),
                        None => {
                            return Err(LoadError::LossyTimestamp {
                                column: name.to_owned(),
                                row: i,
                            });
                        }
                    }
                    self.valid.push(valid);
                }
            }
            _ => unreachable!("from_arrow accepted the type"),
        }
        Ok(nans)
    }
}

/// Reads any Arrow string array, including dictionary-encoded and view arrays.
fn arrow_cast_strings(array: &dyn Array) -> Vec<Option<String>> {
    match array.data_type() {
        DataType::Utf8 => array
            .as_string::<i32>()
            .iter()
            .map(|s| s.map(str::to_owned))
            .collect(),
        DataType::LargeUtf8 => array
            .as_string::<i64>()
            .iter()
            .map(|s| s.map(str::to_owned))
            .collect(),
        DataType::Utf8View => array
            .as_string_view()
            .iter()
            .map(|s| s.map(str::to_owned))
            .collect(),
        DataType::Dictionary(_, _) => {
            let dictionary = array.as_any_dictionary();
            let values = arrow_cast_strings(dictionary.values().as_ref());
            let keys = dictionary.normalized_keys();
            (0..array.len())
                .map(|i| {
                    if array.is_valid(i) {
                        values[keys[i]].clone()
                    } else {
                        None
                    }
                })
                .collect()
        }
        _ => unreachable!("only string types reach here"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Float32Array, Int32Array, TimestampNanosecondArray};

    #[test]
    fn nan_becomes_null_and_is_counted() {
        let mut c = Column::new(ColumnType::Float64);
        let a = Float64Array::from(vec![Some(1.0), Some(f64::NAN), None, Some(-0.0)]);
        assert_eq!(c.extend_from_arrow("x", &a).unwrap(), 1);
        assert_eq!(c.f64(0), Some(1.0));
        assert_eq!(c.f64(1), None);
        assert_eq!(c.f64(2), None);
        assert_eq!(c.f64(3).map(f64::to_bits), Some((-0.0f64).to_bits()));
    }

    #[test]
    fn narrow_types_widen() {
        let mut ints = Column::new(ColumnType::Int64);
        ints.extend_from_arrow("i", &Int32Array::from(vec![Some(-5), None]))
            .unwrap();
        assert_eq!(ints.exact(0), Some(-5));
        assert_eq!(ints.exact(1), None);
        let mut floats = Column::new(ColumnType::Float64);
        floats
            .extend_from_arrow("f", &Float32Array::from(vec![1.5f32]))
            .unwrap();
        assert_eq!(floats.f64(0), Some(1.5));
    }

    #[test]
    fn type_changes_are_errors() {
        let mut c = Column::new(ColumnType::Int64);
        let err = c
            .extend_from_arrow("qty", &Float64Array::from(vec![1.0]))
            .unwrap_err();
        assert!(matches!(err, LoadError::TypeChanged { ref column, .. } if column == "qty"));
    }

    #[test]
    fn sub_microsecond_timestamps_are_rejected() {
        let ty = ColumnType::Timestamp { timezone: None };
        let mut c = Column::new(ty);
        // The column type comes from the array's own type, so build a matching column.
        let exact = TimestampNanosecondArray::from(vec![1_000, 2_000]);
        let mut ok = Column::new(ColumnType::from_arrow(exact.data_type()).unwrap());
        ok.extend_from_arrow("t", &exact).unwrap();
        assert_eq!(ok.key(1), Key::Ts(2));
        let lossy = TimestampNanosecondArray::from(vec![1_001]);
        assert!(matches!(
            ok.extend_from_arrow("t", &lossy),
            Err(LoadError::LossyTimestamp { row: 0, .. })
        ));
        assert!(c.extend_from_arrow("t", &exact).is_ok());
    }

    #[test]
    fn negative_scale_values_are_scaled_up() {
        let a = Decimal128Array::from(vec![Some(123), None])
            .with_precision_and_scale(10, -2)
            .unwrap();
        let ty = ColumnType::from_arrow(a.data_type()).unwrap();
        let mut c = Column::new(ty);
        c.extend_from_arrow("n", &a).unwrap();
        assert_eq!(c.exact(0), Some(12_300));
        assert_eq!(c.exact(1), None);
    }

    #[test]
    fn round_trips_to_arrow_in_slot_order() {
        let mut c = Column::new(ColumnType::Utf8);
        c.extend_from_arrow("s", &StringArray::from(vec![Some("b"), None, Some("a")]))
            .unwrap();
        let out = c.to_arrow(&[2, 1, 0]);
        let strings: Vec<Option<&str>> = out.as_string::<i32>().iter().collect();
        assert_eq!(strings, vec![Some("a"), None, Some("b")]);
    }
}
