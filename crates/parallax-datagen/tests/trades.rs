//! Trades: ordering, time zones, settlement and hostile coverage.

mod common;

use std::collections::HashSet;

use common::{decimals, hash, i32s, i64s, strings};
use jiff::Timestamp;
use jiff::tz::{AmbiguousOffset, TimeZone};
use parallax_datagen::Dataset;
use parallax_datagen::calendar::{epoch_days, from_epoch_days, is_trading_day, next_trading_day};
use parallax_datagen::decimal::mul_round;
use parallax_datagen::trades::{Trades, TradesOptions, schema};

fn trades(rows: u64, hostile: bool) -> Trades {
    Trades::new(TradesOptions {
        rows,
        hostile,
        ..TradesOptions::default()
    })
    .unwrap()
}

fn naive(micros: i64) -> jiff::civil::DateTime {
    Timestamp::from_microsecond(micros)
        .unwrap()
        .to_zoned(TimeZone::UTC)
        .datetime()
}

#[test]
fn schema_has_twenty_five_columns() {
    assert_eq!(schema().fields().len(), 25);
    assert_eq!(trades(5, false).batch(0..5).schema().as_ref(), &schema());
}

#[test]
fn any_range_equals_the_same_rows_of_a_full_run() {
    let dataset = trades(4_000, true);
    let full = dataset.batch(0..4_000);
    assert_eq!(
        hash(&full.slice(999, 2_001)),
        hash(&dataset.batch(999..3_000))
    );
    assert_eq!(hash(&full), hash(&trades(4_000, true).batch(0..4_000)));
}

#[test]
fn golden_rows_are_stable() {
    assert_eq!(
        (
            hash(&trades(1_000, false).batch(0..1_000)),
            hash(&trades(1_000, true).batch(0..1_000))
        ),
        (GOLDEN_CALM, GOLDEN_HOSTILE)
    );
}

const GOLDEN_CALM: u64 = 12_724_781_792_629_693_938;
const GOLDEN_HOSTILE: u64 = 4_905_705_083_864_376_887;

#[test]
fn trades_are_ordered_timed_and_settled_correctly() {
    let options = TradesOptions::default();
    let batch = trades(30_000, false).batch(0..30_000);
    let ids = i64s(&batch, "trade_id");
    assert!(
        ids.iter()
            .enumerate()
            .all(|(i, id)| *id == Some(i as i64 + 1))
    );

    let trade_dates: Vec<i32> = i32s(&batch, "trade_date").into_iter().flatten().collect();
    assert!(
        trade_dates.windows(2).all(|w| w[0] <= w[1]),
        "trade dates are sorted"
    );
    assert_eq!(trade_dates[0], epoch_days(next_or_same(options.start)));
    let settle: Vec<i32> = i32s(&batch, "settle_date").into_iter().flatten().collect();

    let new_york = TimeZone::get("America/New_York").unwrap();
    let utc = i64s(&batch, "trade_time_utc");
    let local = i64s(&batch, "trade_time_local");
    for i in 0..batch.num_rows() {
        let day = from_epoch_days(trade_dates[i]);
        assert!(is_trading_day(day) && day >= options.start && day <= options.end);
        assert_eq!(
            from_epoch_days(settle[i]),
            next_trading_day(day),
            "row {i}: T+1"
        );

        let instant = Timestamp::from_microsecond(utc[i].expect("calm rows have UTC")).unwrap();
        let wall = instant.to_zoned(new_york.clone()).datetime();
        assert_eq!(
            naive(local[i].unwrap()),
            wall,
            "row {i}: local is New York wall time"
        );
        assert_eq!(wall.date(), day, "row {i}: trade date is the New York date");
        let minutes = i32::from(wall.hour()) * 60 + i32::from(wall.minute());
        assert!((570..960).contains(&minutes), "row {i}: inside 09:30-16:00");
    }
}

fn next_or_same(day: jiff::civil::Date) -> jiff::civil::Date {
    if is_trading_day(day) {
        day
    } else {
        next_trading_day(day)
    }
}

#[test]
fn amounts_follow_quantity_price_and_side() {
    let batch = trades(30_000, true).batch(0..30_000);
    let quantity = decimals(&batch, "quantity");
    let price = decimals(&batch, "price_local");
    let multiplier = decimals(&batch, "contract_multiplier");
    let gross = decimals(&batch, "gross_amount_local");
    let commission = decimals(&batch, "commission_local");
    let net = decimals(&batch, "net_amount_local");
    let sides = strings(&batch, "side");
    for i in 0..batch.num_rows() {
        assert_eq!(
            gross[i],
            mul_round(&[(quantity[i], 4), (price[i], 6), (multiplier[i], 2)], 2),
            "row {i}: gross"
        );
        let expected = match sides[i] {
            Some("Buy" | "Cover") => gross[i] + commission[i],
            Some("Sell" | "Short") => gross[i] - commission[i],
            other => panic!("unexpected side {other:?}"),
        };
        assert_eq!(net[i], expected, "row {i}: net");
        assert!(
            quantity[i] >= 0,
            "row {i}: quantity is unsigned; side gives direction"
        );
    }
}

#[test]
fn hostile_rows_cover_every_category() {
    let batch = trades(40_000, true).batch(0..40_000);
    let new_york = TimeZone::get("America/New_York").unwrap();
    let utc = i64s(&batch, "trade_time_utc");
    let local = i64s(&batch, "trade_time_local");
    let mut gaps = 0;
    let mut folds = 0;
    for (u, l) in utc.iter().zip(&local) {
        if u.is_none() {
            match new_york.to_ambiguous_zoned(naive(l.unwrap())).offset() {
                AmbiguousOffset::Gap { .. } => gaps += 1,
                AmbiguousOffset::Fold { .. } => folds += 1,
                AmbiguousOffset::Unambiguous { .. } => panic!("null UTC with a valid local time"),
            }
        }
    }
    assert!(
        gaps > 0 && folds > 0,
        "{gaps} missing and {folds} ambiguous local times"
    );

    assert!(strings(&batch, "broker").contains(&None), "null broker");
    assert!(strings(&batch, "venue").contains(&None), "null venue");
    assert!(
        decimals(&batch, "commission_local").iter().any(|c| *c < 0),
        "rebate"
    );
    let quantities = decimals(&batch, "quantity");
    let statuses = strings(&batch, "status");
    assert!(
        quantities
            .iter()
            .zip(&statuses)
            .any(|(q, s)| *q == 0 && *s == Some("Cancelled")),
        "cancelled zero-quantity trade"
    );
    assert!(
        decimals(&batch, "gross_amount_local")
            .iter()
            .any(|g| g.unsigned_abs() >= 10u128.pow(14 + 2)),
        "gross amount of at least 10^14"
    );
}

#[test]
fn duplicate_keys_only_when_asked() {
    let dataset = Trades::new(TradesOptions {
        rows: 20_000,
        duplicate_key_rate: 0.01,
        ..TradesOptions::default()
    })
    .unwrap();
    let ids: Vec<i64> = i64s(&dataset.batch(0..20_000), "trade_id")
        .into_iter()
        .flatten()
        .collect();
    let distinct: HashSet<i64> = ids.iter().copied().collect();
    assert!((100..=300).contains(&(ids.len() - distinct.len())));
}

#[test]
fn rejects_a_range_without_trading_days() {
    let saturday = jiff::civil::date(2026, 10, 31);
    let err = Trades::new(TradesOptions {
        start: saturday,
        end: saturday,
        ..TradesOptions::default()
    })
    .unwrap_err();
    assert!(err.to_string().contains("no NYSE trading days"), "{err}");
}
