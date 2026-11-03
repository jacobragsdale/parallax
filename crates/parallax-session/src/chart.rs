//! Chart frames: a reduced chart as protocol messages, with each value's exact
//! text formatted by the engine, as grid cells are.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use parallax_engine::chart::{ChartData, ChartRequest, ChartView, Interval, Reduced};
use parallax_engine::format::Format;
use parallax_engine::{ColumnType, Formatter, Scalar, Table};
use parallax_protocol::ipc::encode_batch;
use parallax_protocol::layout::LayoutChart;
use parallax_protocol::v1::{ChartFrame, ChartSeries, ChartTick};

/// A chart a view shows, and the window last asked for.
pub(crate) struct ChartState {
    pub(crate) view: ChartView,
    pub(crate) layout: LayoutChart,
    pub(crate) formats: BTreeMap<String, Format>,
    pub(crate) window: Option<ChartRequest>,
}

/// Formats a column's values as the layout says. Made once per column for a
/// frame, not once per value.
struct Texts<'a> {
    table: &'a Table,
    index: Option<usize>,
    ty: ColumnType,
    formatter: Formatter,
}

impl<'a> Texts<'a> {
    fn new(table: &'a Table, formats: &BTreeMap<String, Format>, column: &str) -> Self {
        let index = table.column_index(column);
        Self {
            table,
            index,
            ty: index.map_or(ColumnType::Float64, |i| {
                table.column(i).column_type().clone()
            }),
            formatter: formats
                .get(column)
                .and_then(|f| Formatter::new(f).ok())
                .unwrap_or_default(),
        }
    }

    /// The text of a row's value.
    fn cell(&self, slot: Option<usize>) -> Option<String> {
        let value = self.table.column(self.index?).scalar(slot?);
        (value != Scalar::Null).then(|| self.formatter.scalar(&self.ty, &value))
    }

    /// The text of a value of the column's type.
    fn value(&self, value: &Scalar) -> String {
        self.formatter.scalar(&self.ty, value)
    }

    /// The text of a double, such as a bin edge between the column's values.
    fn double(&self, value: f64) -> String {
        self.formatter
            .scalar(&ColumnType::Float64, &Scalar::Float64(value))
    }
}

fn field(name: &str, data_type: DataType, nullable: bool) -> Field {
    Field::new(name, data_type, nullable)
        .with_metadata([("parallax.role".to_owned(), name.to_owned())])
}

fn batch(columns: Vec<(Field, ArrayRef)>) -> Result<Vec<u8>, String> {
    let (fields, arrays): (Vec<Field>, Vec<ArrayRef>) = columns.into_iter().unzip();
    let batch =
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| e.to_string())?;
    encode_batch(&batch).map_err(|e| e.to_string())
}

fn floats(name: &str, values: Vec<f64>) -> (Field, ArrayRef) {
    (
        field(name, DataType::Float64, false),
        Arc::new(Float64Array::from(values)),
    )
}

fn texts(name: &str, values: Vec<Option<String>>) -> (Field, ArrayRef) {
    (
        field(name, DataType::Utf8, true),
        Arc::new(StringArray::from(values)),
    )
}

fn interval_name(interval: Interval) -> String {
    match interval {
        Interval::Minutes(m) => format!("{m}m"),
        Interval::Day => "day".into(),
        Interval::Week => "week".into(),
        Interval::Month => "month".into(),
        Interval::Quarter => "quarter".into(),
        Interval::Year => "year".into(),
        Interval::Years(n) => format!("{n}y"),
    }
}

