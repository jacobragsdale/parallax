//! Views updated by change sets must equal views rebuilt from scratch.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Decimal128Type, Float64Type, Int64Type};
use arrow_array::{Array, ArrayRef, Decimal128Array, Float64Array, RecordBatch, StringArray};
use common::{Picker, positions};
use parallax_engine::{
    Aggregate, CellValue, Filter, FilterOp, GroupCell, Measure, Table, TableOptions, View,
    ViewConfig,
};

fn table_from(batch: &RecordBatch) -> Table {
    Table::new(
        &batch.schema(),
        TableOptions {
            primary_key: Some("position_id".into()),
            ..TableOptions::default()
        },
    )
    .unwrap()
}

type Snapshot = HashMap<String, (i64, Vec<CellValue>)>;

fn snapshot(cells: Vec<GroupCell>) -> Snapshot {
    cells
        .into_iter()
        .map(|c| (format!("{:?}", (&c.path, &c.split)), (c.rows, c.values)))
        .collect()
}

/// Mutates a batch: some rows change values, some keys disappear, new keys appear.
fn mutate(batch: &RecordBatch, picker: &mut Picker, next_id: &mut i64) -> RecordBatch {
    let n = batch.num_rows();
    let keep: Vec<usize> = (0..n).filter(|_| !picker.chance(8)).collect();
    let extra: Vec<usize> = (0..n / 20).map(|_| picker.below(n)).collect();
    let rows: Vec<usize> = keep.iter().chain(&extra).copied().collect();
    let schema = batch.schema();
    let mut columns: Vec<ArrayRef> = Vec::new();
    for (c, field) in schema.fields().iter().enumerate() {
        let array = batch.column(c);
        let column: ArrayRef = match field.name().as_str() {
            "position_id" => {
                let ids = array.as_primitive::<Int64Type>();
                let mut out = Vec::with_capacity(rows.len());
                for (i, &r) in rows.iter().enumerate() {
                    if i < keep.len() {
                        out.push(ids.value(r));
                    } else {
                        *next_id += 1;
                        out.push(*next_id);
                    }
                }
                Arc::new(arrow_array::Int64Array::from(out))
            }
            "market_value_usd" | "daily_pnl_usd" => {
                let a = array.as_primitive::<Decimal128Type>();
                let values: Decimal128Array = rows
                    .iter()
                    .map(|&r| {
                        let v = a.value(r);
                        Some(if picker.chance(15) {
                            v + picker.below(1_000_000) as i128 - 500_000
                        } else {
                            v
                        })
                    })
                    .collect();
                Arc::new(values.with_data_type(field.data_type().clone()))
            }
            "beta" | "weight_in_fund" => {
                let a = array.as_primitive::<Float64Type>();
                let values: Float64Array = rows
                    .iter()
                    .map(|&r| {
                        if picker.chance(10) {
                            None
                        } else if picker.chance(15) {
                            Some(a.value(r) * 1.5 + 1e-9)
                        } else {
                            a.is_valid(r).then(|| a.value(r))
                        }
                    })
                    .collect();
                Arc::new(values)
            }
            "sector" => {
                let a = array.as_string::<i32>();
                let values: StringArray = rows
                    .iter()
                    .map(|&r| {
                        if picker.chance(5) {
                            Some(
                                picker
                                    .pick(&["Energy", "Utilities", "New Sector", "énergie"])
                                    .to_string(),
                            )
                        } else {
                            a.is_valid(r).then(|| a.value(r).to_owned())
                        }
                    })
                    .collect();
                Arc::new(values)
            }
            _ => arrow_select_take(array, &rows),
        };
        columns.push(column);
    }
    RecordBatch::try_new(schema, columns).unwrap()
}

fn arrow_select_take(array: &ArrayRef, rows: &[usize]) -> ArrayRef {
    let indices = arrow_array::UInt32Array::from_iter_values(rows.iter().map(|&r| r as u32));
    arrow_select::take::take(array.as_ref(), &indices, None).unwrap()
}

