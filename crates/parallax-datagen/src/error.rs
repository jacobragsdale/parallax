//! Errors for dataset options that cannot produce data.

use jiff::civil::Date;

/// Options that cannot produce a dataset.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum OptionsError {
    /// The universe would have no securities.
    #[error("the universe needs at least one security")]
    EmptyUniverse,
    /// The as-of date is a weekend, holiday or special closure.
    #[error("{0} is not a NYSE trading day")]
    NotATradingDay(Date),
    /// The date range contains no trading day.
    #[error("there are no NYSE trading days from {start} to {end}")]
    NoTradingDays {
        /// First date of the range.
        start: Date,
        /// Last date of the range.
        end: Date,
    },
    /// The universe has fewer equities than prices were asked for.
    #[error(
        "asked for prices of {wanted} equities, but {securities} securities include only {available}"
    )]
    NotEnoughEquities {
        /// Equities asked for.
        wanted: usize,
        /// Equities in the universe.
        available: usize,
        /// Securities in the universe.
        securities: usize,
    },
    /// A rate is outside `[0, 1]` or not a number.
    #[error("{name} must be between 0 and 1, not {value}")]
    InvalidRate {
        /// The option's name.
        name: &'static str,
        /// The rejected value.
        value: f64,
    },
}

/// Checks that `value` is a probability.
pub(crate) fn check_rate(name: &'static str, value: f64) -> Result<(), OptionsError> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(OptionsError::InvalidRate { name, value })
    }
}
