//! Helpers shared by the integration tests.

#![allow(dead_code, missing_docs)]

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal128Type, Float64Type, Int64Type, TimestampMicrosecondType,
};
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, TimeUnit};

/// FNV-1a over every value, row by row, at the bit level. Splitting the same rows
/// into different batches gives the same hash, and NaN payloads and the sign of zero
/// count.
pub struct RowHasher(u64);

impl Default for RowHasher {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl RowHasher {
    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn update(&mut self, batch: &RecordBatch) {
        for row in 0..batch.num_rows() {
            for column in batch.columns() {
                if column.is_null(row) {
                    self.bytes(&[0]);
                    continue;
                }
                self.bytes(&[1]);
                match column.data_type() {
                    DataType::Int64 => {
                        self.bytes(&column.as_primitive::<Int64Type>().value(row).to_le_bytes());
                    }
                    DataType::Date32 => {
                        self.bytes(&column.as_primitive::<Date32Type>().value(row).to_le_bytes());
                    }
                    DataType::Float64 => self.bytes(
                        &column
                            .as_primitive::<Float64Type>()
                            .value(row)
                            .to_bits()
                            .to_le_bytes(),
                    ),
                    DataType::Decimal128(_, _) => self.bytes(
                        &column
                            .as_primitive::<Decimal128Type>()
                            .value(row)
                            .to_le_bytes(),
                    ),
                    DataType::Timestamp(TimeUnit::Microsecond, _) => self.bytes(
                        &column
                            .as_primitive::<TimestampMicrosecondType>()
                            .value(row)
                            .to_le_bytes(),
                    ),
                    DataType::Utf8 => {
                        let s = column.as_string::<i32>().value(row);
                        self.bytes(&(s.len() as u64).to_le_bytes());
                        self.bytes(s.as_bytes());
                    }
                    other => panic!("RowHasher does not handle {other}"),
                }
            }
        }
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

pub fn hash(batch: &RecordBatch) -> u64 {
    let mut hasher = RowHasher::default();
    hasher.update(batch);
    hasher.finish()
}

pub fn column<'a>(batch: &'a RecordBatch, name: &str) -> &'a dyn Array {
    batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("no column {name}"))
        .as_ref()
}

pub fn i64s(batch: &RecordBatch, name: &str) -> Vec<Option<i64>> {
    let c = column(batch, name);
    match c.data_type() {
        DataType::Int64 => c.as_primitive::<Int64Type>().iter().collect(),
        DataType::Timestamp(TimeUnit::Microsecond, _) => c
            .as_primitive::<TimestampMicrosecondType>()
            .iter()
            .collect(),
        other => panic!("{name} is {other}, not a 64-bit integer column"),
    }
}

pub fn i32s(batch: &RecordBatch, name: &str) -> Vec<Option<i32>> {
    column(batch, name)
        .as_primitive::<Date32Type>()
        .iter()
        .collect()
}

pub fn f64s(batch: &RecordBatch, name: &str) -> Vec<Option<f64>> {
    column(batch, name)
        .as_primitive::<Float64Type>()
        .iter()
        .collect()
}

pub fn decimals(batch: &RecordBatch, name: &str) -> Vec<i128> {
    let c = column(batch, name).as_primitive::<Decimal128Type>();
    assert_eq!(c.null_count(), 0, "{name} has nulls");
    c.values().to_vec()
}

pub fn strings<'a>(batch: &'a RecordBatch, name: &str) -> Vec<Option<&'a str>> {
    column(batch, name).as_string::<i32>().iter().collect()
}
