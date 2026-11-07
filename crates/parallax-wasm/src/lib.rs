//! The parallax engine for the browser.
//!
//! A [`Worker`] hosts tables and one protocol session inside a Web Worker. The
//! page sends it the same protobuf messages it would send the server over a
//! WebSocket, and gets the same replies, so a dataset can live in the browser or on
//! the server without the UI noticing. The worker never reads a clock; callers pass
//! times in.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use parallax_engine::format::Formatter;
use parallax_engine::{Table, TableOptions};
use parallax_export::{ExportSource, ReadTable, write_csv, write_xlsx};
use parallax_protocol::ipc::decode_stream;
use parallax_protocol::layout::Layout;
use parallax_session::{
    Catalog, MemoryCatalog, Session, SharedTable, TableState, no_context, read_table,
};
use wasm_bindgen::prelude::*;

/// Tables and a protocol session, for one page.
#[wasm_bindgen]
pub struct Worker {
    catalog: Arc<MemoryCatalog>,
    session: Session,
}

#[wasm_bindgen]
impl Worker {
    /// Creates a worker with no tables.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        let catalog = Arc::new(MemoryCatalog::new());
        let session = Session::new(Arc::clone(&catalog) as Arc<dyn parallax_session::Catalog>);
        Self { catalog, session }
    }

    /// Loads or reloads dataset `name` from an Arrow IPC stream. A reload of an
    /// existing dataset applies as a diff to its open views on the next
    /// [`Worker::refresh`].
    ///
    /// # Errors
    ///
    /// Returns an error if the stream is invalid or the load is refused.
    #[wasm_bindgen(js_name = loadTable)]
    pub fn load_table(
        &mut self,
        name: &str,
        arrow_ipc: &[u8],
        primary_key: Option<String>,
        now_micros: f64,
        source_as_of_micros: f64,
    ) -> Result<u64, JsError> {
        let (schema, batches) = decode_stream(arrow_ipc)?;
        let now = now_micros as i64;
        let as_of = source_as_of_micros as i64;
        if let Some(existing) = self.catalog.get(name) {
            let report = existing
                .write()
                .map_err(|_| JsError::new("the table is unavailable after an internal error"))?
                .reload(&batches, now, as_of)?;
            return Ok(report.rows as u64);
        }
        let state = TableState::from_batches(
            &schema,
            &batches,
            TableOptions {
                primary_key,
                ..TableOptions::default()
            },
            now,
            as_of,
        )?;
        let rows = state.table.num_rows() as u64;
        self.catalog.insert(name, Arc::new(RwLock::new(state)));
        Ok(rows)
    }

    /// Spike 0.3: loads a dataset as [`Worker::load_table`] does, but decodes the
    /// stream in place, so that its arrays point into the bytes rather than
    /// copying them, and frees the bytes as soon as the table holds the rows.
    ///
    /// # Errors
    ///
    /// Returns an error if the stream is invalid or the load is refused.
    #[wasm_bindgen(js_name = loadTableInPlace)]
    pub fn load_table_in_place(
        &mut self,
        name: &str,
        arrow_ipc: Vec<u8>,
        primary_key: Option<String>,
        now_micros: f64,
        source_as_of_micros: f64,
    ) -> Result<u64, JsError> {
        let mut buffer = arrow_buffer::Buffer::from_vec(arrow_ipc);
        let mut decoder = arrow_ipc::reader::StreamDecoder::new();
        let mut batches = Vec::new();
        while !buffer.is_empty() {
            if let Some(batch) = decoder.decode(&mut buffer)? {
                batches.push(batch);
            }
        }
        decoder.finish()?;
        let schema = decoder
            .schema()
            .ok_or_else(|| JsError::new("the stream has no schema"))?;
        let state = TableState::from_batches(
            &schema,
            &batches,
            TableOptions {
                primary_key,
                ..TableOptions::default()
            },
            now_micros as i64,
            source_as_of_micros as i64,
        )?;
        let rows = state.table.num_rows() as u64;
        self.catalog.insert(name, Arc::new(RwLock::new(state)));
        Ok(rows)
    }

    /// Loads a copy of a server table, as `GET /copy/{name}` sends it, for one set
    /// of parameters given as a JSON object of strings. Views of the copy show what
    /// views of the server's table show, status included. A second copy for the
    /// same name and parameters replaces the first for views opened afterwards.
    ///
    /// # Errors
    ///
    /// Returns an error if the parameters are not an object of strings, or the bytes
    /// are not a copy.
    #[wasm_bindgen(js_name = loadCopy)]
    pub fn load_copy(
        &mut self,
        name: &str,
        parameters_json: &str,
        copy: &[u8],
    ) -> Result<u64, JsError> {
        let parameters: BTreeMap<String, String> =
            serde_json::from_str(parameters_json).map_err(|e| {
                JsError::new(&format!("the parameters must be an object of strings: {e}"))
            })?;
        let state = TableState::read_copy(copy).map_err(|e| JsError::new(&e))?;
        let rows = state.table.num_rows() as u64;
        self.catalog
            .insert_with_parameters(name, parameters, Arc::new(RwLock::new(state)));
        Ok(rows)
    }

    /// Writes the layout's view as an Excel workbook, held in memory, and returns
    /// its bytes. The layout names its dataset and parameters; `load_version` is
    /// the load the screen shows, and an export of any other load is refused.
    ///
    /// # Errors
    ///
    /// Returns the reason the export cannot be made, as the server would.
    #[wasm_bindgen(js_name = exportXlsx)]
    pub fn export_xlsx(
        &self,
        layout_json: &str,
        load_version: Option<f64>,
    ) -> Result<Vec<u8>, JsError> {
        let (shared, layout, source) = self.export_source(layout_json, load_version)?;
        let mut out = std::io::Cursor::new(Vec::new());
        write_xlsx(&shared, &layout, &source, &mut out)?;
        Ok(out.into_inner())
    }

    /// Writes the layout's view as CSV and returns its bytes, as
    /// [`Worker::export_xlsx`] does for Excel.
    ///
    /// # Errors
    ///
    /// Returns the reason the export cannot be made, as the server would.
    #[wasm_bindgen(js_name = exportCsv)]
    pub fn export_csv(
        &self,
        layout_json: &str,
        load_version: Option<f64>,
    ) -> Result<Vec<u8>, JsError> {
        let (shared, layout, source) = self.export_source(layout_json, load_version)?;
        let mut out = Vec::new();
        write_csv(&shared, &layout, &source, &mut out)?;
        Ok(out)
    }

    fn export_source(
        &self,
        layout_json: &str,
        load_version: Option<f64>,
    ) -> Result<(Shared, Layout, ExportSource), JsError> {
        let layout = Layout::from_json(layout_json)?;
        let table = self
            .catalog
            .open(&layout.dataset, &layout.parameters, &no_context())
            .map_err(|e| JsError::new(&e))?;
        let as_of_micros = read_table(&table)
            .map_err(|e| JsError::new(&e))?
            .source_as_of_micros();
        let as_of = Formatter::default().timestamp(as_of_micros, true);
        let source = ExportSource {
            dataset: layout.dataset.clone(),
            as_of: format!("{as_of} New York"),
            load_version: load_version.map(|v| v as u64),
        };
        Ok((Shared(table), layout, source))
    }

    /// Handles one encoded client message and returns the encoded replies.
    #[must_use]
    pub fn handle(&mut self, message: &[u8]) -> js_sys::Array {
        to_array(self.session.handle_bytes(message))
    }

    /// Brings open views up to date with reloaded tables and returns the pushed
    /// messages.
    #[must_use]
    pub fn refresh(&mut self) -> js_sys::Array {
        use prost::Message;
        to_array(
            self.session
                .refresh()
                .iter()
                .map(Message::encode_to_vec)
                .collect(),
        )
    }
}

impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}

/// A table that exports read under its lock.
struct Shared(SharedTable);

/// A read lock on a shared table, seen as its table.
struct Guard<'a>(std::sync::RwLockReadGuard<'a, TableState>);

impl std::ops::Deref for Guard<'_> {
    type Target = Table;

    fn deref(&self) -> &Table {
        &self.0.table
    }
}

impl ReadTable for Shared {
    type Guard<'a> = Guard<'a>;

    fn read(&self) -> Result<Guard<'_>, String> {
        read_table(&self.0).map(Guard)
    }
}

/// Spike 0.3 only: generated positions as an Arrow IPC stream, the bytes an
/// app's API would send, so that the measurement page needs no server.
///
/// # Errors
///
/// Returns an error if the options are invalid or the stream cannot be written.
#[cfg(feature = "datagen")]
#[wasm_bindgen(js_name = generatePositions)]
pub fn generate_positions(rows: f64, seed: f64) -> Result<Vec<u8>, JsError> {
    use parallax_datagen::Dataset;
    use parallax_datagen::positions::{Positions, PositionsOptions};
    let rows = rows as u64;
    let dataset = Positions::new(PositionsOptions {
        rows,
        seed: seed as u64,
        ..PositionsOptions::default()
    })?;
    let mut out = Vec::new();
    let mut writer = arrow_ipc::writer::StreamWriter::try_new(&mut out, &dataset.schema())?;
    for range in parallax_datagen::batch_ranges(rows, 65_536) {
        writer.write(&dataset.batch(range))?;
    }
    writer.finish()?;
    drop(writer);
    Ok(out)
}

fn to_array(messages: Vec<Vec<u8>>) -> js_sys::Array {
    messages
        .into_iter()
        .map(|m| JsValue::from(js_sys::Uint8Array::from(m.as_slice())))
        .collect()
}
