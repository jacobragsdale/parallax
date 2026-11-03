//! Times an Excel and a CSV export of generated positions:
//! `cargo run --release -p parallax-export --example measure -- 1000000`.

use std::time::Instant;

use parallax_datagen::Dataset;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_engine::{Table, TableOptions};
use parallax_export::{ExportSource, write_csv, write_xlsx};
use parallax_protocol::layout::Layout;

fn main() {
    let rows: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1_000_000);
    let batch = Positions::new(PositionsOptions {
        rows,
        ..PositionsOptions::default()
    })
    .unwrap()
    .batch(0..rows);
    let mut table = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    table.load(&[batch]).unwrap();
    let layout = Layout::default();
    let mut file = tempfile::tempfile().unwrap();
    let started = Instant::now();
    let summary = write_xlsx(&table, &layout, &ExportSource::default(), &mut file).unwrap();
    let size = file.metadata().unwrap().len();
    println!(
        "xlsx: {} rows × {} columns in {:.1} s, {:.0} MB",
        summary.rows,
        summary.columns,
        started.elapsed().as_secs_f64(),
        size as f64 / 1e6
    );
    let mut csv = tempfile::tempfile().unwrap();
    let started = Instant::now();
    write_csv(&table, &layout, std::io::BufWriter::new(&mut csv)).unwrap();
    println!(
        "csv: {:.1} s, {:.0} MB",
        started.elapsed().as_secs_f64(),
        csv.metadata().unwrap().len() as f64 / 1e6
    );
}
