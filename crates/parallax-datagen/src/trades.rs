//! Trades: one row per execution, in trade-date order.

use std::ops::Range;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Date32Array, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use jiff::civil::{Date, DateTime, date};
use jiff::tz::TimeZone;

use crate::calendar::{epoch_days, next_trading_day, trading_days};
use crate::decimal::{from_f64, mul_round};
use crate::dist::{log_normal, standard_normal};
use crate::error::{OptionsError, check_rate};
use crate::positions::HOSTILE_RATE;
use crate::rng::{Rng, Stream};
use crate::shared::{decimal_array, duplicate_of, extreme_quantity, quote};
use crate::universe::{AssetClass, Universe};
use crate::write::Dataset;

const SIDES: [(&str, f64); 4] = [
    ("Buy", 45.0),
    ("Sell", 35.0),
    ("Short", 12.0),
    ("Cover", 8.0),
];
const BROKERS: [&str; 12] = [
    "Goldman Sachs",
    "Morgan Stanley",
    "J.P. Morgan",
    "Citigroup",
    "Bank of America",
    "Barclays",
    "UBS",
    "Deutsche Bank",
    "BNP Paribas",
    "Jefferies",
    "Nomura",
    "Virtu",
];
const VENUES: [&str; 11] = [
    "NYSE",
    "NASDAQ",
    "ARCA",
    "CBOE",
    "IEX",
    "LSE",
    "XETRA",
    "TSE",
    "HKEX",
    "ASX",
    "Dark Pool",
];
const ORDER_TYPES: [&str; 5] = ["Market", "Limit", "VWAP", "TWAP", "Close"];

/// A local time that occurs twice in New York: 2025-11-02 between 01:00 and 01:59.
const AMBIGUOUS_LOCAL: DateTime = date(2025, 11, 2).at(1, 0, 0, 0);
/// A local time that never occurs in New York: 2026-03-08 between 02:00 and 02:59.
const MISSING_LOCAL: DateTime = date(2026, 3, 8).at(2, 0, 0, 0);

/// Options for [`Trades`].
#[derive(Clone, Debug)]
pub struct TradesOptions {
    /// Seed shared with the universe.
    pub seed: u64,
    /// Number of trades.
    pub rows: u64,
    /// First trade date.
    pub start: Date,
    /// Last trade date.
    pub end: Date,
    /// Number of securities in the universe.
    pub securities: usize,
    /// Inject hostile values into about [`HOSTILE_RATE`] of rows.
    pub hostile: bool,
    /// Fraction of rows that reuse an earlier row's `trade_id`.
    pub duplicate_key_rate: f64,
}

impl Default for TradesOptions {
    fn default() -> Self {
        Self {
            seed: 42,
            rows: 100_000,
            start: date(2024, 11, 1),
            end: date(2026, 10, 30),
            securities: 10_000,
            hostile: false,
            duplicate_key_rate: 0.0,
        }
    }
}

/// The trades dataset.
#[derive(Debug)]
pub struct Trades {
    options: TradesOptions,
    universe: Universe,
    schema: SchemaRef,
    days: Vec<TradeDay>,
    side_frequency: crate::dist::Weighted,
    new_york: TimeZone,
}

/// A trading day with its settlement date, both precomputed as epoch days.
#[derive(Clone, Copy, Debug)]
struct TradeDay {
    date: Date,
    epoch_day: i32,
    settle_epoch_day: i32,
}

#[derive(Clone, Debug, Default)]
struct Fill {
    trade_id: i64,
    trade_date: i32,
    trade_time_utc: Option<i64>,
    trade_time_local: i64,
    settle_date: i32,
    book: usize,
    security: usize,
    trader: usize,
    side: &'static str,
    quantity: i128,
    price: i128,
    gross: i128,
    commission: i128,
    net: i128,
    broker: Option<&'static str>,
    venue: Option<&'static str>,
    order_type: &'static str,
    status: &'static str,
}

impl Trades {
    /// Builds the dataset description. Rows are generated on demand.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no trading day between `start` and `end`, the
    /// universe is empty, or the duplicate-key rate is not a probability.
    pub fn new(options: TradesOptions) -> Result<Self, OptionsError> {
        let days: Vec<TradeDay> = trading_days(options.start, options.end)
            .into_iter()
            .map(|date| TradeDay {
                date,
                epoch_day: epoch_days(date),
                settle_epoch_day: epoch_days(next_trading_day(date)),
            })
            .collect();
        if days.is_empty() {
            return Err(OptionsError::NoTradingDays {
                start: options.start,
                end: options.end,
            });
        }
        check_rate("duplicate_key_rate", options.duplicate_key_rate)?;
        Ok(Self {
            universe: Universe::new(
                options.seed,
                options.securities,
                options.hostile,
                options.end,
            )?,
            schema: Arc::new(schema()),
            days,
            side_frequency: crate::dist::Weighted::new(&SIDES.map(|s| s.1)),
            new_york: TimeZone::get("America/New_York").expect("bundled time-zone database"),
            options,
        })
    }

