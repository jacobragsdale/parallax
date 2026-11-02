//! Command-line entry point: writes a generated dataset to an Arrow IPC file.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};
use jiff::civil::Date;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_datagen::prices::{Prices, PricesOptions};
use parallax_datagen::trades::{Trades, TradesOptions};
use parallax_datagen::{Dataset, write_ipc_file};

#[derive(Parser)]
#[command(
    version,
    about = "Write reproducible synthetic finance data as Arrow IPC files"
)]
struct Cli {
    #[command(subcommand)]
    dataset: Command,
}

#[derive(Args)]
struct Common {
    /// Output file.
    #[arg(long)]
    out: PathBuf,
    /// Seed. The same seed gives the same securities in every dataset.
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Securities in the universe.
    #[arg(long, default_value_t = 10_000)]
    securities: usize,
    /// Inject hostile values into about 2% of rows.
    #[arg(long)]
    hostile: bool,
    /// Rows per record batch. Does not change the values.
    #[arg(long, default_value_t = 65_536)]
    batch_rows: u64,
}

#[derive(Subcommand)]
enum Command {
    /// Position lots on one as-of date.
    Positions {
        #[command(flatten)]
        common: Common,
        /// Number of lots.
        #[arg(long, default_value_t = 5_000_000)]
        rows: u64,
        /// As-of date, which must be a NYSE trading day.
        #[arg(long, default_value = "2026-10-30")]
        as_of: Date,
        /// Fraction of rows that reuse an earlier position_id.
        #[arg(long, default_value_t = 0.0)]
        duplicate_key_rate: f64,
    },
    /// Executions in trade-date order.
    Trades {
        #[command(flatten)]
        common: Common,
        /// Number of trades.
        #[arg(long, default_value_t = 20_000_000)]
        rows: u64,
        /// First trade date.
        #[arg(long, default_value = "2024-11-01")]
        start: Date,
        /// Last trade date.
        #[arg(long, default_value = "2026-10-30")]
        end: Date,
        /// Fraction of rows that reuse an earlier trade_id.
        #[arg(long, default_value_t = 0.0)]
        duplicate_key_rate: f64,
    },
    /// Daily price bars for the most-held equities.
    Prices {
        #[command(flatten)]
        common: Common,
        /// Number of equities.
        #[arg(long, default_value_t = 500)]
        tickers: usize,
        /// First date.
        #[arg(long, default_value = "2016-01-04")]
        start: Date,
        /// Last date.
        #[arg(long, default_value = "2026-10-30")]
        end: Date,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (dataset, common): (Box<dyn Dataset>, Common) = match cli.dataset {
        Command::Positions {
            common,
            rows,
            as_of,
            duplicate_key_rate,
        } => (
            Box::new(Positions::new(PositionsOptions {
                seed: common.seed,
                rows,
                as_of,
                securities: common.securities,
                hostile: common.hostile,
                duplicate_key_rate,
            })?),
            common,
        ),
        Command::Trades {
            common,
            rows,
            start,
            end,
            duplicate_key_rate,
        } => (
            Box::new(Trades::new(TradesOptions {
                seed: common.seed,
                rows,
                start,
                end,
                securities: common.securities,
                hostile: common.hostile,
                duplicate_key_rate,
            })?),
            common,
        ),
        Command::Prices {
            common,
            tickers,
            start,
            end,
        } => (
            Box::new(Prices::new(PricesOptions {
                seed: common.seed,
                tickers,
                start,
                end,
                securities: common.securities,
                hostile: common.hostile,
            })?),
            common,
        ),
    };
    let started = Instant::now();
    let summary = write_ipc_file(dataset.as_ref(), &common.out, common.batch_rows)?;
    println!(
        "wrote {} rows in {} batches to {} ({:.1} MB) in {:.1} s",
        summary.rows,
        summary.batches,
        common.out.display(),
        summary.bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
