//! Charts reduced to the screen, checked against the full data.

#![allow(missing_docs)]

use std::sync::Arc;

use arrow_array::{ArrayRef, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parallax_datagen::Dataset;
use parallax_datagen::calendar::{from_epoch_days, trading_days};
use parallax_datagen::prices::{Prices, PricesOptions};
use parallax_engine::chart::{
    Candle, ChartConfig, ChartData, ChartKind, ChartRequest, ChartView, Interval, OhlcColumns,
    Point, XAxis,
};
use parallax_engine::{Filter, FilterOp, Scalar, Table, TableOptions};
use proptest::prelude::*;

fn table(x: &[f64], y: &[Option<f64>]) -> Table {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, true),
        ])),
        vec![
            Arc::new(Float64Array::from(x.to_vec())) as ArrayRef,
            Arc::new(Float64Array::from(y.to_vec())) as ArrayRef,
        ],
    )
    .unwrap();
    let mut table = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    table.load(&[batch]).unwrap();
    table
}

fn line(axis: XAxis) -> ChartConfig {
    ChartConfig {
        kind: ChartKind::Line,
        x: "x".into(),
        y: vec!["y".into()],
        series: None,
        ohlc: None,
        filters: Vec::new(),
        axis,
    }
}

/// The engine's pixel column of `x`, as the renderer computes it.
fn column(x: f64, from: f64, to: f64, width: u32) -> i64 {
    if x < from {
        return -1;
    }
    if x > to {
        return i64::from(width);
    }
    (((x - from) / (to - from) * f64::from(width)).floor() as i64).min(i64::from(width) - 1)
}

/// The lowest and highest y a polyline reaches in each pixel column: its points
/// there, and where its segments cross the column's edges. A NaN y breaks the
/// line.
fn coverage(points: &[(f64, f64)], from: f64, to: f64, width: u32) -> Vec<Option<(f64, f64)>> {
    let mut out: Vec<Option<(f64, f64)>> = vec![None; width as usize];
    let mut reach = |c: i64, y: f64| {
        if c >= 0 && c < i64::from(width) {
            let slot = &mut out[c as usize];
            *slot = Some(slot.map_or((y, y), |(lo, hi)| (lo.min(y), hi.max(y))));
        }
    };
    for &(x, y) in points {
        if !y.is_nan() {
            reach(column(x, from, to, width), y);
        }
    }
    for pair in points.windows(2) {
        let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
        if ay.is_nan() || by.is_nan() {
            continue;
        }
        let (ca, cb) = (column(ax, from, to, width), column(bx, from, to, width));
        for k in ca + 1..=cb {
            let edge = from + (to - from) * k as f64 / f64::from(width);
            let y = if bx == ax {
                ay
            } else {
                ay + (by - ay) * ((edge - ax) / (bx - ax))
            };
            reach(k - 1, y);
            reach(k, y);
        }
    }
    out
}

