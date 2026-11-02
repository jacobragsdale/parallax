//! Position lots: one row per lot held on the as-of date.

use std::ops::Range;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Date32Array, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use jiff::civil::{Date, date};
use jiff::tz::TimeZone;

use crate::calendar::{epoch_days, is_trading_day, trading_days};
use crate::decimal::{from_f64, mul_round, to_f64};
use crate::dist::{log_normal, standard_normal, student_t_unit_variance};
use crate::error::{OptionsError, check_rate};
use crate::rng::{Rng, Stream};
use crate::shared::{decimal_array, duplicate_of, extreme_quantity, quote};
use crate::universe::{AssetClass, Universe};
use crate::write::Dataset;

/// Probability that a row in a hostile dataset carries a hostile value.
pub const HOSTILE_RATE: f64 = 0.02;

/// The z-score of a one-sided 95% value at risk.
const VAR_95_Z: f64 = 1.644_853_626_951_472_2;

/// UTC instants on either side of the 2026 New York daylight-saving changes.
pub const DST_INSTANTS_MICROS: [i64; 4] = [
    1_772_953_199_999_999, // 2026-03-08T06:59:59.999999Z, the last instant of EST
    1_772_953_200_000_000, // 2026-03-08T07:00:00Z, the first instant of EDT
    1_793_512_799_999_999, // 2026-11-01T05:59:59.999999Z, the last instant of EDT
    1_793_512_800_000_000, // 2026-11-01T06:00:00Z, the first instant of EST
];

/// Options for [`Positions`].
#[derive(Clone, Debug)]
pub struct PositionsOptions {
    /// Seed shared with the universe.
    pub seed: u64,
    /// Number of lots.
    pub rows: u64,
    /// The as-of date. It must be a trading day.
    pub as_of: Date,
    /// Number of securities in the universe.
    pub securities: usize,
    /// Inject hostile values into about [`HOSTILE_RATE`] of rows.
    pub hostile: bool,
    /// Fraction of rows that reuse an earlier row's `position_id`.
    pub duplicate_key_rate: f64,
}

impl Default for PositionsOptions {
    fn default() -> Self {
        Self {
            seed: 42,
            rows: 100_000,
            as_of: date(2026, 10, 30),
            securities: 10_000,
            hostile: false,
            duplicate_key_rate: 0.0,
        }
    }
}

/// The positions dataset.
#[derive(Debug)]
pub struct Positions {
    options: PositionsOptions,
    universe: Universe,
    schema: SchemaRef,
    lot_open_days: Vec<i32>,
    new_york: TimeZone,
}

/// One generated lot before it is split into columns.
#[derive(Clone, Debug, Default)]
struct Lot {
    position_id: i64,
    book: usize,
    security: usize,
    sector: Option<&'static str>,
    industry: Option<&'static str>,
    short: bool,
    quantity: i128,
    price: i128,
    market_value_local: i128,
    market_value_usd: i128,
    cost_basis_usd: i128,
    unrealized_pnl_usd: i128,
    daily_pnl_usd: i128,
    notional_usd: i128,
    weight_in_fund: Option<f64>,
    beta: Option<f64>,
    delta: Option<f64>,
    delta_adjusted_exposure_usd: Option<f64>,
    volatility_30d: Option<f64>,
    var_95_1d_usd: Option<f64>,
    duration: Option<f64>,
    dv01_usd: Option<f64>,
    yield_to_maturity: Option<f64>,
    coupon_rate: Option<f64>,
    maturity_date: Option<i32>,
    credit_rating: Option<&'static str>,
    days_to_liquidate: Option<f64>,
    lot_open_date: i32,
    last_updated: i64,
}

impl Positions {
    /// Builds the dataset description. Rows are generated on demand.
    ///
    /// # Errors
    ///
    /// Returns an error if `as_of` is not a trading day, the universe is empty, or
    /// the duplicate-key rate is not a probability.
    pub fn new(options: PositionsOptions) -> Result<Self, OptionsError> {
        if !is_trading_day(options.as_of) {
            return Err(OptionsError::NotATradingDay(options.as_of));
        }
        check_rate("duplicate_key_rate", options.duplicate_key_rate)?;
        let universe = Universe::new(
            options.seed,
            options.securities,
            options.hostile,
            options.as_of,
        )?;
        let three_years_back = date(options.as_of.year() - 3, options.as_of.month(), 1);
        let lot_open_days = trading_days(three_years_back, options.as_of)
            .into_iter()
            .map(epoch_days)
            .collect();
        Ok(Self {
            options,
            universe,
            schema: Arc::new(schema()),
            lot_open_days,
            new_york: TimeZone::get("America/New_York").expect("bundled time-zone database"),
        })
    }

