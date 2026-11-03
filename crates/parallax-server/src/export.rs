//! `POST /export`: a view as an Excel workbook or a CSV file, downloaded.
//!
//! The body is a form with the layout as JSON in `layout` and `xlsx` or `csv` in
//! `format`, so a page can start the download with a plain form post and the
//! browser streams the file to disk instead of holding it in memory. The layout
//! names the dataset and its parameters. The export runs the layout through the
//! same engine as the screen, taking the table's read lock one chunk at a time so
//! that reloads are not held up, and is written to a temporary file first,
//! because an Excel workbook is a zip whose index comes last.
//!
//! A page posts into a hidden frame, which it cannot read when the server is on
//! another origin. When the form carries a `token`, a refusal therefore answers
//! with a small page that posts `{ source: "parallax-export", token, error }` to
//! the parent window.

use std::io::{Seek, SeekFrom};
use std::sync::{Arc, RwLockReadGuard};

use axum::Extension;
use axum::Form;
use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use parallax_engine::{Formatter, Table};
use parallax_export::{ExportSource, ReadTable, write_csv, write_xlsx};
use parallax_protocol::layout::Layout;
use parallax_session::{
    Catalog, ConnectionContext, SharedTable, TableState, no_context, read_table,
};
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
    /// Echoed in a refusal posted to the parent window.
    #[serde(default)]
    pub token: Option<String>,
}

/// A shared table, read one chunk at a time.
struct Shared(SharedTable);

/// A read lock on a shared table, seen as its table.
struct Guard<'a>(RwLockReadGuard<'a, TableState>);

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

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A refusal: plain text, or for a page's hidden frame, a page that posts the
/// reason to the parent window, which can read it across origins.
fn refusal(status: StatusCode, message: impl Into<String>, token: Option<&str>) -> Response {
    let message = message.into();
    let Some(token) = token else {
        return (status, message).into_response();
    };
    let payload =
        serde_json::json!({ "source": "parallax-export", "token": token, "error": message });
    // Inside a script element, "</" would end it early.
    let script = payload.to_string().replace("</", "<\\/");
    let page = format!(
        "<!doctype html><meta charset=utf-8><script>parent.postMessage({script}, \"*\");</script>{}",
        html_escape(&message)
    );
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        page,
    )
        .into_response()
}

/// A file name for the download: the dataset, the time in New York, and the
/// extension.
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
    let zone = jiff::tz::TimeZone::get(parallax_engine::format::DEFAULT_ZONE)
        .unwrap_or(jiff::tz::TimeZone::UTC);
    let now = jiff::Zoned::now().with_time_zone(zone);
    format!("{safe}-{}.{extension}", now.strftime("%Y-%m-%d-%H%M"))
}

pub(crate) async fn export(
    State(registry): State<Arc<Registry>>,
    context: Option<Extension<ConnectionContext>>,
    Form(request): Form<ExportRequest>,
) -> Response {
    let context = context.map_or_else(no_context, |Extension(c)| c);
    let token = request.token.as_deref();
    let (extension, content_type) = match request.format.as_str() {
        "xlsx" => (
            "xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
        "csv" => ("csv", "text/csv; charset=utf-8"),
        other => {
            return refusal(
                StatusCode::BAD_REQUEST,
                format!("unknown export format {other}; use xlsx or csv"),
                token,
            );
        }
    };
    let layout = match Layout::from_json(&request.layout) {
        Ok(layout) => layout,
        Err(e) => return refusal(StatusCode::BAD_REQUEST, e.to_string(), token),
    };
    if layout.dataset.is_empty() {
        return refusal(
            StatusCode::BAD_REQUEST,
            "the layout must name its dataset",
            token,
        );
    }
    if !registry.has_dataset(&layout.dataset) {
        return refusal(
            StatusCode::NOT_FOUND,
            format!("unknown dataset {}", layout.dataset),
            token,
        );
    }
    if let Some(refused) = registry.unknown_parameter(&layout.dataset, &layout.parameters) {
        return refusal(StatusCode::BAD_REQUEST, refused, token);
    }
    let name = file_name(&layout.dataset, extension);
    let work =
        tokio::task::spawn_blocking(move || -> Result<std::fs::File, (StatusCode, String)> {
            let catalog: Arc<dyn Catalog> = registry;
            // A refused or failed load, or the limit on cached tables.
            let table = catalog
                .open(&layout.dataset, &layout.parameters, &context)
                .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
            let as_of_micros = read_table(&table)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
                .source_as_of_micros();
            let as_of = Formatter::default().timestamp(as_of_micros, true);
            let source = ExportSource {
                dataset: layout.dataset.clone(),
                as_of: format!("{as_of} New York"),
            };
            let mut file = tempfile::tempfile()
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let shared = Shared(table);
            let written = if extension == "xlsx" {
                write_xlsx(&shared, &layout, &source, &mut file)
            } else {
                write_csv(&shared, &layout, std::io::BufWriter::new(&mut file))
            };
            written.map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
            file.seek(SeekFrom::Start(0))
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Ok(file)
        });
    let file = match work.await {
        Ok(Ok(file)) => file,
        Ok(Err((status, message))) => return refusal(status, message, token),
        Err(_) => {
            return refusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the export failed",
                token,
            );
        }
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
