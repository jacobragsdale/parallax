//! Built-in sources: Arrow IPC files and the synthetic data generator.
//!
//! Database, Parquet, MongoDB and HTTP connectors arrive in later milestone 1 steps.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;

use parallax_datagen::Dataset as Generated;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_datagen::prices::{Prices, PricesOptions};
use parallax_datagen::trades::{Trades, TradesOptions};
use serde::Deserialize;

use crate::registry::{Fetched, Source, now_micros};

/// Reads every batch of an Arrow IPC file.
#[derive(Debug, Clone)]
pub struct ArrowFile {
    /// The file.
    pub path: PathBuf,
}

impl Source for ArrowFile {
    fn fetch(&self, _: &BTreeMap<String, String>) -> Result<Fetched, String> {
        let file = File::open(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))?;
        let reader =
            arrow_ipc::reader::FileReader::try_new(file, None).map_err(|e| e.to_string())?;
        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let modified = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| jiff::Timestamp::try_from(t).ok())
            .map_or_else(now_micros, |t| t.as_microsecond());
        Ok(Fetched {
            schema,
            batches,
            as_of_micros: modified,
        })
    }
}

/// Which generated dataset a [`Generator`] produces.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedKind {
    /// Position lots.
    Positions,
    /// Executions.
    Trades,
    /// Daily bars.
    Prices,
}

/// Synthetic data from `parallax-datagen`, for demos and tests.
///
/// Each fetch can advance the seed, so that a reload produces different values and
/// views visibly refresh: the parameter `seed` overrides it, otherwise the
/// generator's `seed` plus the number of fetches so far is used when `drift` is
/// set.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    /// The dataset. In configuration files this is `dataset`, because `kind`
    /// names the source type.
    #[serde(rename = "dataset")]
    pub kind: GeneratedKind,
    /// Rows for positions and trades.
    #[serde(default = "default_rows")]
    pub rows: u64,
    /// Equities for prices.
    #[serde(default = "default_tickers")]
    pub tickers: usize,
    /// Securities in the universe.
    #[serde(default = "default_securities")]
    pub securities: usize,
    /// Base seed.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Inject hostile values.
    #[serde(default)]
    pub hostile: bool,
    /// Change the data on every fetch.
    #[serde(default)]
    pub drift: bool,
    #[serde(skip)]
    fetches: std::sync::atomic::AtomicU64,
}

fn default_rows() -> u64 {
    100_000
}
fn default_tickers() -> usize {
    100
}
fn default_securities() -> usize {
    10_000
}
fn default_seed() -> u64 {
    42
}

impl Generator {
    /// A generator of `rows` rows of `kind`.
    #[must_use]
    pub fn new(kind: GeneratedKind, rows: u64) -> Self {
        Self {
            kind,
            rows,
            tickers: default_tickers(),
            securities: default_securities(),
            seed: default_seed(),
            hostile: false,
            drift: false,
            fetches: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Makes every fetch produce different values.
    #[must_use]
    pub fn drifting(mut self) -> Self {
        self.drift = true;
        self
    }
}

impl Source for Generator {
    fn fetch(&self, parameters: &BTreeMap<String, String>) -> Result<Fetched, String> {
        let fetch = self
            .fetches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let seed = match parameters.get("seed") {
            Some(s) => s
                .parse()
                .map_err(|_| format!("seed {s:?} is not a number"))?,
            None if self.drift => self.seed + fetch,
            None => self.seed,
        };
        let batch_rows = 65_536;
        let dataset: Box<dyn Generated> = match self.kind {
            GeneratedKind::Positions => Box::new(
                Positions::new(PositionsOptions {
                    seed,
                    rows: self.rows,
                    securities: self.securities,
                    hostile: self.hostile,
                    ..PositionsOptions::default()
                })
                .map_err(|e| e.to_string())?,
            ),
            GeneratedKind::Trades => Box::new(
                Trades::new(TradesOptions {
                    seed,
                    rows: self.rows,
                    securities: self.securities,
                    hostile: self.hostile,
                    ..TradesOptions::default()
                })
                .map_err(|e| e.to_string())?,
            ),
            GeneratedKind::Prices => Box::new(
                Prices::new(PricesOptions {
                    seed,
                    tickers: self.tickers,
                    securities: self.securities,
                    hostile: self.hostile,
                    ..PricesOptions::default()
                })
                .map_err(|e| e.to_string())?,
            ),
        };
        let batches = parallax_datagen::batch_ranges(dataset.num_rows(), batch_rows)
            .into_iter()
            .map(|range| dataset.batch(range))
            .collect();
        Ok(Fetched {
            schema: dataset.schema(),
            batches,
            as_of_micros: now_micros(),
        })
    }
}
