//! Daily price bars for equities, with fat-tailed returns.
//!
//! Rows are ordered by security, then date. Each security's path is its own random
//! stream, so any range of rows can be generated on its own.

use std::ops::Range;
use std::sync::Arc;

use arrow_array::{ArrayRef, Date32Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use jiff::civil::{Date, date};

use crate::calendar::{epoch_days, trading_days};
use crate::dist::{log_normal, standard_normal, student_t_unit_variance};
use crate::error::OptionsError;
use crate::positions::HOSTILE_RATE;
use crate::rng::{Rng, Stream};
use crate::universe::{AssetClass, Universe};
use crate::write::Dataset;

/// Options for [`Prices`].
#[derive(Clone, Debug)]
pub struct PricesOptions {
    /// Seed shared with the universe.
    pub seed: u64,
    /// Number of equities with a price history, taken in order of how often they are
    /// held.
    pub tickers: usize,
    /// First date.
    pub start: Date,
    /// Last date.
    pub end: Date,
    /// Number of securities in the universe.
    pub securities: usize,
    /// Inject hostile values into about [`HOSTILE_RATE`] of rows.
    pub hostile: bool,
}

impl Default for PricesOptions {
    fn default() -> Self {
        Self {
            seed: 42,
            tickers: 100,
            start: date(2016, 1, 4),
            end: date(2026, 10, 30),
            securities: 10_000,
            hostile: false,
        }
    }
}

/// The prices dataset.
#[derive(Debug)]
pub struct Prices {
    options: PricesOptions,
    universe: Universe,
    schema: SchemaRef,
    equities: Vec<usize>,
    days: Vec<i32>,
}

#[derive(Clone, Copy, Debug)]
struct Bar {
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: Option<i64>,
    vwap: Option<f64>,
    return_1d: Option<f64>,
}

fn round4(x: f64) -> f64 {
    libm::round(x * 1e4) / 1e4
}

impl Prices {
    /// Builds the dataset description. Rows are generated on demand.
    ///
    /// # Errors
    ///
    /// Returns an error if the universe is empty or has fewer equities than
    /// `tickers`, or if there is no trading day between `start` and `end`.
    pub fn new(options: PricesOptions) -> Result<Self, OptionsError> {
        let universe = Universe::new(
            options.seed,
            options.securities,
            options.hostile,
            options.end,
        )?;
        let all_equities: Vec<usize> = universe
            .securities
            .iter()
            .enumerate()
            .filter(|(_, s)| s.asset_class == AssetClass::Equity)
            .map(|(i, _)| i)
            .collect();
        if all_equities.len() < options.tickers {
            return Err(OptionsError::NotEnoughEquities {
                wanted: options.tickers,
                available: all_equities.len(),
                securities: options.securities,
            });
        }
        let equities = all_equities[..options.tickers].to_vec();
        let days: Vec<i32> = trading_days(options.start, options.end)
            .into_iter()
            .map(epoch_days)
            .collect();
        if days.is_empty() {
            return Err(OptionsError::NoTradingDays {
                start: options.start,
                end: options.end,
            });
        }
        Ok(Self {
            options,
            universe,
            schema: Arc::new(schema()),
            equities,
            days,
        })
    }

    /// The trading days covered, as days since the Unix epoch.
    #[must_use]
    pub fn days(&self) -> &[i32] {
        &self.days
    }

    /// Simulates one security's whole path. Hostile values are bad prints: they
    /// change what is reported, and returns are always computed from the reported
    /// closes, but the true price path continues underneath.
    fn path(&self, ticker: usize) -> Vec<Bar> {
        let security = &self.universe.securities[self.equities[ticker]];
        let mut rng = Rng::for_key(self.options.seed, Stream::Prices, ticker as u64);
        let sigma = security.volatility / libm::sqrt(252.0);
        let drift = 0.07 / 252.0;
        let mut previous_close = round4(security.base_price);
        let mut previous_reported_close = previous_close;
        let first_row = ticker as u64 * self.days.len() as u64;
        let mut bars = Vec::with_capacity(self.days.len());
        for day in 0..self.days.len() {
            let ret = drift + sigma * student_t_unit_variance(&mut rng, 3);
            let gap = 0.3 * sigma * standard_normal(&mut rng);
            let open = round4((previous_close * (1.0 + gap)).max(0.0001));
            let close = round4((previous_close * (1.0 + ret)).max(0.0001));
            let spread_up = (0.5 * sigma * standard_normal(&mut rng)).abs();
            let spread_down = (0.5 * sigma * standard_normal(&mut rng)).abs();
            let high = round4(open.max(close) * (1.0 + spread_up));
            let low = round4((open.min(close) * (1.0 - spread_down)).max(0.0001));
            let volume = (security.average_daily_volume * log_normal(&mut rng, 0.0, 0.5)).round();
            let vwap = round4(low + (high - low) * rng.uniform(0.3, 0.7));
            let mut bar = Bar {
                open,
                high,
                low,
                close,
                volume: Some(volume as i64),
                vwap: Some(vwap),
                return_1d: None,
            };
            if self.options.hostile {
                self.inject_hostile(first_row + day as u64, &mut bar);
            }
            if day > 0 {
                bar.return_1d = Some(bar.close / previous_reported_close - 1.0);
            }
            previous_reported_close = bar.close;
            previous_close = close;
            bars.push(bar);
        }
        bars
    }

    fn inject_hostile(&self, row: u64, bar: &mut Bar) {
        let mut rng = Rng::for_key(self.options.seed, Stream::PricesHostile, row);
        if !rng.chance(HOSTILE_RATE) {
            return;
        }
        match rng.below(5) {
            // A missing close: this return and the next are NaN.
            0 => bar.close = f64::NAN,
            1 => bar.volume = None,
            2 => bar.vwap = None,
            // A halted day: one flat price and no volume.
            3 => {
                bar.open = bar.close;
                bar.high = bar.close;
                bar.low = bar.close;
                bar.vwap = Some(bar.close);
                bar.volume = Some(0);
            }
            // A zero close: this return is -1 and the next is infinite.
            _ => bar.close = 0.0,
        }
    }
}

/// The prices schema: 9 columns.
#[must_use]
pub fn schema() -> Schema {
    Schema::new(vec![
        Field::new("ticker", DataType::Utf8, false),
        Field::new("date", DataType::Date32, false),
        Field::new("open", DataType::Float64, false),
        Field::new("high", DataType::Float64, false),
        Field::new("low", DataType::Float64, false),
        Field::new("close", DataType::Float64, false),
        Field::new("volume", DataType::Int64, true),
        Field::new("vwap", DataType::Float64, true),
        Field::new("return_1d", DataType::Float64, true),
    ])
}

impl Dataset for Prices {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn num_rows(&self) -> u64 {
        (self.equities.len() * self.days.len()) as u64
    }

    fn batch(&self, rows: Range<u64>) -> RecordBatch {
        let per_ticker = self.days.len() as u64;
        let mut tickers = Vec::new();
        let mut dates = Vec::new();
        let mut bars = Vec::new();
        let mut row = rows.start;
        while row < rows.end {
            let ticker = (row / per_ticker) as usize;
            let path = self.path(ticker);
            let first_day = (row % per_ticker) as usize;
            let last_day = ((rows.end - (row - first_day as u64)).min(per_ticker)) as usize;
            for (day, bar) in path.iter().enumerate().take(last_day).skip(first_day) {
                tickers.push(
                    self.universe.securities[self.equities[ticker]]
                        .ticker
                        .as_str(),
                );
                dates.push(self.days[day]);
                bars.push(*bar);
                row += 1;
            }
        }
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(tickers)),
            Arc::new(Date32Array::from(dates)),
            Arc::new(Float64Array::from_iter_values(bars.iter().map(|b| b.open))),
            Arc::new(Float64Array::from_iter_values(bars.iter().map(|b| b.high))),
            Arc::new(Float64Array::from_iter_values(bars.iter().map(|b| b.low))),
            Arc::new(Float64Array::from_iter_values(bars.iter().map(|b| b.close))),
            Arc::new(bars.iter().map(|b| b.volume).collect::<Int64Array>()),
            Arc::new(bars.iter().map(|b| b.vwap).collect::<Float64Array>()),
            Arc::new(bars.iter().map(|b| b.return_1d).collect::<Float64Array>()),
        ];
        RecordBatch::try_new(Arc::clone(&self.schema), columns).expect("columns match schema")
    }
}
