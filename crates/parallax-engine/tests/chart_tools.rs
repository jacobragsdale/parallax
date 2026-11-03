//! Compare mode, event markers and measuring, on charts of a few hand-made rows.

#![allow(missing_docs)]

use std::sync::Arc;

use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray};
use arrow_schema::{Field, Schema};
use parallax_engine::chart::{
    ChartConfig, ChartData, ChartKind, ChartRequest, ChartView, MAX_EVENTS, OhlcColumns, Point,
    XAxis, percent_change,
};
use parallax_engine::indicator::Indicator;
use parallax_engine::{ConfigError, Scalar, Table, TableOptions};

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

/// Two tickers on days 0 to 9: AAA from 10 to 19, BBB from 200 to 290; every
/// third day is an event.
fn two_tickers() -> Table {
    let days: Vec<f64> = (0..10).map(f64::from).collect();
    let x: Vec<f64> = days.iter().chain(&days).copied().collect();
    let ticker: Vec<&str> = std::iter::repeat_n("AAA", 10)
        .chain(std::iter::repeat_n("BBB", 10))
        .collect();
    let close: Vec<f64> = days
        .iter()
        .map(|d| 10.0 + d)
        .chain(days.iter().map(|d| 200.0 + 10.0 * d))
        .collect();
    let event: Vec<Option<&str>> = (0..20).map(|i| (i % 3 == 0).then_some("event")).collect();
    table(vec![
        ("x", Arc::new(Float64Array::from(x)) as ArrayRef),
        ("ticker", Arc::new(StringArray::from(ticker)) as ArrayRef),
        ("close", Arc::new(Float64Array::from(close)) as ArrayRef),
        ("event", Arc::new(StringArray::from(event)) as ArrayRef),
    ])
}

fn line(compare: bool) -> ChartConfig {
    ChartConfig {
        kind: ChartKind::Line,
        x: "x".into(),
        y: vec!["close".into()],
        series: Some("ticker".into()),
        ohlc: None,
        filters: Vec::new(),
        axis: XAxis::Value,
        indicators: Vec::new(),
        volume: None,
        compare,
        events: Some("event".into()),
    }
}

fn request(x: Option<(f64, f64)>) -> ChartRequest {
    ChartRequest {
        x,
        y: None,
        width: 400,
        height: 200,
    }
}

