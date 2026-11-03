//! Charts, reduced to the screen before they leave the engine.
//!
//! A [`ChartView`] reads a table through a layout's filters, like any view, and
//! answers a request for an x range at a pixel size with no more than the screen
//! can show:
//!
//! - [`ChartKind::Line`] and [`ChartKind::Area`] series keep, in each pixel
//!   column, the first, lowest, highest and last point (M4). The line through
//!   them covers exactly the same pixels as the line through every point, so
//!   spikes never vanish, and a chart 2,000 pixels wide needs at most 8,000
//!   points per series however long the series is.
//! - [`ChartKind::Scatter`] becomes a density: counts in cells of a few pixels.
//! - [`ChartKind::Histogram`] counts values in bins with stated edges.
//! - [`ChartKind::Candlestick`] and [`ChartKind::Ohlc`] bucket rows by a calendar
//!   interval cut in New York time, chosen so that candles stay a few pixels
//!   wide, and keep each bucket's first open, highest high, lowest low and last
//!   close.
//!
//! Bar charts and categorical heatmaps are grouped views, which already arrive
//! aggregated.
//!
//! The x axis is either the x values themselves or trading time, where closed
//! days and the hours outside the session take no space. Each reduced value
//! carries the slot of the row it came from, so callers can show its exact
//! value, and the x axis's ticks come with their labels.

use std::ops::Range;

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;

use crate::calendar::{TradingCalendar, epoch_days, from_epoch_days};
use crate::column::Column;
use crate::error::ConfigError;
use crate::filter::{CompiledFilter, Filter};
use crate::table::Table;
use crate::types::{ColumnType, Key, Scalar};

/// What a chart draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartKind {
    /// Lines through each series.
    Line,
    /// Lines through each series, filled down to zero.
    Area,
    /// Points, drawn as a density.
    Scatter,
    /// Counts of the x values in bins.
    Histogram,
    /// Candles of each bucket's open, high, low and close.
    Candlestick,
    /// Bars of each bucket's open, high, low and close.
    Ohlc,
}

/// How x values are placed along the axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XAxis {
    /// By value: an instant, a date or a number.
    #[default]
    Value,
    /// By trading time on the NYSE calendar: closed days and the hours outside
    /// the session take no space.
    Trading,
}

/// The price columns of a candlestick or OHLC chart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OhlcColumns {
    /// The opening price.
    pub open: String,
    /// The highest price.
    pub high: String,
    /// The lowest price.
    pub low: String,
    /// The closing price.
    pub close: String,
}

/// What a chart shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartConfig {
    /// What it draws.
    pub kind: ChartKind,
    /// The x column: an instant, a date or a number. A histogram bins it.
    pub x: String,
    /// The plotted columns of line, area and scatter charts; a scatter plot uses
    /// the first.
    pub y: Vec<String>,
    /// A column whose values split every plotted column into series, such as the
    /// ticker.
    pub series: Option<String>,
    /// The price columns of candlestick and OHLC charts.
    pub ohlc: Option<OhlcColumns>,
    /// Row filters.
    pub filters: Vec<Filter>,
    /// How x values are placed.
    pub axis: XAxis,
}

/// The part of a chart to reduce, and the pixels it fills.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartRequest {
    /// The x range in axis positions, or the whole data when absent.
    pub x: Option<(f64, f64)>,
    /// The y range of a density, or the whole data when absent.
    pub y: Option<(f64, f64)>,
    /// The plot's width in pixels.
    pub width: u32,
    /// The plot's height in pixels.
    pub height: u32,
}

/// Pixels per density cell side.
pub const DENSITY_CELL: u32 = 4;
/// The narrowest candle, in pixels: the bucket interval grows until candles are
/// at least this wide.
pub const MIN_CANDLE: u32 = 4;
/// Pixels per histogram bin.
pub const BIN_WIDTH: u32 = 8;
/// Pixels between x-axis ticks, roughly.
pub const TICK_SPACING: u32 = 100;

/// A series of a chart: one plotted column, for one value of the series column.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// The plotted column, by index in [`ChartConfig::y`].
    pub y: usize,
    /// The series column's value, or `None` without a series column.
    pub key: Option<Scalar>,
}

/// A point that survived reduction, or a break in the line where values are
/// missing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// The x position on the axis.
    pub x: f64,
    /// The y value, or NaN at a break.
    pub y: f64,
    /// The row it came from, or `None` at a break.
    pub slot: Option<usize>,
}

/// One calendar bucket of a candlestick or OHLC chart, by the rows that give its
/// prices. A price no row of the bucket has is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candle {
    /// The first row with an open, by time and then insertion order.
    pub open: Option<usize>,
    /// The row with the highest high; the first such on ties.
    pub high: Option<usize>,
    /// The row with the lowest low; the first such on ties.
    pub low: Option<usize>,
    /// The last row with a close.
    pub close: Option<usize>,
}

/// A calendar bucket and where it spans on the axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bucket {
    /// The bucket's start as an x value: days since the epoch for dates,
    /// microseconds for instants.
    pub start: i64,
    /// The axis position of the bucket's start.
    pub from: f64,
    /// The axis position of the next bucket's start.
    pub to: f64,
}

