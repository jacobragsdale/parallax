//! Excel and CSV exports of parallax views.
//!
//! An export runs the layout through the same engine and formatter as the screen,
//! so it holds the same numbers. Excel cells hold real numbers with Excel number
//! formats that show the digits the screen shows; CSV holds every digit as text.
//! A pivot exports every group, with Excel outline levels; Excel's outline
//! buttons collapse them.
//!
//! The table is read in chunks, each under its own read lock, so a long export
//! never holds up a reload for its whole length. If the table reloads part way,
//! the export stops rather than mix two loads.
//!
//! Excel keeps 15 significant digits. Where a value has more, or a feature cannot
//! be expressed in Excel, a Notes sheet says so rather than leaving the
//! difference to be found later.

use std::collections::BTreeSet;
use std::io::{Seek, Write};

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, Decimal128Type, Float64Type, Int64Type};
use arrow_array::{Array, ArrayRef, Int32Array, RecordBatch, StringArray, UInt8Array};
use arrow_schema::DataType;
use parallax_engine::format::{Formatter, exact_decimal};
use parallax_engine::view::{flag, meta, role};
use parallax_engine::{ConfigError, Table, View};
use parallax_protocol::layout::{Layout, LayoutError};
use rust_xlsxwriter::{ExcelDateTime, Format, FormatBorder, Workbook, Worksheet, XlsxError};

/// Excel's most rows per sheet, the header included.
pub const EXCEL_MAX_ROWS: usize = 1_048_576;
/// Excel's longest text in a cell.
pub const EXCEL_MAX_TEXT: usize = 32_767;
/// Excel's deepest row outline.
const EXCEL_MAX_OUTLINE: usize = 7;
/// Cells read from the view at a time, which bounds memory whatever the size.
const CHUNK_CELLS: usize = 200_000;
/// Excel's most columns per sheet.
const EXCEL_MAX_COLUMNS: usize = 16_384;
/// The most cells a pivot writes with an outline. rust_xlsxwriter writes row
/// outline levels only when it holds the sheet in memory, at roughly 100 bytes a
/// cell; larger pivots stream without an outline.
const OUTLINE_MAX_CELLS: usize = 2_000_000;

/// Rows read at a time for a view this wide.
fn chunk_rows(columns: usize) -> usize {
    (CHUNK_CELLS / columns.max(1)).max(100)
}
/// Excel's day 0 is 1899-12-30, 25,569 days before 1970-01-01.
const EXCEL_EPOCH_OFFSET: f64 = 25_569.0;

/// Access to a table that other threads may reload: each read holds a lock for
/// as long as its guard lives.
pub trait ReadTable {
    /// The guard of one read.
    type Guard<'a>: std::ops::Deref<Target = Table>
    where
        Self: 'a;

    /// Locks the table for reading.
    ///
    /// # Errors
    ///
    /// Returns a message if the table cannot be read, such as a poisoned lock.
    fn read(&self) -> Result<Self::Guard<'_>, String>;
}

impl ReadTable for Table {
    type Guard<'a> = &'a Table;

    fn read(&self) -> Result<&Table, String> {
        Ok(self)
    }
}

/// An export that cannot be made.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The layout does not fit the table.
    #[error(transparent)]
    Layout(#[from] LayoutError),
    /// The view cannot be built.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// More rows than an Excel sheet holds.
    #[error(
        "the view has {rows} rows, more than the {limit} an Excel sheet holds; export it as CSV instead"
    )]
    TooManyRows {
        /// Rows the sheet would need, the header included.
        rows: usize,
        /// Excel's limit.
        limit: usize,
    },
    /// Text longer than an Excel cell holds; it is never cut.
    #[error(
        "row {row}, column {column}: {length} characters is more than the {EXCEL_MAX_TEXT} an Excel cell holds"
    )]
    TextTooLong {
        /// The view row, from 0.
        row: usize,
        /// The column title.
        column: String,
        /// The text's length in characters.
        length: usize,
    },
    /// The Excel writer failed.
    #[error("Excel: {0}")]
    Excel(#[from] XlsxError),
    /// Writing the output failed.
    #[error("writing the export: {0}")]
    Io(#[from] std::io::Error),
    /// The table could not be read.
    #[error("reading the table: {0}")]
    Read(String),
    /// The table reloaded part way through the export.
    #[error("the data reloaded while it was being exported; export again for the new data")]
    Reloaded,
}

/// What an export wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// Data rows, the header excluded.
    pub rows: usize,
    /// Data columns, the group column of a pivot excluded.
    pub columns: usize,
    /// Where Excel shows something other than the screen, and why.
    pub notes: Vec<String>,
}