fn reduced(view: &ChartView, table: &Table, request: &ChartRequest) -> Vec<Point> {
    match view.reduce(table, request).data {
        ChartData::Series(mut series) => series.remove(0).1,
        other => panic!("not a series: {other:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn m4_lines_cover_the_same_pixels_as_every_point(
        rows in prop::collection::vec((0u16..300, prop::option::weighted(0.9, -1_000.0f64..1_000.0)), 1..400),
        width in 1u32..120,
        window in prop::option::of((0u16..300, 1u16..300)),
    ) {
        let x: Vec<f64> = rows.iter().map(|r| f64::from(r.0) / 3.0).collect();
        let y: Vec<Option<f64>> = rows.iter().map(|r| r.1).collect();
        let t = table(&x, &y);
        let view = ChartView::new(&t, line(XAxis::Value)).unwrap();
        let request = ChartRequest {
            x: window.map(|(a, span)| (f64::from(a) / 3.0, f64::from(a + span) / 3.0)),
            y: None,
            width,
            height: 100,
        };
        let (from, to) = view.reduce(&t, &request).x;
        // Every point in series order, by x and then insertion, with the nearest
        // point on each side of the range.
        let mut all: Vec<(f64, f64)> = x.iter().zip(&y).map(|(&x, y)| (x, y.unwrap_or(f64::NAN))).collect();
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        let start = all.partition_point(|p| p.0 < from).saturating_sub(1);
        let end = (all.partition_point(|p| p.0 <= to) + 1).min(all.len());
        // Missing values in a row make one break, as the reduction does.
        let mut full: Vec<(f64, f64)> = Vec::new();
        for &p in &all[start..end] {
            if !(p.1.is_nan() && full.last().is_some_and(|q: &(f64, f64)| q.1.is_nan())) {
                full.push(p);
            }
        }
        let kept: Vec<(f64, f64)> = reduced(&view, &t, &request).iter().map(|p| (p.x, p.y)).collect();
        prop_assert!(kept.len() <= full.len());
        prop_assert!(kept.len() <= 4 * (width as usize + 2) + full.iter().filter(|p| p.1.is_nan()).count() * 5);
        prop_assert_eq!(coverage(&kept, from, to, width), coverage(&full, from, to, width));
    }

    #[test]
    fn every_value_lands_in_the_bin_its_edges_name(
        values in prop::collection::vec(-1e6f64..1e6, 1..500),
        width in 8u32..400,
    ) {
        let t = table(&values, &vec![None; values.len()]);
        let config = ChartConfig { kind: ChartKind::Histogram, y: Vec::new(), ..line(XAxis::Value) };
        let view = ChartView::new(&t, config).unwrap();
        let ChartData::Histogram { edges, counts } = view.reduce(&t, &ChartRequest { x: None, y: None, width, height: 100 }).data else {
            panic!("not a histogram")
        };
        let counts = &counts[0].1;
        prop_assert_eq!(edges.len(), counts.len() + 1);
        prop_assert!(edges.windows(2).all(|e| e[0] <= e[1]));
        prop_assert_eq!(counts.iter().sum::<u64>(), values.len() as u64);
        let last = counts.len() - 1;
        for (bin, &count) in counts.iter().enumerate() {
            let inside = values.iter().filter(|&&v| {
                v >= edges[bin] && (v < edges[bin + 1] || (bin == last && v <= edges[bin + 1]))
            });
            // A value on an inner edge belongs to the bin above it.
            let expected = inside.filter(|&&v| bin == last || v < edges[bin + 1]).count() as u64;
            prop_assert_eq!(count, expected, "bin {}", bin);
        }
    }
}

#[test]
fn a_spike_survives_any_width() {
    let mut y: Vec<Option<f64>> = (0..10_000).map(|i| Some(f64::from(i % 7))).collect();
    y[6_543] = Some(1e6);
    y[2_000] = None;
    let x: Vec<f64> = (0..10_000).map(f64::from).collect();
    let t = table(&x, &y);
    let view = ChartView::new(&t, line(XAxis::Value)).unwrap();
    for width in [1, 7, 300, 2_000] {
        let points = reduced(
            &view,
            &t,
            &ChartRequest {
                x: None,
                y: None,
                width,
                height: 100,
            },
        );
        // Four points per column and per edge, plus the gap's break and the run
        // it starts in a column already holding four.
        assert!(
            points.len() <= 4 * (width as usize + 2) + 5,
            "{width}: {}",
            points.len()
        );
        assert!(points.iter().any(|p| p.y == 1e6), "{width}");
        assert!(
            points.iter().any(|p| p.y.is_nan() && p.slot.is_none()),
            "the gap at 2,000 stays"
        );
    }
}

fn prices(tickers: usize) -> Table {
    let generator = Prices::new(PricesOptions {
        tickers,
        ..PricesOptions::default()
    })
    .unwrap();
    let batch = generator.batch(0..generator.num_rows());
    let mut table = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    table.load(&[batch]).unwrap();
    table
}

fn value(table: &Table, column: &str, slot: usize) -> f64 {
    match table
        .column(table.column_index(column).unwrap())
        .scalar(slot)
    {
        Scalar::Float64(v) => v,
        other => panic!("{column}: {other:?}"),
    }
}

#[test]
fn weekly_candles_match_the_daily_prices() {
    let t = prices(3);
    let ticker = match t.column(t.column_index("ticker").unwrap()).scalar(0) {
        Scalar::Utf8(s) => s,
        other => panic!("{other:?}"),
    };
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
        filters: vec![Filter::new(
            "ticker",
            FilterOp::Eq(Scalar::Utf8(ticker.clone())),
        )],
        axis: XAxis::Value,
    };
    let view = ChartView::new(&t, config).unwrap();
    let (from, to) = view.x_extent().unwrap();
    let weeks = (to - from) / 7.0;
    let width = (weeks * 4.0).ceil() as u32 + 4;
    let reduced = view.reduce(
        &t,
        &ChartRequest {
            x: None,
            y: None,
            width,
            height: 300,
        },
    );
    let ChartData::Candles { interval, candles } = reduced.data else {
        panic!("not candles")
    };
    assert_eq!(interval, Interval::Week);
    // The expected candles, by Monday, from the rows themselves.
    let date = t.column_index("date").unwrap();
    let tick = t.column_index("ticker").unwrap();
    let mut rows: Vec<(i32, usize)> = t
        .live_slots()
        .filter(|&s| t.column(tick).scalar(s) == Scalar::Utf8(ticker.clone()))
        .map(|s| match t.column(date).scalar(s) {
            Scalar::Date32(d) => (d, s),
            other => panic!("{other:?}"),
        })
        .collect();
    rows.sort_unstable();
    let monday = |d: i32| {
        let day = from_epoch_days(d);
        d - i32::from(day.weekday().to_monday_zero_offset())
    };
    let mut expected: Vec<(i32, f64, f64, f64, f64)> = Vec::new();
    for (d, s) in rows {
        let (o, h, l, c) = (
            value(&t, "open", s),
            value(&t, "high", s),
            value(&t, "low", s),
            value(&t, "close", s),
        );
        match expected.last_mut() {
            Some(e) if e.0 == monday(d) => {
                e.2 = e.2.max(h);
                e.3 = e.3.min(l);
                e.4 = c;
            }
            _ => expected.push((monday(d), o, h, l, c)),
        }
    }
    assert_eq!(candles.len(), expected.len());
    for ((bucket, candle), e) in candles.iter().zip(&expected) {
        assert_eq!(bucket.from, f64::from(e.0));
        assert_eq!(bucket.to, f64::from(e.0 + 7));
        assert_eq!(value(&t, "open", candle.open.unwrap()), e.1);
        assert_eq!(value(&t, "high", candle.high.unwrap()), e.2);
        assert_eq!(value(&t, "low", candle.low.unwrap()), e.3);
        assert_eq!(value(&t, "close", candle.close.unwrap()), e.4);
    }
}