/// A bucket interval for candles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interval {
    /// Minutes, for instants.
    Minutes(u32),
    /// Days.
    Day,
    /// Weeks starting on Monday.
    Week,
    /// Calendar months.
    Month,
    /// Calendar quarters.
    Quarter,
    /// Calendar years.
    Year,
    /// A number of calendar years, starting in years divisible by it, for ticks
    /// over long ranges.
    Years(u16),
}

/// A tick on the x axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    /// Its axis position.
    pub x: f64,
    /// Its label.
    pub label: String,
}

/// A reduced chart.
#[derive(Clone, Debug, PartialEq)]
pub enum ChartData {
    /// Line and area series.
    Series(Vec<(Series, Vec<Point>)>),
    /// A density of points: `counts[row * columns + column]`, with row 0 at the
    /// bottom.
    Density {
        /// The x range the cells span.
        x: (f64, f64),
        /// The y range the cells span.
        y: (f64, f64),
        /// Cells across.
        columns: u32,
        /// Cells up.
        rows: u32,
        /// Points per cell.
        counts: Vec<u32>,
    },
    /// Bin edges and the counts in each bin, per series. The last bin includes
    /// its upper edge.
    Histogram {
        /// `bins + 1` edges, ascending.
        edges: Vec<f64>,
        /// Counts per series, one per bin.
        counts: Vec<(Option<Scalar>, Vec<u64>)>,
    },
    /// Candles in x order.
    Candles {
        /// The bucket interval chosen.
        interval: Interval,
        /// Each bucket's span and the rows giving its prices.
        candles: Vec<(Bucket, Candle)>,
    },
}

/// A reduced chart and its x axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Reduced {
    /// The data.
    pub data: ChartData,
    /// The x range shown, in axis positions.
    pub x: (f64, f64),
    /// The x axis's ticks.
    pub ticks: Vec<Tick>,
}

/// How the x column's values become axis positions.
#[derive(Clone, Debug)]
enum Placement {
    /// Numbers as they are; a decimal's unscaled value over `10^scale`.
    Number { scale: Option<i8> },
    /// Days since the epoch.
    Date,
    /// Microseconds since the epoch.
    Instant,
    /// Trading days on the calendar.
    TradingDate(TradingCalendar),
    /// Microseconds of open market on the calendar.
    TradingInstant(TradingCalendar),
}

impl Placement {
    fn new(ty: &ColumnType, axis: XAxis, column: &str) -> Result<Self, ConfigError> {
        let wrong = || ConfigError::ChartColumn {
            role: match axis {
                XAxis::Value => "x axis".into(),
                XAxis::Trading => "trading-time x axis".into(),
            },
            column: column.to_owned(),
            data_type: ty.to_arrow().to_string(),
        };
        Ok(match (ty, axis) {
            (ColumnType::Int64 | ColumnType::Float64, XAxis::Value) => Self::Number { scale: None },
            (ColumnType::Decimal { scale, .. }, XAxis::Value) => Self::Number {
                scale: Some(*scale),
            },
            (ColumnType::Date32, XAxis::Value) => Self::Date,
            (ColumnType::Timestamp { .. }, XAxis::Value) => Self::Instant,
            (ColumnType::Date32, XAxis::Trading) => Self::TradingDate(TradingCalendar::nyse()),
            (ColumnType::Timestamp { .. }, XAxis::Trading) => {
                Self::TradingInstant(TradingCalendar::nyse())
            }
            _ => return Err(wrong()),
        })
    }

    /// The axis position of a key of the x column, or `None` for a null or NaN.
    fn position(&self, key: Key) -> Option<f64> {
        let position = match (self, key) {
            (_, Key::Null) => return None,
            (Self::Number { scale }, Key::Int(_) | Key::Float(_) | Key::Decimal(_)) => {
                crate::aggregate::key_to_f64(key, scale.unwrap_or(0))
            }
            (Self::Date, Key::Date(d)) => f64::from(d),
            (Self::Instant, Key::Ts(t)) => t as f64,
            (Self::TradingDate(c), Key::Date(d)) => c.date_position(d),
            (Self::TradingInstant(c), Key::Ts(t)) => c.instant_position(t),
            _ => return None,
        };
        position.is_finite().then_some(position)
    }
}

/// A numeric column's value at `slot` as a double, a decimal correctly rounded,
/// or `None` for a null, a NaN or a column that is not numeric.
#[must_use]
pub fn number(column: &Column, slot: usize) -> Option<f64> {
    y_value(column, slot)
}

/// The y value of a slot as a double, or `None` for a null or NaN.
fn y_value(column: &Column, slot: usize) -> Option<f64> {
    let scale = match column.column_type() {
        ColumnType::Decimal { scale, .. } => *scale,
        _ => 0,
    };
    let value = crate::aggregate::key_to_f64(column.key(slot), scale);
    value.is_finite().then_some(value)
}