/// Where the data came from, for the Notes sheet.
#[derive(Clone, Debug, Default)]
pub struct ExportSource {
    /// The dataset's name.
    pub dataset: String,
    /// When the source was read, as text.
    pub as_of: String,
}

/// One data column of a frame.
struct Column<'a> {
    title: String,
    values: &'a ArrayRef,
    flags: Option<&'a UInt8Array>,
    text: &'a StringArray,
    copy: Option<&'a StringArray>,
    measure: Option<usize>,
    name: String,
}

/// The tree columns of a pivot frame.
struct Tree<'a> {
    depth: &'a Int32Array,
    kind: &'a StringArray,
    label: &'a StringArray,
}

/// A frame read by field role.
struct Frame<'a> {
    tree: Option<Tree<'a>>,
    groups: Vec<&'a ArrayRef>,
    columns: Vec<Column<'a>>,
}

fn role_of(batch: &RecordBatch, i: usize) -> &str {
    batch
        .schema_ref()
        .field(i)
        .metadata()
        .get(meta::ROLE)
        .map_or("", String::as_str)
}

fn index_of(batch: &RecordBatch, i: usize, key: &str) -> Option<usize> {
    batch
        .schema_ref()
        .field(i)
        .metadata()
        .get(key)?
        .parse()
        .ok()
}

impl<'a> Frame<'a> {
    fn read(batch: &'a RecordBatch) -> Self {
        let schema = batch.schema_ref();
        let by_role = |r: &str| (0..schema.fields().len()).find(|&i| role_of(batch, i) == r);
        let tree = match (
            by_role(role::DEPTH),
            by_role(role::KIND),
            by_role(role::LABEL),
        ) {
            (Some(d), Some(k), Some(l)) => Some(Tree {
                depth: batch.column(d).as_primitive(),
                kind: batch.column(k).as_string(),
                label: batch.column(l).as_string(),
            }),
            _ => None,
        };
        let mut groups: Vec<(usize, &ArrayRef)> = (0..schema.fields().len())
            .filter(|&i| role_of(batch, i) == role::GROUP)
            .map(|i| {
                (
                    index_of(batch, i, meta::LEVEL).unwrap_or(0),
                    batch.column(i),
                )
            })
            .collect();
        groups.sort_by_key(|(level, _)| *level);
        let part = |r: &str, index: usize| {
            (0..schema.fields().len())
                .find(|&i| role_of(batch, i) == r && index_of(batch, i, meta::INDEX) == Some(index))
        };
        let columns = (0..schema.fields().len())
            .filter(|&i| role_of(batch, i) == role::VALUE)
            .filter_map(|i| {
                let index = index_of(batch, i, meta::INDEX)?;
                let field = schema.field(i);
                Some(Column {
                    title: field
                        .metadata()
                        .get(meta::TITLE)
                        .cloned()
                        .unwrap_or_else(|| field.name().clone()),
                    values: batch.column(i),
                    flags: part(role::FLAG, index).map(|f| batch.column(f).as_primitive()),
                    text: batch.column(part(role::TEXT, index)?).as_string(),
                    copy: part(role::COPY, index).map(|c| batch.column(c).as_string()),
                    measure: index_of(batch, i, meta::MEASURE),
                    name: field.name().clone(),
                })
            })
            .collect();
        Self {
            tree,
            groups: groups.into_iter().map(|(_, g)| g).collect(),
            columns,
        }
    }
}

/// Builds the export's view: the layout's, showing every group.
fn view_of(table: &Table, layout: &Layout) -> Result<View, ExportError> {
    let resolved = layout.resolve(table)?;
    let mut view = View::new(table, resolved.config)?;
    for (path, expanded) in resolved.expanded {
        view.set_expanded(path, expanded);
    }
    view.show_all(true);
    Ok(view)
}

