//! `POST /export`: a view as an Excel workbook or a CSV file, downloaded.
//!
//! The body is a form with the layout as JSON in `layout` and `xlsx` or `csv` in
//! `format`, so a page can start the download with a plain form post and the
//! browser streams the file to disk instead of holding it in memory. The layout
//! names the dataset and its parameters. The export runs the layout through the
//! same engine as the screen, under the same read lock as a view, and is written
//! to a temporary file first, because an Excel workbook is a zip whose index comes
//! last.

use std::io::{Seek, SeekFrom};
use std::sync::Arc;

use axum::Extension;
use axum::Form;
use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use parallax_engine::Formatter;
use parallax_export::{ExportSource, write_csv, write_xlsx};
use parallax_protocol::layout::Layout;
use parallax_session::{Catalog, ConnectionContext, no_context, read_table};
use serde::Deserialize;
use tokio_util::io::ReaderStream;

use crate::registry::Registry;

/// What to export.
#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    /// The layout document, as JSON.
    pub layout: String,
    /// `xlsx` or `csv`.
    pub format: String,
}

fn failure(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

/// A file name for the download: the dataset, the time, and the extension.
fn file_name(dataset: &str, extension: &str) -> String {
    let safe: String = dataset
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let now = jiff::Zoned::now().with_time_zone(
        jiff::tz::TimeZone::get(parallax_engine::format::DEFAULT_ZONE)
            .unwrap_or(jiff::tz::TimeZone::UTC),
    );
    format!("{safe}-{}.{extension}", now.strftime("%Y-%m-%d-%H%M"))
}

pub(crate) async fn export(
    State(registry): State<Arc<Registry>>,
    context: Option<Extension<ConnectionContext>>,
    Form(request): Form<ExportRequest>,
) -> Response {
    let context = context.map_or_else(no_context, |Extension(c)| c);
    let (extension, content_type) = match request.format.as_str() {
        "xlsx" => (
            "xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
        "csv" => ("csv", "text/csv; charset=utf-8"),
        other => {
            return failure(
                StatusCode::BAD_REQUEST,
                format!("unknown export format {other}; use xlsx or csv"),
            );
        }
    };
    let layout = match Layout::from_json(&request.layout) {
        Ok(layout) => layout,
        Err(e) => return failure(StatusCode::BAD_REQUEST, e.to_string()),
    };
    if layout.dataset.is_empty() {
        return failure(StatusCode::BAD_REQUEST, "the layout must name its dataset");
    }
    let name = file_name(&layout.dataset, extension);
    let work =
        tokio::task::spawn_blocking(move || -> Result<std::fs::File, (StatusCode, String)> {
            let catalog: Arc<dyn Catalog> = registry;
            let table = catalog
                .open(&layout.dataset, &layout.parameters, &context)
                .map_err(|e| (StatusCode::NOT_FOUND, e))?;
            let state = read_table(&table).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            let mut file = tempfile::tempfile()
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let as_of = Formatter::default().timestamp(state.source_as_of_micros(), true);
            let source = ExportSource {
                dataset: layout.dataset.clone(),
                as_of: format!("{as_of} New York"),
            };
            let written = if extension == "xlsx" {
                write_xlsx(&state.table, &layout, &source, &mut file)
            } else {
                write_csv(&state.table, &layout, std::io::BufWriter::new(&mut file))
            };
            written.map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
            file.seek(SeekFrom::Start(0))
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Ok(file)
        });
    let file = match work.await {
        Ok(Ok(file)) => file,
        Ok(Err((status, message))) => return failure(status, message),
        Err(_) => return failure(StatusCode::INTERNAL_SERVER_ERROR, "the export failed"),
    };
    let body = Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(file)));
    (
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response()
}
