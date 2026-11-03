//! The shared JSON converter.

#![allow(missing_docs)]

use arrow_array::Array;
use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal128Type, Float64Type, Int64Type, TimestampMicrosecondType,
};
use parallax_sources::json::{
    Ambiguous, FieldType, JsonField, parse_records, quote_bare_numbers, to_batch,
};

fn field(name: &str, ty: &str) -> JsonField {
    JsonField::new(name, FieldType::try_from(ty.to_owned()).unwrap())
}

#[test]
fn types_are_read_from_their_text() {
    let docs = parse_records(
        r#"[{"id": 9007199254740993, "px": 0.1, "amt": 12345678901234567.89, "big": "1234567890123456789012345678.90",
            "on": "2026-11-02", "ok": true, "name": "Atlas"}]"#,
        None,
    )
    .unwrap();
    let fields = [
        field("id", "integer"),
        field("px", "float"),
        field("amt", "decimal(20, 2)"),
        field("big", "decimal(38, 2)"),
        field("on", "date"),
        field("ok", "boolean"),
        field("name", "text"),
    ];
    let batch = to_batch(&fields, &docs, None).unwrap();
    assert_eq!(
        batch.column(0).as_primitive::<Int64Type>().value(0),
        9_007_199_254_740_993
    );
    assert_eq!(
        batch
            .column(1)
            .as_primitive::<Float64Type>()
            .value(0)
            .to_bits(),
        0.1f64.to_bits()
    );
    // Exact: a double would have lost the last digits.
    assert_eq!(
        batch.column(2).as_primitive::<Decimal128Type>().value(0),
        1_234_567_890_123_456_789
    );
    assert_eq!(
        batch.column(3).as_primitive::<Decimal128Type>().value(0),
        123_456_789_012_345_678_901_234_567_890
    );
    assert_eq!(
        batch.column(4).as_primitive::<Date32Type>().value(0),
        20_759
    );
    assert!(batch.column(5).as_boolean().value(0));
    assert_eq!(batch.column(6).as_string::<i32>().value(0), "Atlas");
}