/// The formatter a data column uses on screen.
fn formatter_of(view: &View, table: &Table, column: &Column<'_>) -> Formatter {
    let found = match column.measure {
        Some(m) => view.measure_formatter(m),
        None => view.column_formatter(table, &column.name),
    };
    found.cloned().unwrap_or_default()
}

/// Notes collected while writing, each said once.
#[derive(Default)]
struct Notes(BTreeSet<String>);

impl Notes {
    fn add(&mut self, note: impl Into<String>) {
        self.0.insert(note.into());
    }
}

/// Significant digits of an exact decimal's digit string.
fn significant_digits(digits: &str) -> usize {
    digits
        .trim_start_matches(['-', '0', '.'])
        .chars()
        .filter(char::is_ascii_digit)
        .count()
}

/// Number formats for one column, made once.
struct ColumnFormats {
    signed: Option<Format>,
    unsigned: Option<Format>,
}

impl ColumnFormats {
    fn new(formatter: &Formatter, data_type: &DataType) -> Self {
        let make = |code: Option<String>| code.map(|c| Format::new().set_num_format(c));
        Self {
            signed: make(formatter.excel_format(data_type)),
            unsigned: make(formatter.excel_unsigned_format(data_type)),
        }
    }
}

/// Writes one number, with the unsigned format when the screen shows it as an unsigned zero.
fn write_number(
    sheet: &mut Worksheet,
    row: u32,
    col: u16,
    value: f64,
    text: &str,
    formats: &ColumnFormats,
) -> Result<(), XlsxError> {
    let format = if value < 0.0 && Formatter::shows_zero(text) {
        formats.unsigned.as_ref()
    } else {
        formats.signed.as_ref()
    };
    match format {
        Some(f) => sheet.write_number_with_format(row, col, value, f)?,
        None => sheet.write_number(row, col, value)?,
    };
    Ok(())
}

/// Writes one data cell as the screen shows it.
fn write_cell(
    sheet: &mut Worksheet,
    (row, col): (u32, u16),
    column: &Column<'_>,
    i: usize,
    formats: &ColumnFormats,
    notes: &mut Notes,
    view_row: usize,
) -> Result<(), ExportError> {
    let text = column.text.value(i);
    let flagged = column.flags.is_some_and(|f| f.value(i) != flag::OK);
    if flagged {
        sheet.write_string(row, col, text)?;
        return Ok(());
    }
    if column.values.is_null(i) {
        return Ok(());
    }
    match column.values.data_type() {
        DataType::Float64 => {
            let v = column.values.as_primitive::<Float64Type>().value(i);
            if v.is_finite() {
                write_number(sheet, row, col, v, text, formats)?;
            } else {
                if v.is_infinite() {
                    notes.add("Excel has no infinity, so infinite values are written as the text ∞ or -∞.");
                }
                sheet.write_string(row, col, text)?;
            }
        }
        DataType::Int64 => {
            let v = column.values.as_primitive::<Int64Type>().value(i);
            if v.unsigned_abs() >= 1_000_000_000_000_000 {
                notes.add("Some integers have more than 15 digits; Excel keeps 15, so they show rounded in Excel. The CSV export has every digit.");
            }
            write_number(sheet, row, col, v as f64, text, formats)?;
        }
        DataType::Decimal128(_, scale) => {
            let exact = exact_decimal(
                column.values.as_primitive::<Decimal128Type>().value(i),
                i32::from(*scale),
            );
            if significant_digits(&exact) > 15 {
                notes.add("Some decimals have more than 15 significant digits; Excel keeps 15, so they show rounded in Excel. The CSV export has every digit.");
            }
            let v: f64 = exact.parse().expect("an exact decimal parses as a double");
            write_number(sheet, row, col, v, text, formats)?;
        }
        DataType::Boolean => {
            sheet.write_boolean(row, col, column.values.as_boolean().value(i))?;
        }
        DataType::Utf8 => {
            let length = text.chars().count();
            if length > EXCEL_MAX_TEXT {
                return Err(ExportError::TextTooLong {
                    row: view_row,
                    column: column.title.clone(),
                    length,
                });
            }
            if text.is_empty() {
                notes.add("Excel shows empty text and blank cells alike.");
            }
            sheet.write_string(row, col, text)?;
        }
        DataType::Date32 => {
            let serial =
                f64::from(column.values.as_primitive::<Date32Type>().value(i)) + EXCEL_EPOCH_OFFSET;
            if serial >= 61.0 {
                write_number(sheet, row, col, serial, text, formats)?;
            } else {
                // Before 1900-03-01, Excel's calendar has a day that never was.
                notes.add("Excel dates before March 1900 are unreliable, so earlier dates are written as text.");
                sheet.write_string(row, col, text)?;
            }
        }
        // Times to the microsecond have no Excel format, so no signed format either.
        DataType::Timestamp(..) => match &formats.signed {
            Some(format) => match ExcelDateTime::parse_from_str(text) {
                Ok(datetime) => {
                    sheet.write_datetime_with_format(row, col, &datetime, format)?;
                }
                Err(_) => {
                    notes.add("Excel dates start in 1900, so earlier times are written as text.");
                    sheet.write_string(row, col, text)?;
                }
            },
            None => {
                notes.add("Excel times stop at milliseconds, so times shown to the microsecond are written as text.");
                sheet.write_string(row, col, text)?;
            }
        },
        _ => {
            sheet.write_string(row, col, text)?;
        }
    }
    Ok(())
}

