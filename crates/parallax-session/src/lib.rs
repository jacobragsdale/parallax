//! Runs the parallax protocol over the engine, independent of transport.
//!
//! A [`Session`] belongs to one connection: a WebSocket on the server, or the
//! browser worker that hosts the WebAssembly engine. It turns client messages into
//! server messages. Tables come from a [`Catalog`] and are shared between sessions;
//! when a table reloads, [`Session::refresh`] brings each view up to date by
//! applying the change sets it missed and pushes fresh frames.
//!
//! The session never reads a clock. Times on tables are supplied by whoever loads
//! them, which keeps the code identical on the server and in WebAssembly.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, RwLock};

use parallax_engine::{ChangeSet, LoadError, LoadReport, Table, View, ViewConfig};
use parallax_protocol::PROTOCOL_VERSION;
use parallax_protocol::ipc::{encode_batch, encode_schema};
use parallax_protocol::layout::{Layout, ResolvedLayout};
use parallax_protocol::v1::{
    ClientMessage, ColumnCount, DatasetInfo, DistinctValuesResult, Error as ErrorMessage, Frame,
    ServerMessage, TableStatus, ViewOpened, Welcome, client_message, server_message,
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

    /// Replaces the table's contents. A refused load keeps the previous contents and
    /// records the error, so views keep showing the last good data.
    ///
    /// # Errors
    ///
    /// Returns the load error after recording it.
    pub fn reload(
        &mut self,
        batches: &[arrow_array::RecordBatch],
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
            .expect("catalog lock")
            .insert(name.to_owned(), table);
    }

    /// Returns a dataset's table.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<SharedTable> {
        self.tables.read().expect("catalog lock").get(name).cloned()
    }
}

