//! Chart frames: a reduced chart as protocol messages, with each value's exact
//! text formatted by the engine, as grid cells are.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use parallax_engine::chart::{
    BucketVolume, ChartConfig, ChartData, ChartRequest, ChartView, IndicatorLines, Interval,
    Reduced,
};
use parallax_engine::format::{Format, MAX_DECIMALS, OVERFLOW_TEXT};
use parallax_engine::{ColumnType, Formatter, Scalar, Table};
use parallax_protocol::ipc::encode_batch;
use parallax_protocol::layout::LayoutChart;
use parallax_protocol::v1::{ChartAnchor, ChartFrame, ChartIndicatorLine, ChartSeries, ChartTick};

/// A chart a view shows, and the window last asked for.
pub(crate) struct ChartState {
    pub(crate) view: ChartView,
    pub(crate) layout: LayoutChart,
    pub(crate) formats: BTreeMap<String, Format>,
    /// The distinct x values the layout's drawings are anchored to, as JSON and as
    /// values of the x column.
    pub(crate) anchors: Vec<(String, Scalar)>,
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

    /// A row's value as exact JSON, for filters that select it, or `None` at a
    /// break.
    fn json(&self, slot: Option<usize>) -> Option<String> {
        let value = self.table.column(self.index?).scalar(slot?);
        Some(parallax_engine::format::scalar_json(&self.ty, &value))
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
    let Reduced {
        data,
        x,
        extent,
        ticks,
        indicators,
    } = state.view.reduce(table, &request);
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
        extent_from: extent.0,
        extent_to: extent.1,
        anchors: state
            .anchors
            .iter()
            .map(|(json, value)| ChartAnchor {
                x_json: json.clone(),
                position: state.view.position(value).unwrap_or(f64::NAN),
            })
            .collect(),
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
            let (mut ids, mut xs, mut ys, mut x_texts, mut y_texts, mut x_json) = (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
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
                    x_json.push(x_text.json(p.slot));
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
                texts("x_json", x_json),
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
        ChartData::Candles {
            interval,
            candles,
            volumes,
        } => {
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
            let x_scalar = |at: i64| match x_text.ty {
                ColumnType::Date32 => Scalar::Date32(i32::try_from(at).unwrap_or_default()),
                _ => Scalar::Timestamp(at),
            };
            let start_text = |start: i64| x_text.value(&x_scalar(start));
            let json = |at: i64| {
                Some(parallax_engine::format::scalar_json(
                    &x_text.ty,
                    &x_scalar(at),
                ))
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
                texts(
                    "x_json",
                    candles.iter().map(|(b, _)| json(b.start)).collect(),
                ),
                texts(
                    "end_json",
                    candles.iter().map(|(b, _)| json(b.end)).collect(),
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
            if let Some(volume) = config.volume.as_deref().filter(|_| !volumes.is_empty()) {
                columns.extend(volume_columns(&texts_of(volume), &volumes));
            }
            frame.batch = batch(columns)?;
        }
    }
    if !indicators.is_empty() {
        let price = config.indicator_price();
        let price_format =
            price.map_or_else(Format::default, |p| price_format(table, &state.formats, p));
        let lines = IndicatorTexts {
            x: &x_text,
            series: series_text.as_ref(),
            price: Formatter::new(&price_format).unwrap_or_default(),
            oscillator: Formatter::new(&Format {
                decimals: Some(2),
                ..Format::default()
            })
            .unwrap_or_default(),
        };
        lines.fill(&mut frame, config, &indicators)?;
    }
    Ok(frame)
}

/// The format of a price-unit indicator: the price column's, showing at least
/// as many decimals as a double does by default, or a decimal column's scale
/// if that is more, up to the most a format allows. An average of whole prices
/// is rarely whole.
fn price_format(table: &Table, formats: &BTreeMap<String, Format>, price: &str) -> Format {
    let mut format = formats.get(price).cloned().unwrap_or_default();
    if format.decimals.is_none() {
        let scale = match table
            .column_index(price)
            .map(|i| table.column(i).column_type())
        {
            Some(ColumnType::Decimal { scale, .. }) => u8::try_from(*scale).unwrap_or(0),
            _ => 0,
        };
        format.decimals = Some(scale.clamp(2, MAX_DECIMALS));
    }
    format
}

/// A candle batch's volume columns: each bucket's total for drawing, NaN for
/// none or an overflow, and its exact text.
fn volume_columns(text: &Texts<'_>, volumes: &[BucketVolume]) -> [(Field, ArrayRef); 2] {
    let number = |v: &BucketVolume| match v {
        BucketVolume::Total(Scalar::Int64(n)) => *n as f64,
        BucketVolume::Total(Scalar::Float64(n)) => *n,
        // For drawing only: the text shows every digit.
        BucketVolume::Total(Scalar::Decimal(n)) => match text.ty {
            ColumnType::Decimal { scale, .. } => *n as f64 / 10f64.powi(i32::from(scale)),
            _ => *n as f64,
        },
        _ => f64::NAN,
    };
    let words = |v: &BucketVolume| match v {
        BucketVolume::None => None,
        BucketVolume::Total(total) => Some(text.value(total)),
        BucketVolume::Overflow => Some(OVERFLOW_TEXT.to_owned()),
    };
    [
        floats("volume", volumes.iter().map(number).collect()),
        texts("volume_text", volumes.iter().map(words).collect()),
    ]
}

/// How a frame's indicator lines are written.
struct IndicatorTexts<'a> {
    x: &'a Texts<'a>,
    series: Option<&'a Texts<'a>>,
    /// Averages, bands, MACD, ATR and VWAP are in the price's units.
    price: Formatter,
    /// RSI and the stochastic run from 0 to 100.
    oscillator: Formatter,
}

impl IndicatorTexts<'_> {
    fn fill(
        &self,
        frame: &mut ChartFrame,
        config: &ChartConfig,
        indicators: &[IndicatorLines],
    ) -> Result<(), String> {
        let (mut ids, mut xs, mut ys, mut x_texts, mut y_texts) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for lines in indicators {
            let indicator = config.indicators[lines.indicator];
            let formatter = if indicator.in_price_units() {
                &self.price
            } else {
                &self.oscillator
            };
            let series = match (&lines.key, self.series) {
                (Some(key), Some(texts)) => texts.value(key),
                _ => String::new(),
            };
            for (name, points) in &lines.lines {
                let id = frame.indicator_lines.len() as u32;
                frame.indicator_lines.push(ChartIndicatorLine {
                    indicator: lines.indicator as u32,
                    line: (*name).to_owned(),
                    series: series.clone(),
                });
                for p in points {
                    ids.push(id);
                    xs.push(p.x);
                    ys.push(p.y);
                    x_texts.push(self.x.cell(p.slot));
                    y_texts
                        .push((!p.y.is_nan()).then(|| {
                            formatter.scalar(&ColumnType::Float64, &Scalar::Float64(p.y))
                        }));
                }
            }
        }
        frame.indicator_batch = batch(vec![
            (
                field("line", DataType::UInt32, false),
                Arc::new(UInt32Array::from(ids)),
            ),
            floats("x", xs),
            floats("y", ys),
            texts("x_text", x_texts),
            texts("y_text", y_texts),
        ])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use arrow_array::{Array, ArrayRef, Decimal128Array, RecordBatch};
    use arrow_schema::{Field, Schema};
    use parallax_engine::format::{Abbreviation, Format};
    use parallax_engine::{Table, TableOptions};

    use super::price_format;

    fn table(scale: i8) -> Table {
        let prices = Decimal128Array::from(vec![101, 102])
            .with_precision_and_scale(38, scale)
            .unwrap();
        let schema = Schema::new(vec![Field::new("close", prices.data_type().clone(), false)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(prices) as ArrayRef]).unwrap();
        let mut t = Table::new(&batch.schema(), TableOptions::default()).unwrap();
        t.load(&[batch]).unwrap();
        t
    }

    #[test]
    fn indicator_texts_keep_the_price_format_and_enough_decimals() {
        // Whole prices still give averages with two decimals.
        let none = BTreeMap::new();
        assert_eq!(price_format(&table(0), &none, "close").decimals, Some(2));
        assert_eq!(price_format(&table(4), &none, "close").decimals, Some(4));
        // A scale past what a format can show is held to the most it can, and
        // the rest of the column's format is kept.
        let abbreviated = BTreeMap::from([(
            "close".to_owned(),
            Format {
                abbreviate: Some(Abbreviation::Thousands),
                ..Format::default()
            },
        )]);
        let format = price_format(&table(32), &abbreviated, "close");
        assert_eq!(format.decimals, Some(30));
        assert_eq!(format.abbreviate, Some(Abbreviation::Thousands));
        assert!(parallax_engine::Formatter::new(&format).is_ok());
        // Decimals the layout names are used as they are.
        let named = BTreeMap::from([(
            "close".to_owned(),
            Format {
                decimals: Some(0),
                ..Format::default()
            },
        )]);
        assert_eq!(price_format(&table(4), &named, "close").decimals, Some(0));
    }
}
