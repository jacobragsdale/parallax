//! Indicators on charts: computed over every row before the chart is reduced,
//! so that no zoom or width changes their values, and reduced like any line.

#![allow(missing_docs)]

use std::sync::Arc;

use arrow_array::{ArrayRef, Float64Array, Int64Array, RecordBatch};
use arrow_schema::{Field, Schema};
use parallax_datagen::Dataset;
use parallax_datagen::prices::{Prices, PricesOptions};
use parallax_engine::chart::{
    BucketVolume, ChartConfig, ChartData, ChartKind, ChartRequest, ChartView, IndicatorLines,
    OhlcColumns, Point, XAxis,
};
use parallax_engine::indicator::{self, Indicator};
use parallax_engine::{ConfigError, Filter, FilterOp, Scalar, Table, TableOptions};

fn prices() -> Table {
    let generator = Prices::new(PricesOptions {
        tickers: 2,
        securities: 200,
        ..PricesOptions::default()
    })
    .unwrap();
    let batch = generator.batch(0..generator.num_rows());
    let mut table = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    table.load(&[batch]).unwrap();
    table
}

fn first_ticker(t: &Table) -> Filter {
    Filter::new(
        "ticker",
        FilterOp::Eq(t.column(t.column_index("ticker").unwrap()).scalar(0)),
    )
}

fn candles(t: &Table, indicators: Vec<Indicator>) -> ChartConfig {
    ChartConfig {
        kind: ChartKind::Candlestick,
        x: "date".into(),
        y: Vec::new(),
        series: None,
        ohlc: Some(OhlcColumns {
            open: "open".into(),
            high: "high".into(),
            low: "low".into(),
            close: "close".into(),
        }),
        filters: vec![first_ticker(t)],
        axis: XAxis::Trading,
        indicators,
        volume: Some("volume".into()),
    }
}

fn request(x: Option<(f64, f64)>, width: u32) -> ChartRequest {
    ChartRequest {
        x,
        y: None,
        width,
        height: 300,
    }
}

fn number(t: &Table, column: &str, slot: usize) -> Option<f64> {
    match t.column(t.column_index(column).unwrap()).scalar(slot) {
        Scalar::Float64(v) => Some(v),
        Scalar::Int64(v) => Some(v as f64),
        _ => None,
    }
}

/// The first ticker's rows in date order: their slots and closes.
fn closes(t: &Table) -> (Vec<usize>, Vec<f64>) {
    let ticker = t.column(t.column_index("ticker").unwrap()).scalar(0);
    let date = t.column_index("date").unwrap();
    let mut rows: Vec<(i32, usize)> = t
        .live_slots()
        .filter(|&s| t.column(t.column_index("ticker").unwrap()).scalar(s) == ticker)
        .map(|s| match t.column(date).scalar(s) {
            Scalar::Date32(d) => (d, s),
            other => panic!("{other:?}"),
        })
        .collect();
    rows.sort_unstable();
    let slots: Vec<usize> = rows.iter().map(|r| r.1).collect();
    let closes = slots
        .iter()
        .map(|&s| number(t, "close", s).unwrap())
        .collect();
    (slots, closes)
}

fn line<'a>(lines: &'a [IndicatorLines], indicator: usize, name: &str) -> &'a [Point] {
    &lines
        .iter()
        .find(|l| l.indicator == indicator)
        .unwrap()
        .lines
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1
}