#[test]
fn trading_time_places_trading_days_one_apart() {
    let t = prices(1);
    let config = ChartConfig {
        x: "date".into(),
        y: vec!["close".into()],
        ..line(XAxis::Trading)
    };
    let view = ChartView::new(&t, config).unwrap();
    let (from, to) = view.x_extent().unwrap();
    let days = (to - from) as u32 + 1;
    // Wide enough that no two days share a pixel column: every point stays.
    let points = reduced(
        &view,
        &t,
        &ChartRequest {
            x: None,
            y: None,
            width: days * 2,
            height: 100,
        },
    );
    assert_eq!(points.len() as u32, days);
    assert!(points.windows(2).all(|p| p[1].x - p[0].x == 1.0));
    let first = from_epoch_days(match t.column(t.column_index("date").unwrap()).scalar(0) {
        Scalar::Date32(d) => d,
        other => panic!("{other:?}"),
    });
    let last = first
        .checked_add(jiff::Span::new().days(i64::from(days) * 2))
        .unwrap();
    assert!(trading_days(first, last).len() as u32 >= days);
}

#[test]
fn ticks_are_in_order_inside_the_range_and_labelled() {
    let t = prices(1);
    for axis in [XAxis::Value, XAxis::Trading] {
        let config = ChartConfig {
            x: "date".into(),
            y: vec!["close".into()],
            ..line(axis)
        };
        let view = ChartView::new(&t, config).unwrap();
        for width in [200, 800, 3_000] {
            let reduced = view.reduce(
                &t,
                &ChartRequest {
                    x: None,
                    y: None,
                    width,
                    height: 100,
                },
            );
            let (from, to) = reduced.x;
            // A narrow axis over years may have room for one tick.
            let least = if width >= 800 { 2 } else { 1 };
            assert!(
                reduced.ticks.len() >= least,
                "{axis:?} {width}: {:?}",
                reduced.ticks
            );
            assert!(
                reduced.ticks.len() as u32 <= width / 50 + 2,
                "{axis:?} {width}: {}",
                reduced.ticks.len()
            );
            assert!(reduced.ticks.windows(2).all(|w| w[0].x < w[1].x));
            assert!(
                reduced
                    .ticks
                    .iter()
                    .all(|t| t.x >= from && t.x <= to && !t.label.is_empty())
            );
        }
    }
    let t = table(&[0.0, 0.3, 10.0], &[Some(1.0); 3]);
    let view = ChartView::new(&t, line(XAxis::Value)).unwrap();
    let labels: Vec<String> = view
        .reduce(
            &t,
            &ChartRequest {
                x: None,
                y: None,
                width: 500,
                height: 100,
            },
        )
        .ticks
        .into_iter()
        .map(|t| t.label)
        .collect();
    assert_eq!(labels, ["0", "2", "4", "6", "8", "10"]);
}

