//! Copies of a table for another engine, such as the one in the browser.
//!
//! A copy is an Arrow IPC stream of the table's rows in insertion order, because
//! flat views break ties by it. Its schema metadata carries what the table's
//! status reports: when the table loaded, when the source was read, the NaNs the
//! load turned blank and a failed refresh. A view of the copy therefore shows the
//! same rows in the same order, with the same status, as a view of the original.

use std::io::Write;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{ArrowError, Schema};
use parallax_engine::TableOptions;
use parallax_protocol::ipc::decode_stream;
use serde_json::{Value, json};

use crate::TableState;

/// The schema metadata key that holds a copy's details.
pub const COPY_METADATA: &str = "parallax.copy";

/// Rows per batch in a copy.
const BATCH_ROWS: usize = 65_536;

fn integer(details: &Value, key: &str) -> Result<i64, String> {
    details[key]
        .as_i64()
        .ok_or_else(|| format!("the copy's {key} is not an integer"))
}

impl TableState {
    /// Writes the table as a copy that [`TableState::read_copy`] reads back.
    ///
    /// # Errors
    ///
    /// Returns an error if the stream cannot be written.
    pub fn write_copy(&self, out: impl Write) -> Result<(), ArrowError> {
        let table = &self.table;
        let mut slots: Vec<usize> = table.live_slots().collect();
        slots.sort_unstable_by_key(|&slot| table.seq(slot));
        let details = json!({
            "primary_key": table.key_column().map(|i| table.schema().field(i).name().clone()),
            "load_version": table.load_version(),
            "loaded_at_micros": self.loaded_at_micros,
            "source_as_of_micros": self.source_as_of_micros,
            "nans_to_null": self.nans_to_null,
            "refresh_error": self.refresh_error,
        });
        let schema = Arc::new(Schema::new_with_metadata(
            table.schema().fields().clone(),
            [(COPY_METADATA.to_owned(), details.to_string())],
        ));
        let mut writer = StreamWriter::try_new(out, &schema)?;
        for chunk in slots.chunks(BATCH_ROWS) {
            let columns = (0..schema.fields().len())
                .map(|i| table.column(i).to_arrow(chunk))
                .collect();
            writer.write(&RecordBatch::try_new(Arc::clone(&schema), columns)?)?;
        }
        writer.finish()
    }

    /// Reads a copy written by [`TableState::write_copy`].
    ///
    /// # Errors
    ///
    /// Returns a message if the bytes are not a copy or the table refuses its rows.
    pub fn read_copy(bytes: &[u8]) -> Result<Self, String> {
        let (schema, batches) =
            decode_stream(bytes).map_err(|e| format!("the copy is not an Arrow stream: {e}"))?;
        let details: Value = schema
            .metadata()
            .get(COPY_METADATA)
            .ok_or_else(|| "the stream is not a parallax copy".to_owned())
            .and_then(|text| {
                serde_json::from_str(text).map_err(|e| format!("the copy's details: {e}"))
            })?;
        let primary_key = match &details["primary_key"] {
            Value::Null => None,
            Value::String(name) => Some(name.clone()),
            _ => return Err("the copy's primary_key is not text".to_owned()),
        };
        let mut nans_to_null = Vec::new();
        for entry in details["nans_to_null"].as_array().into_iter().flatten() {
            let (Some(column), Some(count)) = (entry[0].as_str(), entry[1].as_u64()) else {
                return Err("the copy's nans_to_null is malformed".to_owned());
            };
            let count = usize::try_from(count).map_err(|e| e.to_string())?;
            nans_to_null.push((column.to_owned(), count));
        }
        let refresh_error = match &details["refresh_error"] {
            Value::Null => None,
            entry => match (entry[0].as_str(), entry[1].as_i64()) {
                (Some(message), Some(at)) => Some((message.to_owned(), at)),
                _ => return Err("the copy's refresh_error is malformed".to_owned()),
            },
        };
        let plain = Arc::new(Schema::new(schema.fields().clone()));
        let batches = batches
            .into_iter()
            .map(|batch| RecordBatch::try_new(Arc::clone(&plain), batch.columns().to_vec()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut state = Self::from_batches(
            &plain,
            &batches,
            TableOptions {
                primary_key,
                ..TableOptions::default()
            },
            integer(&details, "loaded_at_micros")?,
            integer(&details, "source_as_of_micros")?,
        )
        .map_err(|e| e.to_string())?;
        let load_version = details["load_version"]
            .as_u64()
            .ok_or_else(|| "the copy's load_version is not an integer".to_owned())?;
        state
            .table
            .continue_load_version(load_version)
            .map_err(|e| e.to_string())?;
        // The original's load already turned these NaNs blank, so the copy's own
        // load found none; its status reports the original's.
        state.nans_to_null = nans_to_null;
        state.refresh_error = refresh_error;
        Ok(state)
    }
}
