//! The Parquet connector, with a local cache in front of a slow share.
//!
//! The file is copied once to a local directory, keyed by its path, size and
//! modification time, and read from there until it changes. Older copies of the
//! same path are removed when a new one is made. `{name}` placeholders in the path
//! take dataset parameters, which may not contain path separators or start with a
//! dot, so a parameter cannot reach outside the configured directory.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;

use crate::{Fetched, Source, fill};

/// Rows read per batch.
const BATCH_ROWS: usize = 65_536;

/// Where a dataset's Parquet file is.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParquetConfig {
    /// The file. `{name}` is replaced by the dataset parameter `name`.
    pub path: String,
    /// A local directory that caches copies of the file.
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,
}

/// A dataset read from a Parquet file.
#[derive(Clone, Debug)]
pub struct Parquet {
    config: ParquetConfig,
}

impl Parquet {
    /// A connector for `config`.
    #[must_use]
    pub fn new(config: ParquetConfig) -> Self {
        Self { config }
    }
}

/// Refuses parameter values that could leave the configured directory.
fn path_component(value: &str) -> Result<String, String> {
    if value.is_empty() || value.starts_with('.') || value.contains(['/', '\\', '\0']) {
        return Err(format!("{value:?} cannot be used in a file path"));
    }
    Ok(value.to_owned())
}

/// FNV-1a: a hash that stays the same across Rust releases, so cache names do.
fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The cached copy of `source`, made if it is missing or out of date. Each copy
/// is written under a name of its own and renamed into place, so concurrent
/// fetches, even from several processes, never see a partial file.
fn cached(dir: &Path, source: &Path, size: u64, modified: i128) -> Result<PathBuf, String> {
    let prefix = format!(
        "{:016x}-",
        stable_hash(source.as_os_str().as_encoded_bytes())
    );
    let local = dir.join(format!("{prefix}{size}-{modified}.parquet"));
    if std::fs::metadata(&local).is_ok_and(|m| m.len() == size) {
        return Ok(local);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let unique = format!("{}-{}", std::process::id(), crate::now_micros());
    let partial = dir.join(format!("{prefix}{size}-{modified}.{unique}.partial"));
    if let Err(e) = std::fs::copy(source, &partial) {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("copying {} to the cache: {e}", source.display()));
    }
    std::fs::rename(&partial, &local).map_err(|e| format!("{}: {e}", local.display()))?;
    // Older finished copies of the same file are no longer needed; partial copies
    // belong to fetches still running and are left alone.
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name.ends_with(".parquet") && entry.path() != local {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(local)
}

impl Source for Parquet {
    fn fetch(&self, parameters: &BTreeMap<String, String>) -> Result<Fetched, String> {
        // Only parameters the path uses are checked; others may hold anything.
        let path = PathBuf::from(fill(&self.config.path, parameters, path_component)?.text);
        let metadata = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| jiff::Timestamp::try_from(t).ok())
            .ok_or_else(|| format!("{}: no modification time", path.display()))?;
        let read_from = match &self.config.cache_dir {
            Some(dir) => cached(dir, &path, metadata.len(), modified.as_nanosecond())?,
            None => path.clone(),
        };
        let file = File::open(&read_from).map_err(|e| format!("{}: {e}", read_from.display()))?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let schema = builder.schema().clone();
        let reader = builder
            .with_batch_size(BATCH_ROWS)
            .build()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Fetched {
            schema,
            batches,
            as_of_micros: modified.as_microsecond(),
        })
    }
}