#[test]
fn a_density_counts_every_point_once() {
    let x: Vec<f64> = (0..5_000).map(|i| f64::from(i % 97)).collect();
    let y: Vec<Option<f64>> = (0..5_000).map(|i| Some(f64::from((i * 31) % 89))).collect();
    let t = table(&x, &y);
    let config = ChartConfig {
        kind: ChartKind::Scatter,
        ..line(XAxis::Value)
    };
    let view = ChartView::new(&t, config).unwrap();
    let ChartData::Density {
        columns,
        rows,
        counts,
        ..
    } = view
        .reduce(
            &t,
            &ChartRequest {
                x: None,
                y: None,
                width: 400,
                height: 200,
            },
        )
        .data
    else {
        panic!("not a density")
    };
    assert_eq!((columns, rows), (100, 50));
    assert_eq!(counts.iter().map(|&c| u64::from(c)).sum::<u64>(), 5_000);
}

#[test]
fn series_follow_the_series_column_in_key_order() {
    let t = prices(3);
    let config = ChartConfig {
        x: "date".into(),
        y: vec!["close".into(), "open".into()],
        series: Some("ticker".into()),
        ..line(XAxis::Value)
    };
    let view = ChartView::new(&t, config).unwrap();
    let ChartData::Series(series) = view
        .reduce(
            &t,
            &ChartRequest {
                x: None,
                y: None,
                width: 300,
                height: 100,
            },
        )
        .data
    else {
        panic!("not series")
    };
    assert_eq!(series.len(), 6);
    let keys: Vec<String> = series
        .iter()
        .step_by(2)
        .map(|(s, _)| match &s.key {
            Some(Scalar::Utf8(k)) => k.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert!(series.iter().all(|(_, points)| !points.is_empty()));
}

#[test]
fn charts_refuse_columns_that_cannot_play_their_part() {
    let t = prices(1);
    let refuse = |config: ChartConfig| ChartView::new(&t, config).unwrap_err().to_string();
    assert!(
        refuse(ChartConfig {
            x: "ticker".into(),
            y: vec!["close".into()],
            ..line(XAxis::Value)
        })
        .contains("x axis")
    );
    assert!(
        refuse(ChartConfig {
            x: "date".into(),
            y: vec!["ticker".into()],
            ..line(XAxis::Value)
        })
        .contains("y axis")
    );
    assert!(
        refuse(ChartConfig {
            x: "date".into(),
            y: Vec::new(),
            ..line(XAxis::Value)
        })
        .contains("needs y")
    );
    assert!(
        refuse(ChartConfig {
            kind: ChartKind::Candlestick,
            x: "date".into(),
            y: Vec::new(),
            ..line(XAxis::Value)
        })
        .contains("open, high, low and close")
    );
    assert!(
        refuse(ChartConfig {
            kind: ChartKind::Histogram,
            x: "date".into(),
            y: Vec::new(),
            ..line(XAxis::Value)
        })
        .contains("histogram")
    );
    assert!(
        refuse(ChartConfig {
            x: "close".into(),
            y: vec!["open".into()],
            ..line(XAxis::Trading)
        })
        .contains("trading-time")
    );
}

fn candlestick(x: &str, filters: Vec<Filter>, axis: XAxis) -> ChartConfig {
    ChartConfig {
        kind: ChartKind::Candlestick,
        x: x.into(),
        y: Vec::new(),
        series: None,
        ohlc: Some(OhlcColumns {
            open: "open".into(),
            high: "high".into(),
            low: "low".into(),
            close: "close".into(),
        }),
        filters,
        axis,
    }
}

fn first_ticker(t: &Table) -> Filter {
    let ticker = t.column(t.column_index("ticker").unwrap()).scalar(0);
    Filter::new("ticker", FilterOp::Eq(ticker))
}

fn candles_of(
    view: &ChartView,
    t: &Table,
    request: &ChartRequest,
) -> (Interval, Vec<(f64, Candle)>) {
    match view.reduce(t, request).data {
        ChartData::Candles { interval, candles } => (
            interval,
            candles.into_iter().map(|(b, c)| (b.from, c)).collect(),
        ),
        other => panic!("not candles: {other:?}"),
    }
}

#[test]
fn candles_at_the_edges_keep_their_whole_bucket() {
    let t = prices(2);
    let view = ChartView::new(
        &t,
        candlestick("date", vec![first_ticker(&t)], XAxis::Value),
    )
    .unwrap();
    let (from, to) = view.x_extent().unwrap();
    let width = (((to - from) / 7.0) * 4.0).ceil() as u32 + 4;
    let (interval, all) = candles_of(
        &view,
        &t,
        &ChartRequest {
            x: None,
            y: None,
            width,
            height: 100,
        },
    );
    assert_eq!(interval, Interval::Week);
    // A window that starts and ends mid-week, at the same pixels per day.
    let (a, b) = (from + 59.0, to - 61.0);
    let narrower = ((f64::from(width) * (b - a) / (to - from)).ceil() as u32).max(1);
    let (interval, part) = candles_of(
        &view,
        &t,
        &ChartRequest {
            x: Some((a, b)),
            y: None,
            width: narrower,
            height: 100,
        },
    );
    assert_eq!(interval, Interval::Week);
    for candle in &part {
        assert!(
            all.contains(candle),
            "{candle:?} differs from the full chart's candle"
        );
    }
}

#[test]
fn a_bucket_takes_each_price_from_a_row_that_has_it() {
    let days: Vec<i32> = vec![20_391, 20_392, 20_393]; // Monday to Wednesday
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("date", DataType::Date32, false),
            Field::new("open", DataType::Float64, true),
            Field::new("high", DataType::Float64, true),
            Field::new("low", DataType::Float64, true),
            Field::new("close", DataType::Float64, true),
        ])),
        vec![
            Arc::new(arrow_array::Date32Array::from(days)) as ArrayRef,
            Arc::new(Float64Array::from(vec![None, Some(10.0), Some(11.0)])) as ArrayRef,
            Arc::new(Float64Array::from(vec![None, Some(12.0), None])) as ArrayRef,
            Arc::new(Float64Array::from(vec![Some(9.0), None, Some(8.0)])) as ArrayRef,
            Arc::new(Float64Array::from(vec![Some(9.5), Some(10.5), None])) as ArrayRef,
        ],
    )
    .unwrap();
    let mut t = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    t.load(&[batch]).unwrap();
    let view = ChartView::new(&t, candlestick("date", Vec::new(), XAxis::Value)).unwrap();
    let (interval, candles) = candles_of(
        &view,
        &t,
        &ChartRequest {
            x: None,
            y: None,
            width: 4,
            height: 100,
        },
    );
    assert_eq!(interval, Interval::Week);
    assert_eq!(candles.len(), 1);
    let candle = candles[0].1;
    let price = |column: &str, slot: Option<usize>| value(&t, column, slot.unwrap());
    assert_eq!(price("open", candle.open), 10.0);
    assert_eq!(price("high", candle.high), 12.0);
    assert_eq!(price("low", candle.low), 8.0);
    assert_eq!(price("close", candle.close), 10.5);
}