#[test]
fn indicators_use_every_row_whatever_the_zoom_or_width() {
    let t = prices();
    let indicators = vec![
        Indicator::Sma { period: 200 },
        Indicator::Bollinger {
            period: 20,
            deviations: 2.0,
        },
        Indicator::Macd {
            fast: 12,
            slow: 26,
            signal: 9,
        },
    ];
    let view = ChartView::new(&t, candles(&t, indicators)).unwrap();
    let (slots, close) = closes(&t);
    let sma = indicator::sma(&close, 200);
    let bands = indicator::bollinger(&close, 20, 2.0);
    let macd = indicator::macd(&close, 12, 26, 9);
    let row_of = |slot: usize| slots.iter().position(|&s| s == slot).unwrap();
    let (from, to) = view.x_extent().unwrap();
    // Weekly candles across the whole history, and a narrow zoom of daily ones.
    for (x, width) in [
        (None, 400),
        (Some((from + 1000.0, from + 1100.0)), 900),
        (Some((from, to)), 37),
    ] {
        let reduced = view.reduce(&t, &request(x, width));
        let lines = &reduced.indicators;
        assert_eq!(lines.len(), 3);
        for (indicator, name, expected) in [
            (0, "sma", &sma),
            (1, "upper", &bands.upper),
            (1, "lower", &bands.lower),
            (2, "macd", &macd.macd),
            (2, "histogram", &macd.histogram),
        ] {
            let points = line(lines, indicator, name);
            assert!(
                points.iter().filter(|p| p.slot.is_some()).count() > 3,
                "{name} at {width}"
            );
            for p in points.iter().filter(|p| p.slot.is_some()) {
                assert_eq!(p.y, expected[row_of(p.slot.unwrap())], "{name} at {width}");
            }
        }
    }
}