    /// The universe the rows refer to.
    #[must_use]
    pub fn universe(&self) -> &Universe {
        &self.universe
    }

    fn fill(&self, row: u64) -> Fill {
        let options = &self.options;
        let mut rng = Rng::for_key(options.seed, Stream::Trades, row);
        let day_index =
            (u128::from(row) * self.days.len() as u128 / u128::from(options.rows.max(1))) as usize;
        let day = self.days[day_index.min(self.days.len() - 1)];
        let security_index = self.universe.security_frequency.sample(&mut rng);
        let security = &self.universe.securities[security_index];
        let book = self.universe.book_frequency.sample(&mut rng);
        let side = SIDES[self.side_frequency.sample(&mut rng)].0;
        let units = match security.asset_class {
            AssetClass::Equity => log_normal(&mut rng, libm::log(800.0), 1.4).round().max(1.0),
            AssetClass::Bond => {
                1_000.0 * log_normal(&mut rng, libm::log(500.0), 1.0).round().max(1.0)
            }
            AssetClass::Future => log_normal(&mut rng, libm::log(5.0), 1.0).round().max(1.0),
            AssetClass::Option => log_normal(&mut rng, libm::log(20.0), 1.0).round().max(1.0),
        };
        let price = quote(
            security,
            security.base_price * (1.0 + 0.01 * standard_normal(&mut rng)),
        );
        let commission_rate = from_f64(rng.uniform(1.0, 5.0) / 10_000.0, 8);
        // Regular session: 09:30 to 16:00 New York time, at microsecond resolution.
        let session_micros = rng.below(6 * 3_600 * 1_000_000 + 30 * 60 * 1_000_000);
        let local = day
            .date
            .at(9, 30, 0, 0)
            .checked_add(jiff::Span::new().microseconds(session_micros as i64))
            .expect("in range");
        let trade_time_utc = local
            .to_zoned(self.new_york.clone())
            .expect("session times are unambiguous")
            .timestamp()
            .as_microsecond();
        let status = match rng.below(100) {
            0..92 => "Filled",
            92..97 => "PartiallyFilled",
            _ => "Cancelled",
        };
        let mut fill = Fill {
            trade_id: row as i64 + 1,
            trade_date: day.epoch_day,
            trade_time_utc: Some(trade_time_utc),
            trade_time_local: naive_micros(local),
            settle_date: day.settle_epoch_day,
            book,
            security: security_index,
            trader: rng.index(self.universe.traders.len()),
            side,
            quantity: from_f64(units, 4),
            price,
            broker: Some(BROKERS[rng.index(BROKERS.len())]),
            venue: Some(VENUES[rng.index(VENUES.len())]),
            order_type: ORDER_TYPES[rng.index(ORDER_TYPES.len())],
            status,
            ..Fill::default()
        };
        self.derive_amounts(&mut fill, commission_rate, false);

        if options.hostile {
            self.inject_hostile(row, &mut fill, commission_rate);
        }
        if let Some(earlier) = duplicate_of(options.seed, row, options.duplicate_key_rate) {
            fill.trade_id = earlier as i64 + 1;
        }
        fill
    }

    fn derive_amounts(&self, fill: &mut Fill, commission_rate: i128, rebate: bool) {
        let multiplier = self.universe.securities[fill.security].multiplier;
        fill.gross = mul_round(&[(fill.quantity, 4), (fill.price, 6), (multiplier, 2)], 2);
        fill.commission = mul_round(&[(fill.gross, 2), (commission_rate, 8)], 2);
        if rebate {
            fill.commission = -fill.commission;
        }
        fill.net = if matches!(fill.side, "Buy" | "Cover") {
            fill.gross + fill.commission
        } else {
            fill.gross - fill.commission
        };
    }

    fn inject_hostile(&self, row: u64, fill: &mut Fill, commission_rate: i128) {
        let mut rng = Rng::for_key(self.options.seed, Stream::TradesHostile, row);
        if !rng.chance(HOSTILE_RATE) {
            return;
        }
        match rng.below(7) {
            0 | 1 => {
                // A wall-clock time that is ambiguous or missing in New York, as a
                // zoneless SQL Server datetime2 column could hold. No UTC instant
                // exists for it without a policy, so the UTC column is null. Both
                // times fall on daylight-saving Sundays, so on these rows the local
                // time deliberately disagrees with trade_date.
                let base = if rng.below(2) == 0 {
                    AMBIGUOUS_LOCAL
                } else {
                    MISSING_LOCAL
                };
                let offset = jiff::Span::new().microseconds(rng.below(3_600 * 1_000_000) as i64);
                fill.trade_time_local = naive_micros(base.checked_add(offset).expect("in range"));
                fill.trade_time_utc = None;
            }
            2 => fill.broker = None,
            3 => fill.venue = None,
            4 => {
                fill.quantity = 0;
                fill.status = "Cancelled";
                self.derive_amounts(fill, commission_rate, false);
            }
            5 => self.derive_amounts(fill, commission_rate, true),
            _ => {
                let security = &self.universe.securities[fill.security];
                fill.quantity = extreme_quantity(security, fill.price);
                self.derive_amounts(fill, commission_rate, false);
            }
        }
    }
}