fn configs() -> Vec<ViewConfig> {
    let measures = vec![
        Measure::count_all(),
        Measure::new(Aggregate::Sum, "market_value_usd"),
        Measure::new(Aggregate::Sum, "beta"),
        Measure::new(Aggregate::Mean, "weight_in_fund"),
        Measure::new(Aggregate::Variance, "daily_pnl_usd"),
        Measure::new(Aggregate::StdDev, "beta"),
        Measure::new(Aggregate::Min, "market_value_usd"),
        Measure::new(Aggregate::Max, "beta"),
        Measure::new(Aggregate::Median, "daily_pnl_usd"),
        Measure::new(Aggregate::DistinctCount, "ticker"),
        Measure::new(Aggregate::First, "sector"),
        Measure::new(Aggregate::Last, "beta"),
        Measure::weighted_mean("beta", "market_value_usd"),
        Measure::new(Aggregate::Sum, "market_value_local").with_unit_column("currency"),
    ];
    vec![
        ViewConfig {
            measures: measures.clone(),
            ..ViewConfig::default()
        },
        ViewConfig {
            group_by: vec!["sector".into(), "fund".into()],
            measures: measures.clone(),
            ..ViewConfig::default()
        },
        ViewConfig {
            group_by: vec!["fund".into(), "strategy".into(), "ticker".into()],
            split_by: vec!["asset_class".into()],
            measures: measures.clone(),
            ..ViewConfig::default()
        },
        ViewConfig {
            group_by: vec!["sector".into()],
            split_by: vec!["long_short".into()],
            measures,
            filters: vec![Filter::new("beta", FilterOp::IsNotNull)],
            ..ViewConfig::default()
        },
    ]
}

#[test]
fn applied_changes_equal_a_rebuild() {
    for (seed, hostile) in [(11, false), (12, true)] {
        let mut batch = positions(3_000, 200, hostile, seed);
        let mut table = table_from(&batch);
        table.load(std::slice::from_ref(&batch)).unwrap();
        let mut views: Vec<View> = configs()
            .into_iter()
            .map(|c| View::new(&table, c).unwrap())
            .collect();
        let mut picker = Picker::new(seed);
        let mut next_id = 10_000_000;
        for round in 0..12 {
            batch = mutate(&batch, &mut picker, &mut next_id);
            let (report, change) = if picker.chance(30) {
                table.upsert(std::slice::from_ref(&batch)).unwrap()
            } else {
                table.load(std::slice::from_ref(&batch)).unwrap()
            };
            assert!(
                report.updated + report.inserted + report.deleted > 0,
                "round {round} changed nothing"
            );
            for (i, view) in views.iter_mut().enumerate() {
                view.apply(&table, &change);
                let mut fresh = View::new(&table, view.config().clone()).unwrap();
                let applied = snapshot(view.cells(&table));
                let rebuilt = snapshot(fresh.cells(&table));
                assert_eq!(
                    applied.len(),
                    rebuilt.len(),
                    "seed {seed} round {round} view {i}: node count"
                );
                for (key, value) in &rebuilt {
                    assert_eq!(
                        applied.get(key),
                        Some(value),
                        "seed {seed} round {round} view {i} at {key:?}"
                    );
                }
                assert_eq!(view.load_version(), table.load_version());
            }
        }
    }
}

#[test]
fn a_refused_load_leaves_the_table_and_views_unchanged() {
    let batch = positions(500, 100, false, 13);
    let mut table = table_from(&batch);
    table.load(std::slice::from_ref(&batch)).unwrap();
    let mut view = View::new(
        &table,
        ViewConfig {
            group_by: vec!["fund".into()],
            measures: vec![Measure::new(Aggregate::Sum, "market_value_usd")],
            ..ViewConfig::default()
        },
    )
    .unwrap();
    let before = snapshot(view.cells(&table));
    let version = table.load_version();
    // A duplicate key anywhere refuses the whole load.
    let mut ids: Vec<i64> = batch
        .column(0)
        .as_primitive::<Int64Type>()
        .values()
        .to_vec();
    ids[400] = ids[3];
    let mut columns = batch.columns().to_vec();
    columns[0] = Arc::new(arrow_array::Int64Array::from(ids));
    let bad = RecordBatch::try_new(batch.schema(), columns).unwrap();
    assert!(table.load(&[bad]).is_err());
    assert_eq!(table.load_version(), version);
    assert_eq!(table.num_rows(), 500);
    let mut fresh = View::new(&table, view.config().clone()).unwrap();
    assert_eq!(snapshot(fresh.cells(&table)), before);
    assert_eq!(snapshot(view.cells(&table)), before);
}