#[test]
fn minute_candles_keep_the_repeated_autumn_hour_apart() {
    // Every ten minutes across New York's change back from daylight time.
    let start = "2026-11-01T00:00:00-04:00"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_microsecond();
    let times: Vec<i64> = (0..24).map(|i| start + i * 600_000_000).collect();
    let n = times.len();
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new(
                "at",
                DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, Some("UTC".into())),
                false,
            ),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float64, false),
        ])),
        vec![
            Arc::new(
                arrow_array::TimestampMicrosecondArray::from(times.clone()).with_timezone("UTC"),
            ) as ArrayRef,
            Arc::new(Float64Array::from(vec![1.0; n])) as ArrayRef,
            Arc::new(Float64Array::from(vec![2.0; n])) as ArrayRef,
            Arc::new(Float64Array::from(vec![0.5; n])) as ArrayRef,
            Arc::new(Float64Array::from(vec![1.5; n])) as ArrayRef,
        ],
    )
    .unwrap();
    let mut t = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    t.load(&[batch]).unwrap();
    let view = ChartView::new(&t, candlestick("at", Vec::new(), XAxis::Value)).unwrap();
    let (from, to) = view.x_extent().unwrap();
    // Wide enough for 30-minute candles, too narrow for 15-minute ones.
    let width = ((to - from) / 1_800_000_000.0 * 4.0).ceil() as u32 + 4;
    let (interval, candles) = candles_of(
        &view,
        &t,
        &ChartRequest {
            x: None,
            y: None,
            width,
            height: 100,
        },
    );
    assert_eq!(interval, Interval::Minutes(30));
    // New York's offsets are whole hours, so its half hours are UTC's.
    let mut expected: Vec<i64> = times
        .iter()
        .map(|t| t - t.rem_euclid(1_800_000_000))
        .collect();
    expected.dedup();
    let starts: Vec<i64> = candles.iter().map(|(from, _)| *from as i64).collect();
    assert_eq!(starts, expected);
}

