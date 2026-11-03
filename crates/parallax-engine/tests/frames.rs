//! What frames carry for display: roles, titles and text in the view's formats.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray, TimestampMicrosecondArray};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use parallax_engine::format::{Abbreviation, Format, Negative};
use parallax_engine::view::{meta, role};
use parallax_engine::{Aggregate, ConfigError, Measure, Table, TableOptions, View, ViewConfig};

fn table() -> Table {
    let schema = Schema::new(vec![
        Field::new("desk", DataType::Utf8, true),
        Field::new("level", DataType::Float64, true),
        Field::new("pnl", DataType::Float64, true),
        Field::new(
            "at",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
    ]);
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![
            Some("Rates"),
            Some("Rates"),
            None,
            Some("(blank)"),
        ])),
        // 1.001 and 1.004 look the same at two decimals.
        Arc::new(Float64Array::from(vec![1.001, 1.004, 1.001, 1.004])),
        Arc::new(Float64Array::from(vec![
            -1_250_000.0,
            2_000_000.0,
            10.0,
            0.0,
        ])),
        Arc::new(
            TimestampMicrosecondArray::from(vec![1_772_953_199_000_000; 4]).with_timezone("UTC"),
        ),
    ];
    let batch = RecordBatch::try_new(Arc::new(schema.clone()), columns).unwrap();
    let mut t = Table::new(&schema, TableOptions::default()).unwrap();
    t.load(&[batch]).unwrap();
    t
}

fn fields_with(batch: &RecordBatch, wanted: &str) -> Vec<usize> {
    let schema = batch.schema();
    (0..schema.fields().len())
        .filter(|&i| {
            schema
                .field(i)
                .metadata()
                .get(meta::ROLE)
                .map(String::as_str)
                == Some(wanted)
        })
        .collect()
}

fn text_of(batch: &RecordBatch, index: usize) -> Vec<String> {
    let schema = batch.schema();
    let i = fields_with(batch, role::TEXT)
        .into_iter()
        .find(|&i| schema.field(i).metadata()[meta::INDEX] == index.to_string())
        .unwrap();
    batch
        .column(i)
        .as_string::<i32>()
        .iter()
        .map(|t| t.unwrap().to_owned())
        .collect()
}

fn title_of(batch: &RecordBatch, index: usize) -> String {
    let schema = batch.schema();
    let i = fields_with(batch, role::VALUE)
        .into_iter()
        .find(|&i| schema.field(i).metadata()[meta::INDEX] == index.to_string())
        .unwrap();
    schema.field(i).metadata()[meta::TITLE].clone()
}

#[test]
fn pivot_columns_that_display_alike_stay_distinct() {
    let t = table();
    let mut view = View::new(
        &t,
        ViewConfig {
            split_by: vec!["level".into()],
            measures: vec![Measure::new(Aggregate::Sum, "pnl")],
            ..ViewConfig::default()
        },
    )
    .unwrap();
    let frame = view.window(&t, 0..10, 0..10);
    let values = fields_with(&frame.batch, role::VALUE);
    // Two pivot columns and the row total.
    assert_eq!(values.len(), 3);
    let schema = frame.batch.schema();
    let names: Vec<&str> = values
        .iter()
        .map(|&i| schema.field(i).name().as_str())
        .collect();
    assert_ne!(names[0], names[1], "names are identities and never merge");
    // Titles are display text, so these two read the same.
    assert_eq!(title_of(&frame.batch, 0), "1.00 | sum(pnl)");
    assert_eq!(title_of(&frame.batch, 1), "1.00 | sum(pnl)");
    assert_eq!(title_of(&frame.batch, 2), "Total | sum(pnl)");
}

#[test]
fn labels_and_measures_use_the_formats() {
    let t = table();
    let mut formats = BTreeMap::new();
    formats.insert(
        "pnl".to_owned(),
        Format {
            abbreviate: Some(Abbreviation::Millions),
            negative: Negative::Parentheses,
            ..Format::default()
        },
    );
    let mut view = View::new(
        &t,
        ViewConfig {
            group_by: vec!["desk".into()],
            measures: vec![
                Measure::new(Aggregate::Sum, "pnl"),
                Measure::count_all(),
                Measure::new(Aggregate::Max, "at"),
            ],
            formats,
            ..ViewConfig::default()
        },
    )
    .unwrap();
    let frame = view.window(&t, 0..10, 0..10);
    let batch = &frame.batch;
    let labels: Vec<&str> = batch
        .column(fields_with(batch, role::LABEL)[0])
        .as_string::<i32>()
        .iter()
        .map(Option::unwrap)
        .collect();
    // A null group reads "(blank)" and sorts last; text that happens to read
    // "(blank)" is an ordinary value.
    assert_eq!(labels, ["Total", "(blank)", "Rates", "(blank)"]);
    assert_eq!(text_of(batch, 0), ["0.8M", "0.0M", "0.8M", "0.0M"]);
    assert_eq!(text_of(batch, 1), ["4", "1", "2", "1"]);
    // The instant shows in New York, and the header says so.
    assert_eq!(text_of(batch, 2)[0], "2026-03-08 01:59:59");
    assert_eq!(title_of(batch, 2), "max(at) (America/New_York)");
    // Timestamps also carry every digit for copying.
    assert_eq!(fields_with(batch, role::COPY).len(), 1);
}

#[test]
fn negative_sums_show_in_parentheses() {
    let t = table();
    let mut formats = BTreeMap::new();
    formats.insert(
        "pnl".to_owned(),
        Format {
            negative: Negative::Parentheses,
            ..Format::default()
        },
    );
    let mut view = View::new(
        &t,
        ViewConfig {
            columns: vec!["pnl".into()],
            formats,
            ..ViewConfig::default()
        },
    )
    .unwrap();
    let frame = view.window(&t, 0..10, 0..10);
    assert_eq!(
        text_of(&frame.batch, 0),
        ["(1,250,000.00)", "2,000,000.00", "10.00", "0.00"]
    );
}

#[test]
fn bad_formats_are_configuration_errors() {
    let t = table();
    let with = |column: &str, format: Format| {
        View::new(
            &t,
            ViewConfig {
                formats: BTreeMap::from([(column.to_owned(), format)]),
                ..ViewConfig::default()
            },
        )
        .map(|_| ())
    };
    assert!(matches!(
        with("nope", Format::default()),
        Err(ConfigError::UnknownColumn(_))
    ));
    assert!(matches!(
        with(
            "at",
            Format {
                zone: Some("Not/AZone".into()),
                ..Format::default()
            }
        ),
        Err(ConfigError::Format { .. })
    ));
}

#[test]
fn flat_columns_keep_the_table_fields_properties() {
    let t = table();
    let mut view = View::new(&t, ViewConfig::default()).unwrap();
    let frame = view.window(&t, 0..1, 0..4);
    let schema = frame.batch.schema();
    for (k, i) in fields_with(&frame.batch, role::VALUE)
        .into_iter()
        .enumerate()
    {
        let field = schema.field(i);
        let source = t.schema().field(k);
        assert_eq!(field.name(), source.name());
        assert_eq!(field.data_type(), source.data_type());
        assert_eq!(field.is_nullable(), source.is_nullable());
        assert_eq!(field.metadata()[meta::INDEX], k.to_string());
    }
}