/// Reads every row's depth, which the outline needs before any row is written.
fn tree_shape(view: &mut View, table: &Table, rows: usize) -> Vec<usize> {
    let mut shape = Vec::with_capacity(rows);
    let chunk = chunk_rows(1);
    for start in (0..rows).step_by(chunk) {
        let frame = view.window(table, start..start + chunk, 0..0);
        let read = Frame::read(&frame.batch);
        if let Some(tree) = read.tree {
            shape.extend(
                (0..frame.batch.num_rows())
                    .map(|i| usize::try_from(tree.depth.value(i)).unwrap_or(0)),
            );
        }
    }
    shape
}

/// Groups each pivot row's descendants into an Excel outline, with the buttons
/// beside the group rows, which come first as on screen.
///
/// Every group starts expanded. rust_xlsxwriter 0.102 marks a collapsed group on
/// the row after it, which is right only when the buttons sit below the groups,
/// so collapsing groups here would leave buttons that toggle the wrong way.
fn outline(sheet: &mut Worksheet, shape: &[usize], notes: &mut Notes) -> Result<(), XlsxError> {
    sheet.group_symbols_above(true);
    for (i, &depth) in shape.iter().enumerate() {
        let end = shape[i + 1..]
            .iter()
            .position(|&d| d <= depth)
            .map_or(shape.len(), |p| i + 1 + p);
        if end <= i + 1 {
            continue;
        }
        if depth + 1 > EXCEL_MAX_OUTLINE {
            notes.add("Excel outlines have at most 7 levels, so deeper groups are not outlined.");
            continue;
        }
        // Row 0 is the header, so view row r is sheet row r + 1.
        let first = u32::try_from(i + 2).unwrap_or(u32::MAX);
        let last = u32::try_from(end).unwrap_or(u32::MAX);
        sheet.group_rows(first, last)?;
    }
    Ok(())
}

/// Reads the table and checks that it has not reloaded since `version`.
fn read_at<T: ReadTable + ?Sized>(source: &T, version: u64) -> Result<T::Guard<'_>, ExportError> {
    let guard = source.read().map_err(ExportError::Read)?;
    if guard.load_version() == version {
        Ok(guard)
    } else {
        Err(ExportError::Reloaded)
    }
}

/// The longest text the Notes sheet puts in one cell, below Excel's limit.
const NOTE_CELL: usize = 30_000;

