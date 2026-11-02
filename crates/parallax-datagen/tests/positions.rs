//! Positions: reproducibility, financial identities and hostile coverage.

mod common;

use std::collections::HashSet;

use common::{decimals, f64s, hash, i32s, i64s, strings};
use jiff::civil::date;
use jiff::tz::TimeZone;
use parallax_datagen::calendar::{epoch_days, from_epoch_days, is_trading_day};
use parallax_datagen::decimal::mul_round;
use parallax_datagen::positions::{DST_INSTANTS_MICROS, Positions, PositionsOptions, schema};
use parallax_datagen::universe::COUNTRIES;
use parallax_datagen::{Dataset, OptionsError};

fn positions(rows: u64, hostile: bool) -> Positions {
    Positions::new(PositionsOptions {
        rows,
        securities: 10_000,
        hostile,
        ..PositionsOptions::default()
    })
    .unwrap()
}

#[test]
fn schema_has_forty_columns() {
    let batch = positions(10, false).batch(0..10);
    assert_eq!(schema().fields().len(), 40);
    assert_eq!(batch.schema().as_ref(), &schema());
}

#[test]
fn same_options_give_same_rows_and_seeds_differ() {
    let a = positions(5_000, true).batch(0..5_000);
    let b = positions(5_000, true).batch(0..5_000);
    assert_eq!(hash(&a), hash(&b));
    let other_seed = Positions::new(PositionsOptions {
        seed: 43,
        rows: 5_000,
        ..PositionsOptions::default()
    })
    .unwrap();
    assert_ne!(hash(&a), hash(&other_seed.batch(0..5_000)));
}

#[test]
fn any_range_equals_the_same_rows_of_a_full_run() {
    let dataset = positions(3_000, true);
    let full = dataset.batch(0..3_000);
    let middle = dataset.batch(1_234..2_345);
    assert_eq!(hash(&full.slice(1_234, 1_111)), hash(&middle));
}

#[test]
fn golden_rows_are_stable() {
    // These hashes lock the generator's output. A change here means every stored
    // benchmark and test expectation built from generated data is invalid.
    assert_eq!(
        (
            hash(&positions(1_000, false).batch(0..1_000)),
            hash(&positions(1_000, true).batch(0..1_000))
        ),
        (GOLDEN_CALM, GOLDEN_HOSTILE)
    );
}

const GOLDEN_CALM: u64 = 8_110_764_695_908_192_369;
const GOLDEN_HOSTILE: u64 = 9_263_514_958_236_493_211;

/// The decimal identities hold in every row, hostile or not, because hostile
/// quantities are re-derived rather than patched.
fn assert_identities(rows: u64, hostile: bool) {
    let dataset = positions(rows, hostile);
    let batch = dataset.batch(0..rows);
    let quantity = decimals(&batch, "quantity");
    let price = decimals(&batch, "price_local");
    let multiplier = decimals(&batch, "contract_multiplier");
    let fx = decimals(&batch, "fx_rate_to_usd");
    let mv_local = decimals(&batch, "market_value_local");
    let mv_usd = decimals(&batch, "market_value_usd");
    let cost = decimals(&batch, "cost_basis_usd");
    let pnl = decimals(&batch, "unrealized_pnl_usd");
    let long_short = strings(&batch, "long_short");
    for i in 0..rows as usize {
        assert_eq!(
            mv_local[i],
            mul_round(&[(quantity[i], 4), (price[i], 6), (multiplier[i], 2)], 2),
            "row {i}: market_value_local"
        );
        assert_eq!(
            mv_usd[i],
            mul_round(&[(mv_local[i], 2), (fx[i], 8)], 2),
            "row {i}: usd"
        );
        assert_eq!(pnl[i], mv_usd[i] - cost[i], "row {i}: unrealized P&L");
        let expected = if quantity[i] < 0 { "Short" } else { "Long" };
        assert_eq!(long_short[i], Some(expected), "row {i}: long_short");
        assert!(price[i] > 0, "row {i}: price");
    }
}

#[test]
fn decimal_identities_hold() {
    assert_identities(20_000, false);
}

#[test]
fn decimal_identities_hold_in_hostile_rows() {
    assert_identities(20_000, true);
}

#[test]
fn reference_data_is_consistent() {
    let options = PositionsOptions::default();
    let dataset = positions(20_000, false);
    let batch = dataset.batch(0..20_000);
    let ids = i64s(&batch, "position_id");
    assert!(
        ids.iter()
            .enumerate()
            .all(|(i, id)| *id == Some(i as i64 + 1))
    );

    let as_of = epoch_days(options.as_of);
    assert!(i32s(&batch, "as_of_date").iter().all(|d| *d == Some(as_of)));
    for open in i32s(&batch, "lot_open_date") {
        let open = open.expect("non-null");
        assert!(open <= as_of && is_trading_day(from_epoch_days(open)));
    }

    let new_york = TimeZone::get("America/New_York").unwrap();
    let close = options
        .as_of
        .at(16, 0, 0, 0)
        .to_zoned(new_york.clone())
        .unwrap();
    let evening = options.as_of.at(20, 0, 0, 0).to_zoned(new_york).unwrap();
    for updated in i64s(&batch, "last_updated") {
        let updated = updated.expect("non-null");
        assert!(updated >= close.timestamp().as_microsecond());
        assert!(updated < evening.timestamp().as_microsecond());
    }

    let countries = strings(&batch, "country");
    let currencies = strings(&batch, "currency");
    for (country, currency) in countries.iter().zip(&currencies) {
        let row = COUNTRIES
            .iter()
            .find(|c| Some(c.0) == *country)
            .expect("known country");
        assert_eq!(Some(row.2), *currency);
    }

    let classes = strings(&batch, "asset_class");
    let sectors = strings(&batch, "sector");
    let maturities = i32s(&batch, "maturity_date");
    let ratings = strings(&batch, "credit_rating");
    let durations = f64s(&batch, "duration");
    for i in 0..batch.num_rows() {
        match classes[i] {
            Some("Equity") => assert!(maturities[i].is_none() && ratings[i].is_none()),
            Some("Bond") => {
                assert!(maturities[i].is_some() && ratings[i].is_some() && durations[i].is_some())
            }
            Some("Future") => assert!(sectors[i].is_none() && maturities[i].is_some()),
            Some("Option") => assert!(maturities[i].is_some()),
            other => panic!("unexpected asset class {other:?}"),
        }
    }
    let all_classes: HashSet<_> = classes.iter().flatten().collect();
    assert_eq!(all_classes.len(), 4);
}

