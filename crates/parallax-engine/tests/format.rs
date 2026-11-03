//! The formatter against independent references: exact big-integer arithmetic for
//! numbers, and DuckDB's ICU time zones for instants.

#![allow(missing_docs)]

use duckdb::Connection;
use num_bigint::BigUint;
use parallax_engine::format::{Format, Formatter, TimePrecision};
use proptest::prelude::*;

fn decimals(d: u8) -> Formatter {
    Formatter::new(&Format {
        decimals: Some(d),
        ..Format::default()
    })
    .unwrap()
}

fn pow10(n: u32) -> BigUint {
    BigUint::from(10u32).pow(n)
}

/// Divides and rounds half away from zero.
fn div_round(num: &BigUint, den: &BigUint) -> BigUint {
    let q = num / den;
    let r = num - &q * den;
    if r * 2u32 >= *den { q + 1u32 } else { q }
}

/// Excel's display of `value` at `d` decimals, computed with exact rationals:
/// round to 15 significant digits, then to `d` places, both half away from zero.
fn reference(value: f64, d: u32) -> String {
    let bits = value.abs().to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, exp2) = if exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), exponent - 1075)
    };
    let (num, den) = if exp2 >= 0 {
        (
            BigUint::from(mantissa) << exp2 as usize,
            BigUint::from(1u32),
        )
    } else {
        (
            BigUint::from(mantissa),
            BigUint::from(1u32) << (-exp2) as usize,
        )
    };
    let integer = if mantissa == 0 {
        BigUint::from(0u32)
    } else {
        // p = floor(log10(value)), found by comparing with powers of ten.
        let mut p: i32 = (value.abs().log10().floor()) as i32;
        let at_least = |p: i32| -> bool {
            if p >= 0 {
                num >= &den * pow10(p as u32)
            } else {
                &num * pow10((-p) as u32) >= den
            }
        };
        while !at_least(p) {
            p -= 1;
        }
        while at_least(p + 1) {
            p += 1;
        }
        // q = round(value × 10^(14 − p)), fifteen significant digits.
        let shift = 14 - p;
        let q = if shift >= 0 {
            div_round(&(&num * pow10(shift as u32)), &den)
        } else {
            div_round(&num, &(&den * pow10((-shift) as u32)))
        };
        // The shown integer is q × 10^(d − shift), rounded.
        let e = d as i32 - shift;
        if e >= 0 {
            q * pow10(e as u32)
        } else {
            div_round(&q, &pow10((-e) as u32))
        }
    };
    let digits = format!("{integer:0>width$}", width = d as usize + 1);
    let (whole, frac) = digits.split_at(digits.len() - d as usize);
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if d > 0 {
        grouped.push('.');
        grouped.push_str(frac);
    }
    let negative = value < 0.0 && integer != BigUint::from(0u32);
    if negative {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// Doubles near decimal midpoints, where rounding mistakes show.
fn near_ties() -> impl Strategy<Value = f64> {
    (-1_000_000_000i64..1_000_000_000, 0u32..9, -2i64..=2).prop_map(|(n, scale, nudge)| {
        let v = (n * 10 + 5) as f64 / 10f64.powi(scale as i32 + 1);
        let bits = v.to_bits() as i64 + nudge;
        f64::from_bits(bits as u64)
    })
}

fn finite() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite", |v| v.is_finite())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20_000))]

    #[test]
    fn any_double_matches_exact_arithmetic(v in finite(), d in 0u8..=20) {
        prop_assert_eq!(decimals(d).float(v), reference(v, u32::from(d)));
    }

    #[test]
    fn doubles_near_midpoints_match_exact_arithmetic(v in near_ties(), d in 0u8..=10) {
        prop_assert_eq!(decimals(d).float(v), reference(v, u32::from(d)));
    }

    #[test]
    fn decimals_round_exactly(unscaled in any::<i128>(), scale in 0i8..=38, d in 0u8..=30) {
        let magnitude = BigUint::from(unscaled.unsigned_abs());
        let e = i32::from(d) - i32::from(scale);
        let integer = if e >= 0 {
            magnitude * pow10(e as u32)
        } else {
            div_round(&magnitude, &pow10((-e) as u32))
        };
        let text = decimals(d).decimal(unscaled, scale);
        let digits: String = text.chars().filter(char::is_ascii_digit).collect();
        let expected = integer.to_string();
        prop_assert_eq!(digits.trim_start_matches('0'), expected.trim_start_matches('0'));
        let zero = integer == BigUint::from(0u32);
        prop_assert_eq!(text.starts_with('-'), unscaled < 0 && !zero);
        prop_assert_eq!(text.find('.').map(|i| text.len() - i - 1), (d > 0).then_some(usize::from(d)));
    }
}

#[test]
fn the_reference_agrees_with_known_excel_results() {
    for (v, d, text) in [
        (2.675, 2, "2.68"),
        (1.005, 2, "1.01"),
        (0.125, 2, "0.13"),
        (-0.001, 2, "0.00"),
        (
            12_345_678_901_234_567_890.0,
            0,
            "12,345,678,901,234,600,000",
        ),
        (5e-324, 2, "0.00"),
    ] {
        assert_eq!(reference(v, d), text, "reference for {v}");
        assert_eq!(decimals(d as u8).float(v), text, "formatter for {v}");
    }
    // The largest double: fifteen digits, then 294 zeros.
    let max = format!("179,769,313,486,232{}", ",000".repeat(98));
    assert_eq!(reference(f64::MAX, 0), max);
    assert_eq!(decimals(0).float(f64::MAX), max);
}

/// Instants around New York's 2026 daylight-saving changes and across a range of
/// years, compared with DuckDB's ICU time zones.
#[test]
fn instants_match_duckdb_time_zones() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("SET TimeZone = 'UTC';").unwrap();
    let mut micros: Vec<i64> = Vec::new();
    // Every 15 minutes across both 2026 transitions, with odd microseconds.
    for start in [1_772_949_600_000_000i64, 1_793_505_600_000_000] {
        for step in 0..48 {
            micros.push(start + step * 900_000_000 + 123_457);
        }
    }
    // A day a year, at varying times, from 1950 to 2049.
    let mut t = -631_152_000_000_000i64;
    while t < 2_524_608_000_000_000 {
        micros.push(t);
        t += 31_557_600_000_000 + 3_601_000_001;
    }
    let ny = Formatter::default();
    let tokyo = Formatter::new(&Format {
        zone: Some("Asia/Tokyo".into()),
        precision: TimePrecision::Microseconds,
        ..Format::default()
    })
    .unwrap();
    for &us in &micros {
        let (expected_ny, expected_tokyo): (String, String) = db
            .query_row(
                "SELECT strftime(timezone('America/New_York', make_timestamp(?::BIGINT)::TIMESTAMPTZ), '%Y-%m-%d %H:%M:%S'),
                        strftime(timezone('Asia/Tokyo', make_timestamp(?::BIGINT)::TIMESTAMPTZ), '%Y-%m-%d %H:%M:%S.%f')",
                [us, us],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(ny.timestamp(us, true), expected_ny, "New York at {us}");
        assert_eq!(tokyo.timestamp(us, true), expected_tokyo, "Tokyo at {us}");
    }
}