fn series(view: &ChartView, t: &Table, x: Option<(f64, f64)>) -> Vec<Vec<Point>> {
    match view.reduce(t, &request(x)).data {
        ChartData::Series(s) => s.into_iter().map(|(_, p)| p).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn compare_mode_puts_each_series_on_its_change_from_the_first_value_shown() {
    let t = two_tickers();
    let view = ChartView::new(&t, line(true)).unwrap();
    let all = series(&view, &t, None);
    // Both start at 0 %, and AAA's day 9 is 90 % up from 10; BBB's 45 % from 200.
    let at = |points: &[Point], x: f64| points.iter().find(|p| p.x == x).unwrap().y;
    assert_eq!((at(&all[0], 0.0), at(&all[1], 0.0)), (0.0, 0.0));
    assert_eq!(at(&all[0], 9.0), 90.0);
    assert_eq!(at(&all[1], 9.0), 45.0);
    // Zoomed to days 5 to 9, the first value shown, day 5's, is the base.
    let zoomed = series(&view, &t, Some((5.0, 9.0)));
    assert_eq!(at(&zoomed[0], 5.0), 0.0);
    assert_eq!(at(&zoomed[0], 9.0), percent_change(15.0, 19.0));
    // Without compare mode, prices are prices.
    let plain = ChartView::new(&t, line(false)).unwrap();
    assert_eq!(at(&series(&plain, &t, None)[1], 9.0), 290.0);
}

#[test]
fn compare_mode_refuses_what_it_cannot_show() {
    let t = two_tickers();
    let refuse = |config: ChartConfig| match ChartView::new(&t, config) {
        Err(ConfigError::Chart(m)) => m,
        other => panic!("{other:?}"),
    };
    assert!(
        refuse(ChartConfig {
            indicators: vec![Indicator::Sma { period: 3 }],
            ..line(true)
        })
        .contains("percentage")
    );
    let candles = ChartConfig {
        kind: ChartKind::Candlestick,
        y: Vec::new(),
        series: None,
        ohlc: Some(OhlcColumns {
            open: "close".into(),
            high: "close".into(),
            low: "close".into(),
            close: "close".into(),
        }),
        ..line(true)
    };
    assert!(refuse(candles).contains("line or area"));
    assert!(
        refuse(ChartConfig {
            kind: ChartKind::Histogram,
            y: Vec::new(),
            series: None,
            ..line(false)
        })
        .contains("events mark")
    );
    // An oscillator in a pane of its own is fine.
    assert!(
        ChartView::new(
            &t,
            ChartConfig {
                indicators: vec![Indicator::Rsi { period: 3 }],
                ..line(true)
            }
        )
        .is_ok()
    );
}

#[test]
fn events_are_the_marked_rows_in_range_thinned_evenly_past_the_most() {
    let t = two_tickers();
    let view = ChartView::new(&t, line(false)).unwrap();
    let reduced = view.reduce(&t, &request(Some((2.0, 7.0))));
    // Rows 3 and 6 of AAA, and BBB's days 2 and 5 (its rows 12 and 15), in x order.
    let marked: Vec<(f64, Option<Scalar>)> = reduced
        .events
        .iter()
        .map(|e| (e.x, e.key.clone()))
        .collect();
    assert_eq!(
        marked,
        vec![
            (2.0, Some(Scalar::Utf8("BBB".into()))),
            (3.0, Some(Scalar::Utf8("AAA".into()))),
            (5.0, Some(Scalar::Utf8("BBB".into()))),
            (6.0, Some(Scalar::Utf8("AAA".into()))),
        ]
    );
    assert_eq!(reduced.events_total, 4);
    // Twice the most there may be: every other one is kept, the first and last among them.
    let n = 2 * MAX_EVENTS;
    let many = table(vec![
        (
            "x",
            Arc::new(Float64Array::from(
                (0..n).map(|i| i as f64).collect::<Vec<_>>(),
            )) as ArrayRef,
        ),
        (
            "close",
            Arc::new(Float64Array::from(vec![1.0; n])) as ArrayRef,
        ),
        (
            "event",
            Arc::new(StringArray::from(vec!["e"; n])) as ArrayRef,
        ),
    ]);
    let view = ChartView::new(
        &many,
        ChartConfig {
            series: None,
            ..line(false)
        },
    )
    .unwrap();
    let reduced = view.reduce(&many, &request(None));
    assert_eq!(
        (reduced.events.len(), reduced.events_total),
        (MAX_EVENTS, n)
    );
    assert_eq!(
        (reduced.events[0].x, reduced.events[MAX_EVENTS - 1].x),
        (0.0, (n - 1) as f64)
    );
}

#[test]
fn measuring_takes_the_nearest_priced_rows_and_counts_the_bars_between() {
    let x: Vec<f64> = (0..10).map(f64::from).collect();
    let close: Vec<Option<f64>> = (0..10)
        .map(|d| (d != 3).then_some(100.0 + f64::from(d)))
        .collect();
    let t = table(vec![
        ("x", Arc::new(Float64Array::from(x)) as ArrayRef),
        ("close", Arc::new(Float64Array::from(close)) as ArrayRef),
    ]);
    let view = ChartView::new(
        &t,
        ChartConfig {
            series: None,
            events: None,
            ..line(false)
        },
    )
    .unwrap();
    // From near day 1 to near day 7, either way round; day 3 has no price.
    let m = view.measure(&t, 7.2, 0.8, None).unwrap();
    assert_eq!((m.from_x, m.to_x), (1.0, 7.0));
    assert_eq!(m.rows, 5, "days 2, 4, 5, 6 and 7");
    // Near the missing day, the nearest priced row is taken instead.
    let m = view.measure(&t, 3.1, 3.4, None).unwrap();
    assert_eq!((m.from_x, m.to_x, m.rows), (4.0, 4.0, 0));
    // With a series column, the series asked for.
    let tickers = two_tickers();
    let view = ChartView::new(&tickers, line(false)).unwrap();
    let m = view
        .measure(&tickers, 0.0, 9.0, Some(&Scalar::Utf8("BBB".into())))
        .unwrap();
    assert_eq!(m.rows, 9);
    assert_eq!(
        tickers
            .column(tickers.column_index("ticker").unwrap())
            .scalar(m.to),
        Scalar::Utf8("BBB".into())
    );
}

#[test]
fn a_comparison_holds_its_line_between_rows_and_reads_losses_as_falls() {
    let t = two_tickers();
    let view = ChartView::new(&t, line(true)).unwrap();
    // A window between two of each series' rows holds none of them: each line
    // still crosses it, based where it enters.
    let between = series(&view, &t, Some((4.25, 4.75)));
    for points in &between {
        let drawn: Vec<f64> = points
            .iter()
            .filter(|p| !p.y.is_nan())
            .map(|p| p.y)
            .collect();
        assert_eq!(drawn.len(), 2, "{points:?}");
        assert_eq!(drawn[0], 0.0);
    }
    // A loss from -100 to -200 has fallen by 100 %; from 100 to 50, by half.
    assert_eq!(percent_change(-100.0, -200.0), -100.0);
    assert_eq!(percent_change(100.0, 50.0), -50.0);
    assert_eq!(percent_change(-100.0, -50.0), 50.0);
}