/// Writes the layout's view of a table as an Excel workbook.
///
/// # Errors
///
/// Returns an error if the layout does not fit the table, the view has more rows
/// than a sheet holds, a text is longer than a cell holds, the table reloads part
/// way, or writing fails.
pub fn write_xlsx<T: ReadTable + ?Sized, W: Write + Seek + Send>(
    tables: &T,
    layout: &Layout,
    source: &ExportSource,
    out: W,
) -> Result<ExportSummary, ExportError> {
    let first = tables.read().map_err(ExportError::Read)?;
    let version = first.load_version();
    let mut view = view_of(&first, layout)?;
    let rows = view.num_rows(&first);
    let columns = view.num_columns(&first);
    if rows + 1 > EXCEL_MAX_ROWS {
        return Err(ExportError::TooManyRows {
            rows: rows + 1,
            limit: EXCEL_MAX_ROWS,
        });
    }
    let mut notes = Notes::default();
    let mut workbook = Workbook::new();
    let grouped = view.is_grouped();
    let outlined = grouped && (rows + 1).saturating_mul(columns + 1) <= OUTLINE_MAX_CELLS;
    let shape = if outlined {
        tree_shape(&mut view, &first, rows)
    } else {
        Vec::new()
    };
    drop(first);
    let sheet = if outlined {
        workbook.add_worksheet()
    } else {
        workbook.add_worksheet_with_constant_memory()?
    };
    sheet.set_name("View")?;
    let header = Format::new()
        .set_bold()
        .set_border_bottom(FormatBorder::Thin);
    let offset: u16 = u16::from(grouped);
    if grouped {
        if outlined {
            outline(sheet, &shape, &mut notes)?;
        } else {
            notes.add("This pivot is too large to outline in Excel, so its groups are listed without outline levels; the group column's indent shows each level.");
        }
        let title = if layout.group_by.is_empty() {
            "Group".to_owned()
        } else {
            layout.group_by.join(" › ")
        };
        sheet.write_string_with_format(0, 0, &title, &header)?;
        let width = layout.column_widths.get("__tree").copied().unwrap_or(260);
        sheet.set_column_width_pixels(0, width)?;
    }
    sheet.set_freeze_panes(1, offset)?;

    let mut formats: Vec<ColumnFormats> = Vec::new();
    let labels: Vec<Format> = (0..=EXCEL_MAX_OUTLINE)
        .map(|d| Format::new().set_indent(u8::try_from(d).unwrap_or(7)))
        .collect();
    let total_label = Format::new().set_bold();
    let fit = columns.min(EXCEL_MAX_COLUMNS - usize::from(offset));
    if fit < columns {
        notes.add(
            "Excel sheets have at most 16,384 columns, so the rightmost columns are not exported.",
        );
    }
    let chunk = chunk_rows(fit);
    for start in (0..rows.max(1)).step_by(chunk) {
        // Each chunk holds the read lock only while it is read, so a reload can
        // run between chunks; one that did ends the export.
        let table = read_at(tables, version)?;
        let frame = view.window(&table, start..start + chunk, 0..fit);
        let read = Frame::read(&frame.batch);
        if start == 0 {
            for (j, column) in read.columns.iter().enumerate() {
                let col = u16::try_from(usize::from(offset) + j).expect("within Excel's columns");
                formats.push(ColumnFormats::new(
                    &formatter_of(&view, &table, column),
                    column.values.data_type(),
                ));
                sheet.write_string_with_format(0, col, &column.title, &header)?;
                let key = column
                    .measure
                    .map_or_else(|| column.name.clone(), |m| format!("__measure:{m}"));
                let width = layout.column_widths.get(&key).copied().unwrap_or(140);
                sheet.set_column_width_pixels(col, width)?;
            }
            if !grouped && rows > 0 && !read.columns.is_empty() {
                let last_col = u16::try_from(read.columns.len() - 1).unwrap_or(u16::MAX);
                sheet.autofilter(0, 0, u32::try_from(rows).unwrap_or(u32::MAX), last_col)?;
            }
        }
        drop(table);
        for i in 0..frame.batch.num_rows() {
            let view_row = start + i;
            let row = u32::try_from(view_row + 1).unwrap_or(u32::MAX);
            if let Some(tree) = &read.tree {
                let depth = usize::try_from(tree.depth.value(i))
                    .unwrap_or(0)
                    .min(EXCEL_MAX_OUTLINE);
                let format = if tree.kind.value(i) == "total" {
                    &total_label
                } else {
                    &labels[depth]
                };
                sheet.write_string_with_format(row, 0, tree.label.value(i), format)?;
            }
            for (j, column) in read.columns.iter().enumerate().take(formats.len()) {
                let col = u16::try_from(usize::from(offset) + j).unwrap_or(u16::MAX);
                write_cell(
                    sheet,
                    (row, col),
                    column,
                    i,
                    &formats[j],
                    &mut notes,
                    view_row,
                )?;
            }
        }
    }

    let notes: Vec<String> = notes.0.into_iter().collect();
    let about = workbook.add_worksheet();
    about.set_name("Notes")?;
    about.set_column_width_pixels(0, 140)?;
    about.set_column_width_pixels(1, 640)?;
    // The layout can be longer than a cell holds, so it spreads over rows.
    let layout_json = layout.to_json();
    let layout_parts: Vec<String> = layout_json
        .chars()
        .collect::<Vec<_>>()
        .chunks(NOTE_CELL)
        .map(|c| c.iter().collect())
        .collect();
    let lines: Vec<(&str, String)> = [
        ("Dataset", source.dataset.clone()),
        ("Source read at", source.as_of.clone()),
        ("Rows", rows.to_string()),
    ]
    .into_iter()
    .chain(layout_parts.into_iter().map(|part| ("Layout", part)))
    .chain(notes.iter().map(|n| ("Note", n.clone())))
    .collect();
    for (r, (term, text)) in lines.iter().enumerate() {
        let r = u32::try_from(r).unwrap_or(u32::MAX);
        about.write_string_with_format(r, 0, *term, &header)?;
        about.write_string(r, 1, text)?;
    }
    workbook.save_to_writer(out)?;
    Ok(ExportSummary {
        rows,
        columns,
        notes,
    })
}