    /// The universe the rows refer to.
    #[must_use]
    pub fn universe(&self) -> &Universe {
        &self.universe
    }

    fn lot(&self, row: u64) -> Lot {
        let options = &self.options;
        let mut rng = Rng::for_key(options.seed, Stream::Positions, row);
        let security_index = self.universe.security_frequency.sample(&mut rng);
        let book = self.universe.book_frequency.sample(&mut rng);
        let security = &self.universe.securities[security_index];
        let short = rng.chance(match security.asset_class {
            AssetClass::Equity | AssetClass::Bond => 0.18,
            AssetClass::Future | AssetClass::Option => 0.4,
        });
        let units = match security.asset_class {
            AssetClass::Equity if rng.chance(0.1) => log_normal(&mut rng, libm::log(500.0), 1.0),
            AssetClass::Equity => log_normal(&mut rng, libm::log(5_000.0), 1.3)
                .round()
                .max(1.0),
            AssetClass::Bond => {
                1_000.0
                    * log_normal(&mut rng, libm::log(2_000.0), 1.0)
                        .round()
                        .max(1.0)
            }
            AssetClass::Future => log_normal(&mut rng, libm::log(20.0), 1.0).round().max(1.0),
            AssetClass::Option => log_normal(&mut rng, libm::log(50.0), 1.2).round().max(1.0),
        };
        let mut quantity = from_f64(units, 4);
        if short {
            quantity = -quantity;
        }
        let price = quote(
            security,
            security.base_price * (1.0 + 0.02 * standard_normal(&mut rng)),
        );
        let total_return = (0.04 + 0.3 * standard_normal(&mut rng)).clamp(-0.95, 5.0);
        let cost_factor = from_f64(1.0 / (1.0 + total_return), 8);
        let daily_sigma = security.volatility / libm::sqrt(252.0);
        let daily_return = from_f64(daily_sigma * student_t_unit_variance(&mut rng, 4), 8);
        let delta = match security.asset_class {
            AssetClass::Equity | AssetClass::Future => Some(1.0),
            AssetClass::Bond => None,
            AssetClass::Option => Some(libm::round(rng.uniform(-1.0, 1.0) * 1e4) / 1e4),
        };
        let volatility_30d = security.volatility * (1.0 + 0.05 * standard_normal(&mut rng));
        let lot_open_date = self.lot_open_days[rng.index(self.lot_open_days.len())];
        let seconds_after_close = rng.below(4 * 3_600);
        let micros = rng.below(1_000_000);
        let updated_local = options
            .as_of
            .at(16, 0, 0, 0)
            .checked_add(
                jiff::Span::new()
                    .seconds(seconds_after_close as i64)
                    .microseconds(micros as i64),
            )
            .expect("in range");
        let last_updated = updated_local
            .to_zoned(self.new_york.clone())
            .expect("after-close times are unambiguous")
            .timestamp()
            .as_microsecond();

        let mut lot = Lot {
            position_id: row as i64 + 1,
            book,
            security: security_index,
            sector: security.sector,
            industry: security.industry,
            short,
            quantity,
            price,
            beta: Some(security.beta),
            delta,
            volatility_30d: Some(volatility_30d),
            duration: security.bond.as_ref().map(|b| b.duration),
            yield_to_maturity: security.bond.as_ref().map(|b| b.yield_to_maturity),
            coupon_rate: security.bond.as_ref().map(|b| b.coupon_rate),
            maturity_date: security
                .bond
                .as_ref()
                .map(|b| b.maturity)
                .or(security.expiry)
                .map(epoch_days),
            credit_rating: security.bond.as_ref().map(|b| b.rating),
            lot_open_date,
            last_updated,
            ..Lot::default()
        };
        self.derive_values(&mut lot, cost_factor, daily_return);

        if options.hostile {
            self.inject_hostile(row, &mut lot, cost_factor, daily_return);
        }
        if let Some(earlier) = duplicate_of(options.seed, row, options.duplicate_key_rate) {
            lot.position_id = earlier as i64 + 1;
        }
        lot
    }

