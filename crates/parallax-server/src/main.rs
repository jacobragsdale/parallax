//! The reference parallax server: datasets from a TOML file, the protocol on `/ws`,
//! and optionally a directory of static files such as the demo app.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use parallax_engine::DuplicatePolicy;
use parallax_server::sources::{ArrowFile, Generator};
use parallax_server::{Dataset, RefreshPolicy, Registry, Source, router, spawn_refresher};
use parallax_sources::http::{Http, HttpConfig};
use parallax_sources::parquet::{Parquet, ParquetConfig};
use serde::Deserialize;
use tower_http::services::ServeDir;

#[derive(Parser)]
#[command(version, about = "Serve parallax datasets over WebSocket")]
struct Cli {
    /// The configuration file.
    #[arg(long, default_value = "parallax.toml")]
    config: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    /// Address to listen on, such as `0.0.0.0:8790`.
    listen: SocketAddr,
    /// A directory served at `/`, such as the demo app's build output.
    #[serde(default)]
    static_dir: Option<PathBuf>,
    #[serde(default, rename = "dataset")]
    datasets: Vec<DatasetConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DatasetConfig {
    name: String,
    #[serde(default)]
    parameters: Vec<String>,
    #[serde(default)]
    primary_key: Option<String>,
    #[serde(default)]
    last_wins: bool,
    #[serde(default)]
    max_age_seconds: Option<u64>,
    #[serde(default)]
    refresh_seconds: Option<u64>,
    /// The most parameter combinations cached at once.
    #[serde(default = "default_max_tables")]
    max_tables: usize,
    source: SourceConfig,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SourceConfig {
    ArrowFile { path: PathBuf },
    Generator(Generator),
    Http(HttpConfig),
    Parquet(ParquetConfig),
}

fn default_max_tables() -> usize {
    32
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("reading {}", cli.config.display()))?;
    let config: Config = toml::from_str(&text).context("parsing the configuration")?;
    let datasets = config
        .datasets
        .into_iter()
        .map(|d| Dataset {
            source: match d.source {
                SourceConfig::ArrowFile { path } => Box::new(ArrowFile { path }) as Box<dyn Source>,
                SourceConfig::Generator(g) => Box::new(g),
                SourceConfig::Http(c) => Box::new(Http::new(c)),
                SourceConfig::Parquet(c) => Box::new(Parquet::new(c)),
            },
            name: d.name,
            parameters: d.parameters,
            primary_key: d.primary_key,
            duplicates: if d.last_wins {
                DuplicatePolicy::LastWins
            } else {
                DuplicatePolicy::Error
            },
            refresh: RefreshPolicy {
                max_age: d.max_age_seconds.map(Duration::from_secs),
                every: d.refresh_seconds.map(Duration::from_secs),
                ..RefreshPolicy::default()
            },
            resolve: None,
            max_tables: d.max_tables,
        })
        .collect();
    let registry = Arc::new(Registry::new(datasets));
    spawn_refresher(Arc::clone(&registry), Duration::from_secs(1));
    let mut app = router(registry);
    if let Some(dir) = config.static_dir {
        app = app.fallback_service(ServeDir::new(dir));
    }
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(address = %config.listen, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn every_example_configuration_parses() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "toml") {
                let text = std::fs::read_to_string(&path).unwrap();
                if let Err(e) = toml::from_str::<Config>(&text) {
                    panic!("{}: {e}", path.display());
                }
                checked += 1;
            }
        }
        assert!(checked >= 3);
    }
}
