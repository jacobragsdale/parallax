//! Exports read back: the Excel file with calamine and its XML, and the CSV as
//! text, compared with the engine's own view of the same layout.

#![allow(missing_docs)]

use std::io::{Cursor, Read};
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, Decimal128Type, Float64Type, Int64Type, UInt8Type};
use arrow_array::{
    Array, ArrayRef, Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use calamine::{Data, Reader, Xlsx, open_workbook_from_rs};
use parallax_datagen::Dataset;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_engine::format::exact_decimal;
use parallax_engine::view::{meta, role};
use parallax_engine::{Table, TableOptions, View};
use parallax_export::{EXCEL_MAX_ROWS, ExportError, ExportSource, write_csv, write_xlsx};
use parallax_protocol::layout::Layout;

fn positions(rows: u64) -> Table {
    let batch = Positions::new(PositionsOptions {
        rows,
        seed: 11,
        securities: 150,
        hostile: true,
        ..PositionsOptions::default()
    })
    .unwrap()
    .batch(0..rows);
    table_of(&batch)
}

fn table_of(batch: &RecordBatch) -> Table {
    let mut table = Table::new(&batch.schema(), TableOptions::default()).unwrap();
    table.load(std::slice::from_ref(batch)).unwrap();
    table
}

fn xlsx(table: &Table, layout: &str) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    write_xlsx(
        table,
        &Layout::from_json(layout).unwrap(),
        &ExportSource::default(),
        &mut out,
    )
    .unwrap();
    out.into_inner()
}

fn sheet(bytes: &[u8]) -> calamine::Range<Data> {
    let mut book: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes.to_vec())).unwrap();
    book.worksheet_range("View").unwrap()
}