    /// Computes every value that depends on quantity and price, so that hostile
    /// quantities keep the same identities as ordinary rows.
    fn derive_values(&self, lot: &mut Lot, cost_factor: i128, daily_return: i128) {
        let security = &self.universe.securities[lot.security];
        lot.market_value_local = mul_round(
            &[(lot.quantity, 4), (lot.price, 6), (security.multiplier, 2)],
            2,
        );
        lot.market_value_usd = mul_round(
            &[(lot.market_value_local, 2), (security.fx_rate_to_usd, 8)],
            2,
        );
        lot.cost_basis_usd = mul_round(&[(lot.market_value_usd, 2), (cost_factor, 8)], 2);
        lot.unrealized_pnl_usd = lot.market_value_usd - lot.cost_basis_usd;
        lot.daily_pnl_usd = mul_round(&[(lot.market_value_usd, 2), (daily_return, 8)], 2);
        let leverage = if security.asset_class == AssetClass::Option {
            10
        } else {
            1
        };
        lot.notional_usd = lot.market_value_usd.abs() * leverage;
        let mv = to_f64(lot.market_value_usd, 2);
        let fund = self.universe.fund_of(lot.book);
        lot.weight_in_fund = Some(mv / fund.nav_usd);
        lot.delta_adjusted_exposure_usd = lot.delta.map(|d| d * mv * leverage as f64);
        lot.var_95_1d_usd = lot
            .volatility_30d
            .map(|v| VAR_95_Z * v / libm::sqrt(252.0) * mv.abs());
        lot.dv01_usd = lot.duration.map(|d| mv * d * 1e-4);
        let liquidity = match security.asset_class {
            AssetClass::Equity => 0.2,
            AssetClass::Bond => 0.1,
            AssetClass::Future | AssetClass::Option => 0.25,
        };
        lot.days_to_liquidate =
            Some(to_f64(lot.quantity.abs(), 4) / (liquidity * security.average_daily_volume));
    }

    fn inject_hostile(&self, row: u64, lot: &mut Lot, cost_factor: i128, daily_return: i128) {
        let mut rng = Rng::for_key(self.options.seed, Stream::PositionsHostile, row);
        if !rng.chance(HOSTILE_RATE) {
            return;
        }
        match rng.below(8) {
            0 => {
                lot.sector = None;
                lot.industry = None;
            }
            1 => *float_slot(lot, rng.index(6)) = Some(f64::NAN),
            2 => {
                let infinity = if rng.chance(0.5) {
                    f64::INFINITY
                } else {
                    f64::NEG_INFINITY
                };
                *float_slot(lot, rng.index(6)) = Some(infinity);
            }
            3 => *float_slot(lot, rng.index(6)) = Some(-0.0),
            4 => {
                // A position worth about 10^15 USD.
                let security = &self.universe.securities[lot.security];
                let sign = if lot.short { -1 } else { 1 };
                lot.quantity = sign * extreme_quantity(security, lot.price);
                self.derive_values(lot, cost_factor, daily_return);
            }
            5 => lot.weight_in_fund = Some(1e-12),
            6 => {
                lot.beta = None;
                lot.volatility_30d = None;
                lot.days_to_liquidate = None;
            }
            _ => lot.last_updated = DST_INSTANTS_MICROS[rng.index(DST_INSTANTS_MICROS.len())],
        }
    }
}

/// Returns one of the floating-point columns that hostile values may replace.
fn float_slot(lot: &mut Lot, index: usize) -> &mut Option<f64> {
    match index {
        0 => &mut lot.weight_in_fund,
        1 => &mut lot.beta,
        2 => &mut lot.volatility_30d,
        3 => &mut lot.var_95_1d_usd,
        4 => &mut lot.days_to_liquidate,
        _ => &mut lot.delta_adjusted_exposure_usd,
    }
}

