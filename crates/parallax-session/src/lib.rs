//! Runs the parallax protocol over the engine, independent of transport.
//!
//! A [`Session`] belongs to one connection: a WebSocket on the server, or the
//! browser worker that hosts the WebAssembly engine. It turns client messages into
//! server messages. Tables come from a [`Catalog`] and are shared between sessions.
//!
//! A table can reload at any moment, and a reload reuses freed row slots. Before a
//! view answers anything, it first applies the change sets it missed, under the same
//! read lock that the answer is computed under. A frame therefore always describes
//! the table as it is, and carries that load's version.
//!
//! The session never reads a clock. Times on tables are supplied by whoever loads
//! them, which keeps the code identical on the server and in WebAssembly.

use std::collections::{BTreeMap, VecDeque};
use std::ops::Range;
use std::sync::{Arc, RwLock, RwLockReadGuard};

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use parallax_engine::{ChangeSet, LoadError, LoadReport, Table, TableOptions, View};
use parallax_protocol::PROTOCOL_VERSION;
use parallax_protocol::ipc::{encode_batch, encode_schema};
use parallax_protocol::layout::{
    AggregateName, FilterOpName, Layout, LayoutExpanded, LayoutFilter, LayoutMeasure,
    ResolvedLayout, ShowAsName,
};
use parallax_protocol::v1::{
    ClientMessage, ColumnCount, DatasetInfo, DistinctValues, DistinctValuesResult,
    Error as ErrorMessage, Frame, ServerMessage, TableStatus, ViewOpened, Welcome, client_message,
    server_message,
};
use prost::Message;

#[cfg(feature = "testing")]
pub mod testing;

/// How many recent change sets a table keeps for views that are behind.
const HISTORY: usize = 16;

/// A table and what the views need to know about its loads.
#[derive(Debug)]
pub struct TableState {
    /// The table.
    pub table: Table,
    history: VecDeque<Arc<ChangeSet>>,
    loaded_at_micros: i64,
    source_as_of_micros: i64,
    nans_to_null: Vec<(String, usize)>,
    refresh_error: Option<(String, i64)>,
}

/// A table shared between sessions.
pub type SharedTable = Arc<RwLock<TableState>>;

impl TableState {
    /// Wraps a table that already holds its first load.
    #[must_use]
    pub fn new(table: Table, loaded_at_micros: i64, source_as_of_micros: i64) -> Self {
        Self {
            table,
            history: VecDeque::new(),
            loaded_at_micros,
            source_as_of_micros,
            nans_to_null: Vec::new(),
            refresh_error: None,
        }
    }

    /// Creates a table from a first load. The schema comes from the source, so an
    /// empty result still opens as an empty table.
    ///
    /// # Errors
    ///
    /// Returns the load error if the table refuses the rows.
    pub fn from_batches(
        schema: &Schema,
        batches: &[RecordBatch],
        options: TableOptions,
        now_micros: i64,
        source_as_of_micros: i64,
    ) -> Result<Self, LoadError> {
        let mut table = Table::new(schema, options)?;
        let (report, _) = table.load(batches)?;
        let mut state = Self::new(table, now_micros, source_as_of_micros);
        state.nans_to_null = report.nans_to_null;
        Ok(state)
    }

    /// Replaces the table's contents. A refused load keeps the previous contents and
    /// records the error, so views keep showing the last good data.
    ///
    /// # Errors
    ///
    /// Returns the load error after recording it.
    pub fn reload(
        &mut self,
        batches: &[RecordBatch],
        now_micros: i64,
        source_as_of_micros: i64,
    ) -> Result<LoadReport, LoadError> {
        match self.table.load(batches) {
            Ok((report, change)) => {
                self.history.push_back(Arc::new(change));
                while self.history.len() > HISTORY {
                    self.history.pop_front();
                }
                self.loaded_at_micros = now_micros;
                self.source_as_of_micros = source_as_of_micros;
                self.nans_to_null.clone_from(&report.nans_to_null);
                self.refresh_error = None;
                Ok(report)
            }
            Err(error) => {
                self.refresh_error = Some((error.to_string(), now_micros));
                Err(error)
            }
        }
    }

    /// Records a refresh that failed before reaching the table, such as an
    /// unreachable source.
    pub fn record_failure(&mut self, message: String, now_micros: i64) {
        self.refresh_error = Some((message, now_micros));
    }