/// An x key's own value, which orders rows that share an axis position: instants
/// outside the session all take its edge on a trading-time axis.
fn x_order(key: Key) -> i128 {
    match key {
        Key::Int(v) | Key::Ts(v) => i128::from(v),
        Key::Date(d) => i128::from(d),
        Key::Decimal(v) => v,
        Key::Float(bits) => i128::from(bits),
        _ => 0,
    }
}

fn numeric(ty: &ColumnType) -> bool {
    matches!(
        ty,
        ColumnType::Int64 | ColumnType::Float64 | ColumnType::Decimal { .. }
    )
}

/// Rows of one series in x order: their x positions and slots.
#[derive(Clone, Debug, Default)]
struct Ordered {
    x: Vec<f64>,
    slots: Vec<usize>,
}

impl Ordered {
    /// The rows whose x position lies in `from..=to`, plus the nearest row on
    /// each side, so that lines run to the plot's edges.
    fn around(&self, from: f64, to: f64) -> Range<usize> {
        let start = self.x.partition_point(|&x| x < from);
        let end = self.x.partition_point(|&x| x <= to);
        start.saturating_sub(1)..(end + 1).min(self.x.len())
    }

    fn within(&self, from: f64, to: f64) -> Range<usize> {
        self.x.partition_point(|&x| x < from)..self.x.partition_point(|&x| x <= to)
    }
}

/// A chart of a table.
#[derive(Clone, Debug)]
pub struct ChartView {
    config: ChartConfig,
    filters: Vec<CompiledFilter>,
    x: usize,
    placement: Placement,
    y: Vec<usize>,
    series: Option<usize>,
    ohlc: Option<[usize; 4]>,
    /// Rows by series key, each in x order.
    rows: Vec<(Option<Key>, Ordered)>,
    load_version: u64,
}

