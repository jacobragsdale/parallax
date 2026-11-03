//! `GET /copy/{dataset}`: a copy of a table for the browser's engine.
//!
//! Only datasets marked `browser` are copied. The query string carries the
//! dataset's parameters, which resolve through the same catalog as a view, with
//! the request's connection context, so a copy is the very table a server view
//! of the same parameters would show that caller. The answer is an Arrow IPC
//! stream with the table's status in its schema metadata; see
//! `parallax_session`'s copy module. The router compresses it with zstd or gzip,
//! whichever the browser offers, and the browser decodes it natively.
//!
//! A copy is written once per load of a table, to a temporary file under one
//! read lock, so it never mixes two loads. Later requests for the same table
//! and load stream the same file, so many pages opening at once neither
//! re-encode the table nor hold up its reload. The cache follows the table
//! itself, not the request's parameters, so callers whose context resolves to
//! different tables never share a copy.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use parallax_session::{
    Catalog, ConnectionContext, SharedTable, TableState, no_context, read_table,
};
use tokio_util::io::ReaderStream;

use crate::registry::Registry;

/// The media type of an Arrow IPC stream.
pub const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

/// A written copy and the load it holds.
struct Written {
    table: Weak<RwLock<TableState>>,
    version: (u64, Option<i64>),
    file: tempfile::TempPath,
}

/// Copies written so far, one per table. A table's entry has its own lock, so
/// concurrent requests for one table wait for a single write.
#[derive(Default)]
pub(crate) struct CopyCache {
    entries: Mutex<HashMap<usize, Arc<Mutex<Option<Written>>>>>,
}

impl CopyCache {
    /// Opens the copy of `table`'s current load, writing it first if needed.
    fn open(&self, table: &SharedTable) -> Result<std::fs::File, String> {
        let entry = {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            // Forget the copies of tables the registry has dropped, skipping entries
            // being written. An entry's weak reference keeps its table's address
            // from being reused meanwhile, so the address identifies one table.
            entries.retain(|_, entry| match entry.try_lock() {
                Ok(written) => written.as_ref().is_none_or(|w| w.table.strong_count() > 0),
                Err(_) => true,
            });
            Arc::clone(entries.entry(Arc::as_ptr(table) as usize).or_default())
        };
        let mut written = entry.lock().unwrap_or_else(PoisonError::into_inner);
        let state = read_table(table)?;
        let version = state.copy_version();
        if let Some(w) = written.as_ref()
            && w.version == version
            && w.table.upgrade().is_some_and(|t| Arc::ptr_eq(&t, table))
        {
            return std::fs::File::open(&w.file).map_err(|e| e.to_string());
        }
        let file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        let mut out = std::io::BufWriter::new(file.as_file());
        state.write_copy(&mut out).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        drop(out);
        drop(state);
        let path = file.into_temp_path();
        let opened = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        *written = Some(Written {
            table: Arc::downgrade(table),
            version,
            file: path,
        });
        Ok(opened)
    }
}

pub(crate) async fn copy(
    State(registry): State<Arc<Registry>>,
    Path(dataset): Path<String>,
    Query(parameters): Query<BTreeMap<String, String>>,
    context: Option<Extension<ConnectionContext>>,
) -> Response {
    if !registry.has_dataset(&dataset) {
        return (StatusCode::NOT_FOUND, format!("unknown dataset {dataset}")).into_response();
    }
    if !registry.is_browser(&dataset) {
        return (
            StatusCode::FORBIDDEN,
            format!("{dataset} is not copied to browsers; open it on the server"),
        )
            .into_response();
    }
    if let Some(refusal) = registry.unknown_parameter(&dataset, &parameters) {
        return (StatusCode::BAD_REQUEST, refusal).into_response();
    }
    let context = context.map_or_else(no_context, |Extension(c)| c);
    let work =
        tokio::task::spawn_blocking(move || -> Result<std::fs::File, (StatusCode, String)> {
            // A refused or failed load, or the limit on cached tables.
            let table = Catalog::open(registry.as_ref(), &dataset, &parameters, &context)
                .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
            registry
                .copies()
                .open(&table)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
        });
    let file = match work.await {
        Ok(Ok(file)) => file,
        Ok(Err((status, message))) => return (status, message).into_response(),
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "the copy failed").into_response();
        }
    };
    let body = Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(file)));
    (
        [
            (header::CONTENT_TYPE, ARROW_STREAM),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}