    fn status(&self, view_id: u32) -> TableStatus {
        TableStatus {
            view_id,
            load_version: self.table.load_version(),
            rows: self.table.num_rows() as u64,
            loaded_at_micros: self.loaded_at_micros,
            refresh_error: self
                .refresh_error
                .as_ref()
                .map(|(m, _)| m.clone())
                .unwrap_or_default(),
            refresh_failed_at_micros: self.refresh_error.as_ref().map_or(0, |(_, t)| *t),
            nans_to_null: self
                .nans_to_null
                .iter()
                .map(|(column, count)| ColumnCount {
                    column: column.clone(),
                    count: *count as u64,
                })
                .collect(),
        }
    }
}

/// Reads a shared table. A lock poisoned by a panic during a reload means the table
/// may be half-updated, so it is refused rather than shown.
///
/// # Errors
///
/// Returns a message if the lock is poisoned.
pub fn read_table(table: &SharedTable) -> Result<RwLockReadGuard<'_, TableState>, String> {
    table
        .read()
        .map_err(|_| "the table is unavailable after an internal error".to_owned())
}

/// A value the host attaches to a connection, such as the signed-in user, and that
/// dataset code receives. The library passes it through without interpreting it.
pub type ConnectionContext = Arc<dyn std::any::Any + Send + Sync>;

/// Returns a context that carries nothing.
#[must_use]
pub fn no_context() -> ConnectionContext {
    Arc::new(())
}

/// Where sessions find tables.
pub trait Catalog: Send + Sync {
    /// The datasets a client may open.
    fn datasets(&self) -> Vec<DatasetInfo>;

    /// Returns the table for a dataset and its parameters, loading it if needed.
    ///
    /// # Errors
    ///
    /// Returns a message for unknown datasets, bad parameters, or a failed load.
    fn open(
        &self,
        dataset: &str,
        parameters: &BTreeMap<String, String>,
        context: &ConnectionContext,
    ) -> Result<SharedTable, String>;
}

/// A catalog of tables held in memory, keyed by dataset name. Parameters are
/// ignored. The WebAssembly worker and the tests use it.
#[derive(Default)]
pub struct MemoryCatalog {
    tables: RwLock<BTreeMap<String, SharedTable>>,
}

impl MemoryCatalog {
    /// Returns an empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a dataset.
    pub fn insert(&self, name: &str, table: SharedTable) {
        self.tables
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_owned(), table);
    }

    /// Returns a dataset's table.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<SharedTable> {
        self.tables
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }
}

impl Catalog for MemoryCatalog {
    fn datasets(&self) -> Vec<DatasetInfo> {
        self.tables
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .map(|name| DatasetInfo {
                name: name.clone(),
                parameters: Vec::new(),
            })
            .collect()
    }