#[test]
fn trading_ticks_are_trading_moments_and_spaced_by_trading_time() {
    let calendar = parallax_engine::calendar::TradingCalendar::nyse();
    let at = |s: &str| {
        s.parse::<jiff::Zoned>()
            .unwrap()
            .timestamp()
            .as_microsecond()
    };
    // Friday 15:00 to Monday 10:00 is an hour and a half of trading.
    let times = [
        at("2026-10-30T15:00:00[America/New_York]"),
        at("2026-11-02T10:00:00[America/New_York]"),
    ];
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new(
                "at",
                DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, Some("UTC".into())),
                false,
            ),
            Field::new("y", DataType::Float64, false),
        ])),
        vec![
            Arc::new(
                arrow_array::TimestampMicrosecondArray::from(times.to_vec()).with_timezone("UTC"),
            ) as ArrayRef,
            Arc::new(Float64Array::from(vec![1.0, 2.0])) as ArrayRef,
        ],
    )
    .unwrap();
    let mut t = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    t.load(&[batch]).unwrap();
    let config = ChartConfig {
        x: "at".into(),
        ..line(XAxis::Trading)
    };
    let view = ChartView::new(&t, config).unwrap();
    let ticks = view
        .reduce(
            &t,
            &ChartRequest {
                x: None,
                y: None,
                width: 1_000,
                height: 100,
            },
        )
        .ticks;
    assert!(ticks.len() >= 3, "{ticks:?}");
    for tick in &ticks {
        let instant = calendar.instant_at(tick.x);
        let local = jiff::Timestamp::from_microsecond(instant)
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::get("America/New_York").unwrap());
        assert_eq!(local.strftime("%H:%M").to_string(), tick.label, "{tick:?}");
        assert!(
            calendar.is_open(parallax_datagen::calendar::epoch_days(local.date())),
            "{tick:?}"
        );
    }
    // On a trading-date axis, no tick names a closed day.
    let t = prices(1);
    let config = ChartConfig {
        x: "date".into(),
        y: vec!["close".into()],
        ..line(XAxis::Trading)
    };
    let view = ChartView::new(&t, config).unwrap();
    let (from, _) = view.x_extent().unwrap();
    let reduced = view.reduce(
        &t,
        &ChartRequest {
            x: Some((from, from + 40.0)),
            y: None,
            width: 2_000,
            height: 100,
        },
    );
    for tick in &reduced.ticks {
        let day = from_epoch_days(calendar.day_at(tick.x as i64));
        assert_eq!(day.strftime("%b %-d").to_string(), tick.label);
        assert!(calendar.is_open(parallax_datagen::calendar::epoch_days(day)));
    }
}

