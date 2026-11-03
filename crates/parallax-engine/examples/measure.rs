//! Times a load and a few views over generated positions.
//!
//! Run with `cargo run --release -p parallax-engine --example measure -- 1000000`.

use std::time::Instant;

use parallax_datagen::Dataset;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_engine::view::{GroupOrder, GroupSort, Sort};
use parallax_engine::{Aggregate, Measure, Table, TableOptions, View, ViewConfig};

fn main() {
    let rows: u64 = std::env::args()
        .nth(1)
        .map_or(1_000_000, |s| s.parse().unwrap());
    let dataset = Positions::new(PositionsOptions {
        rows,
        ..PositionsOptions::default()
    })
    .unwrap();
    let started = Instant::now();
    let batches: Vec<_> = parallax_datagen::batch_ranges(rows, 65_536)
        .into_iter()
        .map(|r| dataset.batch(r))
        .collect();
    println!(
        "generate {rows} rows: {:.2} s",
        started.elapsed().as_secs_f64()
    );

    let mut table = Table::new(
        &batches[0].schema(),
        TableOptions {
            primary_key: Some("position_id".into()),
            ..TableOptions::default()
        },
    )
    .unwrap();
    let started = Instant::now();
    table.load(&batches).unwrap();
    println!("load: {:.2} s", started.elapsed().as_secs_f64());
    drop(batches);

    let pivot = ViewConfig {
        group_by: vec![
            "fund".into(),
            "strategy".into(),
            "sector".into(),
            "ticker".into(),
        ],
        split_by: vec!["asset_class".into()],
        measures: vec![
            Measure::new(Aggregate::Sum, "market_value_usd"),
            Measure::new(Aggregate::Sum, "daily_pnl_usd"),
            Measure::new(Aggregate::Sum, "weight_in_fund"),
        ],
        group_sort: GroupSort {
            by: GroupOrder::Measure(0),
            descending: true,
        },
        ..ViewConfig::default()
    };
    let started = Instant::now();
    let mut view = View::new(&table, pivot).unwrap();
    let built = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let n = view.num_rows(&table);
    let frame = view.window(&table, 0..60, 0..15);
    println!(
        "pivot fund > strategy > sector > ticker by asset class: build {built:.2} s, first window {:.3} s ({n} visible rows, {} columns)",
        started.elapsed().as_secs_f64(),
        frame.total_columns
    );

    let started = Instant::now();
    let mut flat = View::new(
        &table,
        ViewConfig {
            sort: vec![Sort {
                column: "market_value_usd".into(),
                descending: true,
            }],
            ..ViewConfig::default()
        },
    )
    .unwrap();
    let built = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let _ = flat.window(&table, 0..60, 0..40);
    println!(
        "flat sorted by market value: build {built:.2} s, window {:.3} s",
        started.elapsed().as_secs_f64()
    );
}
