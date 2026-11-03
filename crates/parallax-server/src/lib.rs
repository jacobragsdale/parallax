//! The parallax server: datasets, a table cache shared by every connection, and the
//! WebSocket protocol.
//!
//! A host embeds [`router`] in its own axum application and wraps it with its own
//! middleware. The library has no authentication, entitlement or audit code. To
//! pass something about the caller to dataset code, the host's middleware inserts a
//! [`ConnectionContext`](parallax_session::ConnectionContext) into the request's
//! extensions. Each WebSocket session then carries it into every dataset request.
//!
//! Routes:
//!
//! - `GET /ws` upgrades to the protocol: binary protobuf messages in both directions.
//! - `POST /invalidate/{dataset}` reloads every cached table of a dataset now.
//! - `POST /export` downloads a view as an Excel workbook or a CSV file; see
//!   [`export`].
//! - `GET /copy/{dataset}` sends a copy of a table for the browser's engine,
//!   compressed; see [`copy`].
//! - `GET /health` answers `ok`.

pub mod copy;
pub mod export;
pub mod registry;
pub mod sources;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Router};
use futures_util::{SinkExt, StreamExt};
use parallax_session::{Catalog, ConnectionContext, Session, no_context};
use prost::Message;

pub use registry::{Dataset, Fetched, RefreshPolicy, Registry, ResolveParameters, Source};

/// Builds the protocol routes over `registry`.
pub fn router(registry: Arc<Registry>) -> Router {
    Router::new()
        .route("/ws", get(upgrade))
        .route("/invalidate/{dataset}", post(invalidate))
        .route("/export", post(export::export))
        .route("/copy/{dataset}", get(copy::copy).layer(compression()))
        .route("/health", get(|| async { "ok" }))
        .with_state(registry)
}

/// The response compression the server uses: zstd or gzip, whichever the client
/// offers. `router` applies it to copies; a host can apply it to its own static
/// files, as the reference binary does.
#[must_use]
pub fn compression() -> tower_http::compression::CompressionLayer {
    tower_http::compression::CompressionLayer::new()
}

/// Reloads tables whose dataset asks for a scheduled refresh, every `tick`.
pub fn spawn_refresher(registry: Arc<Registry>, tick: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        loop {
            interval.tick().await;
            let registry = Arc::clone(&registry);
            let _ = tokio::task::spawn_blocking(move || registry.refresh_due()).await;
        }
    })
}

async fn invalidate(
    State(registry): State<Arc<Registry>>,
    Path(dataset): Path<String>,
) -> impl IntoResponse {
    let name = dataset.clone();
    match tokio::task::spawn_blocking(move || registry.invalidate(&dataset)).await {
        Ok(Some(reloaded)) => (StatusCode::OK, format!("{reloaded}")),
        Ok(None) => (StatusCode::NOT_FOUND, format!("unknown dataset {name}")),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("reloading {name} failed"),
        ),
    }
}

/// Runs `work` on the session off the async threads. Returns `None` if the work
/// panicked, which ends the connection: the session's state is lost with it.
async fn on_session<T: Send + 'static>(
    session: &mut Option<Session>,
    work: impl FnOnce(&mut Session) -> T + Send + 'static,
) -> Option<T> {
    let mut s = session.take()?;
    match tokio::task::spawn_blocking(move || {
        let out = work(&mut s);
        (s, out)
    })
    .await
    {
        Ok((s, out)) => {
            *session = Some(s);
            Some(out)
        }
        Err(e) => {
            tracing::error!(error = %e, "session task failed; closing the connection");
            None
        }
    }
}

async fn upgrade(
    State(registry): State<Arc<Registry>>,
    context: Option<Extension<ConnectionContext>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let context = context.map_or_else(no_context, |Extension(c)| c);
    ws.max_message_size(64 << 20)
        .on_upgrade(move |socket| serve(socket, registry, context))
}

/// Runs one connection: protocol messages from the client, and refreshes whenever
/// a table reloads.
async fn serve(socket: WebSocket, registry: Arc<Registry>, context: ConnectionContext) {
    let (mut sink, mut stream) = socket.split();
    let mut changes = registry.subscribe();
    let catalog: Arc<dyn Catalog> = Arc::clone(&registry) as Arc<dyn Catalog>;
    let mut session = Some(Session::with_context(catalog, context));
    loop {
        let replies: Option<Vec<Vec<u8>>> = tokio::select! {
            incoming = stream.next() => match incoming {
                Some(Ok(WsMessage::Binary(bytes))) => {
                    on_session(&mut session, move |s| s.handle_bytes(&bytes)).await
                }
                Some(Ok(WsMessage::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => Some(Vec::new()),
            },
            changed = changes.recv() => match changed {
                Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    on_session(&mut session, |s| {
                        s.refresh().iter().map(Message::encode_to_vec).collect()
                    })
                    .await
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
        };
        let Some(replies) = replies else {
            let _ = sink.send(WsMessage::Close(None)).await;
            return;
        };
        for reply in replies {
            if sink.send(WsMessage::Binary(reply.into())).await.is_err() {
                return;
            }
        }
    }
}