    fn open(
        &self,
        dataset: &str,
        _: &BTreeMap<String, String>,
        _: &ConnectionContext,
    ) -> Result<SharedTable, String> {
        self.get(dataset)
            .ok_or_else(|| format!("unknown dataset {dataset}"))
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Viewport {
    first_row: u64,
    row_count: u64,
    first_column: u32,
    column_count: u32,
}

/// Converts a client's start and count to an index range, saturating instead of
/// overflowing and clamping to the platform's index size.
fn index_range(start: u64, count: u64) -> Range<usize> {
    let end = start.saturating_add(count);
    let clamp = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
    clamp(start)..clamp(end)
}

struct OpenView {
    table: SharedTable,
    /// The dataset and parameters the view was opened with; layouts sent later
    /// may not name others.
    dataset: String,
    parameters: BTreeMap<String, String>,
    layout: Layout,
    view: View,
    config_version: u64,
    viewport: Option<Viewport>,
    status_version: u64,
    status_error: Option<String>,
    /// The load version a rebuild last failed at, so the error is reported once.
    broken_at: Option<u64>,
}

/// The most cells one [`FetchWindow`](parallax_protocol::v1::FetchWindow) may ask for.
pub const MAX_FETCH_CELLS: u64 = 1_000_000;

/// Refuses a layout that names a dataset or parameters other than the view's.
fn same_source(
    layout: &Layout,
    dataset: &str,
    parameters: &BTreeMap<String, String>,
) -> Result<(), String> {
    if !layout.dataset.is_empty() && layout.dataset != dataset {
        return Err(format!(
            "the layout is for dataset {}, but the view reads {dataset}",
            layout.dataset
        ));
    }
    if !layout.parameters.is_empty() && &layout.parameters != parameters {
        return Err("the layout's parameters differ from the view's".to_owned());
    }
    Ok(())
}

/// Builds a view from a resolved layout and applies its expanded groups.
fn build_view(table: &Table, resolved: ResolvedLayout) -> Result<View, String> {
    let mut view = View::new(table, resolved.config).map_err(|e| e.to_string())?;
    for (path, expanded) in resolved.expanded {
        view.set_expanded(path, expanded);
    }
    Ok(view)
}

impl OpenView {
    /// Applies the change sets the view missed, or rebuilds it if the kept history
    /// does not reach back far enough. Returns `true` if the view changed.
    fn catch_up(&mut self, state: &TableState) -> Result<bool, String> {
        let current = state.table.load_version();
        let behind = self.view.load_version();
        if behind >= current {
            return Ok(false);
        }
        let missing: Vec<&Arc<ChangeSet>> = state
            .history
            .iter()
            .filter(|c| c.load_version > behind)
            .collect();
        let contiguous = missing
            .first()
            .is_some_and(|c| c.load_version == behind + 1)
            && missing.last().is_some_and(|c| c.load_version == current);
        if contiguous {
            for change in missing {
                self.view.apply(&state.table, change);
            }
        } else {
            let resolved = self
                .layout
                .resolve(&state.table)
                .map_err(|e| e.to_string())?;
            self.view = build_view(&state.table, resolved)?;
        }
        Ok(true)
    }
}

/// The protocol state of one connection.
pub struct Session {
    catalog: Arc<dyn Catalog>,
    context: ConnectionContext,
    views: BTreeMap<u32, OpenView>,
}

fn error(request_id: u64, view_id: u32, message: impl Into<String>) -> ServerMessage {
    ServerMessage {
        request_id,
        body: Some(server_message::Body::Error(ErrorMessage {
            view_id,
            message: message.into(),
        })),
    }
}

fn reply(request_id: u64, body: server_message::Body) -> ServerMessage {
    ServerMessage {
        request_id,
        body: Some(body),
    }
}

impl Session {
    /// Starts a session over `catalog` with no connection context.
    #[must_use]
    pub fn new(catalog: Arc<dyn Catalog>) -> Self {
        Self::with_context(catalog, no_context())
    }

    /// Starts a session whose dataset requests carry `context`.
    #[must_use]
    pub fn with_context(catalog: Arc<dyn Catalog>, context: ConnectionContext) -> Self {
        Self {
            catalog,
            context,
            views: BTreeMap::new(),
        }
    }

    /// The number of open views.
    #[must_use]
    pub fn open_views(&self) -> usize {
        self.views.len()
    }

    /// Decodes a protobuf client message, handles it and encodes the replies.
    #[must_use]
    pub fn handle_bytes(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let replies = match ClientMessage::decode(bytes) {
            Ok(message) => self.handle(message),
            Err(e) => vec![error(0, 0, format!("undecodable message: {e}"))],
        };
        replies.iter().map(Message::encode_to_vec).collect()
    }

    /// Handles one client message.
    pub fn handle(&mut self, message: ClientMessage) -> Vec<ServerMessage> {
        let id = message.request_id;
        let Some(body) = message.body else {
            return vec![error(id, 0, "empty message")];
        };
        let view_id = match &body {
            client_message::Body::Hello(_) => 0,
            client_message::Body::OpenView(m) => m.view_id,
            client_message::Body::UpdateView(m) => m.view_id,
            client_message::Body::SetViewport(m) => m.view_id,
            client_message::Body::SetExpanded(m) => m.view_id,
            client_message::Body::CloseView(m) => m.view_id,
            client_message::Body::DrillThrough(m) => m.view_id,
            client_message::Body::DistinctValues(m) => m.view_id,
            client_message::Body::FetchWindow(m) => m.view_id,
        };
        match self.dispatch(id, body) {
            Ok(messages) => messages,
            Err(message) => vec![error(id, view_id, message)],
        }
    }

    fn dispatch(
        &mut self,
        id: u64,
        body: client_message::Body,
    ) -> Result<Vec<ServerMessage>, String> {
        Ok(match body {
            client_message::Body::Hello(_) => vec![reply(
                id,
                server_message::Body::Welcome(Welcome {
                    protocol_version: PROTOCOL_VERSION,
                    datasets: self.catalog.datasets(),
                }),
            )],
            client_message::Body::OpenView(open) => {
                let parameters: BTreeMap<String, String> = open.parameters.into_iter().collect();
                let layout = Layout::from_json(&open.layout_json).map_err(|e| e.to_string())?;
                same_source(&layout, &open.dataset, &parameters)?;
                let table = self
                    .catalog
                    .open(&open.dataset, &parameters, &self.context)?;
                self.open_view(id, open.view_id, table, &open.dataset, parameters, layout)?
            }
            client_message::Body::UpdateView(update) => {
                let open = self.view_mut(update.view_id)?;
                let layout = Layout::from_json(&update.layout_json).map_err(|e| e.to_string())?;
                same_source(&layout, &open.dataset, &open.parameters)?;
                let table = Arc::clone(&open.table);
                {
                    let state = read_table(&table)?;
                    // A view that cannot catch up, because its old layout no longer
                    // fits the table, is rebuilt from the new layout instead, so a
                    // corrected layout repairs it.
                    let current = open.catch_up(&state).is_ok();
                    let resolved = layout.resolve(&state.table).map_err(|e| e.to_string())?;
                    // Display-only changes, such as formats, widths or newly expanded
                    // groups, keep the computed view; anything else rebuilds it.
                    let kept = current
                        && open
                            .layout
                            .expanded
                            .iter()
                            .all(|old| layout.expanded.iter().any(|new| new.path == old.path))
                        && open
                            .view
                            .reformat(&state.table, &resolved.config)
                            .map_err(|e| e.to_string())?;
                    if kept {
                        for (path, expanded) in resolved.expanded {
                            open.view.set_expanded(path, expanded);
                        }
                    } else {
                        open.view = build_view(&state.table, resolved)?;
                    }
                }
                open.layout = layout;
                open.config_version = update.config_version;
                open.broken_at = None;
                self.frame(id, update.view_id, false).into_iter().collect()
            }
            client_message::Body::SetViewport(v) => {
                let viewport = Viewport {
                    first_row: v.first_row,
                    row_count: v.row_count,
                    first_column: v.first_column,
                    column_count: v.column_count,
                };
                self.view_mut(v.view_id)?.viewport = Some(viewport);
                self.frame(id, v.view_id, false).into_iter().collect()
            }
            client_message::Body::FetchWindow(f) => {
                let viewport = Viewport {
                    first_row: f.first_row,
                    row_count: f.row_count,
                    first_column: f.first_column,
                    column_count: f.column_count,
                };
                self.window_frame(id, f.view_id, viewport, false, Some(MAX_FETCH_CELLS))
                    .into_iter()
                    .collect()
            }
            client_message::Body::SetExpanded(e) => {
                let path: Vec<serde_json::Value> =
                    serde_json::from_str(&e.path_json).map_err(|err| err.to_string())?;
                let open = self.view_mut(e.view_id)?;
                let table = Arc::clone(&open.table);
                let state = read_table(&table)?;
                open.catch_up(&state)?;
                let probe = Layout {
                    group_by: open.layout.group_by.clone(),
                    expanded: vec![LayoutExpanded {
                        path: path.clone(),
                        expanded: e.expanded,
                    }],
                    ..Layout::default()
                };
                let resolved = probe.resolve(&state.table).map_err(|err| err.to_string())?;
                drop(state);
                for (scalars, flag) in resolved.expanded {
                    open.view.set_expanded(scalars, flag);
                }
                open.layout.expanded.retain(|x| x.path != path);
                open.layout.expanded.push(LayoutExpanded {
                    path,
                    expanded: e.expanded,
                });
                self.frame(id, e.view_id, false).into_iter().collect()
            }
            client_message::Body::CloseView(c) => {
                self.views.remove(&c.view_id);
                Vec::new()
            }
            client_message::Body::DrillThrough(d) => {
                let open = self.view_mut(d.view_id)?;
                let table = Arc::clone(&open.table);
                let layout = {
                    let state = read_table(&table)?;
                    open.catch_up(&state)?;
                    let config = open
                        .view
                        .drill_through(&state.table, usize::try_from(d.row).unwrap_or(usize::MAX))
                        .ok_or_else(|| "that row has no single group to drill into".to_owned())?;
                    Layout::from_flat_config(&config, &state.table)
                };
                let (dataset, parameters) = (open.dataset.clone(), open.parameters.clone());
                self.open_view(id, d.new_view_id, table, &dataset, parameters, layout)?
            }
            client_message::Body::DistinctValues(request) => vec![reply(
                id,
                server_message::Body::DistinctValues(self.distinct_values(&request)?),
            )],
        })
    }

    fn view_mut(&mut self, view_id: u32) -> Result<&mut OpenView, String> {
        self.views
            .get_mut(&view_id)
            .ok_or_else(|| format!("no view {view_id}"))
    }

    fn open_view(
        &mut self,
        request_id: u64,
        view_id: u32,
        table: SharedTable,
        dataset: &str,
        parameters: BTreeMap<String, String>,
        layout: Layout,
    ) -> Result<Vec<ServerMessage>, String> {
        let (view, schema, status) = {
            let state = read_table(&table)?;
            let resolved = layout.resolve(&state.table).map_err(|e| e.to_string())?;
            let view = build_view(&state.table, resolved)?;
            let schema = encode_schema(state.table.schema()).map_err(|e| e.to_string())?;
            (view, schema, state.status(view_id))
        };
        let layout_json = layout.to_json();
        let status_version = status.load_version;
        let status_error = (!status.refresh_error.is_empty()).then(|| status.refresh_error.clone());
        self.views.insert(
            view_id,
            OpenView {
                table,
                dataset: dataset.to_owned(),
                parameters,
                layout,
                view,
                config_version: 0,
                viewport: None,
                status_version,
                status_error,
                broken_at: None,
            },
        );
        Ok(vec![reply(
            request_id,
            server_message::Body::ViewOpened(ViewOpened {
                view_id,
                table_schema: schema,
                status: Some(status),
                layout_json,
            }),
        )])
    }

    /// Builds the frame for a view's current viewport, if it has one.
    fn frame(&mut self, request_id: u64, view_id: u32, pushed: bool) -> Option<ServerMessage> {
        let viewport = self.views.get(&view_id)?.viewport?;
        self.window_frame(request_id, view_id, viewport, pushed, None)
    }

    /// Builds the frame for one window of a view, refusing windows of more than
    /// `max_cells` cells. The view first catches up with its table, under the same
    /// lock the window is read under.
    fn window_frame(
        &mut self,
        request_id: u64,
        view_id: u32,
        viewport: Viewport,
        pushed: bool,
        max_cells: Option<u64>,
    ) -> Option<ServerMessage> {
        let Some(open) = self.views.get_mut(&view_id) else {
            return Some(error(request_id, view_id, format!("no view {view_id}")));
        };
        let table = Arc::clone(&open.table);
        let state = match read_table(&table) {
            Ok(state) => state,
            Err(message) => return Some(error(request_id, view_id, message)),
        };
        if let Err(message) = open.catch_up(&state) {
            return Some(error(request_id, view_id, message));
        }
        let rows = index_range(viewport.first_row, viewport.row_count);
        let columns = index_range(
            u64::from(viewport.first_column),
            u64::from(viewport.column_count),
        );
        if let Some(max) = max_cells {
            let total_rows = open.view.num_rows(&state.table);
            let total_columns = open.view.num_columns(&state.table);
            let height = rows.end.min(total_rows).saturating_sub(rows.start) as u64;
            let width = columns.end.min(total_columns).saturating_sub(columns.start) as u64;
            if height.saturating_mul(width) > max {
                return Some(error(
                    request_id,
                    view_id,
                    format!(
                        "a window of {height} rows by {width} columns is more than the {max} cells one request may fetch"
                    ),
                ));
            }
        }
        let window = open.view.window(&state.table, rows, columns);
        let batch = match encode_batch(&window.batch) {
            Ok(bytes) => bytes,
            Err(e) => return Some(error(request_id, view_id, e.to_string())),
        };
        Some(reply(
            request_id,
            server_message::Body::Frame(Frame {
                view_id,
                config_version: open.config_version,
                load_version: window.load_version,
                first_row: viewport.first_row,
                first_column: viewport.first_column,
                total_rows: window.total_rows as u64,
                total_columns: window.total_columns as u32,
                batch,
                source_as_of_micros: state.source_as_of_micros,
                pushed,
            }),
        ))
    }

    /// Brings every view up to date with its table and pushes frames for the views
    /// that changed, along with a status message when a load happened or failed.
    pub fn refresh(&mut self) -> Vec<ServerMessage> {
        let mut out = Vec::new();
        let ids: Vec<u32> = self.views.keys().copied().collect();
        for id in ids {
            let open = self.views.get_mut(&id).expect("listed view");
            let table = Arc::clone(&open.table);
            let state = match read_table(&table) {
                Ok(state) => state,
                Err(message) => {
                    out.push(error(0, id, message));
                    continue;
                }
            };
            let current = state.table.load_version();
            let changed = match open.catch_up(&state) {
                Ok(changed) => changed,
                Err(message) => {
                    // Report a failed rebuild once per load, not on every refresh.
                    if open.broken_at != Some(current) {
                        open.broken_at = Some(current);
                        out.push(error(0, id, message));
                    }
                    false
                }
            };
            let error_now = state.refresh_error.as_ref().map(|(m, _)| m.clone());
            if open.status_version != current || open.status_error != error_now {
                open.status_version = current;
                open.status_error = error_now;
                out.push(reply(
                    0,
                    server_message::Body::TableStatus(state.status(id)),
                ));
            }
            drop(state);
            if changed {
                out.extend(self.frame(0, id, true));
            }
        }
        out
    }

    fn distinct_values(
        &mut self,
        request: &DistinctValues,
    ) -> Result<DistinctValuesResult, String> {
        let open = self
            .views
            .get(&request.view_id)
            .ok_or_else(|| format!("no view {}", request.view_id))?;
        let state = read_table(&open.table)?;
        // Other columns' filters apply, as in a spreadsheet's filter list; the
        // column's own filter does not, so its other values stay choosable.
        let mut filters: Vec<LayoutFilter> = open
            .layout
            .filters
            .iter()
            .filter(|f| f.column != request.column)
            .cloned()
            .collect();
        if !request.search.is_empty() {
            filters.push(LayoutFilter {
                column: request.column.clone(),
                op: FilterOpName::Contains,
                value: Some(serde_json::Value::String(request.search.clone())),
                values: Vec::new(),
            });
        }
        let layout = Layout {
            filters,
            group_by: vec![request.column.clone()],
            measures: vec![LayoutMeasure {
                column: None,
                aggregate: AggregateName::CountAll,
                percentile: None,
                weight: None,
                unit_column: None,
                show_as: ShowAsName::Value,
                format: None,
            }],
            // The values show as they do in the view.
            formats: open
                .layout
                .formats
                .get(&request.column)
                .map(|f| (request.column.clone(), f.clone()))
                .into_iter()
                .collect(),
            ..Layout::default()
        };
        let resolved = layout.resolve(&state.table).map_err(|e| e.to_string())?;
        let mut view = View::new(&state.table, resolved.config).map_err(|e| e.to_string())?;
        let total = view.num_rows(&state.table).saturating_sub(1);
        let limit = if request.limit == 0 {
            1_000
        } else {
            request.limit as usize
        };
        let window = view.window(&state.table, 1..limit.saturating_add(1), 0..1);
        let column = |name: &str| {
            window
                .batch
                .column_by_name(name)
                .cloned()
                .ok_or_else(|| format!("the distinct-values window has no {name} column"))
        };
        // Fixed names, so no source column name can collide with them.
        let out = RecordBatch::try_from_iter([
            ("value", column(&request.column)?),
            ("text", column("__label")?),
            ("count", column("count(*)")?),
        ])
        .map_err(|e| e.to_string())?;
        Ok(DistinctValuesResult {
            view_id: request.view_id,
            column: request.column.clone(),
            values: encode_batch(&out).map_err(|e| e.to_string())?,
            truncated: total > limit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ranges_saturate() {
        assert_eq!(index_range(5, 10), 5..15);
        assert_eq!(index_range(1, u64::MAX), 1..usize::MAX);
        assert_eq!(index_range(u64::MAX, 1), usize::MAX..usize::MAX);
    }
}
