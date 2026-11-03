//! Helpers for the engine's differential tests.

#![allow(dead_code, missing_docs)]

use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, Decimal128Type, Float64Type, Int64Type};
use arrow_array::{Array, ArrayRef, Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use duckdb::Connection;
use parallax_datagen::Dataset;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_engine::{Scalar, Table, TableOptions};

/// A small positions table, so that groups collide often.
pub fn positions(rows: u64, securities: usize, hostile: bool, seed: u64) -> RecordBatch {
    Positions::new(PositionsOptions {
        seed,
        rows,
        securities,
        hostile,
        ..PositionsOptions::default()
    })
    .unwrap()
    .batch(0..rows)
}

/// Loads `batch` into a new engine table keyed by `position_id`.
pub fn engine_table(batch: &RecordBatch) -> Table {
    let mut table = Table::new(
        &batch.schema(),
        TableOptions {
            primary_key: Some("position_id".into()),
            ..TableOptions::default()
        },
    )
    .unwrap();
    table.load(std::slice::from_ref(batch)).unwrap();
    table
}

/// Replaces NaN with null, as the engine does on load.
pub fn nan_to_null(batch: &RecordBatch) -> RecordBatch {
    let columns: Vec<ArrayRef> = batch
        .columns()
        .iter()
        .map(|c| match c.data_type() {
            DataType::Float64 => {
                let a = c.as_primitive::<Float64Type>();
                Arc::new(
                    a.iter()
                        .map(|v| v.filter(|x| !x.is_nan()))
                        .collect::<Float64Array>(),
                ) as ArrayRef
            }
            _ => Arc::clone(c),
        })
        .collect();
    RecordBatch::try_new(batch.schema(), columns).unwrap()
}

fn sql_type(data_type: &DataType) -> String {
    match data_type {
        DataType::Boolean => "BOOLEAN".into(),
        DataType::Int64 => "BIGINT".into(),
        DataType::Float64 => "DOUBLE".into(),
        DataType::Decimal128(p, s) => format!("DECIMAL({p}, {s})"),
        DataType::Utf8 => "VARCHAR".into(),
        DataType::Date32 => "DATE".into(),
        DataType::Timestamp(TimeUnit::Microsecond, Some(_)) => "TIMESTAMPTZ".into(),
        DataType::Timestamp(TimeUnit::Microsecond, None) => "TIMESTAMP".into(),
        other => panic!("no DuckDB type for {other}"),
    }
}

/// Opens DuckDB with `batch` as table `t`, plus `__seq`, the row's load order.
pub fn duck(batch: &RecordBatch) -> Connection {
    let batch = nan_to_null(batch);
    let conn = Connection::open_in_memory().unwrap();
    let mut ddl: Vec<String> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| format!("\"{}\" {}", f.name(), sql_type(f.data_type())))
        .collect();
    ddl.push("__seq BIGINT".into());
    conn.execute_batch(&format!("CREATE TABLE t ({})", ddl.join(", ")))
        .unwrap();
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    fields.push(Field::new("__seq", DataType::Int64, false));
    let mut columns = batch.columns().to_vec();
    columns.push(Arc::new(Int64Array::from_iter_values(
        0..batch.num_rows() as i64,
    )));
    let with_seq = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap();
    let mut appender = conn.appender("t").unwrap();
    appender.append_record_batch(with_seq).unwrap();
    appender.flush().unwrap();
    drop(appender);
    conn
}

/// Runs a query and returns its batches.
pub fn query(conn: &Connection, sql: &str) -> Vec<RecordBatch> {
    let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{e}\n{sql}"));
    stmt.query_arrow([]).unwrap().collect()
}