impl ChartView {
    /// Opens a chart of `table`.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration names unknown columns, or columns of
    /// the wrong type for the chart.
    pub fn new(table: &Table, config: ChartConfig) -> Result<Self, ConfigError> {
        let index = |name: &str| {
            table
                .column_index(name)
                .ok_or_else(|| ConfigError::UnknownColumn(name.to_owned()))
        };
        let numeric_column = |name: &str, role: &str| -> Result<usize, ConfigError> {
            let i = index(name)?;
            let ty = table.column(i).column_type();
            if numeric(ty) {
                Ok(i)
            } else {
                Err(ConfigError::ChartColumn {
                    role: role.to_owned(),
                    column: name.to_owned(),
                    data_type: ty.to_arrow().to_string(),
                })
            }
        };
        let filters = config
            .filters
            .iter()
            .map(|f| CompiledFilter::compile(table, f))
            .collect::<Result<Vec<_>, _>>()?;
        let x = index(&config.x)?;
        let placement = Placement::new(table.column(x).column_type(), config.axis, &config.x)?;
        let y = config
            .y
            .iter()
            .map(|c| numeric_column(c, "y axis"))
            .collect::<Result<Vec<_>, _>>()?;
        let ohlc = match &config.ohlc {
            Some(o) => Some([
                numeric_column(&o.open, "open price")?,
                numeric_column(&o.high, "high price")?,
                numeric_column(&o.low, "low price")?,
                numeric_column(&o.close, "close price")?,
            ]),
            None => None,
        };
        let needs_y = matches!(
            config.kind,
            ChartKind::Line | ChartKind::Area | ChartKind::Scatter
        );
        if needs_y && y.is_empty() {
            return Err(ConfigError::MissingChartColumns("y".into()));
        }
        if matches!(config.kind, ChartKind::Candlestick | ChartKind::Ohlc) && ohlc.is_none() {
            return Err(ConfigError::MissingChartColumns(
                "open, high, low and close".into(),
            ));
        }
        let candles = matches!(config.kind, ChartKind::Candlestick | ChartKind::Ohlc);
        if candles && matches!(placement, Placement::Number { .. }) {
            return Err(ConfigError::ChartColumn {
                role: "x axis of a candlestick or OHLC chart, which buckets by calendar".into(),
                column: config.x.clone(),
                data_type: table.column(x).column_type().to_arrow().to_string(),
            });
        }
        if candles && !config.y.is_empty() {
            return Err(ConfigError::Chart(
                "a candlestick or OHLC chart draws its prices only; plot other columns in a chart of their own".into(),
            ));
        }
        if candles && config.series.is_some() {
            return Err(ConfigError::Chart(
                "a candlestick or OHLC chart shows one instrument; filter to it instead of splitting by a series column".into(),
            ));
        }
        if config.kind == ChartKind::Histogram && !matches!(placement, Placement::Number { .. }) {
            return Err(ConfigError::ChartColumn {
                role: "binned column of a histogram".into(),
                column: config.x.clone(),
                data_type: table.column(x).column_type().to_arrow().to_string(),
            });
        }
        let series = config.series.as_deref().map(index).transpose()?;
        let mut view = Self {
            config,
            filters,
            x,
            placement,
            y,
            series,
            ohlc,
            rows: Vec::new(),
            load_version: table.load_version(),
        };
        view.rebuild(table);
        Ok(view)
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &ChartConfig {
        &self.config
    }

    /// The table load this chart reflects.
    #[must_use]
    pub fn load_version(&self) -> u64 {
        self.load_version
    }

    /// Recomputes the chart's rows from the table.
    pub fn rebuild(&mut self, table: &Table) {
        self.load_version = table.load_version();
        let x = table.column(self.x);
        // Each row's position, its own x value, insertion number and slot.
        type Placed = (f64, i128, u64, usize);
        let mut by_key: rustc_hash::FxHashMap<Option<Key>, Vec<Placed>> =
            rustc_hash::FxHashMap::default();
        for slot in table.live_slots() {
            if !self
                .filters
                .iter()
                .all(|f| f.passes(table.column(f.column), slot))
            {
                continue;
            }
            let x_key = x.key(slot);
            let Some(position) = self.placement.position(x_key) else {
                continue;
            };
            let key = self.series.map(|s| table.column(s).key(slot));
            by_key
                .entry(key)
                .or_default()
                .push((position, x_order(x_key), table.seq(slot), slot));
        }
        let mut rows: Vec<(Option<Key>, Ordered)> = by_key
            .into_iter()
            .map(|(key, mut points)| {
                // By position, then by the value itself, then by insertion.
                points.sort_unstable_by(|a, b| {
                    a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
                });
                let ordered = Ordered {
                    x: points.iter().map(|p| p.0).collect(),
                    slots: points.iter().map(|p| p.3).collect(),
                };
                (key, ordered)
            })
            .collect();
        // Series in the order of their keys, nulls last.
        if let Some(s) = self.series {
            let column = table.column(s);
            rows.sort_by(|a, b| match (a.0, b.0) {
                (Some(Key::Null), Some(Key::Null)) => std::cmp::Ordering::Equal,
                (Some(Key::Null), _) => std::cmp::Ordering::Greater,
                (_, Some(Key::Null)) => std::cmp::Ordering::Less,
                (Some(x), Some(y)) => column.cmp_keys(x, y),
                _ => std::cmp::Ordering::Equal,
            });
        }
        self.rows = rows;
    }

    /// The x range of the data, in axis positions, or `None` without rows.
    #[must_use]
    pub fn x_extent(&self) -> Option<(f64, f64)> {
        let first = self.rows.iter().filter_map(|(_, o)| o.x.first()).copied();
        let last = self.rows.iter().filter_map(|(_, o)| o.x.last()).copied();
        Some((first.reduce(f64::min)?, last.reduce(f64::max)?))
    }

    fn scalar(&self, table: &Table, key: Option<Key>) -> Option<Scalar> {
        let (s, key) = (self.series?, key?);
        Some(table.column(s).key_to_scalar(key))
    }

    /// Reduces the chart for `request`.
    #[must_use]
    pub fn reduce(&self, table: &Table, request: &ChartRequest) -> Reduced {
        let width = request.width.max(1);
        let (from, to) = request.x.or_else(|| self.x_extent()).unwrap_or((0.0, 1.0));
        let (from, to) = if to > from {
            (from, to)
        } else {
            (from - 0.5, from + 0.5)
        };
        let data = match self.config.kind {
            ChartKind::Line | ChartKind::Area => {
                ChartData::Series(self.series_m4(table, from, to, width))
            }
            ChartKind::Scatter => self.density(table, from, to, request),
            ChartKind::Histogram => self.histogram(table, from, to, request),
            ChartKind::Candlestick | ChartKind::Ohlc => self.candles(table, from, to, width),
        };
        let ticks = self.ticks(from, to, width);
        Reduced {
            data,
            x: (from, to),
            ticks,
        }
    }

    fn series_m4(
        &self,
        table: &Table,
        from: f64,
        to: f64,
        width: u32,
    ) -> Vec<(Series, Vec<Point>)> {
        let mut out = Vec::new();
        for (key, ordered) in &self.rows {
            let range = ordered.around(from, to);
            for (i, &y) in self.y.iter().enumerate() {
                let column = table.column(y);
                let points = range.clone().map(|r| {
                    let slot = ordered.slots[r];
                    (ordered.x[r], y_value(column, slot), slot)
                });
                out.push((
                    Series {
                        y: i,
                        key: self.scalar(table, *key),
                    },
                    m4(points, from, to, width),
                ));
            }
        }
        out
    }

    fn density(&self, table: &Table, from: f64, to: f64, request: &ChartRequest) -> ChartData {
        let columns = (request.width / DENSITY_CELL).max(1);
        let rows = (request.height / DENSITY_CELL).max(1);
        let y_column = table.column(self.y[0]);
        let points: Vec<(f64, f64)> = self
            .rows
            .iter()
            .flat_map(|(_, o)| {
                o.within(from, to)
                    .filter_map(|r| Some((o.x[r], y_value(y_column, o.slots[r])?)))
            })
            .collect();
        let (bottom, top) = request
            .y
            .or_else(|| {
                let low = points.iter().map(|p| p.1).reduce(f64::min)?;
                let high = points.iter().map(|p| p.1).reduce(f64::max)?;
                Some((low, high))
            })
            .unwrap_or((0.0, 1.0));
        let (bottom, top) = if top > bottom {
            (bottom, top)
        } else {
            (bottom - 0.5, bottom + 0.5)
        };
        let mut counts = vec![0u32; (columns * rows) as usize];
        for (x, y) in points {
            if y < bottom || y > top {
                continue;
            }
            let c = cell(x, from, to, columns);
            let r = cell(y, bottom, top, rows);
            counts[(r * columns + c) as usize] += 1;
        }
        ChartData::Density {
            x: (from, to),
            y: (bottom, top),
            columns,
            rows,
            counts,
        }
    }

    fn histogram(&self, table: &Table, from: f64, to: f64, request: &ChartRequest) -> ChartData {
        let bins = (request.width / BIN_WIDTH).max(1);
        // Edges are computed once and every value is placed by comparing with
        // them, so a value on an edge lands in the bin the edge's label opens.
        let mut edges: Vec<f64> = (0..=bins)
            .map(|i| from + (to - from) * f64::from(i) / f64::from(bins))
            .collect();
        if let Some(last) = edges.last_mut() {
            *last = to;
        }
        let counts = self
            .rows
            .iter()
            .map(|(key, o)| {
                let mut counts = vec![0u64; bins as usize];
                for r in o.within(from, to) {
                    let bin = edges.partition_point(|&e| e <= o.x[r]).saturating_sub(1);
                    counts[bin.min(bins as usize - 1)] += 1;
                }
                (self.scalar(table, *key), counts)
            })
            .collect();
        ChartData::Histogram { edges, counts }
    }

    fn candles(&self, table: &Table, from: f64, to: f64, width: u32) -> ChartData {
        let instants = matches!(
            self.placement,
            Placement::Instant | Placement::TradingInstant(_)
        );
        let interval = self.interval(from, to, width, instants);
        let (Some([open, high, low, close]), Some((_, rows))) = (self.ohlc, self.rows.first())
        else {
            return ChartData::Candles {
                interval,
                candles: Vec::new(),
            };
        };
        let x = table.column(self.x);
        let zone = TimeZone::get(crate::format::DEFAULT_ZONE).unwrap_or(TimeZone::UTC);
        let bucket_of = |r: usize| bucket_start(x.key(rows.slots[r]), interval, &zone);
        // Buckets crossing either edge of the range keep all their rows, so an
        // edge candle shows its whole bucket however the chart is panned.
        let within = rows.within(from, to);
        let mut start = within.start;
        if let Some(first) = (start < within.end).then(|| bucket_of(start)).flatten() {
            while start > 0 && bucket_of(start - 1) == Some(first) {
                start -= 1;
            }
        }
        let mut end = within.end;
        if let Some(last) = (end > start).then(|| bucket_of(end - 1)).flatten() {
            while end < rows.slots.len() && bucket_of(end) == Some(last) {
                end += 1;
            }
        }
        let (o, h, l, c) = (
            table.column(open),
            table.column(high),
            table.column(low),
            table.column(close),
        );
        let mut candles: Vec<(Bucket, Candle)> = Vec::new();
        let mut current: Option<(i64, Bucket, Candle, f64, f64)> = None;
        for r in start..end {
            let slot = rows.slots[r];
            let Some(bucket) = bucket_of(r) else { continue };
            if current.as_ref().is_none_or(|(key, ..)| *key != bucket) {
                if let Some((_, span, candle, _, _)) = current.take() {
                    candles.push((span, candle));
                }
                let span = Bucket {
                    start: bucket,
                    from: self.bucket_position(bucket, interval, instants, &zone, false),
                    to: self.bucket_position(bucket, interval, instants, &zone, true),
                };
                let empty = Candle {
                    open: None,
                    high: None,
                    low: None,
                    close: None,
                };
                current = Some((bucket, span, empty, f64::NEG_INFINITY, f64::INFINITY));
            }
            let Some((_, _, candle, highest, lowest)) = current.as_mut() else {
                continue;
            };
            // Each price comes from a row that has it.
            if candle.open.is_none() && y_value(o, slot).is_some() {
                candle.open = Some(slot);
            }
            if let Some(v) = y_value(h, slot)
                && v > *highest
            {
                *highest = v;
                candle.high = Some(slot);
            }
            if let Some(v) = y_value(l, slot)
                && v < *lowest
            {
                *lowest = v;
                candle.low = Some(slot);
            }
            if y_value(c, slot).is_some() {
                candle.close = Some(slot);
            }
        }
        if let Some((_, span, candle, _, _)) = current {
            candles.push((span, candle));
        }
        ChartData::Candles { interval, candles }
    }

    /// The smallest interval whose buckets in `from..to` are at least
    /// [`MIN_CANDLE`] pixels wide.
    fn interval(&self, from: f64, to: f64, width: u32, instants: bool) -> Interval {
        let fits = |buckets: f64| buckets * f64::from(MIN_CANDLE) <= f64::from(width);
        let span = to - from;
        let candidates: &[Interval] = if instants {
            &[
                Interval::Minutes(1),
                Interval::Minutes(5),
                Interval::Minutes(15),
                Interval::Minutes(30),
                Interval::Minutes(60),
                Interval::Day,
                Interval::Week,
                Interval::Month,
                Interval::Quarter,
                Interval::Year,
            ]
        } else {
            &[
                Interval::Day,
                Interval::Week,
                Interval::Month,
                Interval::Quarter,
                Interval::Year,
            ]
        };
        // An interval's length in axis units: trading time gives a day 6.5 hours
        // and a week five days.
        let length = |interval: Interval| -> f64 {
            let day = match &self.placement {
                Placement::TradingInstant(c) => c.session_micros() as f64,
                Placement::Instant => 86_400_000_000.0,
                _ => 1.0,
            };
            let trading = matches!(
                self.placement,
                Placement::TradingDate(_) | Placement::TradingInstant(_)
            );
            let week = if trading { 5.0 } else { 7.0 };
            let year = if trading { 252.0 } else { 365.25 };
            match interval {
                Interval::Minutes(m) => f64::from(m) * 60_000_000.0,
                Interval::Day => day,
                Interval::Week => day * week,
                Interval::Month => day * year / 12.0,
                Interval::Quarter => day * year / 4.0,
                Interval::Year => day * year,
                Interval::Years(n) => day * year * f64::from(n),
            }
        };
        candidates
            .iter()
            .copied()
            .find(|&i| fits(span / length(i)))
            .unwrap_or(Interval::Year)
    }

    /// The axis position of a bucket's start, or with `next`, of the next
    /// bucket's start.
    fn bucket_position(
        &self,
        start: i64,
        interval: Interval,
        instants: bool,
        zone: &TimeZone,
        next: bool,
    ) -> f64 {
        let start = if next {
            advance(start, interval, instants, zone)
        } else {
            start
        };
        let key = if instants {
            Key::Ts(start)
        } else {
            Key::Date(i32::try_from(start).unwrap_or(i32::MAX))
        };
        self.placement.position(key).unwrap_or(f64::NAN)
    }

    /// Ticks for the x axis between `from` and `to`, about [`TICK_SPACING`]
    /// pixels apart, labelled in New York time for dates and instants.
    fn ticks(&self, from: f64, to: f64, width: u32) -> Vec<Tick> {
        let wanted = (f64::from(width) / f64::from(TICK_SPACING)).max(1.0);
        match &self.placement {
            Placement::Number { .. } => {
                let (values, step) = nice_ticks(from, to, wanted);
                let labels = number_labels(&values, step);
                values
                    .into_iter()
                    .zip(labels)
                    .map(|(x, label)| Tick { x, label })
                    .collect()
            }
            placement => time_ticks(placement, from, to, wanted),
        }
    }
}

/// The cell of `value` among `cells` equal cells spanning `from..=to`.
fn cell(value: f64, from: f64, to: f64, cells: u32) -> u32 {
    let at = ((value - from) / (to - from) * f64::from(cells)).floor();
    (at.max(0.0) as u32).min(cells - 1)
}

/// The pixel column of `x`: `-1` before the plot and `width` after it.
fn column(x: f64, from: f64, to: f64, width: u32) -> i64 {
    if x < from {
        return -1;
    }
    if x > to {
        return i64::from(width);
    }
    let at = ((x - from) / (to - from) * f64::from(width)).floor() as i64;
    at.min(i64::from(width) - 1)
}

/// M4: per pixel column, the first, lowest, highest and last point of each run
/// of values, in the order given, with a break wherever values are missing.
/// `points` must be in series order: by x, then insertion.
fn m4(
    points: impl Iterator<Item = (f64, Option<f64>, usize)>,
    from: f64,
    to: f64,
    width: u32,
) -> Vec<Point> {
    /// A point and its place in the series.
    type Placed = (usize, Point);
    struct Run {
        column: i64,
        first: Placed,
        low: Placed,
        high: Placed,
        last: Placed,
    }
    fn flush(run: Option<Run>, out: &mut Vec<Point>) {
        let Some(run) = run else { return };
        let mut kept = [run.first, run.low, run.high, run.last];
        // In series order; the same point once.
        kept.sort_by_key(|p| p.0);
        let mut previous = None;
        for (place, point) in kept {
            if previous != Some(place) {
                out.push(point);
                previous = Some(place);
            }
        }
    }
    let mut out = Vec::new();
    let mut run: Option<Run> = None;
    let mut broken = false;
    for (place, (x, y, slot)) in points.enumerate() {
        let Some(y) = y else {
            flush(run.take(), &mut out);
            if !broken {
                out.push(Point {
                    x,
                    y: f64::NAN,
                    slot: None,
                });
                broken = true;
            }
            continue;
        };
        broken = false;
        let point = (
            place,
            Point {
                x,
                y,
                slot: Some(slot),
            },
        );
        let c = column(x, from, to, width);
        match &mut run {
            Some(r) if r.column == c => {
                if y < r.low.1.y {
                    r.low = point;
                }
                if y > r.high.1.y {
                    r.high = point;
                }
                r.last = point;
            }
            _ => {
                flush(run.take(), &mut out);
                run = Some(Run {
                    column: c,
                    first: point,
                    low: point,
                    high: point,
                    last: point,
                });
            }
        }
    }
    flush(run, &mut out);
    out
}

/// The start of the bucket holding an x key: microseconds for instants, days for
/// dates, cut in `zone`.
fn bucket_start(key: Key, interval: Interval, zone: &TimeZone) -> Option<i64> {
    match key {
        Key::Date(d) => Some(i64::from(epoch_days(truncate_date(
            from_epoch_days(d),
            interval,
        )))),
        Key::Ts(t) => {
            let zoned = jiff::Timestamp::from_microsecond(t)
                .ok()?
                .to_zoned(zone.clone());
            match interval {
                Interval::Minutes(m) => {
                    // Cut on local time by the instant's own offset, so the
                    // repeated autumn hour keeps its own buckets.
                    let offset = i64::from(zoned.offset().seconds()) * 1_000_000;
                    let step = i64::from(m) * 60_000_000;
                    let local = t + offset;
                    Some(local - local.rem_euclid(step) - offset)
                }
                other => Some(
                    DateTime::from(truncate_date(zoned.date(), other))
                        .to_zoned(zone.clone())
                        .ok()?
                        .timestamp()
                        .as_microsecond(),
                ),
            }
        }
        _ => None,
    }
}

fn truncate_date(day: Date, interval: Interval) -> Date {
    match interval {
        Interval::Minutes(_) | Interval::Day => day,
        Interval::Week => day
            .checked_sub(jiff::Span::new().days(i64::from(day.weekday().to_monday_zero_offset())))
            .unwrap_or(day),
        Interval::Month => day.first_of_month(),
        Interval::Quarter => {
            let month = (day.month() - 1) / 3 * 3 + 1;
            jiff::civil::date(day.year(), month, 1)
        }
        Interval::Year => day.first_of_year(),
        Interval::Years(n) => {
            let n = i16::try_from(n.max(1)).unwrap_or(i16::MAX);
            jiff::civil::date(day.year() - day.year().rem_euclid(n), 1, 1)
        }
    }
}

/// The start of the bucket after the one starting at `start`.
fn advance(start: i64, interval: Interval, instants: bool, zone: &TimeZone) -> i64 {
    let span = match interval {
        Interval::Minutes(m) => jiff::Span::new().minutes(i64::from(m)),
        Interval::Day => jiff::Span::new().days(1),
        Interval::Week => jiff::Span::new().weeks(1),
        Interval::Month => jiff::Span::new().months(1),
        Interval::Quarter => jiff::Span::new().months(3),
        Interval::Year => jiff::Span::new().years(1),
        Interval::Years(n) => jiff::Span::new().years(i64::from(n)),
    };
    if instants {
        jiff::Timestamp::from_microsecond(start)
            .ok()
            .and_then(|t| t.to_zoned(zone.clone()).checked_add(span).ok())
            .map_or(start, |z| z.timestamp().as_microsecond())
    } else {
        let day = from_epoch_days(i32::try_from(start).unwrap_or(0));
        day.checked_add(span)
            .map_or(start, |d| i64::from(epoch_days(d)))
    }
}

/// Round numbers between `from` and `to`, about `wanted` of them: steps of 1, 2
/// or 5 times a power of ten.
fn nice_ticks(from: f64, to: f64, wanted: f64) -> (Vec<f64>, f64) {
    let raw = (to - from) / wanted;
    if !(raw.is_finite() && raw > 0.0) {
        return (Vec::new(), 1.0);
    }
    let power = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|&s| s >= raw)
        .unwrap_or(10.0 * power);
    let first = (from / step).ceil() as i64;
    let last = (to / step).floor() as i64;
    ((first..=last).map(|i| i as f64 * step).collect(), step)
}

