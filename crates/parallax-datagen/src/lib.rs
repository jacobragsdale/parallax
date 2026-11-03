//! Reproducible synthetic finance data for parallax tests, benchmarks and demos.
//!
//! Every value is a pure function of the seed, the dataset options and the row index.
//! Any range of rows can therefore be generated on its own and equals the same rows
//! of a full run, and the output never depends on thread count or batch size.
//!
//! Three datasets share one [`universe::Universe`] per seed:
//!
//! - [`positions::Positions`]: position lots on an as-of date, 40 columns.
//! - [`trades::Trades`]: executions in trade-date order, 25 columns.
//! - [`prices::Prices`]: daily bars for equities, 9 columns.
//!
//! Each can inject hostile values: nulls, NaN, infinities, negative zero, extreme
//! magnitudes, Unicode and blank tickers, duplicate keys, and wall-clock times that
//! daylight-saving changes make ambiguous or impossible.

/// NYSE trading days, from the engine, which also places charts in trading time.
pub use parallax_engine::calendar;
pub mod decimal;
pub mod dist;
pub mod error;
pub mod positions;
pub mod prices;
pub mod rng;
mod shared;
pub mod trades;
pub mod universe;
pub mod write;

pub use error::OptionsError;
pub use write::{Dataset, WriteSummary, batch_ranges, write_ipc_file};