/// Text that spreadsheets would run as a formula: it starts with `=`, `+`, `-`,
/// `@`, a tab or a carriage return.
fn formula_like(text: &str) -> bool {
    text.starts_with(['=', '+', '-', '@', '\t', '\r'])
}

/// Quotes a CSV field when it holds a comma, a quote or a line break.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// Every digit of a data cell: exact numbers, full timestamps, and the screen's
/// text for everything else.
fn exact_text(column: &Column<'_>, i: usize) -> String {
    let flagged = column.flags.is_some_and(|f| f.value(i) != flag::OK);
    if flagged {
        return column.text.value(i).to_owned();
    }
    if column.values.is_null(i) {
        return String::new();
    }
    match column.values.data_type() {
        DataType::Float64 => {
            let v = column.values.as_primitive::<Float64Type>().value(i);
            if v.is_finite() {
                format!("{v:?}")
            } else {
                column.text.value(i).to_owned()
            }
        }
        DataType::Int64 => column
            .values
            .as_primitive::<Int64Type>()
            .value(i)
            .to_string(),
        DataType::Decimal128(_, scale) => exact_decimal(
            column.values.as_primitive::<Decimal128Type>().value(i),
            i32::from(*scale),
        ),
        DataType::Timestamp(..) => column.copy.map_or_else(
            || column.text.value(i).to_owned(),
            |c| c.value(i).to_owned(),
        ),
        DataType::Utf8 => column.values.as_string::<i32>().value(i).to_owned(),
        _ => column.text.value(i).to_owned(),
    }
}

/// Every digit of each group key in a chunk, formatted once for the whole chunk.
fn key_texts(array: &ArrayRef, formatter: &Formatter) -> Vec<String> {
    match array.data_type() {
        DataType::Float64 => {
            let a = array.as_primitive::<Float64Type>();
            (0..a.len())
                .map(|i| {
                    if a.is_null(i) {
                        String::new()
                    } else {
                        format!("{:?}", a.value(i))
                    }
                })
                .collect()
        }
        DataType::Int64 => {
            let a = array.as_primitive::<Int64Type>();
            (0..a.len())
                .map(|i| {
                    if a.is_null(i) {
                        String::new()
                    } else {
                        a.value(i).to_string()
                    }
                })
                .collect()
        }
        DataType::Decimal128(_, scale) => {
            let a = array.as_primitive::<Decimal128Type>();
            (0..a.len())
                .map(|i| {
                    if a.is_null(i) {
                        String::new()
                    } else {
                        exact_decimal(a.value(i), i32::from(*scale))
                    }
                })
                .collect()
        }
        DataType::Timestamp(..) => formatter.timestamps_full(array),
        _ => formatter.texts(array),
    }
}

/// Writes text so that spreadsheets show it as text: one that looks like a
/// formula gets a leading apostrophe, which Excel hides and treats as "this is
/// text", so opening the file never runs anything.
fn csv_text(text: &str) -> String {
    if formula_like(text) {
        format!("'{text}")
    } else {
        text.to_owned()
    }
}

