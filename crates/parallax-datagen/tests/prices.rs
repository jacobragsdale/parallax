//! Prices: bar consistency, ordering, fat tails and hostile coverage.

mod common;

use common::{f64s, hash, i32s, i64s, strings};
use parallax_datagen::Dataset;
use parallax_datagen::prices::{Prices, PricesOptions, schema};

fn prices(tickers: usize, hostile: bool) -> Prices {
    Prices::new(PricesOptions {
        tickers,
        hostile,
        ..PricesOptions::default()
    })
}

#[test]
fn rows_are_tickers_by_trading_days_in_order() {
    let dataset = prices(3, false);
    let days = dataset.days().to_vec();
    assert_eq!(dataset.num_rows(), 3 * days.len() as u64);
    let batch = dataset.batch(0..dataset.num_rows());
    assert_eq!(batch.schema().as_ref(), &schema());
    let dates: Vec<i32> = i32s(&batch, "date").into_iter().flatten().collect();
    let tickers = strings(&batch, "ticker");
    for t in 0..3 {
        let block = t * days.len()..(t + 1) * days.len();
        assert_eq!(&dates[block.clone()], days.as_slice());
        assert!(tickers[block].windows(2).all(|w| w[0] == w[1]));
    }
    assert_ne!(tickers[0], tickers[days.len()]);
}

#[test]
fn ranges_crossing_tickers_match_a_full_run() {
    let dataset = prices(4, true);
    let per = dataset.days().len() as u64;
    let full = dataset.batch(0..dataset.num_rows());
    let start = per - 17;
    let end = 3 * per + 5;
    let part = dataset.batch(start..end);
    assert_eq!(
        hash(&full.slice(start as usize, (end - start) as usize)),
        hash(&part)
    );
}

#[test]
fn golden_rows_are_stable() {
    assert_eq!(
        (
            hash(&prices(2, false).batch(0..1_000)),
            hash(&prices(2, true).batch(0..1_000))
        ),
        (GOLDEN_CALM, GOLDEN_HOSTILE)
    );
}

const GOLDEN_CALM: u64 = 10_600_207_667_285_220_552;
const GOLDEN_HOSTILE: u64 = 17_049_446_282_210_177_676;

#[test]
fn bars_are_internally_consistent() {
    let dataset = prices(20, false);
    let batch = dataset.batch(0..dataset.num_rows());
    let col = |n| {
        f64s(&batch, n)
            .into_iter()
            .map(Option::unwrap)
            .collect::<Vec<f64>>()
    };
    let (open, high, low, close, vwap) = (
        col("open"),
        col("high"),
        col("low"),
        col("close"),
        col("vwap"),
    );
    let returns = f64s(&batch, "return_1d");
    let volume = i64s(&batch, "volume");
    let per = dataset.days().len();
    for i in 0..batch.num_rows() {
        assert!(high[i] >= open[i].max(close[i]), "row {i}: high");
        assert!(
            low[i] <= open[i].min(close[i]) && low[i] > 0.0,
            "row {i}: low"
        );
        assert!(vwap[i] >= low[i] && vwap[i] <= high[i], "row {i}: vwap");
        assert!(volume[i].unwrap() >= 0, "row {i}: volume");
        if i % per == 0 {
            assert_eq!(returns[i], None, "row {i}: first day has no return");
        } else {
            let expected = close[i] / close[i - 1] - 1.0;
            assert_eq!(
                returns[i].map(f64::to_bits),
                Some(expected.to_bits()),
                "row {i}"
            );
        }
    }
}

#[test]
fn returns_have_fat_tails() {
    let dataset = prices(50, false);
    let batch = dataset.batch(0..dataset.num_rows());
    let returns: Vec<f64> = f64s(&batch, "return_1d").into_iter().flatten().collect();
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
    let kurtosis = returns.iter().map(|r| (r - mean).powi(4)).sum::<f64>() / n / var.powi(2);
    assert!(
        kurtosis > 5.0,
        "kurtosis {kurtosis} is too close to a normal's 3"
    );
}

#[test]
fn hostile_rows_cover_every_category() {
    let dataset = prices(10, true);
    let batch = dataset.batch(0..dataset.num_rows());
    let close = f64s(&batch, "close");
    let returns = f64s(&batch, "return_1d");
    assert!(close.iter().flatten().any(|c| c.is_nan()), "NaN close");
    assert!(close.contains(&Some(0.0)), "zero close");
    assert!(returns.contains(&Some(f64::INFINITY)), "infinite return");
    assert!(
        returns
            .iter()
            .flatten()
            .any(|r| r.to_bits() == (-0.0f64).to_bits()),
        "negative-zero return"
    );
    assert!(i64s(&batch, "volume").contains(&None), "null volume");
    assert!(f64s(&batch, "vwap").contains(&None), "null vwap");
}