/// Reads one value of a DuckDB result as an engine scalar.
pub fn scalar_at(array: &dyn Array, i: usize) -> Scalar {
    if array.is_null(i) {
        return Scalar::Null;
    }
    match array.data_type() {
        DataType::Boolean => Scalar::Boolean(array.as_boolean().value(i)),
        DataType::Int64 => Scalar::Int64(array.as_primitive::<Int64Type>().value(i)),
        DataType::Int32 => Scalar::Int64(i64::from(
            array
                .as_primitive::<arrow_array::types::Int32Type>()
                .value(i),
        )),
        DataType::Float64 => Scalar::Float64(array.as_primitive::<Float64Type>().value(i)),
        DataType::Decimal128(_, _) => {
            Scalar::Decimal(array.as_primitive::<Decimal128Type>().value(i))
        }
        DataType::Utf8 => Scalar::Utf8(array.as_string::<i32>().value(i).to_owned()),
        DataType::LargeUtf8 => Scalar::Utf8(array.as_string::<i64>().value(i).to_owned()),
        DataType::Utf8View => Scalar::Utf8(array.as_string_view().value(i).to_owned()),
        DataType::Date32 => Scalar::Date32(array.as_primitive::<Date32Type>().value(i)),
        other => panic!("scalar_at does not handle {other}"),
    }
}

/// Reads a DuckDB numeric result as a double.
pub fn number_at(array: &dyn Array, i: usize) -> Option<f64> {
    if array.is_null(i) {
        return None;
    }
    Some(match array.data_type() {
        DataType::Int64 => array.as_primitive::<Int64Type>().value(i) as f64,
        DataType::Float64 => array.as_primitive::<Float64Type>().value(i),
        DataType::Decimal128(_, s) => {
            array.as_primitive::<Decimal128Type>().value(i) as f64 / 10f64.powi(i32::from(*s))
        }
        DataType::Decimal256(_, s) => {
            let v = array
                .as_primitive::<arrow_array::types::Decimal256Type>()
                .value(i);
            v.to_string().parse::<f64>().unwrap() / 10f64.powi(i32::from(*s))
        }
        other => panic!("number_at does not handle {other}"),
    })
}

/// Shewchuk's exactly rounded summation, ported from CPython's `math.fsum`.
pub fn fsum(values: &[f64]) -> f64 {
    if values.iter().any(|v| v.is_infinite()) {
        let pos = values.contains(&f64::INFINITY);
        let neg = values.contains(&f64::NEG_INFINITY);
        return match (pos, neg) {
            (true, true) => f64::NAN,
            (true, false) => f64::INFINITY,
            _ => f64::NEG_INFINITY,
        };
    }
    let mut partials: Vec<f64> = Vec::new();
    for &value in values {
        let mut x = value;
        let mut i = 0;
        for j in 0..partials.len() {
            let mut y = partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                partials[i] = lo;
                i += 1;
            }
            x = hi;
        }
        partials.truncate(i);
        partials.push(x);
    }
    let mut n = partials.len();
    if n == 0 {
        return 0.0;
    }
    n -= 1;
    let mut hi = partials[n];
    let mut lo = 0.0;
    while n > 0 {
        let x = hi;
        n -= 1;
        let y = partials[n];
        hi = x + y;
        let yr = hi - x;
        lo = y - yr;
        if lo != 0.0 {
            break;
        }
    }
    if n > 0 && ((lo < 0.0 && partials[n - 1] < 0.0) || (lo > 0.0 && partials[n - 1] > 0.0)) {
        let y = lo * 2.0;
        let x = hi + y;
        let yr = x - hi;
        if y == yr {
            hi = x;
        }
    }
    hi
}

/// A small deterministic random source for generating configurations.
pub struct Picker(u64);

impl Picker {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// Picks `k` distinct items.
    pub fn sample<T: Clone>(&mut self, items: &[T], k: usize) -> Vec<T> {
        let mut pool: Vec<T> = items.to_vec();
        let mut out = Vec::new();
        for _ in 0..k.min(pool.len()) {
            let i = self.below(pool.len());
            out.push(pool.swap_remove(i));
        }
        out
    }
}