/// Returns a civil date-time as microseconds since the Unix epoch, as if it were UTC.
/// This is how a zoneless timestamp column stores a wall-clock time.
fn naive_micros(local: DateTime) -> i64 {
    local
        .to_zoned(TimeZone::UTC)
        .expect("UTC has no gaps")
        .timestamp()
        .as_microsecond()
}

/// The trades schema: 25 columns.
#[must_use]
pub fn schema() -> Schema {
    Schema::new(vec![
        Field::new("trade_id", DataType::Int64, false),
        Field::new("trade_date", DataType::Date32, false),
        Field::new(
            "trade_time_utc",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
        Field::new(
            "trade_time_local",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("settle_date", DataType::Date32, false),
        Field::new("fund", DataType::Utf8, false),
        Field::new("strategy", DataType::Utf8, false),
        Field::new("portfolio_manager", DataType::Utf8, false),
        Field::new("book", DataType::Utf8, false),
        Field::new("trader", DataType::Utf8, false),
        Field::new("ticker", DataType::Utf8, false),
        Field::new("asset_class", DataType::Utf8, false),
        Field::new("side", DataType::Utf8, false),
        Field::new("quantity", DataType::Decimal128(20, 4), false),
        Field::new("price_local", DataType::Decimal128(18, 6), false),
        Field::new("contract_multiplier", DataType::Decimal128(10, 2), false),
        Field::new("currency", DataType::Utf8, false),
        Field::new("fx_rate_to_usd", DataType::Decimal128(18, 8), false),
        Field::new("gross_amount_local", DataType::Decimal128(22, 2), false),
        Field::new("commission_local", DataType::Decimal128(18, 2), false),
        Field::new("net_amount_local", DataType::Decimal128(22, 2), false),
        Field::new("broker", DataType::Utf8, true),
        Field::new("venue", DataType::Utf8, true),
        Field::new("order_type", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
    ])
}

impl Dataset for Trades {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn num_rows(&self) -> u64 {
        self.options.rows
    }

    fn batch(&self, rows: Range<u64>) -> RecordBatch {
        let fills: Vec<Fill> = rows.map(|row| self.fill(row)).collect();
        let u = &self.universe;
        let sec = |f: &Fill| &u.securities[f.security];
        let book = |f: &Fill| &u.books[f.book];
        let strings = |values: Vec<&str>| -> ArrayRef { Arc::new(StringArray::from(values)) };
        let columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from_iter_values(
                fills.iter().map(|f| f.trade_id),
            )),
            Arc::new(Date32Array::from_iter_values(
                fills.iter().map(|f| f.trade_date),
            )),
            Arc::new(
                fills
                    .iter()
                    .map(|f| f.trade_time_utc)
                    .collect::<TimestampMicrosecondArray>()
                    .with_timezone("UTC"),
            ),
            Arc::new(TimestampMicrosecondArray::from_iter_values(
                fills.iter().map(|f| f.trade_time_local),
            )),
            Arc::new(Date32Array::from_iter_values(
                fills.iter().map(|f| f.settle_date),
            )),
            strings(fills.iter().map(|f| u.fund_of(f.book).name).collect()),
            strings(
                fills
                    .iter()
                    .map(|f| u.strategies[book(f).strategy].name)
                    .collect(),
            ),
            strings(
                fills
                    .iter()
                    .map(|f| book(f).portfolio_manager.as_str())
                    .collect(),
            ),
            strings(fills.iter().map(|f| book(f).name.as_str()).collect()),
            strings(fills.iter().map(|f| u.traders[f.trader].as_str()).collect()),
            strings(fills.iter().map(|f| sec(f).ticker.as_str()).collect()),
            strings(fills.iter().map(|f| sec(f).asset_class.label()).collect()),
            strings(fills.iter().map(|f| f.side).collect()),
            decimal_array(fills.iter().map(|f| f.quantity), 20, 4),
            decimal_array(fills.iter().map(|f| f.price), 18, 6),
            decimal_array(fills.iter().map(|f| sec(f).multiplier), 10, 2),
            strings(fills.iter().map(|f| sec(f).currency).collect()),
            decimal_array(fills.iter().map(|f| sec(f).fx_rate_to_usd), 18, 8),
            decimal_array(fills.iter().map(|f| f.gross), 22, 2),
            decimal_array(fills.iter().map(|f| f.commission), 18, 2),
            decimal_array(fills.iter().map(|f| f.net), 22, 2),
            Arc::new(fills.iter().map(|f| f.broker).collect::<StringArray>()),
            Arc::new(fills.iter().map(|f| f.venue).collect::<StringArray>()),
            strings(fills.iter().map(|f| f.order_type).collect()),
            strings(fills.iter().map(|f| f.status).collect()),
        ];
        RecordBatch::try_new(Arc::clone(&self.schema), columns).expect("columns match schema")
    }
}
