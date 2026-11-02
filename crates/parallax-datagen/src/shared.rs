//! Logic shared by the positions and trades generators.

use std::sync::Arc;

use arrow_array::{ArrayRef, Decimal128Array};

use crate::decimal::{from_f64, to_f64};
use crate::rng::{Rng, Stream};
use crate::universe::Security;

/// The largest unit count that fits a `Decimal128(20, 4)` quantity column with room
/// to spare.
const MAX_UNITS: f64 = 9.0e15;

/// Builds a decimal array of the given type.
pub(crate) fn decimal_array(
    values: impl Iterator<Item = i128>,
    precision: u8,
    scale: i8,
) -> ArrayRef {
    Arc::new(
        Decimal128Array::from_iter_values(values)
            .with_precision_and_scale(precision, scale)
            .expect("valid decimal type"),
    )
}

/// Quantizes a price to the security's quoting precision and stores it at scale 6.
pub(crate) fn quote(security: &Security, price: f64) -> i128 {
    from_f64(price.max(0.01), security.price_decimals) * 10i128.pow(6 - security.price_decimals)
}

/// Returns a quantity at scale 4, unsigned, whose value is about 10^15 USD at
/// `price` (scale 6), capped so that it fits the quantity column.
pub(crate) fn extreme_quantity(security: &Security, price: i128) -> i128 {
    let unit_value_usd =
        to_f64(price, 6) * to_f64(security.multiplier, 2) * to_f64(security.fx_rate_to_usd, 8);
    let units = (1e15 / unit_value_usd).min(MAX_UNITS).round();
    from_f64(units, 0) * 10_000
}

/// Returns the earlier row whose key `row` reuses, if it is chosen as a duplicate.
/// The earlier row is at most 1,000 rows back.
pub(crate) fn duplicate_of(seed: u64, row: u64, rate: f64) -> Option<u64> {
    if rate <= 0.0 || row == 0 {
        return None;
    }
    let mut keys = Rng::for_key(seed, Stream::Keys, row);
    keys.chance(rate)
        .then(|| row - 1 - keys.below(row.min(1_000)))
}
