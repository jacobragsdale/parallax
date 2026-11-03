//! Writes a workbook for checking, in real Excel, that Excel shows the digits the
//! screen shows: `cargo run -p parallax-export --example verification -- out.xlsx`.
//! Open it in Excel on a target desktop; column C must read exactly as column B.

use arrow_schema::{DataType, TimeUnit};
use parallax_engine::format::{Abbreviation, Format, Formatter, Negative};
use rust_xlsxwriter::{Format as Cell, Workbook};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "verification.xlsx".to_owned());
    let cases: Vec<(&str, f64, Format)> = vec![
        ("Excel's rounding of 2.675", 2.675, Format::default()),
        ("1.005 rounds up", 1.005, Format::default()),
        ("a negative that rounds to zero", -0.001, Format::default()),
        (
            "a negative that rounds to zero, parentheses",
            -0.004,
            Format {
                negative: Negative::Parentheses,
                ..Format::default()
            },
        ),
        (
            "a tie past 15 digits",
            1_000_000_000_000_005.0,
            Format {
                decimals: Some(0),
                ..Format::default()
            },
        ),
        (
            "digits past 15 show as zeros",
            12_345_678_901_234_567_890.0,
            Format {
                decimals: Some(0),
                ..Format::default()
            },
        ),
        (
            "percent",
            0.125,
            Format {
                percent: true,
                ..Format::default()
            },
        ),
        (
            "a tiny percent",
            0.000_05,
            Format {
                percent: true,
                ..Format::default()
            },
        ),
        (
            "millions",
            1_250_000.0,
            Format {
                abbreviate: Some(Abbreviation::Millions),
                ..Format::default()
            },
        ),
        (
            "negative millions in parentheses",
            -1_249_999.0,
            Format {
                abbreviate: Some(Abbreviation::Millions),
                negative: Negative::Parentheses,
                ..Format::default()
            },
        ),
        (
            "billions, two decimals",
            12_345_678_901.23,
            Format {
                abbreviate: Some(Abbreviation::Billions),
                decimals: Some(2),
                ..Format::default()
            },
        ),
        (
            "no thousands separator",
            1_234_567.891,
            Format {
                thousands: Some(false),
                decimals: Some(3),
                ..Format::default()
            },
        ),
    ];
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let bold = Cell::new().set_bold();
    for (c, title) in [
        "Case",
        "The screen shows",
        "Excel shows",
        "Excel number format",
    ]
    .iter()
    .enumerate()
    {
        sheet.write_string_with_format(0, u16::try_from(c)?, *title, &bold)?;
    }
    for (r, (case, value, format)) in cases.iter().enumerate() {
        let row = u32::try_from(r + 1)?;
        let formatter = Formatter::new(format)?;
        let shown = formatter.float(*value);
        let code = if *value < 0.0 && Formatter::shows_zero(&shown) {
            formatter.excel_unsigned_format(&DataType::Float64)
        } else {
            formatter.excel_format(&DataType::Float64)
        }
        .unwrap_or_default();
        sheet.write_string(row, 0, *case)?;
        sheet.write_string(row, 1, &shown)?;
        sheet.write_number_with_format(row, 2, *value, &Cell::new().set_num_format(&code))?;
        sheet.write_string(row, 3, &code)?;
    }
    // An instant shown in New York, across the spring change.
    let instant = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    let formatter = Formatter::default();
    let row = u32::try_from(cases.len() + 1)?;
    let shown = formatter.timestamp(1_772_953_200_000_000, true);
    let code = formatter.excel_format(&instant).unwrap_or_default();
    sheet.write_string(row, 0, "an instant just after the 2026 spring change")?;
    sheet.write_string(row, 1, &shown)?;
    sheet.write_datetime_with_format(
        row,
        2,
        &rust_xlsxwriter::ExcelDateTime::parse_from_str(&shown)?,
        &Cell::new().set_num_format(&code),
    )?;
    sheet.write_string(row, 3, &code)?;
    sheet.set_column_width(0, 48)?;
    sheet.set_column_width(1, 34)?;
    sheet.set_column_width(2, 34)?;
    sheet.set_column_width(3, 40)?;
    workbook.save(&path)?;
    println!("wrote {path}");
    Ok(())
}