#[test]
fn indicator_lines_cover_the_same_pixels_as_every_value() {
    let t = prices();
    let view = ChartView::new(&t, candles(&t, vec![Indicator::Ema { period: 10 }])).unwrap();
    let (_, close) = closes(&t);
    let ema = indicator::ema(&close, 10);
    let (from, to) = view.x_extent().unwrap();
    let width = 150;
    let points = line(&view.reduce(&t, &request(None, width)).indicators, 0, "ema").to_vec();
    // Every day sits one apart on the trading axis, from `from`.
    let column = |x: f64| {
        (((x - from) / (to - from) * f64::from(width)).floor() as i64).min(i64::from(width) - 1)
    };
    let mut full: Vec<Option<(f64, f64)>> = vec![None; width as usize];
    for (i, &v) in ema.iter().enumerate() {
        if !v.is_nan() {
            let slot = &mut full[column(from + i as f64) as usize];
            *slot = Some(slot.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
        }
    }
    let mut kept: Vec<Option<(f64, f64)>> = vec![None; width as usize];
    for p in points.iter().filter(|p| !p.y.is_nan()) {
        let slot = &mut kept[column(p.x) as usize];
        *slot = Some(slot.map_or((p.y, p.y), |(lo, hi)| (lo.min(p.y), hi.max(p.y))));
    }
    assert_eq!(kept, full);
    assert!(points.len() <= 4 * width as usize + 1);
}

fn table(columns: Vec<(&str, ArrayRef)>) -> Table {
    let schema = Schema::new(
        columns
            .iter()
            .map(|(name, array)| Field::new(*name, array.data_type().clone(), true))
            .collect::<Vec<_>>(),
    );
    let batch =
        RecordBatch::try_new(Arc::new(schema), columns.into_iter().map(|c| c.1).collect()).unwrap();
    let mut t = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    t.load(&[batch]).unwrap();
    t
}

#[test]
fn rows_without_a_price_are_skipped_not_counted() {
    let x: Vec<f64> = (0..8).map(f64::from).collect();
    let y = vec![
        Some(1.0),
        Some(2.0),
        None,
        Some(3.0),
        Some(f64::NAN),
        Some(4.0),
        Some(5.0),
        Some(6.0),
    ];
    let t = table(vec![
        ("x", Arc::new(Float64Array::from(x)) as ArrayRef),
        ("y", Arc::new(Float64Array::from(y)) as ArrayRef),
    ]);
    let config = ChartConfig {
        kind: ChartKind::Line,
        x: "x".into(),
        y: vec!["y".into()],
        series: None,
        ohlc: None,
        filters: Vec::new(),
        axis: XAxis::Value,
        indicators: vec![Indicator::Sma { period: 2 }],
        volume: None,
    };
    let view = ChartView::new(&t, config).unwrap();
    let points = line(&view.reduce(&t, &request(None, 100)).indicators, 0, "sma").to_vec();
    // The average of each value with the one before it that has a value: the
    // null and the NaN are skipped, not read as zero or as a break.
    let values: Vec<(f64, f64)> = points
        .iter()
        .filter(|p| !p.y.is_nan())
        .map(|p| (p.x, p.y))
        .collect();
    assert_eq!(
        values,
        vec![(1.0, 1.5), (3.0, 2.5), (5.0, 3.5), (6.0, 4.5), (7.0, 5.5)]
    );
}

#[test]
fn vwap_starts_again_each_new_york_day_and_volume_sums_per_candle() {
    // Two New York trading days of hourly bars.
    let day = |d: &str, hour: i64| {
        format!("{d}T09:30:00-05:00")
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_microsecond()
            + hour * 3_600_000_000
    };
    let times: Vec<i64> = (0..6)
        .map(|h| day("2026-11-02", h))
        .chain((0..6).map(|h| day("2026-11-03", h)))
        .collect();
    let n = times.len();
    let price: Vec<f64> = (0..n).map(|i| 10.0 + i as f64).collect();
    let volume: Vec<i64> = (0..n).map(|i| 100 * (i as i64 + 1)).collect();
    let t = table(vec![
        (
            "at",
            Arc::new(arrow_array::TimestampMicrosecondArray::from(times).with_timezone("UTC"))
                as ArrayRef,
        ),
        (
            "open",
            Arc::new(Float64Array::from(price.clone())) as ArrayRef,
        ),
        (
            "high",
            Arc::new(Float64Array::from(
                price.iter().map(|p| p + 1.0).collect::<Vec<_>>(),
            )) as ArrayRef,
        ),
        (
            "low",
            Arc::new(Float64Array::from(
                price.iter().map(|p| p - 1.0).collect::<Vec<_>>(),
            )) as ArrayRef,
        ),
        (
            "close",
            Arc::new(Float64Array::from(price.clone())) as ArrayRef,
        ),
        (
            "volume",
            Arc::new(Int64Array::from(volume.clone())) as ArrayRef,
        ),
    ]);
    let config = ChartConfig {
        kind: ChartKind::Candlestick,
        x: "at".into(),
        y: Vec::new(),
        series: None,
        ohlc: Some(OhlcColumns {
            open: "open".into(),
            high: "high".into(),
            low: "low".into(),
            close: "close".into(),
        }),
        filters: Vec::new(),
        axis: XAxis::Trading,
        indicators: vec![Indicator::Vwap, Indicator::Volume],
        volume: Some("volume".into()),
    };
    let view = ChartView::new(&t, config).unwrap();
    let reduced = view.reduce(&t, &request(None, 1000));
    let vwap = line(&reduced.indicators, 0, "vwap");
    // The typical price is the close here, so each day's VWAP is its closes
    // weighted by volume, starting again at the second day's first bar.
    let expected = |range: std::ops::Range<usize>, i: usize| {
        let (w, v) = range.take_while(|&j| j <= i).fold((0.0, 0.0), |(w, v), j| {
            (w + price[j] * volume[j] as f64, v + volume[j] as f64)
        });
        w / v
    };
    let values: Vec<f64> = vwap.iter().filter(|p| !p.y.is_nan()).map(|p| p.y).collect();
    let wanted: Vec<f64> = (0..n)
        .map(|i| {
            if i < 6 {
                expected(0..6, i)
            } else {
                expected(6..12, i)
            }
        })
        .collect();
    assert_eq!(values.len(), n);
    for (a, b) in values.iter().zip(&wanted) {
        assert!((a - b).abs() < 1e-12, "{a} against {b}");
    }
    assert_eq!(values[6], price[6]);
    // Hourly candles: each holds one bar, and its volume is that bar's.
    let ChartData::Candles {
        candles, volumes, ..
    } = reduced.data
    else {
        panic!("not candles")
    };
    assert_eq!(candles.len(), n);
    assert_eq!(
        volumes,
        volume
            .iter()
            .map(|&v| BucketVolume::Total(Scalar::Int64(v)))
            .collect::<Vec<_>>()
    );
    // Daily candles sum each day's volume exactly.
    let narrow = view.reduce(&t, &request(None, 12));
    let ChartData::Candles { volumes, .. } = narrow.data else {
        panic!("not candles")
    };
    let day_one: i64 = volume[..6].iter().sum();
    let day_two: i64 = volume[6..].iter().sum();
    assert_eq!(
        volumes,
        vec![
            BucketVolume::Total(Scalar::Int64(day_one)),
            BucketVolume::Total(Scalar::Int64(day_two))
        ]
    );
}

#[test]
fn charts_refuse_indicators_they_cannot_compute() {
    let t = prices();
    let refuse = |config: ChartConfig| match ChartView::new(&t, config) {
        Err(ConfigError::Chart(m) | ConfigError::MissingChartColumns(m)) => m,
        other => panic!("{other:?}"),
    };
    let line = |indicators: Vec<Indicator>| ChartConfig {
        kind: ChartKind::Line,
        x: "date".into(),
        y: vec!["close".into()],
        series: Some("ticker".into()),
        ohlc: None,
        filters: Vec::new(),
        axis: XAxis::Value,
        indicators,
        volume: None,
    };
    assert!(refuse(line(vec![Indicator::Atr { period: 14 }])).contains("candlestick or OHLC"));
    assert!(refuse(line(vec![Indicator::Volume])).contains("candlestick or OHLC"));
    assert!(refuse(line(vec![Indicator::Sma { period: 0 }])).contains("from 1 to"));
    assert!(
        refuse(line(vec![Indicator::Macd {
            fast: 26,
            slow: 12,
            signal: 9
        }]))
        .contains("shorter")
    );
    assert!(
        refuse(line(vec![Indicator::Bollinger {
            period: 20,
            deviations: f64::NAN
        }]))
        .contains("deviations")
    );
    assert!(
        refuse(ChartConfig {
            volume: None,
            ..candles(&t, vec![Indicator::Vwap])
        })
        .contains("volume")
    );
    assert!(
        refuse(ChartConfig {
            kind: ChartKind::Histogram,
            x: "close".into(),
            y: Vec::new(),
            ..line(vec![Indicator::Sma { period: 5 }])
        })
        .contains("line, area")
    );
    // A line chart takes the price indicators, one set per series.
    let view = ChartView::new(
        &t,
        line(vec![
            Indicator::Sma { period: 5 },
            Indicator::Rsi { period: 14 },
        ]),
    )
    .unwrap();
    let reduced = view.reduce(&t, &request(None, 300));
    assert_eq!(reduced.indicators.len(), 4);
    assert!(reduced.indicators.iter().all(|l| l.key.is_some()));
}

#[test]
fn a_volume_too_large_to_hold_says_so() {
    // Three days of a 38-digit decimal volume: each fits, their total does not,
    // and overflows even 128 bits, where wrapping round would land back inside
    // 38 digits as a wrong, negative total.
    let days = [20_395, 20_396, 20_397]; // 2025-11-03 to 05, Monday to Wednesday
    let price = || Arc::new(Float64Array::from(vec![1.0; 3])) as ArrayRef;
    let huge = 90_000_000_000_000_000_000_000_000_000_000_000_000_i128;
    let volume = arrow_array::Decimal128Array::from(vec![Some(huge); 3])
        .with_precision_and_scale(38, 0)
        .unwrap();
    let t = table(vec![
        (
            "date",
            Arc::new(arrow_array::Date32Array::from(days.to_vec())) as ArrayRef,
        ),
        ("open", price()),
        ("high", price()),
        ("low", price()),
        ("close", price()),
        ("volume", Arc::new(volume) as ArrayRef),
    ]);
    let config = ChartConfig {
        kind: ChartKind::Candlestick,
        x: "date".into(),
        y: Vec::new(),
        series: None,
        ohlc: Some(OhlcColumns {
            open: "open".into(),
            high: "high".into(),
            low: "low".into(),
            close: "close".into(),
        }),
        filters: Vec::new(),
        axis: XAxis::Value,
        indicators: vec![Indicator::Volume],
        volume: Some("volume".into()),
    };
    let view = ChartView::new(&t, config).unwrap();
    let volumes = |width| match view.reduce(&t, &request(None, width)).data {
        ChartData::Candles { volumes, .. } => volumes,
        other => panic!("{other:?}"),
    };
    // Daily candles each hold one day; a weekly candle holds all three.
    assert_eq!(
        volumes(400),
        vec![BucketVolume::Total(Scalar::Decimal(huge)); 3]
    );
    assert_eq!(volumes(1), vec![BucketVolume::Overflow]);
}
