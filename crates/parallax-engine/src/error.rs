//! Errors for loads and view configurations.

use arrow_schema::DataType;

/// A load that was refused. A refused load never changes the table.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LoadError {
    /// The table's schema has a column type the engine does not support.
    #[error("column {column} has unsupported type {data_type}")]
    UnsupportedType {
        /// Column name.
        column: String,
        /// The unsupported Arrow type.
        data_type: DataType,
    },
    /// The primary key names a column the schema does not have.
    #[error("primary key column {0} is not in the schema")]
    UnknownKeyColumn(String),
    /// A column the table has is missing from the load.
    #[error("column {column} is missing from the load")]
    MissingColumn {
        /// Column name.
        column: String,
    },
    /// The load has a column the table does not.
    #[error("column {column} is not in the table")]
    UnexpectedColumn {
        /// Column name.
        column: String,
    },
    /// A column's type differs from the table's.
    #[error("column {column} changed type from {expected} to {found}")]
    TypeChanged {
        /// Column name.
        column: String,
        /// The table's type.
        expected: DataType,
        /// The type in the load.
        found: DataType,
    },
    /// A nanosecond timestamp is not a whole number of microseconds.
    #[error("column {column} row {row}: timestamp has precision finer than a microsecond")]
    LossyTimestamp {
        /// Column name.
        column: String,
        /// Row within its batch.
        row: usize,
    },
    /// The load repeats primary keys and the table does not allow it.
    #[error("column {column} has {count} duplicate primary keys, for example {examples:?}")]
    DuplicateKeys {
        /// The key column.
        column: String,
        /// How many rows repeat an earlier key.
        count: usize,
        /// Up to five repeated keys.
        examples: Vec<String>,
    },
    /// A primary key is null.
    #[error("column {column} row {row}: primary key is null")]
    NullKey {
        /// The key column.
        column: String,
        /// Row within the load.
        row: usize,
    },
}

/// A view configuration that cannot be computed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A column the configuration names is not in the table.
    #[error("column {0} is not in the table")]
    UnknownColumn(String),
    /// An aggregate does not apply to the column's type.
    #[error("{aggregate} does not apply to column {column} of type {data_type}")]
    AggregateType {
        /// The aggregate.
        aggregate: String,
        /// Column name.
        column: String,
        /// The column's Arrow type.
        data_type: String,
    },
    /// A filter value does not match the column's type.
    #[error("filter on {column}: {reason}")]
    Filter {
        /// Column name.
        column: String,
        /// What is wrong.
        reason: String,
    },
    /// A sort, top-N or percentage refers to a measure that does not exist.
    #[error("measure {0} does not exist")]
    UnknownMeasure(usize),
    /// A percentile is outside `[0, 1]`.
    #[error("percentile {0} is outside 0..=1")]
    Percentile(String),
    /// Column pivots produced more columns than the view allows.
    #[error("column pivots produce {0} columns, more than the limit of {1}")]
    TooManyColumns(usize, usize),
    /// A display format cannot be used.
    #[error("format of {column}: {reason}")]
    Format {
        /// The column or measure.
        column: String,
        /// What is wrong.
        reason: String,
    },
}