/// The positions schema: 40 columns.
#[must_use]
pub fn schema() -> Schema {
    let money = DataType::Decimal128(22, 2);
    Schema::new(vec![
        Field::new("position_id", DataType::Int64, false),
        Field::new("as_of_date", DataType::Date32, false),
        Field::new("fund", DataType::Utf8, false),
        Field::new("strategy", DataType::Utf8, false),
        Field::new("portfolio_manager", DataType::Utf8, false),
        Field::new("book", DataType::Utf8, false),
        Field::new("asset_class", DataType::Utf8, false),
        Field::new("ticker", DataType::Utf8, false),
        Field::new("security_name", DataType::Utf8, false),
        Field::new("sector", DataType::Utf8, true),
        Field::new("industry", DataType::Utf8, true),
        Field::new("country", DataType::Utf8, false),
        Field::new("region", DataType::Utf8, false),
        Field::new("currency", DataType::Utf8, false),
        Field::new("long_short", DataType::Utf8, false),
        Field::new("quantity", DataType::Decimal128(20, 4), false),
        Field::new("price_local", DataType::Decimal128(18, 6), false),
        Field::new("contract_multiplier", DataType::Decimal128(10, 2), false),
        Field::new("fx_rate_to_usd", DataType::Decimal128(18, 8), false),
        Field::new("market_value_local", money.clone(), false),
        Field::new("market_value_usd", money.clone(), false),
        Field::new("cost_basis_usd", money.clone(), false),
        Field::new("unrealized_pnl_usd", money.clone(), false),
        Field::new("daily_pnl_usd", money, false),
        Field::new("notional_usd", DataType::Decimal128(24, 2), false),
        Field::new("weight_in_fund", DataType::Float64, true),
        Field::new("beta", DataType::Float64, true),
        Field::new("delta", DataType::Float64, true),
        Field::new("delta_adjusted_exposure_usd", DataType::Float64, true),
        Field::new("volatility_30d", DataType::Float64, true),
        Field::new("var_95_1d_usd", DataType::Float64, true),
        Field::new("duration", DataType::Float64, true),
        Field::new("dv01_usd", DataType::Float64, true),
        Field::new("yield_to_maturity", DataType::Float64, true),
        Field::new("coupon_rate", DataType::Float64, true),
        Field::new("maturity_date", DataType::Date32, true),
        Field::new("credit_rating", DataType::Utf8, true),
        Field::new("days_to_liquidate", DataType::Float64, true),
        Field::new("lot_open_date", DataType::Date32, false),
        Field::new(
            "last_updated",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
    ])
}

fn floats(values: impl Iterator<Item = Option<f64>>) -> ArrayRef {
    Arc::new(values.collect::<Float64Array>())
}

impl Dataset for Positions {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn num_rows(&self) -> u64 {
        self.options.rows
    }

    fn batch(&self, rows: Range<u64>) -> RecordBatch {
        let lots: Vec<Lot> = rows.map(|row| self.lot(row)).collect();
        let u = &self.universe;
        let sec = |l: &Lot| &u.securities[l.security];
        let book = |l: &Lot| &u.books[l.book];
        let strategy = |l: &Lot| &u.strategies[book(l).strategy];
        let as_of = epoch_days(self.options.as_of);
        let columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from_iter_values(
                lots.iter().map(|l| l.position_id),
            )),
            Arc::new(Date32Array::from_iter_values(lots.iter().map(|_| as_of))),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| u.fund_of(l.book).name),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| strategy(l).name),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| book(l).portfolio_manager.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| book(l).name.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| sec(l).asset_class.label()),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| sec(l).ticker.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| sec(l).name.as_str()),
            )),
            Arc::new(lots.iter().map(|l| l.sector).collect::<StringArray>()),
            Arc::new(lots.iter().map(|l| l.industry).collect::<StringArray>()),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| sec(l).country),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| sec(l).region),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| sec(l).currency),
            )),
            Arc::new(StringArray::from_iter_values(
                lots.iter().map(|l| if l.short { "Short" } else { "Long" }),
            )),
            decimal_array(lots.iter().map(|l| l.quantity), 20, 4),
            decimal_array(lots.iter().map(|l| l.price), 18, 6),
            decimal_array(lots.iter().map(|l| sec(l).multiplier), 10, 2),
            decimal_array(lots.iter().map(|l| sec(l).fx_rate_to_usd), 18, 8),
            decimal_array(lots.iter().map(|l| l.market_value_local), 22, 2),
            decimal_array(lots.iter().map(|l| l.market_value_usd), 22, 2),
            decimal_array(lots.iter().map(|l| l.cost_basis_usd), 22, 2),
            decimal_array(lots.iter().map(|l| l.unrealized_pnl_usd), 22, 2),
            decimal_array(lots.iter().map(|l| l.daily_pnl_usd), 22, 2),
            decimal_array(lots.iter().map(|l| l.notional_usd), 24, 2),
            floats(lots.iter().map(|l| l.weight_in_fund)),
            floats(lots.iter().map(|l| l.beta)),
            floats(lots.iter().map(|l| l.delta)),
            floats(lots.iter().map(|l| l.delta_adjusted_exposure_usd)),
            floats(lots.iter().map(|l| l.volatility_30d)),
            floats(lots.iter().map(|l| l.var_95_1d_usd)),
            floats(lots.iter().map(|l| l.duration)),
            floats(lots.iter().map(|l| l.dv01_usd)),
            floats(lots.iter().map(|l| l.yield_to_maturity)),
            floats(lots.iter().map(|l| l.coupon_rate)),
            Arc::new(
                lots.iter()
                    .map(|l| l.maturity_date)
                    .collect::<Date32Array>(),
            ),
            Arc::new(
                lots.iter()
                    .map(|l| l.credit_rating)
                    .collect::<StringArray>(),
            ),
            floats(lots.iter().map(|l| l.days_to_liquidate)),
            Arc::new(Date32Array::from_iter_values(
                lots.iter().map(|l| l.lot_open_date),
            )),
            Arc::new(
                TimestampMicrosecondArray::from_iter_values(lots.iter().map(|l| l.last_updated))
                    .with_timezone("UTC"),
            ),
        ];
        RecordBatch::try_new(Arc::clone(&self.schema), columns).expect("columns match schema")
    }
}
