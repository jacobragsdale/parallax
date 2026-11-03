//! `GET /copy/{dataset}`: a copy of a table for the browser's engine.
//!
//! Only datasets marked `browser` are copied. The query string carries the
//! dataset's parameters, which resolve through the same catalog as a view, so a
//! copy is the very table a server view of the same parameters would show. The
//! answer is an Arrow IPC stream with the table's status in its schema metadata;
//! see `parallax_session`'s copy module. The router compresses it with zstd or
//! gzip, whichever the browser offers, and the browser decodes it natively.
//!
//! The copy is written to a temporary file under one read lock, so it never mixes
//! two loads. A reload of the table waits for the write to finish.

use std::collections::BTreeMap;
use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use parallax_session::{Catalog, ConnectionContext, no_context, read_table};
use tokio_util::io::ReaderStream;

use crate::registry::Registry;

/// The media type of an Arrow IPC stream.
pub const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

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
    let context = context.map_or_else(no_context, |Extension(c)| c);
    let work =
        tokio::task::spawn_blocking(move || -> Result<std::fs::File, (StatusCode, String)> {
            let catalog: Arc<dyn Catalog> = registry;
            let table = catalog
                .open(&dataset, &parameters, &context)
                .map_err(|e| (StatusCode::NOT_FOUND, e))?;
            let internal = |e: String| (StatusCode::INTERNAL_SERVER_ERROR, e);
            let mut file = tempfile::tempfile().map_err(|e| internal(e.to_string()))?;
            let mut out = std::io::BufWriter::new(&mut file);
            read_table(&table)
                .map_err(internal)?
                .write_copy(&mut out)
                .map_err(|e| internal(e.to_string()))?;
            out.flush().map_err(|e| internal(e.to_string()))?;
            drop(out);
            file.seek(SeekFrom::Start(0))
                .map_err(|e| internal(e.to_string()))?;
            Ok(file)
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
