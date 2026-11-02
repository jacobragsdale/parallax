//! Arrow IPC files: round trips and batch-size independence.

mod common;

use std::fs::File;
use std::path::PathBuf;

use arrow_ipc::reader::FileReader;
use common::RowHasher;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_datagen::prices::{Prices, PricesOptions};
use parallax_datagen::{Dataset, write_ipc_file};

fn temp_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn file_hash(path: &PathBuf) -> (u64, usize) {
    let reader = FileReader::try_new(File::open(path).unwrap(), None).unwrap();
    let mut hasher = RowHasher::default();
    let mut rows = 0;
    for batch in reader {
        let batch = batch.unwrap();
        rows += batch.num_rows();
        hasher.update(&batch);
    }
    (hasher.finish(), rows)
}

#[test]
fn files_read_back_identically_whatever_the_batch_size() {
    let dataset = Positions::new(PositionsOptions {
        rows: 10_000,
        hostile: true,
        ..PositionsOptions::default()
    })
    .unwrap();
    let mut expected = RowHasher::default();
    expected.update(&dataset.batch(0..10_000));

    for batch_rows in [1_000, 4_096, 65_536] {
        let path = temp_path(&format!("positions-{batch_rows}.arrow"));
        let summary = write_ipc_file(&dataset, &path, batch_rows).unwrap();
        assert_eq!(summary.rows, 10_000);
        assert_eq!(summary.batches, 10_000u64.div_ceil(batch_rows));
        assert_eq!(file_hash(&path), (expected.finish(), 10_000));
    }
}

#[test]
fn price_files_round_trip() {
    let dataset = Prices::new(PricesOptions {
        tickers: 5,
        hostile: true,
        ..PricesOptions::default()
    })
    .unwrap();
    let mut expected = RowHasher::default();
    expected.update(&dataset.batch(0..dataset.num_rows()));
    let path = temp_path("prices.arrow");
    write_ipc_file(&dataset, &path, 777).unwrap();
    assert_eq!(
        file_hash(&path),
        (expected.finish(), dataset.num_rows() as usize)
    );
}

#[test]
fn a_rejected_write_leaves_no_file_behind() {
    let dataset = Positions::new(PositionsOptions {
        rows: 10,
        ..PositionsOptions::default()
    })
    .unwrap();
    let path = temp_path("zero-batch.arrow");
    let _ = std::fs::remove_file(&path);
    assert!(write_ipc_file(&dataset, &path, 0).is_err());
    assert!(!path.exists());
    assert!(!temp_path("zero-batch.arrow.partial").exists());
}

#[test]
fn a_finished_write_leaves_no_partial_file() {
    let dataset = Positions::new(PositionsOptions {
        rows: 100,
        ..PositionsOptions::default()
    })
    .unwrap();
    let path = temp_path("complete.arrow");
    write_ipc_file(&dataset, &path, 64).unwrap();
    assert!(path.exists());
    assert!(!temp_path("complete.arrow.partial").exists());
}
