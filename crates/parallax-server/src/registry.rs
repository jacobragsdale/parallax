//! Datasets, their sources, and the table cache shared by every connection.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use arrow_array::RecordBatch;
use parallax_engine::{DuplicatePolicy, Table, TableOptions};
use parallax_protocol::v1::DatasetInfo;
use parallax_session::{Catalog, ConnectionContext, SharedTable, TableState};
use tokio::sync::broadcast;

/// Rows read from a source.
#[derive(Debug)]
pub struct Fetched {
    /// The rows.
    pub batches: Vec<RecordBatch>,
    /// When the source produced them, in microseconds since the Unix epoch.
    pub as_of_micros: i64,
}

/// Where a dataset's rows come from.
pub trait Source: Send + Sync {
    /// Reads every row for `parameters`.
    ///
    /// # Errors
    ///
    /// Returns a message if the source cannot be read.
    fn fetch(&self, parameters: &BTreeMap<String, String>) -> Result<Fetched, String>;
}

/// Maps a request's parameters and connection context to the parameters the table
/// is cached by. Tables are shared by every connection with the same resolved
/// parameters, so a host that filters by user must put the user into them.
pub type ResolveParameters = Arc<
    dyn Fn(
            &BTreeMap<String, String>,
            &ConnectionContext,
        ) -> Result<BTreeMap<String, String>, String>
        + Send
        + Sync,
>;

/// When a cached table is reloaded.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefreshPolicy {
    /// Reload when a view opens on a table older than this.
    pub max_age: Option<Duration>,
    /// Reload on this interval while the table is cached.
    pub every: Option<Duration>,
}

/// A dataset clients may open.
pub struct Dataset {
    /// The name clients use.
    pub name: String,
    /// The parameters it accepts.
    pub parameters: Vec<String>,
    /// Where its rows come from.
    pub source: Box<dyn Source>,
    /// The table's key, which makes reloads diffs.
    pub primary_key: Option<String>,
    /// What to do with repeated keys.
    pub duplicates: DuplicatePolicy,
    /// When to reload.
    pub refresh: RefreshPolicy,
    /// How parameters and context become the cache key.
    pub resolve: Option<ResolveParameters>,
}

struct Cached {
    table: SharedTable,
    loaded: Instant,
}

type CacheKey = (String, BTreeMap<String, String>);

/// The datasets and the table cache.
pub struct Registry {
    datasets: BTreeMap<String, Dataset>,
    cache: Mutex<HashMap<CacheKey, Cached>>,
    changes: broadcast::Sender<()>,
}

fn now_micros() -> i64 {
    jiff::Timestamp::now().as_microsecond()
}

impl Registry {
    /// Creates a registry of `datasets`.
    #[must_use]
    pub fn new(datasets: Vec<Dataset>) -> Self {
        let (changes, _) = broadcast::channel(64);
        Self {
            datasets: datasets.into_iter().map(|d| (d.name.clone(), d)).collect(),
            cache: Mutex::new(HashMap::new()),
            changes,
        }
    }