/// Reduces `state`'s chart for its window and builds the frame, without its
/// view id and versions, which the session fills in.
pub(crate) fn chart_frame(table: &Table, state: &ChartState) -> Result<ChartFrame, String> {
    let request = state.window.unwrap_or(ChartRequest {
        x: None,
        y: None,
        width: 1,
        height: 1,
    });
    let Reduced { data, x, ticks } = state.view.reduce(table, &request);
    let config = state.view.config();
    let texts_of = |column: &str| Texts::new(table, &state.formats, column);
    let x_text = texts_of(&config.x);
    let series_text = config.series.as_deref().map(texts_of);
    let left = state.layout.y.len();
    let label = |column: &str, key: Option<&Scalar>| match (key, &series_text) {
        (Some(key), Some(series)) => format!("{column} {}", series.value(key)),
        _ => column.to_owned(),
    };
    let mut frame = ChartFrame {
        x_from: x.0,
        x_to: x.1,
        ticks: ticks
            .into_iter()
            .map(|t| ChartTick {
                x: t.x,
                label: t.label,
            })
            .collect(),
        ..ChartFrame::default()
    };
    match data {
        ChartData::Series(series) => {
            let (mut ids, mut xs, mut ys, mut x_texts, mut y_texts) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let y_texts_by_column: Vec<Texts<'_>> = config.y.iter().map(|c| texts_of(c)).collect();
            for (i, (s, points)) in series.iter().enumerate() {
                let column = &config.y[s.y];
                let y_text = &y_texts_by_column[s.y];
                frame.series.push(ChartSeries {
                    label: label(column, s.key.as_ref()),
                    column: column.clone(),
                    right: s.y >= left,
                });
                for p in points {
                    ids.push(i as u32);
                    xs.push(p.x);
                    ys.push(p.y);
                    x_texts.push(x_text.cell(p.slot));
                    y_texts.push(y_text.cell(p.slot));
                }
            }
            frame.batch = batch(vec![
                (
                    field("series", DataType::UInt32, false),
                    Arc::new(UInt32Array::from(ids)),
                ),
                floats("x", xs),
                floats("y", ys),
                texts("x_text", x_texts),
                texts("y_text", y_texts),
            ])?;
        }
        ChartData::Density {
            y,
            columns,
            rows,
            counts,
            ..
        } => {
            frame.columns = columns;
            frame.rows = rows;
            frame.y_from = y.0;
            frame.y_to = y.1;
            frame.batch = batch(vec![(
                field("count", DataType::UInt32, false),
                Arc::new(UInt32Array::from(counts)),
            )])?;
        }
        ChartData::Histogram { edges, counts } => {
            let (mut ids, mut froms, mut tos, mut ns, mut from_texts, mut to_texts) = (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
            // Edges are doubles between the column's values, the same for every
            // series.
            let edge_texts: Vec<String> = edges.iter().map(|&e| x_text.double(e)).collect();
            for (i, (key, bins)) in counts.iter().enumerate() {
                frame.series.push(ChartSeries {
                    label: label(&config.x, key.as_ref()),
                    column: config.x.clone(),
                    right: false,
                });
                for (bin, &n) in bins.iter().enumerate() {
                    ids.push(i as u32);
                    froms.push(edges[bin]);
                    tos.push(edges[bin + 1]);
                    ns.push(n);
                    from_texts.push(Some(edge_texts[bin].clone()));
                    to_texts.push(Some(edge_texts[bin + 1].clone()));
                }
            }
            frame.batch = batch(vec![
                (
                    field("series", DataType::UInt32, false),
                    Arc::new(UInt32Array::from(ids)),
                ),
                floats("from", froms),
                floats("to", tos),
                (
                    field("count", DataType::UInt64, false),
                    Arc::new(UInt64Array::from(ns)),
                ),
                texts("from_text", from_texts),
                texts("to_text", to_texts),
            ])?;
        }
        ChartData::Candles { interval, candles } => {
            frame.interval = interval_name(interval);
            let ohlc = config
                .ohlc
                .as_ref()
                .ok_or_else(|| "a candlestick chart needs its price columns".to_owned())?;
            let price = |column: &str, slot: Option<usize>| -> f64 {
                slot.zip(table.column_index(column))
                    .and_then(|(s, i)| parallax_engine::chart::number(table.column(i), s))
                    .unwrap_or(f64::NAN)
            };
            let start_text = |start: i64| {
                let value = match x_text.ty {
                    ColumnType::Date32 => Scalar::Date32(i32::try_from(start).unwrap_or_default()),
                    _ => Scalar::Timestamp(start),
                };
                x_text.value(&value)
            };
            let mut columns: Vec<(Field, ArrayRef)> = vec![
                floats("from", candles.iter().map(|(b, _)| b.from).collect()),
                floats("to", candles.iter().map(|(b, _)| b.to).collect()),
                texts(
                    "x_text",
                    candles
                        .iter()
                        .map(|(b, _)| Some(start_text(b.start)))
                        .collect(),
                ),
            ];
            for (role, column, pick) in [
                ("open", &ohlc.open, 0),
                ("high", &ohlc.high, 1),
                ("low", &ohlc.low, 2),
                ("close", &ohlc.close, 3),
            ] {
                let slot =
                    |c: &parallax_engine::chart::Candle| [c.open, c.high, c.low, c.close][pick];
                columns.push(floats(
                    role,
                    candles
                        .iter()
                        .map(|(_, c)| price(column, slot(c)))
                        .collect(),
                ));
                let column_text = texts_of(column);
                columns.push(texts(
                    &format!("{role}_text"),
                    candles
                        .iter()
                        .map(|(_, c)| column_text.cell(slot(c)))
                        .collect(),
                ));
            }
            frame.batch = batch(columns)?;
        }
    }
    Ok(frame)
}