#[test]
fn hostile_rows_cover_every_category() {
    let batch = positions(40_000, true).batch(0..40_000);
    let floats = [
        "weight_in_fund",
        "beta",
        "volatility_30d",
        "var_95_1d_usd",
        "days_to_liquidate",
        "delta_adjusted_exposure_usd",
    ];
    let values: Vec<f64> = floats
        .iter()
        .flat_map(|name| f64s(&batch, name))
        .flatten()
        .collect();
    assert!(values.iter().any(|v| v.is_nan()), "NaN");
    assert!(values.contains(&f64::INFINITY), "+inf");
    assert!(values.contains(&f64::NEG_INFINITY), "-inf");
    assert!(
        values.iter().any(|v| v.to_bits() == (-0.0f64).to_bits()),
        "negative zero"
    );
    assert!(
        f64s(&batch, "weight_in_fund").contains(&Some(1e-12)),
        "tiny weight"
    );

    let classes = strings(&batch, "asset_class");
    let sectors = strings(&batch, "sector");
    assert!(
        classes
            .iter()
            .zip(&sectors)
            .any(|(c, s)| *c != Some("Future") && s.is_none()),
        "null sector on a security that has one"
    );
    assert!(
        f64s(&batch, "beta").iter().any(Option::is_none),
        "null beta"
    );

    let huge = decimals(&batch, "market_value_usd")
        .into_iter()
        .filter(|v| v.unsigned_abs() >= 10u128.pow(14 + 2))
        .count();
    assert!(huge > 0, "a market value of at least 10^14 USD");

    let updated = i64s(&batch, "last_updated");
    assert!(
        updated
            .iter()
            .flatten()
            .any(|t| DST_INSTANTS_MICROS.contains(t)),
        "DST instant"
    );

    let tickers = strings(&batch, "ticker");
    assert!(
        tickers.iter().flatten().any(|t| !t.is_ascii()),
        "non-ASCII ticker"
    );
    for hostile in ["NULL", "", "e\u{301}cole", "\u{e9}cole"] {
        assert!(tickers.contains(&Some(hostile)), "ticker {hostile:?}");
    }
}

#[test]
fn duplicate_keys_only_when_asked() {
    let with_duplicates = Positions::new(PositionsOptions {
        rows: 20_000,
        duplicate_key_rate: 0.01,
        ..PositionsOptions::default()
    })
    .unwrap();
    let ids: Vec<i64> = i64s(&with_duplicates.batch(0..20_000), "position_id")
        .into_iter()
        .flatten()
        .collect();
    let distinct: HashSet<i64> = ids.iter().copied().collect();
    let duplicates = ids.len() - distinct.len();
    assert!(
        (100..=300).contains(&duplicates),
        "{duplicates} duplicates at a 1% rate"
    );
    assert!(
        ids.iter().enumerate().all(|(i, id)| *id <= i as i64 + 1),
        "ids point backwards"
    );

    let calm = positions(20_000, false).batch(0..20_000);
    let distinct_calm: HashSet<_> = i64s(&calm, "position_id").into_iter().collect();
    assert_eq!(distinct_calm.len(), 20_000);
}

#[test]
fn rejects_options_that_cannot_produce_data() {
    let sunday = date(2026, 11, 1);
    let err = Positions::new(PositionsOptions {
        as_of: sunday,
        ..PositionsOptions::default()
    })
    .unwrap_err();
    assert_eq!(err, OptionsError::NotATradingDay(sunday));
    assert!(
        Positions::new(PositionsOptions {
            duplicate_key_rate: 1.5,
            ..PositionsOptions::default()
        })
        .is_err()
    );
    assert_eq!(
        Positions::new(PositionsOptions {
            securities: 0,
            ..PositionsOptions::default()
        })
        .unwrap_err(),
        OptionsError::EmptyUniverse
    );
}

#[test]
fn contracts_held_have_not_expired() {
    let options = PositionsOptions::default();
    let batch = positions(20_000, false).batch(0..20_000);
    let as_of = epoch_days(options.as_of);
    for maturity in i32s(&batch, "maturity_date").into_iter().flatten() {
        assert!(
            maturity > as_of,
            "maturity {} is not after the as-of date",
            from_epoch_days(maturity)
        );
    }
}
