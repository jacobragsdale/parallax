//! Every indicator against TA-Lib, over the bars in `fixtures/talib.json`, which
//! `fixtures/generate_talib.py` writes: three instruments, around 100, 12,000 and
//! a twentieth of a cent, with fat-tailed returns.

use parallax_engine::indicator;
use serde_json::Value;

struct Case {
    name: String,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Vec<f64>,
    volume: Vec<f64>,
    results: Vec<Value>,
}

fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap_or(f64::NAN))
        .collect()
}

fn cases() -> Vec<Case> {
    let text = include_str!("fixtures/talib.json");
    let document: Value = serde_json::from_str(text).unwrap();
    assert_eq!(document["talib"], "0.8.3");
    document["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Case {
            name: c["name"].as_str().unwrap().to_owned(),
            high: numbers(&c["high"]),
            low: numbers(&c["low"]),
            close: numbers(&c["close"]),
            volume: numbers(&c["volume"]),
            results: c["results"].as_array().unwrap().clone(),
        })
        .collect()
}

fn param(result: &Value, name: &str) -> usize {
    usize::try_from(result["params"][name].as_u64().unwrap()).unwrap()
}

/// Ours and TA-Lib's agree where TA-Lib has a value, to within `tolerance`
/// times `scale`, and have no value at the same rows.
fn agree(context: &str, ours: &[f64], theirs: &[f64], scale: f64, tolerance: f64) -> f64 {
    assert_eq!(ours.len(), theirs.len(), "{context}: lengths");
    let mut worst = 0.0f64;
    for (i, (&a, &b)) in ours.iter().zip(theirs).enumerate() {
        assert_eq!(
            a.is_nan(),
            b.is_nan(),
            "{context}: row {i}, ours {a}, TA-Lib {b}"
        );
        if !b.is_nan() {
            let error = (a - b).abs() / scale;
            assert!(
                error <= tolerance,
                "{context}: row {i}, ours {a}, TA-Lib {b}"
            );
            worst = worst.max(error);
        }
    }
    worst
}

#[test]
fn every_indicator_matches_ta_lib() {
    let flat = cases()
        .into_iter()
        .find(|c| c.name == "flat-stretch")
        .unwrap();
    let rsi = flat
        .results
        .iter()
        .find(|r| r["function"] == "rsi" && r["params"]["period"] == 14)
        .unwrap();
    // With no movement at all yet, TA-Lib's RSI reads 50, as ours does.
    assert_eq!(numbers(&rsi["outputs"]["real"])[14], 50.0);
    let mut checked = 0;
    for case in cases() {
        // Prices are compared relative to the instrument's price; oscillators,
        // which run from 0 to 100, relative to 100.
        let price = case.close.iter().fold(0.0f64, |m, &c| m.max(c.abs()));
        for result in &case.results {
            let function = result["function"].as_str().unwrap();
            let context = format!("{} {function} {}", case.name, result["params"]);
            let expected = |name: &str| numbers(&result["outputs"][name]);
            let c = &case.close;
            match function {
                "sma" => {
                    agree(
                        &context,
                        &indicator::sma(c, param(result, "period")),
                        &expected("real"),
                        price,
                        1e-12,
                    );
                }
                "ema" => {
                    agree(
                        &context,
                        &indicator::ema(c, param(result, "period")),
                        &expected("real"),
                        price,
                        1e-12,
                    );
                }
                "wma" => {
                    agree(
                        &context,
                        &indicator::wma(c, param(result, "period")),
                        &expected("real"),
                        price,
                        1e-12,
                    );
                }
                "bollinger" => {
                    let deviations = result["params"]["deviations"].as_f64().unwrap();
                    let bands = indicator::bollinger(c, param(result, "period"), deviations);
                    agree(&context, &bands.upper, &expected("upper"), price, 1e-9);
                    agree(&context, &bands.middle, &expected("middle"), price, 1e-12);
                    agree(&context, &bands.lower, &expected("lower"), price, 1e-9);
                }
                "rsi" => {
                    agree(
                        &context,
                        &indicator::rsi(c, param(result, "period")),
                        &expected("real"),
                        100.0,
                        1e-12,
                    );
                }
                "macd" => {
                    let m = indicator::macd(
                        c,
                        param(result, "fast"),
                        param(result, "slow"),
                        param(result, "signal"),
                    );
                    agree(&context, &m.macd, &expected("macd"), price, 1e-12);
                    agree(&context, &m.signal, &expected("signal"), price, 1e-12);
                    agree(&context, &m.histogram, &expected("histogram"), price, 1e-12);
                }
                "atr" => {
                    let atr = indicator::atr(&case.high, &case.low, c, param(result, "period"));
                    agree(&context, &atr, &expected("real"), price, 1e-12);
                }
                "stochastic" => {
                    let s = indicator::stochastic(
                        &case.high,
                        &case.low,
                        c,
                        param(result, "k"),
                        param(result, "k_smooth"),
                        param(result, "d"),
                    );
                    agree(&context, &s.k, &expected("k"), 100.0, 1e-12);
                    agree(&context, &s.d, &expected("d"), 100.0, 1e-12);
                }
                "vwap" => {
                    let session = param(result, "session");
                    let starts: Vec<usize> = if session == 0 {
                        vec![0]
                    } else {
                        (0..c.len()).step_by(session).collect()
                    };
                    let vwap = indicator::vwap(&case.high, &case.low, c, &case.volume, &starts);
                    agree(&context, &vwap, &expected("real"), price, 1e-12);
                }
                other => panic!("unknown function {other}"),
            }
            checked += 1;
        }
    }
    // Three instruments with every indicator, and the flat stretch without the
    // stochastic.
    assert_eq!(checked, 3 * 24 + 22, "every fixture compared");
}
