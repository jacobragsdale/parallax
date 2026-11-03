//! Datasets, their sources, and the table cache shared by every connection.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use parallax_engine::{DuplicatePolicy, TableOptions};
use parallax_protocol::v1::DatasetInfo;
use parallax_session::{Catalog, ConnectionContext, SharedTable, TableState};
use tokio::sync::broadcast;

/// The current time in microseconds since the Unix epoch.
#[must_use]
pub fn now_micros() -> i64 {
    jiff::Timestamp::now().as_microsecond()
}

/// Rows read from a source.
#[derive(Debug)]
pub struct Fetched {
    /// The schema, known even when there are no rows.
    pub schema: SchemaRef,
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

/// When a cached table is reloaded and dropped.
#[derive(Clone, Copy, Debug)]
pub struct RefreshPolicy {
    /// Reload when a view opens on a table older than this.
    pub max_age: Option<Duration>,
    /// Reload on this interval while some view uses the table.
    pub every: Option<Duration>,
    /// Drop a table no view has used for this long.
    pub idle: Duration,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            max_age: None,
            every: None,
            idle: Duration::from_secs(600),
        }
    }
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
    /// When to reload and drop tables.
    pub refresh: RefreshPolicy,
    /// How parameters and context become the cache key.
    pub resolve: Option<ResolveParameters>,
    /// The most parameter combinations cached at once; more are refused, so that
    /// clients cannot fill memory by varying parameters.
    pub max_tables: usize,
}

/// A cached table. The slot's own lock serializes loading, so concurrent first
/// opens load once and reloads never overlap.
#[derive(Default)]
struct Slot {
    table: Option<SharedTable>,
    loaded: Option<Instant>,
    used: Option<Instant>,
}

type CacheKey = (String, BTreeMap<String, String>);

/// The datasets and the table cache.
pub struct Registry {
    datasets: BTreeMap<String, Dataset>,
    cache: Mutex<HashMap<CacheKey, Arc<Mutex<Slot>>>>,
    changes: broadcast::Sender<()>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Slots and the cache map hold no invariant a panic could break halfway.
    m.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// Returns `true` if `name` is a dataset.
    #[must_use]
    pub fn has_dataset(&self, name: &str) -> bool {
        self.datasets.contains_key(name)
    }

    fn load(
        dataset: &Dataset,
        parameters: &BTreeMap<String, String>,
    ) -> Result<SharedTable, String> {
        let fetched = dataset.source.fetch(parameters)?;
        let state = TableState::from_batches(
            &fetched.schema,
            &fetched.batches,
            TableOptions {
                primary_key: dataset.primary_key.clone(),
                duplicates: dataset.duplicates,
            },
            now_micros(),
            fetched.as_of_micros,
        )
        .map_err(|e| e.to_string())?;
        Ok(Arc::new(RwLock::new(state)))
    }

    /// Reloads the slot's table. A failure keeps the previous rows and records the
    /// error on the table, where views show it. A table poisoned by an earlier panic
    /// is replaced by a fresh load.
    fn reload(&self, dataset: &Dataset, parameters: &BTreeMap<String, String>, slot: &mut Slot) {
        let Some(table) = slot.table.clone() else {
            return;
        };
        slot.loaded = Some(Instant::now());
        if table.is_poisoned() {
            match Self::load(dataset, parameters) {
                Ok(fresh) => slot.table = Some(fresh),
                Err(message) => {
                    tracing::warn!(dataset = %dataset.name, error = %message, "reload failed")
                }
            }
            let _ = self.changes.send(());
            return;
        }
        let result = dataset.source.fetch(parameters);
        let Ok(mut state) = table.write() else {
            return;
        };
        match result {
            Ok(fetched) => {
                if let Err(e) = state.reload(&fetched.batches, now_micros(), fetched.as_of_micros) {
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

    fn slots(&self) -> Vec<(CacheKey, Arc<Mutex<Slot>>)> {
        lock(&self.cache)
            .iter()
            .map(|(k, s)| (k.clone(), Arc::clone(s)))
            .collect()
    }

    /// Reloads tables due under their dataset's `every` interval, and drops tables
    /// no view has used for the dataset's idle time.
    pub fn refresh_due(&self) {
        for ((name, parameters), slot) in self.slots() {
            let Some(dataset) = self.datasets.get(&name) else {
                continue;
            };
            let mut slot = lock(&slot);
            let in_use = slot
                .table
                .as_ref()
                .is_some_and(|t| Arc::strong_count(t) > 1);
            if in_use {
                slot.used = Some(Instant::now());
            }
            let idle = slot
                .used
                .is_some_and(|u| u.elapsed() >= dataset.refresh.idle);
            if !in_use && idle {
                slot.table = None;
                continue;
            }
            let due = dataset
                .refresh
                .every
                .is_some_and(|every| slot.loaded.is_some_and(|l| l.elapsed() >= every));
            if in_use && due {
                self.reload(dataset, &parameters, &mut slot);
            }
        }
        lock(&self.cache)
            .retain(|_, slot| lock(slot).table.is_some() || Arc::strong_count(slot) > 1);
    }

    /// Reloads every cached table of `dataset` now, for publishers that know the
    /// data changed. Returns how many tables were reloaded, or `None` for an
    /// unknown dataset.
    #[must_use]
    pub fn invalidate(&self, dataset: &str) -> Option<usize> {
        let definition = self.datasets.get(dataset)?;
        let mut reloaded = 0;
        for ((name, parameters), slot) in self.slots() {
            if name != dataset {
                continue;
            }
            let mut slot = lock(&slot);
            if slot.table.is_some() {
                self.reload(definition, &parameters, &mut slot);
                reloaded += 1;
            }
        }
        Some(reloaded)
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
        let slot = {
            let mut cache = lock(&self.cache);
            let key = (name.to_owned(), resolved.clone());
            if !cache.contains_key(&key) {
                let cached = cache.keys().filter(|(n, _)| n == name).count();
                if cached >= dataset.max_tables {
                    return Err(format!(
                        "{name} already has {cached} cached parameter combinations, the limit"
                    ));
                }
            }
            Arc::clone(cache.entry(key).or_default())
        };
        let mut slot = lock(&slot);
        slot.used = Some(Instant::now());
        match slot.table.clone() {
            Some(table) => {
                let stale = dataset
                    .refresh
                    .max_age
                    .is_some_and(|age| slot.loaded.is_some_and(|l| l.elapsed() >= age));
                if stale || table.is_poisoned() {
                    self.reload(dataset, &resolved, &mut slot);
                }
                slot.table
                    .clone()
                    .ok_or_else(|| format!("{name} could not be reloaded"))
            }
            None => {
                let table = Self::load(dataset, &resolved)?;
                slot.table = Some(Arc::clone(&table));
                slot.loaded = Some(Instant::now());
                Ok(table)
            }
        }
    }
}