/// Labels for round tick values, one style per axis: in full with thousands
/// separators while the axis stays under 10,000, compact (K, M, B, T) once it
/// reaches it, with as many decimals as the step needs.
fn number_labels(values: &[f64], step: f64) -> Vec<String> {
    let largest = values.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let (divisor, suffix) = if largest >= 1e4 {
        [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")]
            .into_iter()
            .find(|(d, _)| largest >= *d)
            .unwrap_or((1.0, ""))
    } else {
        (1.0, "")
    };
    let scaled = step / divisor;
    // Ticks are multiples of the step, so the step's own decimals show each exactly.
    let mut decimals = 0usize;
    while decimals < 12 {
        let shifted = scaled * 10f64.powi(decimals as i32);
        if (shifted.round() - shifted).abs() <= 1e-9 * shifted.abs().max(1.0) {
            break;
        }
        decimals += 1;
    }
    values
        .iter()
        .map(|&v| {
            if v == 0.0 {
                return "0".to_owned();
            }
            let text = format!("{:.decimals$}", v / divisor);
            let (whole, fraction) = text
                .split_once('.')
                .map_or((text.as_str(), ""), |(w, f)| (w, f));
            let (sign, digits) = whole.strip_prefix('-').map_or(("", whole), |d| ("-", d));
            let mut grouped = String::new();
            for (i, ch) in digits.chars().enumerate() {
                if i > 0 && (digits.len() - i) % 3 == 0 {
                    grouped.push(',');
                }
                grouped.push(ch);
            }
            let fraction = if fraction.is_empty() {
                String::new()
            } else {
                format!(".{fraction}")
            };
            format!("{sign}{grouped}{fraction}{suffix}")
        })
        .collect()
}

/// Ticks on a date or instant axis at a calendar step, labelled in New York.
fn time_ticks(placement: &Placement, from: f64, to: f64, wanted: f64) -> Vec<Tick> {
    let zone = TimeZone::get(crate::format::DEFAULT_ZONE).unwrap_or(TimeZone::UTC);
    let (instants, calendar) = match placement {
        Placement::Instant => (true, None),
        Placement::TradingInstant(c) => (true, Some(c)),
        Placement::Date => (false, None),
        Placement::TradingDate(c) => (false, Some(c)),
        Placement::Number { .. } => return Vec::new(),
    };
    // The x value at a position; on a trading axis, the open moment there.
    let value_at = |position: f64| -> i64 {
        match (calendar, instants) {
            (Some(c), true) => c.instant_at(position),
            (Some(c), false) => i64::from(c.day_at(position.floor() as i64)),
            (None, _) => position as i64,
        }
    };
    // A step's length in axis units: trading time gives a day the session and a
    // year 252 trading days.
    let day = match calendar {
        Some(c) if instants => c.session_micros() as f64,
        None if instants => 86_400_000_000.0,
        _ => 1.0,
    };
    let (week, month, year) = if calendar.is_some() {
        (5.0, 21.0, 252.0)
    } else {
        (7.0, 30.44, 365.25)
    };
    let length = |i: Interval| match i {
        Interval::Minutes(m) => f64::from(m) * 60_000_000.0,
        Interval::Day => day,
        Interval::Week => day * week,
        Interval::Month => day * month,
        Interval::Quarter => day * month * 3.0,
        Interval::Year => day * year,
        Interval::Years(n) => day * year * f64::from(n),
    };
    let steps = [
        Interval::Minutes(1),
        Interval::Minutes(5),
        Interval::Minutes(15),
        Interval::Minutes(30),
        Interval::Minutes(60),
        Interval::Day,
        Interval::Week,
        Interval::Month,
        Interval::Quarter,
        Interval::Year,
        Interval::Years(2),
        Interval::Years(5),
        Interval::Years(10),
        Interval::Years(20),
        Interval::Years(50),
        Interval::Years(100),
    ];
    let step = steps
        .into_iter()
        .filter(|i| instants || !matches!(i, Interval::Minutes(_)))
        .find(|&i| (to - from) / length(i) <= wanted)
        .unwrap_or(Interval::Years(100));
    let label = |value: i64| -> String {
        if instants {
            let Ok(t) = jiff::Timestamp::from_microsecond(value) else {
                return String::new();
            };
            let local = t.to_zoned(zone.clone());
            match step {
                Interval::Minutes(_) => local.strftime("%H:%M").to_string(),
                Interval::Day | Interval::Week => local.strftime("%b %-d").to_string(),
                Interval::Month | Interval::Quarter => local.strftime("%b %Y").to_string(),
                Interval::Year | Interval::Years(_) => local.strftime("%Y").to_string(),
            }
        } else {
            let day = from_epoch_days(i32::try_from(value).unwrap_or(0));
            match step {
                Interval::Minutes(_) | Interval::Day | Interval::Week => {
                    day.strftime("%b %-d").to_string()
                }
                Interval::Month | Interval::Quarter => day.strftime("%b %Y").to_string(),
                Interval::Year | Interval::Years(_) => day.strftime("%Y").to_string(),
            }
        }
    };
    let key = |value: i64| {
        if instants {
            Key::Ts(value)
        } else {
            Key::Date(i32::try_from(value).unwrap_or(0))
        }
    };
    let (start, end) = (value_at(from), value_at(to));
    let mut ticks = Vec::new();
    let Some(mut at) = bucket_start(key(start), step, &zone) else {
        return ticks;
    };
    let mut guard = 0;
    while at <= end && guard < 1_000 {
        guard += 1;
        if let Some(x) = placement.position(key(at))
            && x >= from
            && x <= to
            && ticks.last().is_none_or(|t: &Tick| t.x < x)
        {
            // On a trading axis a closed moment sits where the market next opens,
            // so the tick there is labelled with that moment.
            let shown = if calendar.is_some() { value_at(x) } else { at };
            ticks.push(Tick {
                x,
                label: label(shown),
            });
        }
        let next = advance(at, step, instants, &zone);
        if next <= at {
            break;
        }
        at = next;
    }
    ticks
}