fn zipped(bytes: &[u8], name: &str) -> String {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

/// The engine's own frame of the whole view, with every group shown.
fn engine_frame(table: &Table, layout: &str) -> RecordBatch {
    let resolved = Layout::from_json(layout).unwrap().resolve(table).unwrap();
    let mut view = View::new(table, resolved.config).unwrap();
    for (path, expanded) in resolved.expanded {
        view.set_expanded(path, expanded);
    }
    view.show_all(true);
    let rows = view.num_rows(table);
    let columns = view.num_columns(table);
    view.window(table, 0..rows, 0..columns).batch
}

/// Each data column's title, values, flags and display text, in order.
fn values_of(batch: &RecordBatch) -> Vec<(String, ArrayRef, Option<ArrayRef>, ArrayRef)> {
    let schema = batch.schema();
    let field_role = |i: usize| {
        schema
            .field(i)
            .metadata()
            .get(meta::ROLE)
            .cloned()
            .unwrap_or_default()
    };
    let index = |i: usize| schema.field(i).metadata().get(meta::INDEX).cloned();
    let part = |r: &str, i: usize| {
        (0..schema.fields().len())
            .find(|&j| field_role(j) == r && index(j) == index(i))
            .map(|j| Arc::clone(batch.column(j)))
    };
    (0..schema.fields().len())
        .filter(|&i| field_role(i) == role::VALUE)
        .map(|i| {
            (
                schema.field(i).metadata()[meta::TITLE].clone(),
                Arc::clone(batch.column(i)),
                part(role::FLAG, i),
                part(role::TEXT, i).expect("display text"),
            )
        })
        .collect()
}

/// The Excel serial of a wall-clock time shown as `YYYY-MM-DD HH:MM:SS`.
fn serial_of(text: &str) -> f64 {
    let civil: jiff::civil::DateTime = text.replacen(' ', "T", 1).parse().unwrap();
    let days = civil
        .date()
        .since(jiff::civil::date(1899, 12, 30))
        .unwrap()
        .get_days();
    let seconds = civil
        .time()
        .since(jiff::civil::time(0, 0, 0, 0))
        .unwrap()
        .total(jiff::Unit::Second)
        .unwrap();
    f64::from(days) + seconds / 86_400.0
}

fn flagged(flags: Option<&ArrayRef>, row: usize) -> bool {
    flags.is_some_and(|f| f.as_primitive::<UInt8Type>().value(row) != 0)
}

/// Checks that an Excel cell holds exactly the engine's value.
fn same(cell: &Data, values: &ArrayRef, text: &ArrayRef, flagged: bool, row: usize, title: &str) {
    if flagged {
        assert!(
            matches!(cell, Data::String(_)),
            "{title} row {row}: a flagged cell is text, got {cell:?}"
        );
        return;
    }
    if values.is_null(row) {
        assert_eq!(cell, &Data::Empty, "{title} row {row}");
        return;
    }
    let number = |c: &Data| match c {
        Data::Float(f) => *f,
        Data::Int(i) => *i as f64,
        Data::DateTime(d) => d.as_f64(),
        other => panic!("{title} row {row}: expected a number, got {other:?}"),
    };
    match values.data_type() {
        DataType::Float64 => {
            let v = values.as_primitive::<Float64Type>().value(row);
            if v.is_finite() {
                assert_eq!(number(cell).to_bits(), v.to_bits(), "{title} row {row}");
            } else {
                assert!(
                    matches!(cell, Data::String(s) if s.contains('∞')),
                    "{title} row {row}: {cell:?}"
                );
            }
        }
        DataType::Int64 => assert_eq!(
            number(cell),
            values.as_primitive::<Int64Type>().value(row) as f64,
            "{title} row {row}"
        ),
        DataType::Decimal128(_, scale) => {
            let exact = exact_decimal(
                values.as_primitive::<Decimal128Type>().value(row),
                i32::from(*scale),
            );
            assert_eq!(
                number(cell),
                exact.parse::<f64>().unwrap(),
                "{title} row {row}"
            );
        }
        DataType::Date32 => assert_eq!(
            number(cell),
            f64::from(values.as_primitive::<Date32Type>().value(row)) + 25_569.0,
            "{title} row {row}"
        ),
        DataType::Utf8 => match values.as_string::<i32>().value(row) {
            // Excel stores empty text as a blank cell.
            "" => assert_eq!(cell, &Data::Empty, "{title} row {row}"),
            text => assert_eq!(cell, &Data::String(text.to_owned()), "{title} row {row}"),
        },
        DataType::Boolean => assert_eq!(
            cell,
            &Data::Bool(values.as_boolean().value(row)),
            "{title} row {row}"
        ),
        DataType::Timestamp(..) => {
            let expected = serial_of(text.as_string::<i32>().value(row));
            assert!(
                (number(cell) - expected).abs() < 1e-9,
                "{title} row {row}: {cell:?} against {expected}"
            );
        }
        other => panic!("unexpected type {other}"),
    }
}

#[test]
fn a_flat_export_holds_the_engine_values_exactly() {
    let table = positions(1_500);
    let layout = r#"{"sort":[{"column":"market_value_usd","descending":true}],
        "formats":{"market_value_usd":{"abbreviate":"millions","negative":"parentheses"},"beta":{"decimals":3}}}"#;
    let range = sheet(&xlsx(&table, layout));
    let frame = engine_frame(&table, layout);
    assert_eq!(range.height(), frame.num_rows() + 1);
    for (c, (title, values, flags, text)) in values_of(&frame).iter().enumerate() {
        assert_eq!(range.get((0, c)), Some(&Data::String(title.clone())));
        for row in 0..frame.num_rows() {
            same(
                range.get((row + 1, c)).unwrap_or(&Data::Empty),
                values,
                text,
                flagged(flags.as_ref(), row),
                row,
                title,
            );
        }
    }
}

#[test]
fn a_pivot_exports_every_group_as_an_outline() {
    let table = positions(3_000);
    let layout = r#"{"group_by":["fund","strategy"],"split_by":["long_short"],
        "measures":[{"column":"market_value_usd","aggregate":"sum"},{"aggregate":"count_all"},
                    {"column":"market_value_usd","aggregate":"sum","show_as":"percent_of_grand_total"}],
        "expand_depth":1,"expanded":[{"path":["Atlas Global Macro"],"expanded":false}]}"#;
    let bytes = xlsx(&table, layout);
    let range = sheet(&bytes);
    let frame = engine_frame(&table, layout);
    assert_eq!(
        range.height(),
        frame.num_rows() + 1,
        "every group, collapsed or not, plus the header"
    );
    let labels = frame
        .column_by_name("__label")
        .unwrap()
        .as_string::<i32>()
        .clone();
    for row in 0..frame.num_rows() {
        assert_eq!(
            range.get((row + 1, 0)),
            Some(&Data::String(labels.value(row).to_owned()))
        );
    }
    for (c, (title, values, flags, text)) in values_of(&frame).iter().enumerate() {
        for row in 0..frame.num_rows() {
            same(
                range.get((row + 1, c + 1)).unwrap_or(&Data::Empty),
                values,
                text,
                flagged(flags.as_ref(), row),
                row,
                title,
            );
        }
    }
    // Every group is exported expanded, collapsed on screen or not: Excel's
    // outline buttons collapse them.
    let xml = zipped(&bytes, "xl/worksheets/sheet1.xml");
    assert_eq!(xml.matches("hidden=\"1\"").count(), 0);
    assert!(!xml.contains("collapsed=\"1\""));
    assert!(
        xml.contains("outlineLevel=\"2\""),
        "strategies sit two levels in"
    );
    assert!(
        xml.contains("summaryBelow=\"0\""),
        "the buttons sit beside the group rows, above their children"
    );
}