impl Catalog for MemoryCatalog {
    fn datasets(&self) -> Vec<DatasetInfo> {
        self.tables
            .read()
            .expect("catalog lock")
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

struct OpenView {
    table: SharedTable,
    layout: Layout,
    view: View,
    config_version: u64,
    viewport: Option<Viewport>,
    status_version: u64,
    status_error: Option<String>,
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

/// Builds a view from a resolved layout and applies its expanded groups.
fn build_view(table: &Table, resolved: ResolvedLayout) -> Result<View, String> {
    let mut view = View::new(table, resolved.config).map_err(|e| e.to_string())?;
    for (path, expanded) in resolved.expanded {
        view.set_expanded(path, expanded);
    }
    Ok(view)
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
        match body {
            client_message::Body::Hello(_) => vec![reply(
                id,
                server_message::Body::Welcome(Welcome {
                    protocol_version: PROTOCOL_VERSION,
                    datasets: self.catalog.datasets(),
                }),
            )],
            client_message::Body::OpenView(open) => {
                let parameters: BTreeMap<String, String> = open.parameters.into_iter().collect();
                let opened = self
                    .catalog
                    .open(&open.dataset, &parameters, &self.context)
                    .and_then(|table| {
                        let layout =
                            Layout::from_json(&open.layout_json).map_err(|e| e.to_string())?;
                        self.open_view(id, open.view_id, table, layout)
                    });
                match opened {
                    Ok(messages) => messages,
                    Err(message) => vec![error(id, open.view_id, message)],
                }
            }
            client_message::Body::UpdateView(update) => {
                let Some(open) = self.views.get_mut(&update.view_id) else {
                    return vec![error(id, update.view_id, "no such view")];
                };
                let rebuilt = Layout::from_json(&update.layout_json)
                    .map_err(|e| e.to_string())
                    .and_then(|layout| {
                        let state = open.table.read().expect("table lock");
                        let resolved = layout.resolve(&state.table).map_err(|e| e.to_string())?;
                        Ok((layout, build_view(&state.table, resolved)?))
                    });
                match rebuilt {
                    Ok((layout, view)) => {
                        open.layout = layout;
                        open.view = view;
                        open.config_version = update.config_version;
                        self.frame(id, update.view_id, false).into_iter().collect()
                    }
                    Err(message) => vec![error(id, update.view_id, message)],
                }
            }
            client_message::Body::SetViewport(v) => {
                let Some(open) = self.views.get_mut(&v.view_id) else {
                    return vec![error(id, v.view_id, "no such view")];
                };
                open.viewport = Some(Viewport {
                    first_row: v.first_row,
                    row_count: v.row_count,
                    first_column: v.first_column,
                    column_count: v.column_count,
                });
                self.frame(id, v.view_id, false).into_iter().collect()
            }
            client_message::Body::SetExpanded(e) => {
                let Some(open) = self.views.get_mut(&e.view_id) else {
                    return vec![error(id, e.view_id, "no such view")];
                };
                let path: Result<Vec<serde_json::Value>, _> = serde_json::from_str(&e.path_json);
                let resolved = path.map_err(|err| err.to_string()).and_then(|path| {
                    let probe = Layout {
                        group_by: open.layout.group_by.clone(),
                        expanded: vec![parallax_protocol::layout::LayoutExpanded {
                            path: path.clone(),
                            expanded: e.expanded,
                        }],
                        ..Layout::default()
                    };
                    let state = open.table.read().expect("table lock");
                    probe
                        .resolve(&state.table)
                        .map(|r| (path, r.expanded))
                        .map_err(|err| err.to_string())
                });
                match resolved {
                    Ok((path_json, expanded)) => {
                        for (path, flag) in expanded {
                            open.view.set_expanded(path, flag);
                        }
                        open.layout.expanded.retain(|x| x.path != path_json);
                        open.layout
                            .expanded
                            .push(parallax_protocol::layout::LayoutExpanded {
                                path: path_json,
                                expanded: e.expanded,
                            });
                        self.frame(id, e.view_id, false).into_iter().collect()
                    }
                    Err(message) => vec![error(id, e.view_id, message)],
                }
            }
            client_message::Body::CloseView(c) => {
                self.views.remove(&c.view_id);
                Vec::new()
            }
            client_message::Body::DrillThrough(d) => {
                let Some(open) = self.views.get_mut(&d.view_id) else {
                    return vec![error(id, d.view_id, "no such view")];
                };
                let table = Arc::clone(&open.table);
                let config = {
                    let state = table.read().expect("table lock");
                    open.view.drill_through(&state.table, d.row as usize)
                };
                let Some(config) = config else {
                    return vec![error(
                        id,
                        d.view_id,
                        "that row has no single group to drill into",
                    )];
                };
                let layout = {
                    let state = table.read().expect("table lock");
                    Layout::from_flat_config(&config, &state.table)
                };
                match self.open_view(id, d.new_view_id, table, layout) {
                    Ok(messages) => messages,
                    Err(message) => vec![error(id, d.new_view_id, message)],
                }
            }
            client_message::Body::DistinctValues(request) => match self.distinct_values(&request) {
                Ok(result) => vec![reply(id, server_message::Body::DistinctValues(result))],
                Err(message) => vec![error(id, request.view_id, message)],
            },
        }
    }

    fn open_view(
        &mut self,
        request_id: u64,
        view_id: u32,
        table: SharedTable,
        layout: Layout,
    ) -> Result<Vec<ServerMessage>, String> {
        let (view, schema, status) = {
            let state = table.read().expect("table lock");
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
                layout,
                view,
                config_version: 0,
                viewport: None,
                status_version,
                status_error,
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
        let open = self.views.get_mut(&view_id)?;
        let viewport = open.viewport?;
        let state = open.table.read().expect("table lock");
        let rows = viewport.first_row as usize..(viewport.first_row + viewport.row_count) as usize;
        let columns = viewport.first_column as usize
            ..(viewport.first_column + viewport.column_count) as usize;
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
            let state = table.read().expect("table lock");
            let current = state.table.load_version();
            let mut changed = false;
            if open.view.load_version() < current {
                let missing: Vec<&Arc<ChangeSet>> = state
                    .history
                    .iter()
                    .filter(|c| c.load_version > open.view.load_version())
                    .collect();
                let contiguous = missing
                    .first()
                    .is_some_and(|c| c.load_version == open.view.load_version() + 1)
                    && missing.last().is_some_and(|c| c.load_version == current);
                if contiguous {
                    for change in missing {
                        open.view.apply(&state.table, change);
                    }
                } else {
                    // Too far behind for the kept history: rebuild from the layout.
                    match open
                        .layout
                        .resolve(&state.table)
                        .map_err(|e| e.to_string())
                        .and_then(|r| build_view(&state.table, r))
                    {
                        Ok(view) => open.view = view,
                        Err(message) => {
                            out.push(error(0, id, message));
                            continue;
                        }
                    }
                }
                changed = true;
            }
            let error_now = state.refresh_error.as_ref().map(|(m, _)| m.clone());
            if changed || open.status_version != current || open.status_error != error_now {
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
        request: &parallax_protocol::v1::DistinctValues,
    ) -> Result<DistinctValuesResult, String> {
        let open = self
            .views
            .get(&request.view_id)
            .ok_or_else(|| "no such view".to_owned())?;
        let state = open.table.read().expect("table lock");
        let mut layout = Layout {
            filters: open
                .layout
                .filters
                .iter()
                .filter(|f| f.column != request.column)
                .cloned()
                .collect(),
            group_by: vec![request.column.clone()],
            measures: vec![parallax_protocol::layout::LayoutMeasure {
                column: None,
                aggregate: parallax_protocol::layout::AggregateName::CountAll,
                percentile: None,
                weight: None,
                unit_column: None,
                show_as: parallax_protocol::layout::ShowAsName::Value,
            }],
            ..Layout::default()
        };
        if !request.search.is_empty() {
            layout
                .filters
                .push(parallax_protocol::layout::LayoutFilter {
                    column: request.column.clone(),
                    op: parallax_protocol::layout::FilterOpName::Contains,
                    value: Some(serde_json::Value::String(request.search.clone())),
                    values: Vec::new(),
                });
        }
        let resolved = layout.resolve(&state.table).map_err(|e| e.to_string())?;
        let config: ViewConfig = resolved.config;
        let mut view = View::new(&state.table, config).map_err(|e| e.to_string())?;
        let total = view.num_rows(&state.table).saturating_sub(1);
        let limit = if request.limit == 0 {
            1_000
        } else {
            request.limit as usize
        };
        let window = view.window(&state.table, 1..1 + limit, 0..1);
        // Keep the value column and the count, dropping the tree columns.
        let batch = window.batch;
        let value = batch
            .column_by_name(&request.column)
            .cloned()
            .ok_or_else(|| "missing value column".to_owned())?;
        let count = batch.column(batch.num_columns() - 2).clone();
        let out = arrow_array::RecordBatch::try_from_iter([
            (request.column.as_str(), value),
            ("count", count),
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
