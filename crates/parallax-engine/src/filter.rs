//! Row filters with SQL semantics: a comparison with a null value is not true.

use rustc_hash::FxHashSet;

use crate::column::Column;
use crate::dictionary::collate;
use crate::error::ConfigError;
use crate::table::Table;
use crate::types::{ColumnType, Key, Scalar};

/// A condition on one column.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterOp {
    /// Equal to the value.
    Eq(Scalar),
    /// Not equal to the value. Nulls do not match.
    Ne(Scalar),
    /// Less than the value.
    Lt(Scalar),
    /// Less than or equal to the value.
    Le(Scalar),
    /// Greater than the value.
    Gt(Scalar),
    /// Greater than or equal to the value.
    Ge(Scalar),
    /// One of the values.
    In(Vec<Scalar>),
    /// None of the values. Nulls do not match.
    NotIn(Vec<Scalar>),
    /// Null.
    IsNull,
    /// Not null.
    IsNotNull,
    /// Text containing the string, ignoring case.
    Contains(String),
    /// Text starting with the string, ignoring case.
    StartsWith(String),
}

/// A filter on one column. A view keeps rows that pass every filter.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    /// The column tested.
    pub column: String,
    /// The condition.
    pub op: FilterOp,
}

impl Filter {
    /// Builds a filter.
    #[must_use]
    pub fn new(column: &str, op: FilterOp) -> Self {
        Self {
            column: column.to_owned(),
            op,
        }
    }
}

/// A filter value converted to the column's representation.
#[derive(Clone, Debug)]
enum Operand {
    Key(Key),
    Text(String),
}

#[derive(Clone, Debug)]
enum Test {
    Compare(std::cmp::Ordering, bool, Operand),
    In(FxHashSet<Key>, FxHashSet<String>, bool),
    Null(bool),
    Contains(String),
    StartsWith(String),
}

/// A filter checked against rows.
#[derive(Clone, Debug)]
pub(crate) struct CompiledFilter {
    pub column: usize,
    test: Test,
}

fn operand(column: &str, ty: &ColumnType, value: &Scalar) -> Result<Operand, ConfigError> {
    let bad = |reason: &str| ConfigError::Filter {
        column: column.to_owned(),
        reason: reason.to_owned(),
    };
    Ok(match (ty, value) {
        (_, Scalar::Null) => return Err(bad("compare with null using IsNull")),
        (ColumnType::Boolean, Scalar::Boolean(b)) => Operand::Key(Key::Bool(*b)),
        (ColumnType::Int64, Scalar::Int64(i)) => Operand::Key(Key::Int(*i)),
        (ColumnType::Float64, Scalar::Float64(f)) if !f.is_nan() => Operand::Key(Key::float(*f)),
        (ColumnType::Float64, Scalar::Int64(i)) => Operand::Key(Key::float(*i as f64)),
        (ColumnType::Decimal { .. }, Scalar::Decimal(d)) => Operand::Key(Key::Decimal(*d)),
        (ColumnType::Decimal { scale, .. }, Scalar::Int64(i)) => Operand::Key(Key::Decimal(
            i128::from(*i)
                .checked_mul(10i128.pow(*scale as u32))
                .ok_or_else(|| bad("value too large"))?,
        )),
        (ColumnType::Utf8, Scalar::Utf8(s)) => Operand::Text(s.clone()),
        (ColumnType::Date32, Scalar::Date32(d)) => Operand::Key(Key::Date(*d)),
        (ColumnType::Timestamp { .. }, Scalar::Timestamp(t)) => Operand::Key(Key::Ts(*t)),
        _ => return Err(bad(&format!("{value:?} does not match type {ty:?}"))),
    })
}

impl CompiledFilter {
    pub fn compile(table: &Table, filter: &Filter) -> Result<Self, ConfigError> {
        let column = table
            .column_index(&filter.column)
            .ok_or_else(|| ConfigError::UnknownColumn(filter.column.clone()))?;
        let ty = table.column(column).column_type().clone();
        let name = &filter.column;
        use std::cmp::Ordering::{Equal, Greater, Less};
        let test = match &filter.op {
            FilterOp::Eq(v) => Test::Compare(Equal, false, operand(name, &ty, v)?),
            FilterOp::Ne(v) => Test::Compare(Equal, true, operand(name, &ty, v)?),
            FilterOp::Lt(v) => Test::Compare(Less, false, operand(name, &ty, v)?),
            FilterOp::Ge(v) => Test::Compare(Less, true, operand(name, &ty, v)?),
            FilterOp::Gt(v) => Test::Compare(Greater, false, operand(name, &ty, v)?),
            FilterOp::Le(v) => Test::Compare(Greater, true, operand(name, &ty, v)?),
            FilterOp::In(values) | FilterOp::NotIn(values) => {
                let mut keys = FxHashSet::default();
                let mut texts = FxHashSet::default();
                for v in values {
                    match operand(name, &ty, v)? {
                        Operand::Key(k) => {
                            keys.insert(k);
                        }
                        Operand::Text(t) => {
                            texts.insert(t);
                        }
                    }
                }
                Test::In(keys, texts, matches!(filter.op, FilterOp::NotIn(_)))
            }
            FilterOp::IsNull => Test::Null(false),
            FilterOp::IsNotNull => Test::Null(true),
            FilterOp::Contains(s) | FilterOp::StartsWith(s) => {
                if ty != ColumnType::Utf8 {
                    return Err(ConfigError::Filter {
                        column: name.clone(),
                        reason: "text matching needs a text column".into(),
                    });
                }
                if matches!(filter.op, FilterOp::Contains(_)) {
                    Test::Contains(s.to_lowercase())
                } else {
                    Test::StartsWith(s.to_lowercase())
                }
            }
        };
        Ok(Self { column, test })
    }

    /// Returns `true` if row `row` of `column` passes.
    pub fn passes(&self, column: &Column, row: usize) -> bool {
        let key = column.key(row);
        match &self.test {
            Test::Null(negate) => (key == Key::Null) == !*negate,
            _ if key == Key::Null => false,
            Test::Compare(ordering, negate, operand) => {
                let actual = match operand {
                    Operand::Key(k) => column.cmp_keys(key, *k),
                    Operand::Text(t) => collate(column.text(key).expect("text"), t),
                };
                (actual == *ordering) != *negate
            }
            Test::In(keys, texts, negate) => {
                let found = match column.text(key) {
                    Some(t) => texts.contains(t),
                    None => keys.contains(&key),
                };
                found != *negate
            }
            Test::Contains(needle) => column
                .text(key)
                .is_some_and(|t| t.to_lowercase().contains(needle.as_str())),
            Test::StartsWith(prefix) => column
                .text(key)
                .is_some_and(|t| t.to_lowercase().starts_with(prefix.as_str())),
        }
    }
}