#[test]
fn negatives_that_round_to_zero_show_no_minus_in_excel() {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)])),
        vec![Arc::new(Float64Array::from(vec![-0.001, -1.5, 2.0])) as ArrayRef],
    )
    .unwrap();
    let bytes = xlsx(&table_of(&batch), "{}");
    let styles = zipped(&bytes, "xl/styles.xml");
    assert!(
        styles.contains("formatCode=\"#,##0.00;#,##0.00\""),
        "{styles}"
    );
    let xml = zipped(&bytes, "xl/worksheets/sheet1.xml");
    // A2 holds -0.001 with the unsigned format; A3 and A4 share the signed one.
    let style = |cell: &str| {
        let at = xml.find(&format!("r=\"{cell}\"")).unwrap();
        let rest = &xml[at..];
        rest[rest.find("s=\"").unwrap() + 3..]
            .split('"')
            .next()
            .unwrap()
            .to_owned()
    };
    assert_ne!(style("A2"), style("A3"));
    assert_eq!(style("A3"), style("A4"));
}

#[test]
fn more_rows_than_a_sheet_holds_is_refused() {
    let rows = i64::try_from(EXCEL_MAX_ROWS).unwrap();
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from_iter_values(0..rows)) as ArrayRef],
    )
    .unwrap();
    let error = write_xlsx(
        &table_of(&batch),
        &Layout::default(),
        &ExportSource::default(),
        Cursor::new(Vec::new()),
    )
    .unwrap_err();
    assert!(matches!(error, ExportError::TooManyRows { .. }), "{error}");
    assert!(error.to_string().contains("CSV"));
}

#[test]
fn text_longer_than_a_cell_is_refused_not_cut() {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("note", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec![
            "short".to_owned(),
            "x".repeat(40_000),
        ])) as ArrayRef],
    )
    .unwrap();
    let error = write_xlsx(
        &table_of(&batch),
        &Layout::default(),
        &ExportSource::default(),
        Cursor::new(Vec::new()),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            ExportError::TextTooLong {
                row: 1,
                length: 40_000,
                ..
            }
        ),
        "{error}"
    );
}

fn long_decimals() -> Table {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("notional", DataType::Decimal128(38, 2), false),
            Field::new("rate", DataType::Float64, false),
        ])),
        vec![
            Arc::new(
                Decimal128Array::from(vec![12_345_678_901_234_567_890_i128, -5])
                    .with_precision_and_scale(38, 2)
                    .unwrap(),
            ) as ArrayRef,
            Arc::new(Float64Array::from(vec![0.1 + 0.2, 1e300])) as ArrayRef,
        ],
    )
    .unwrap();
    table_of(&batch)
}

#[test]
fn the_notes_sheet_says_where_excel_cannot_match_the_screen() {
    let mut out = Cursor::new(Vec::new());
    let source = ExportSource {
        dataset: "rates".into(),
        as_of: "2026-11-02 16:00:00".into(),
    };
    let summary = write_xlsx(&long_decimals(), &Layout::default(), &source, &mut out).unwrap();
    assert!(
        summary
            .notes
            .iter()
            .any(|n| n.contains("15 significant digits")),
        "{:?}",
        summary.notes
    );
    let mut book: Xlsx<_> = open_workbook_from_rs(Cursor::new(out.into_inner())).unwrap();
    let notes = book.worksheet_range("Notes").unwrap();
    assert_eq!(notes.get((0, 1)), Some(&Data::String("rates".into())));
    assert!(
        notes
            .rows()
            .any(|r| matches!(&r[1], Data::String(s) if s.contains("15 significant digits")))
    );
}