#[test]
fn decimals_are_never_rounded() {
    let docs = parse_records(r#"[{"amt": 1.005}]"#, None).unwrap();
    let error = to_batch(&[field("amt", "decimal(10, 2)")], &docs, None).unwrap_err();
    assert!(error.contains("decimal places"), "{error}");
}

#[test]
fn missing_paths_are_null_and_dotted_paths_reach_inside() {
    let docs = parse_records(
        r#"{"data": {"rows": [{"risk": {"beta": 1.2}}, {"other": 1}]}}"#,
        Some("data.rows"),
    )
    .unwrap();
    let mut beta = field("beta", "float");
    beta.path = Some("risk.beta".into());
    let batch = to_batch(&[beta], &docs, None).unwrap();
    let column = batch.column(0).as_primitive::<Float64Type>();
    assert_eq!(column.value(0), 1.2);
    assert!(column.is_null(1));
}

#[test]
fn wrong_types_name_the_row_path_and_value() {
    let docs = parse_records(
        r#"[{"id": "A1", "qty": 5}, {"id": "A2", "qty": "five"}]"#,
        None,
    )
    .unwrap();
    let error = to_batch(
        &[field("id", "text"), field("qty", "integer")],
        &docs,
        Some("id"),
    )
    .unwrap_err();
    assert_eq!(
        error,
        r#"row with id = "A2", qty: "five" is not an integer"#
    );
    let arrays = parse_records(r#"[{"tags": ["a", "b"]}]"#, None).unwrap();
    assert!(
        to_batch(&[field("tags", "text")], &arrays, None)
            .unwrap_err()
            .contains("arrays are not supported")
    );
}

#[test]
fn pythons_bare_nan_and_infinity_are_read() {
    let quoted = quote_bare_numbers(r#"{"a": NaN, "b": "NaN text", "c": -Infinity}"#);
    assert!(quoted.contains(r#""b": "NaN text""#), "{quoted}");
    assert!(
        !quoted.contains(": NaN") && !quoted.contains(": -Infinity"),
        "{quoted}"
    );
    let docs = parse_records(r#"[{"x": NaN}, {"x": Infinity}, {"x": -Infinity}]"#, None).unwrap();
    let batch = to_batch(&[field("x", "float")], &docs, None).unwrap();
    let x = batch.column(0).as_primitive::<Float64Type>();
    assert!(x.value(0).is_nan());
    assert_eq!(x.value(1), f64::INFINITY);
    assert_eq!(x.value(2), f64::NEG_INFINITY);
}

#[test]
fn wall_clock_times_follow_the_zone_rules() {
    let at = |text: &str, ambiguous: Ambiguous| {
        let docs = parse_records(&format!(r#"[{{"t": "{text}"}}]"#), None).unwrap();
        let mut t = field("t", "timestamp");
        t.ambiguous = ambiguous;
        to_batch(&[t], &docs, None).map(|b| {
            b.column(0)
                .as_primitive::<TimestampMicrosecondType>()
                .value(0)
        })
    };
    // With an offset, exact.
    assert_eq!(
        at("2026-11-01T05:30:00Z", Ambiguous::Error).unwrap(),
        1_793_511_000_000_000
    );
    // Without one, New York: 01:30 EDT on 2026-11-01 repeats as 01:30 EST.
    assert!(
        at("2026-11-01T01:30:00", Ambiguous::Error)
            .unwrap_err()
            .contains("repeats")
    );
    assert_eq!(
        at("2026-11-01T01:30:00", Ambiguous::Earlier).unwrap(),
        1_793_511_000_000_000
    );
    assert_eq!(
        at("2026-11-01T01:30:00", Ambiguous::Later).unwrap(),
        1_793_514_600_000_000
    );
    // 02:30 on 2026-03-08 never happened in New York.
    assert!(
        at("2026-03-08T02:30:00", Ambiguous::Later)
            .unwrap_err()
            .contains("skipped")
    );
    assert_eq!(
        at("2026-06-01T09:30:00.123456", Ambiguous::Error).unwrap(),
        1_780_320_600_123_456
    );
    // Nanoseconds are refused, not cut.
    assert!(
        at("2026-06-01T13:30:00.123456789Z", Ambiguous::Error)
            .unwrap_err()
            .contains("microsecond")
    );
}

#[test]
fn types_parse_from_configuration_text() {
    assert_eq!(
        FieldType::try_from("decimal(18, 2)".to_owned()),
        Ok(FieldType::Decimal(18, 2))
    );
    assert_eq!(
        FieldType::try_from("Wall_Clock".to_owned()),
        Ok(FieldType::WallClock)
    );
    assert!(FieldType::try_from("decimal(40, 2)".to_owned()).is_err());
    assert!(FieldType::try_from("money".to_owned()).is_err());
}

#[test]
fn decimals_longer_than_their_precision_are_refused() {
    let docs = parse_records(r#"[{"amt": 1e30}]"#, None).unwrap();
    let error = to_batch(&[field("amt", "decimal(20, 2)")], &docs, None).unwrap_err();
    assert!(error.contains("more digits than decimal(20, 2)"), "{error}");
    let fits = parse_records(r#"[{"amt": "999999999999999999.99"}]"#, None).unwrap();
    to_batch(&[field("amt", "decimal(20, 2)")], &fits, None).unwrap();
}

#[test]
fn a_bare_nan_is_not_text() {
    let docs = parse_records(r#"[{"ticker": NaN}]"#, None).unwrap();
    let error = to_batch(&[field("ticker", "text")], &docs, None).unwrap_err();
    assert!(error.contains("bare NaN"), "{error}");
    // A real string "NaN" is still text.
    let docs = parse_records(r#"[{"ticker": "NaN"}]"#, None).unwrap();
    let batch = to_batch(&[field("ticker", "text")], &docs, None).unwrap();
    assert_eq!(
        arrow_array::cast::AsArray::as_string::<i32>(batch.column(0).as_ref()).value(0),
        "NaN"
    );
}
