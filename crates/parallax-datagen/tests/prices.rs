//! Prices: bar consistency, ordering, fat tails and hostile coverage.

mod common;

use arrow_array::Array;
use arrow_array::cast::AsArray;
use common::{f64s, hash, i32s, i64s, strings};
use parallax_datagen::Dataset;
use parallax_datagen::prices::{Prices, PricesOptions, schema};

fn prices(tickers: usize, hostile: bool) -> Prices {
    Prices::new(PricesOptions {
        tickers,
        hostile,
        ..PricesOptions::default()
    })
    .unwrap()
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

const GOLDEN_CALM: u64 = 10_655_051_621_311_973_884;
const GOLDEN_HOSTILE: u64 = 6_299_174_920_531_724_708;

/// The golden hashes of the nine columns before `event` was added, which every
/// price, volume and return still matches.
const GOLDEN_PRICES_CALM: u64 = 10_600_207_667_285_220_552;
const GOLDEN_PRICES_HOSTILE: u64 = 14_734_069_032_027_083_984;

#[test]
fn events_change_no_price() {
    let nine: Vec<usize> = (0..9).collect();
    let prices_only = |hostile| hash(&prices(2, hostile).batch(0..1_000).project(&nine).unwrap());
    assert_eq!(
        (prices_only(false), prices_only(true)),
        (GOLDEN_PRICES_CALM, GOLDEN_PRICES_HOSTILE)
    );
}

#[test]
fn each_ticker_reports_every_quarter_and_some_pay_dividends() {
    let dataset = prices(6, false);
    let batch = dataset.batch(0..dataset.num_rows());
    let tickers = batch.column_by_name("ticker").unwrap().as_string::<i32>();
    let events = batch.column_by_name("event").unwrap().as_string::<i32>();
    let years = 10.8; // 2016-01-04 to 2026-10-30
    let mut by_ticker: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    for i in 0..batch.num_rows() {
        let entry = by_ticker.entry(tickers.value(i)).or_default();
        let event = if events.is_null(i) {
            ""
        } else {
            events.value(i)
        };
        match event {
            "" => {}
            "Dividend" => entry.1 += 1,
            e => {
                assert!(e.ends_with("earnings"), "{e}");
                entry.0 += 1;
            }
        }
    }
    for (ticker, (earnings, dividends)) in &by_ticker {
        // Four a year, give or take the first and last partial years.
        assert!(
            (earnings.abs_diff((4.0 * years) as usize)) <= 2,
            "{ticker}: {earnings} earnings"
        );
        assert!(
            *dividends == 0 || dividends.abs_diff((4.0 * years) as usize) <= 2,
            "{ticker}: {dividends} dividends"
        );
    }
    // Every third ticker pays: two of the six.
    assert_eq!(by_ticker.values().filter(|(_, d)| *d > 0).count(), 2);
}

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
    let volume = i64s(&batch, "volume");
    let high = f64s(&batch, "high");
    let low = f64s(&batch, "low");
    assert!(
        (0..batch.num_rows()).any(|i| volume[i] == Some(0) && high[i] == low[i]),
        "halted day"
    );
    assert!(i64s(&batch, "volume").contains(&None), "null volume");
    assert!(f64s(&batch, "vwap").contains(&None), "null vwap");
}

#[test]
fn returns_follow_reported_closes_even_in_hostile_rows() {
    let dataset = prices(10, true);
    let batch = dataset.batch(0..dataset.num_rows());
    let close: Vec<f64> = f64s(&batch, "close")
        .into_iter()
        .map(Option::unwrap)
        .collect();
    let returns = f64s(&batch, "return_1d");
    let per = dataset.days().len();
    let mut infinite = 0;
    for i in 0..batch.num_rows() {
        if i % per == 0 {
            assert_eq!(returns[i], None);
            continue;
        }
        let expected = close[i] / close[i - 1] - 1.0;
        let actual = returns[i].unwrap();
        if expected.is_nan() {
            assert!(actual.is_nan(), "row {i}: {actual} should be NaN");
        } else {
            assert_eq!(actual.to_bits(), expected.to_bits(), "row {i}");
        }
        if close[i - 1] == 0.0 && close[i] > 0.0 {
            assert_eq!(actual, f64::INFINITY, "row {i}: the bar after a zero close");
            infinite += 1;
        }
    }
    assert!(infinite > 0, "no zero close was followed by a priced bar");
}

#[test]
fn rejects_more_tickers_than_equities() {
    let err = Prices::new(PricesOptions {
        tickers: 500,
        securities: 100,
        ..PricesOptions::default()
    })
    .unwrap_err();
    assert!(
        err.to_string().contains("asked for prices of 500 equities"),
        "{err}"
    );
}

#[test]
fn the_first_day_takes_no_event_that_fell_before_it() {
    // The data starts on Monday 2016-01-25: a report due before it is not put on
    // its first day as if it had been due then.
    let dataset = Prices::new(PricesOptions {
        tickers: 10,
        start: jiff::civil::date(2016, 1, 25),
        end: jiff::civil::date(2016, 2, 5),
        securities: 200,
        ..PricesOptions::default()
    })
    .unwrap();
    let batch = dataset.batch(0..dataset.num_rows());
    let events = batch.column_by_name("event").unwrap().as_string::<i32>();
    let dates = batch
        .column_by_name("date")
        .unwrap()
        .as_primitive::<arrow_array::types::Date32Type>();
    let first = dates.value(0);
    // Tickers report from the 18th to the 27th. The 23rd and 24th are a weekend,
    // so those reporting on the 23rd, 24th and 25th report on Monday the 25th;
    // those reporting before, on the 22nd or earlier, did so before the data.
    let on_first_day = (0..batch.num_rows())
        .filter(|&i| dates.value(i) == first && !events.is_null(i))
        .count();
    assert_eq!(on_first_day, 3);
}