/// Writes the layout's view of a table as CSV, with every digit. A pivot writes
/// a `level` column (0 for the grand total) and one column per group level, then
/// the values. The file starts with a UTF-8 byte-order mark, which tells Excel
/// the encoding; without it Excel reads non-ASCII text in the system code page.
/// Text that would run as a formula in a spreadsheet is written with a leading
/// apostrophe.
///
/// # Errors
///
/// Returns an error if the layout does not fit the table, the table reloads part
/// way, or writing fails.
pub fn write_csv<T: ReadTable + ?Sized, W: Write>(
    tables: &T,
    layout: &Layout,
    mut out: W,
) -> Result<ExportSummary, ExportError> {
    let first = tables.read().map_err(ExportError::Read)?;
    let version = first.load_version();
    let mut view = view_of(&first, layout)?;
    let rows = view.num_rows(&first);
    let columns = view.num_columns(&first);
    let grouped = view.is_grouped();
    let key_formatters: Vec<Formatter> = layout
        .group_by
        .iter()
        .map(|g| {
            view.column_formatter(&first, g)
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    drop(first);
    out.write_all("\u{feff}".as_bytes())?;
    let mut line = String::new();
    let chunk = chunk_rows(columns);
    for start in (0..rows.max(1)).step_by(chunk) {
        let table = read_at(tables, version)?;
        let frame = view.window(&table, start..start + chunk, 0..columns);
        drop(table);
        let read = Frame::read(&frame.batch);
        if start == 0 {
            let mut titles: Vec<String> = Vec::new();
            if grouped {
                titles.push("level".to_owned());
                titles.extend(layout.group_by.iter().cloned());
            }
            titles.extend(read.columns.iter().map(|c| c.title.clone()));
            writeln!(
                out,
                "{}",
                titles
                    .iter()
                    .map(|t| csv_field(&csv_text(t)))
                    .collect::<Vec<_>>()
                    .join(",")
            )?;
        }
        let keys: Vec<Vec<String>> = read
            .groups
            .iter()
            .zip(&key_formatters)
            .map(|(group, formatter)| {
                let texts = key_texts(group, formatter);
                if matches!(group.data_type(), DataType::Utf8) {
                    texts.iter().map(|t| csv_text(t)).collect()
                } else {
                    texts
                }
            })
            .collect();
        #[allow(clippy::needless_range_loop)] // `i` indexes several arrays of the frame.
        for i in 0..frame.batch.num_rows() {
            line.clear();
            let mut fields: Vec<String> =
                Vec::with_capacity(read.columns.len() + read.groups.len() + 1);
            if let Some(tree) = &read.tree {
                let depth = usize::try_from(tree.depth.value(i)).unwrap_or(0);
                fields.push(depth.to_string());
                for (level, texts) in keys.iter().enumerate() {
                    fields.push(if level >= depth {
                        String::new()
                    } else if tree.kind.value(i) == "other" && level + 1 == depth {
                        // An Other row has no key of its own at its level.
                        tree.label.value(i).to_owned()
                    } else {
                        texts[i].clone()
                    });
                }
            }
            fields.extend(read.columns.iter().map(|c| {
                let text = exact_text(c, i);
                if matches!(c.values.data_type(), DataType::Utf8) {
                    csv_text(&text)
                } else {
                    text
                }
            }));
            line.push_str(
                &fields
                    .iter()
                    .map(|f| csv_field(f))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            writeln!(out, "{line}")?;
        }
    }
    out.flush()?;
    Ok(ExportSummary {
        rows,
        columns,
        notes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_are_quoted_only_when_needed() {
        assert_eq!(csv_field("Atlas"), "Atlas");
        assert_eq!(csv_field("Cedar, Inc."), "\"Cedar, Inc.\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn text_that_looks_like_a_formula_stays_text() {
        assert_eq!(csv_text("=HYPERLINK(\"x\")"), "'=HYPERLINK(\"x\")");
        assert_eq!(csv_text("+1"), "'+1");
        assert_eq!(csv_text("-5 bps"), "'-5 bps");
        assert_eq!(csv_text("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_text("Atlas"), "Atlas");
    }

    #[test]
    fn significant_digits_ignore_sign_and_leading_zeros() {
        assert_eq!(significant_digits("-0.00123"), 3);
        assert_eq!(significant_digits("1234567890123456.78"), 18);
        assert_eq!(significant_digits("100"), 3);
    }
}
