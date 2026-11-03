//! Edge cases where a plausible but wrong number would be easy to produce.

use std::sync::Arc;

use arrow_array::{ArrayRef, Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parallax_engine::{
    Aggregate, CellValue, ConfigError, Measure, Scalar, Table, TableOptions, View, ViewConfig,
};

fn table(columns: Vec<(&str, ArrayRef)>) -> Table {
    let schema = Schema::new(
        columns
            .iter()
            .map(|(name, a)| Field::new(*name, a.data_type().clone(), true))
            .collect::<Vec<_>>(),
    );
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        columns.into_iter().map(|(_, a)| a).collect(),
    )
    .unwrap();
    let mut t = Table::new(&schema, TableOptions::default()).unwrap();
    t.load(&[batch]).unwrap();
    t
}

fn total(table: &Table, measures: Vec<Measure>) -> Vec<CellValue> {
    let mut view = View::new(
        table,
        ViewConfig {
            measures,
            ..ViewConfig::default()
        },
    )
    .unwrap();
    view.cells(table)
        .into_iter()
        .find(|c| c.path.is_empty() && c.split.is_none())
        .unwrap()
        .values
}

fn floats(values: &[f64]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}

#[test]
fn identical_values_have_zero_variance() {
    let t = table(vec![("x", floats(&[0.1, 0.1, 0.1, 0.1]))]);
    assert_eq!(
        total(&t, vec![Measure::new(Aggregate::Variance, "x")]),
        vec![CellValue::Float(0.0)]
    );
}

#[test]
fn float_variance_is_exact_for_large_values() {
    let t = table(vec![("x", floats(&[1e150, 2e150]))]);
    // The variance of {1e150, 2e150} is (1e150)^2 / 2 = 5e299.
    let expected = 5e299;
    match &total(&t, vec![Measure::new(Aggregate::Variance, "x")])[0] {
        CellValue::Float(v) => assert!((v - expected).abs() <= expected * 1e-15, "{v}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn squares_beyond_the_double_range_overflow_rather_than_vanish() {
    let t = table(vec![("x", floats(&[1e200, -1e200]))]);
    assert_eq!(
        total(&t, vec![Measure::new(Aggregate::Variance, "x")]),
        vec![CellValue::Overflow]
    );
}

#[test]
fn huge_decimals_overflow_variance_and_weighted_means_instead_of_wrapping() {
    let big = 10i128.pow(37);
    let values: Decimal128Array = (0..500).map(|i| Some(big + i)).collect();
    let values = values.with_precision_and_scale(38, 0).unwrap();
    let weights: Decimal128Array = (0..500).map(|_| Some(big)).collect();
    let weights = weights.with_precision_and_scale(38, 0).unwrap();
    let t = table(vec![("x", Arc::new(values)), ("w", Arc::new(weights))]);
    let out = total(
        &t,
        vec![
            Measure::new(Aggregate::Variance, "x"),
            Measure::weighted_mean("x", "w"),
            Measure::new(Aggregate::Sum, "x"),
        ],
    );
    assert_eq!(out[0], CellValue::Overflow);
    assert_eq!(out[1], CellValue::Overflow);
    // 500 values of about 10^37 sum to about 5 * 10^39, beyond 38 digits.
    assert_eq!(out[2], CellValue::Overflow);
}

#[test]
fn decimal_variance_is_exact() {
    // 1.00, 1.01, 1.02 at scale 2: variance of {100, 101, 102} / 100^2 = 1 / 10^4.
    let values = Decimal128Array::from(vec![100, 101, 102])
        .with_precision_and_scale(10, 2)
        .unwrap();
    let t = table(vec![("x", Arc::new(values))]);
    assert_eq!(
        total(&t, vec![Measure::new(Aggregate::Variance, "x")]),
        vec![CellValue::Float(1e-4)]
    );
}

#[test]
fn medians_of_infinities_are_infinite() {
    let inf = f64::INFINITY;
    let t = table(vec![("x", floats(&[inf, inf, inf, 1.0]))]);
    assert_eq!(
        total(
            &t,
            vec![
                Measure::new(Aggregate::Median, "x"),
                Measure::new(Aggregate::Percentile(0.9), "x"),
            ]
        ),
        vec![CellValue::Float(inf), CellValue::Float(inf)]
    );
}

#[test]
fn negative_decimal_scales_aggregate_correctly() {
    let values = Decimal128Array::from(vec![12, 3])
        .with_precision_and_scale(10, -2)
        .unwrap();
    let t = table(vec![("n", Arc::new(values))]);
    // 1200 + 300, held at scale 0.
    assert_eq!(
        total(
            &t,
            vec![
                Measure::new(Aggregate::Sum, "n"),
                Measure::new(Aggregate::Mean, "n")
            ]
        ),
        vec![CellValue::Decimal(1_500), CellValue::Float(750.0)]
    );
}

#[test]
fn column_pivots_beyond_the_limit_are_refused() {
    let ids: Int64Array = (0..20_000).collect();
    let t = table(vec![
        ("id", Arc::new(ids)),
        ("x", floats(&vec![1.0; 20_000])),
    ]);
    let err = View::new(
        &t,
        ViewConfig {
            split_by: vec!["id".into()],
            measures: vec![Measure::new(Aggregate::Sum, "x")],
            ..ViewConfig::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, ConfigError::TooManyColumns(_, _)), "{err}");
}

#[test]
fn text_min_and_max_follow_the_engine_collation() {
    let names = StringArray::from(vec!["beta", "Alpha", "alpha", "Gamma"]);
    let t = table(vec![("s", Arc::new(names))]);
    assert_eq!(
        total(
            &t,
            vec![
                Measure::new(Aggregate::Min, "s"),
                Measure::new(Aggregate::Max, "s")
            ]
        ),
        vec![
            CellValue::Value(Scalar::Utf8("Alpha".into())),
            CellValue::Value(Scalar::Utf8("Gamma".into()))
        ]
    );
    let _ = DataType::Utf8;
}

#[test]
fn an_in_list_can_match_blanks() {
    use parallax_engine::{Filter, FilterOp};
    let t = table(vec![(
        "desk",
        Arc::new(StringArray::from(vec![Some("Rates"), None, Some("FX")])) as ArrayRef,
    )]);
    let count = |op: FilterOp| {
        let mut view = View::new(
            &t,
            ViewConfig {
                filters: vec![Filter::new("desk", op)],
                ..ViewConfig::default()
            },
        )
        .unwrap();
        view.num_rows(&t)
    };
    assert_eq!(count(FilterOp::In(vec![Scalar::Null])), 1);
    assert_eq!(
        count(FilterOp::In(vec![Scalar::Null, Scalar::Utf8("FX".into())])),
        2
    );
    assert_eq!(count(FilterOp::In(vec![Scalar::Utf8("FX".into())])), 1);
    // "Not in" keeps SQL's rule: nulls never match, and listing null is an error.
    assert_eq!(count(FilterOp::NotIn(vec![Scalar::Utf8("FX".into())])), 1);
    assert!(
        View::new(
            &t,
            ViewConfig {
                filters: vec![Filter::new("desk", FilterOp::NotIn(vec![Scalar::Null]))],
                ..ViewConfig::default()
            },
        )
        .is_err()
    );
}