#[test]
fn csv_holds_every_digit() {
    let mut out = Vec::new();
    write_csv(&long_decimals(), &Layout::default(), &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let text = text
        .strip_prefix('\u{feff}')
        .expect("a byte-order mark for Excel");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "notional,rate");
    assert_eq!(lines[1], "123456789012345678.90,0.30000000000000004");
    assert_eq!(lines[2], "-0.05,1e300");
    assert_eq!("0.30000000000000004".parse::<f64>().unwrap(), 0.1 + 0.2);
}

#[test]
fn a_pivot_csv_names_each_level() {
    let table = positions(800);
    let mut out = Vec::new();
    let layout = Layout::from_json(
        r#"{"group_by":["fund","strategy"],"measures":[{"aggregate":"count_all"}]}"#,
    )
    .unwrap();
    write_csv(&table, &layout, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    assert_eq!(lines.next(), Some("level,fund,strategy,count(*)"));
    assert_eq!(lines.next(), Some("0,,,800"));
    let fund = lines.next().unwrap();
    assert!(fund.starts_with("1,"), "{fund}");
    let strategy = lines.next().unwrap();
    assert!(
        strategy.starts_with("2,") && strategy.split(',').nth(1) == fund.split(',').nth(1),
        "{strategy} under {fund}"
    );
}

/// A table that reloads after its first read, as one can between export chunks.
struct Reloading {
    reads: std::sync::atomic::AtomicUsize,
    before: Table,
    after: Table,
}

impl parallax_export::ReadTable for Reloading {
    type Guard<'a> = &'a Table;

    fn read(&self) -> Result<&Table, String> {
        let n = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(if n == 0 { &self.before } else { &self.after })
    }
}

#[test]
fn a_reload_part_way_stops_the_export() {
    let before = positions(200);
    let mut after = positions(200);
    let batch = Positions::new(PositionsOptions {
        rows: 200,
        seed: 12,
        securities: 150,
        ..PositionsOptions::default()
    })
    .unwrap()
    .batch(0..200);
    after.load(&[batch]).unwrap();
    assert_ne!(before.load_version(), after.load_version());
    let tables = Reloading {
        reads: std::sync::atomic::AtomicUsize::new(0),
        before,
        after,
    };
    let error = write_xlsx(
        &tables,
        &Layout::default(),
        &ExportSource::default(),
        Cursor::new(Vec::new()),
    )
    .unwrap_err();
    assert!(matches!(error, ExportError::Reloaded), "{error}");
    tables.reads.store(0, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        write_csv(&tables, &Layout::default(), Vec::new()).unwrap_err(),
        ExportError::Reloaded
    ));
}

#[test]
fn a_layout_longer_than_a_cell_spreads_over_note_rows() {
    let table = positions(100);
    let values: Vec<String> = (0..3_000)
        .map(|i| format!("\"fund number {i:05}\""))
        .collect();
    let layout = format!(
        r#"{{"filters":[{{"column":"fund","op":"not_in","values":[{}]}}]}}"#,
        values.join(",")
    );
    let mut out = Cursor::new(Vec::new());
    write_xlsx(
        &table,
        &Layout::from_json(&layout).unwrap(),
        &ExportSource::default(),
        &mut out,
    )
    .unwrap();
    let mut book: Xlsx<_> = open_workbook_from_rs(Cursor::new(out.into_inner())).unwrap();
    let notes = book.worksheet_range("Notes").unwrap();
    let parts: Vec<String> = notes
        .rows()
        .filter(|r| r[0] == Data::String("Layout".into()))
        .map(|r| r[1].to_string())
        .collect();
    assert!(parts.len() > 1);
    assert_eq!(
        parts.concat(),
        Layout::from_json(&layout).unwrap().to_json()
    );
}

#[test]
fn csv_text_that_looks_like_a_formula_stays_text() {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("note", DataType::Utf8, false),
            Field::new("pnl", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["=1+1", "plain"])) as ArrayRef,
            Arc::new(Float64Array::from(vec![-5.0, 2.0])) as ArrayRef,
        ],
    )
    .unwrap();
    let mut out = Vec::new();
    write_csv(&table_of(&batch), &Layout::default(), &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.trim_start_matches('\u{feff}').lines().collect();
    // Text is guarded; a negative number is still a number.
    assert_eq!(lines[1], "'=1+1,-5.0");
    assert_eq!(lines[2], "plain,2.0");
}