#[test]
fn candles_refuse_a_number_axis_and_a_series_column() {
    let t = prices(2);
    let error = ChartView::new(&t, candlestick("close", Vec::new(), XAxis::Value))
        .unwrap_err()
        .to_string();
    assert!(error.contains("buckets by calendar"), "{error}");
    let config = ChartConfig {
        y: vec!["volume".into()],
        ..candlestick("date", Vec::new(), XAxis::Value)
    };
    let error = ChartView::new(&t, config).unwrap_err().to_string();
    assert!(error.contains("prices only"), "{error}");
    let config = ChartConfig {
        series: Some("ticker".into()),
        ..candlestick("date", Vec::new(), XAxis::Value)
    };
    let error = ChartView::new(&t, config).unwrap_err().to_string();
    assert!(error.contains("one instrument"), "{error}");
}

#[test]
fn number_ticks_turn_compact_once_the_axis_reaches_ten_thousand() {
    let labels = |values: &[f64], width: u32| -> Vec<String> {
        let t = table(values, &vec![Some(1.0); values.len()]);
        let view = ChartView::new(&t, line(XAxis::Value)).unwrap();
        view.reduce(
            &t,
            &ChartRequest {
                x: None,
                y: None,
                width,
                height: 100,
            },
        )
        .ticks
        .into_iter()
        .map(|t| t.label)
        .collect()
    };
    assert_eq!(labels(&[0.0, 2e7], 500), ["0", "5M", "10M", "15M", "20M"]);
    assert_eq!(
        labels(&[0.0, 5e6], 1_000),
        [
            "0", "0.5M", "1.0M", "1.5M", "2.0M", "2.5M", "3.0M", "3.5M", "4.0M", "4.5M", "5.0M"
        ]
    );
    assert_eq!(
        labels(&[-4_000.0, 4_000.0], 400),
        ["-4,000", "-2,000", "0", "2,000", "4,000"]
    );
}