    /// Notifies on every reload, so connections can refresh their views.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    fn load(
        &self,
        dataset: &Dataset,
        parameters: &BTreeMap<String, String>,
    ) -> Result<SharedTable, String> {
        let fetched = dataset.source.fetch(parameters)?;
        let schema = fetched
            .batches
            .first()
            .map(RecordBatch::schema)
            .ok_or_else(|| format!("{} returned no batches", dataset.name))?;
        let mut table = Table::new(
            &schema,
            TableOptions {
                primary_key: dataset.primary_key.clone(),
                duplicates: dataset.duplicates,
            },
        )
        .map_err(|e| e.to_string())?;
        table.load(&fetched.batches).map_err(|e| e.to_string())?;
        Ok(Arc::new(RwLock::new(TableState::new(
            table,
            now_micros(),
            fetched.as_of_micros,
        ))))
    }

    /// Reloads one cached table now. A failure keeps the previous rows and records
    /// the error on the table, where views show it.
    fn reload(
        &self,
        dataset: &Dataset,
        parameters: &BTreeMap<String, String>,
        table: &SharedTable,
    ) {
        let result = dataset.source.fetch(parameters);
        let mut state = table.write().expect("table lock");
        match result {
            Ok(fetched) => {
                let outcome = state.reload(&fetched.batches, now_micros(), fetched.as_of_micros);
                if let Err(e) = outcome {
                    tracing::warn!(dataset = %dataset.name, error = %e, "reload refused");
                }
            }
            Err(message) => {
                tracing::warn!(dataset = %dataset.name, error = %message, "source failed");
                state.record_failure(message, now_micros());
            }
        }
        drop(state);
        let _ = self.changes.send(());
    }

    /// Reloads the tables that are due under their dataset's `every` interval.
    pub fn refresh_due(&self) {
        let due: Vec<(CacheKey, SharedTable)> = {
            let mut cache = self.cache.lock().expect("cache lock");
            cache
                .iter_mut()
                .filter(|((name, _), cached)| {
                    self.datasets
                        .get(name)
                        .and_then(|d| d.refresh.every)
                        .is_some_and(|every| cached.loaded.elapsed() >= every)
                })
                .map(|(key, cached)| {
                    cached.loaded = Instant::now();
                    (key.clone(), Arc::clone(&cached.table))
                })
                .collect()
        };
        for ((name, parameters), table) in due {
            if let Some(dataset) = self.datasets.get(&name) {
                self.reload(dataset, &parameters, &table);
            }
        }
    }

    /// Reloads every cached table of `dataset` now, for publishers that know the
    /// data changed. Returns how many tables were reloaded.
    pub fn invalidate(&self, dataset: &str) -> usize {
        let tables: Vec<(BTreeMap<String, String>, SharedTable)> = {
            let mut cache = self.cache.lock().expect("cache lock");
            cache
                .iter_mut()
                .filter(|((name, _), _)| name == dataset)
                .map(|((_, parameters), cached)| {
                    cached.loaded = Instant::now();
                    (parameters.clone(), Arc::clone(&cached.table))
                })
                .collect()
        };
        let Some(definition) = self.datasets.get(dataset) else {
            return 0;
        };
        for (parameters, table) in &tables {
            self.reload(definition, parameters, table);
        }
        tables.len()
    }
}

impl Catalog for Registry {
    fn datasets(&self) -> Vec<DatasetInfo> {
        self.datasets
            .values()
            .map(|d| DatasetInfo {
                name: d.name.clone(),
                parameters: d.parameters.clone(),
            })
            .collect()
    }

    fn open(
        &self,
        name: &str,
        parameters: &BTreeMap<String, String>,
        context: &ConnectionContext,
    ) -> Result<SharedTable, String> {
        let dataset = self
            .datasets
            .get(name)
            .ok_or_else(|| format!("unknown dataset {name}"))?;
        for key in parameters.keys() {
            if !dataset.parameters.contains(key) {
                return Err(format!("{name} has no parameter {key}"));
            }
        }
        let resolved = match &dataset.resolve {
            Some(resolve) => resolve(parameters, context)?,
            None => parameters.clone(),
        };
        let key = (name.to_owned(), resolved.clone());
        let existing = {
            let cache = self.cache.lock().expect("cache lock");
            cache.get(&key).map(|c| (Arc::clone(&c.table), c.loaded))
        };
        match existing {
            Some((table, loaded)) => {
                if dataset
                    .refresh
                    .max_age
                    .is_some_and(|age| loaded.elapsed() >= age)
                {
                    if let Some(c) = self.cache.lock().expect("cache lock").get_mut(&key) {
                        c.loaded = Instant::now();
                    }
                    self.reload(dataset, &resolved, &table);
                }
                Ok(table)
            }
            None => {
                let table = self.load(dataset, &resolved)?;
                self.cache.lock().expect("cache lock").insert(
                    key,
                    Cached {
                        table: Arc::clone(&table),
                        loaded: Instant::now(),
                    },
                );
                Ok(table)
            }
        }
    }
}
