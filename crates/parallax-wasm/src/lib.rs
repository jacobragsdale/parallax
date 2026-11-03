//! The parallax engine for the browser.
//!
//! A [`Worker`] hosts tables and one protocol session inside a Web Worker. The
//! page sends it the same protobuf messages it would send the server over a
//! WebSocket, and gets the same replies, so a dataset can live in the browser or on
//! the server without the UI noticing. The worker never reads a clock; callers pass
//! times in.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use parallax_engine::TableOptions;
use parallax_protocol::ipc::decode_stream;
use parallax_session::{MemoryCatalog, Session, TableState};
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

fn to_array(messages: Vec<Vec<u8>>) -> js_sys::Array {
    messages
        .into_iter()
        .map(|m| JsValue::from(js_sys::Uint8Array::from(m.as_slice())))
        .collect()
}
