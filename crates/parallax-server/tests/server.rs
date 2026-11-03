//! The server over a real socket: protocol messages, pushed refreshes, invalidation
//! and the connection context.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parallax_engine::DuplicatePolicy;
use parallax_protocol::v1::{
    ClientMessage, Hello, OpenView, ServerMessage, SetViewport, client_message,
    server_message::Body,
};
use parallax_server::sources::{GeneratedKind, Generator};
use parallax_server::{Dataset, RefreshPolicy, Registry, router};
use parallax_session::{Catalog, ConnectionContext};
use prost::Message;
use tokio_tungstenite::tungstenite::Message as Ws;

fn positions_dataset(rows: u64) -> Dataset {
    Dataset {
        name: "positions".into(),
        parameters: vec!["seed".into()],
        source: Box::new(Generator::new(GeneratedKind::Positions, rows).drifting()),
        primary_key: Some("position_id".into()),
        duplicates: DuplicatePolicy::Error,
        refresh: RefreshPolicy::default(),
        resolve: None,
        max_tables: 4,
    }
}

const PIVOT: &str = r#"{"version":1,"group_by":["fund"],
"measures":[{"column":"market_value_usd","aggregate":"sum"},{"aggregate":"count_all"}]}"#;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Socket, request_id: u64, body: client_message::Body) {
    let bytes = ClientMessage {
        request_id,
        body: Some(body),
    }
    .encode_to_vec();
    ws.send(Ws::Binary(bytes.into())).await.unwrap();
}

async fn receive(ws: &mut Socket) -> ServerMessage {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("a reply in time")
            .expect("open socket")
            .expect("valid frame");
        if let Ws::Binary(bytes) = message {
            return ServerMessage::decode(bytes.as_ref()).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_get_frames_and_pushed_refreshes() {
    let registry = Arc::new(Registry::new(vec![positions_dataset(3_000)]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, router(Arc::clone(&registry))).into_future());
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .unwrap();

    send(
        &mut ws,
        1,
        client_message::Body::Hello(Hello {
            protocol_version: 1,
        }),
    )
    .await;
    let Some(Body::Welcome(welcome)) = receive(&mut ws).await.body else {
        panic!("expected a welcome")
    };
    assert_eq!(welcome.datasets[0].name, "positions");
    assert_eq!(welcome.datasets[0].parameters, vec!["seed".to_owned()]);

    send(
        &mut ws,
        2,
        client_message::Body::OpenView(OpenView {
            view_id: 7,
            dataset: "positions".into(),
            parameters: Default::default(),
            layout_json: PIVOT.into(),
        }),
    )
    .await;
    let reply = receive(&mut ws).await;
    assert_eq!(reply.request_id, 2);
    let Some(Body::ViewOpened(opened)) = reply.body else {
        panic!("expected the view to open: {reply:?}")
    };
    assert_eq!(opened.status.unwrap().rows, 3_000);

    send(
        &mut ws,
        3,
        client_message::Body::SetViewport(SetViewport {
            view_id: 7,
            first_row: 0,
            row_count: 100,
            first_column: 0,
            column_count: 10,
        }),
    )
    .await;
    let Some(Body::Frame(first)) = receive(&mut ws).await.body else {
        panic!("expected a frame")
    };
    assert_eq!((first.load_version, first.pushed), (1, false));

    // A publisher says the data changed: the drifting generator returns new values,
    // the table reloads as a diff, and the open view is pushed a fresh frame.
    let reloaded = tokio::task::spawn_blocking({
        let registry = Arc::clone(&registry);
        move || registry.invalidate("positions")
    })
    .await
    .unwrap();
    assert_eq!(reloaded, Some(1));
    let Some(Body::TableStatus(status)) = receive(&mut ws).await.body else {
        panic!("expected a status")
    };
    assert_eq!((status.view_id, status.load_version), (7, 2));
    let Some(Body::Frame(pushed)) = receive(&mut ws).await.body else {
        panic!("expected a pushed frame")
    };
    assert_eq!((pushed.load_version, pushed.pushed), (2, true));
    assert_ne!(pushed.batch, first.batch, "new values reached the client");
}

#[tokio::test]
async fn the_invalidate_endpoint_reloads_cached_tables() {
    use tower::ServiceExt;
    let registry = Arc::new(Registry::new(vec![positions_dataset(500)]));
    let context: ConnectionContext = Arc::new(());
    let table = registry
        .open("positions", &BTreeMap::new(), &context)
        .unwrap();
    let response = router(Arc::clone(&registry))
        .oneshot(
            axum::http::Request::post("/invalidate/positions")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"1");
    assert_eq!(table.read().unwrap().table.load_version(), 2);
}

#[test]
fn the_connection_context_reaches_dataset_code_and_separates_tables() {
    let mut dataset = positions_dataset(200);
    dataset.resolve = Some(Arc::new(|parameters, context| {
        let user = context
            .downcast_ref::<String>()
            .ok_or_else(|| "no signed-in user".to_owned())?;
        let mut resolved = parameters.clone();
        resolved.insert("user".into(), user.clone());
        // A real host would also map the user to a fund here. The seed stands in.
        resolved.insert("seed".into(), (user.len() as u64).to_string());
        resolved.remove("user");
        Ok(resolved)
    }));
    let registry = Registry::new(vec![dataset]);
    let ana: ConnectionContext = Arc::new("ana".to_owned());
    let benjamin: ConnectionContext = Arc::new("benjamin".to_owned());
    let a = registry.open("positions", &BTreeMap::new(), &ana).unwrap();
    let b = registry
        .open("positions", &BTreeMap::new(), &benjamin)
        .unwrap();
    let a_again = registry.open("positions", &BTreeMap::new(), &ana).unwrap();
    assert!(
        Arc::ptr_eq(&a, &a_again),
        "the same resolved parameters share one table"
    );
    assert!(
        !Arc::ptr_eq(&a, &b),
        "different resolved parameters get different tables"
    );
    let anonymous: ConnectionContext = Arc::new(());
    assert!(
        registry
            .open("positions", &BTreeMap::new(), &anonymous)
            .is_err()
    );
    let mut unknown = BTreeMap::new();
    unknown.insert("fund".to_owned(), "x".to_owned());
    assert!(
        registry
            .open("positions", &unknown, &ana)
            .unwrap_err()
            .contains("no parameter fund")
    );
}

#[tokio::test]
async fn invalidating_an_unknown_dataset_is_not_found() {
    use tower::ServiceExt;
    let registry = Arc::new(Registry::new(vec![positions_dataset(10)]));
    let response = router(registry)
        .oneshot(
            axum::http::Request::post("/invalidate/postions")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}

#[test]
fn concurrent_first_opens_share_one_table() {
    let registry = Arc::new(Registry::new(vec![positions_dataset(2_000)]));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || {
                let context: ConnectionContext = Arc::new(());
                registry
                    .open("positions", &BTreeMap::new(), &context)
                    .unwrap()
            })
        })
        .collect();
    let tables: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(tables.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
    // One load, not four: the table is still at its first load version.
    assert_eq!(tables[0].read().unwrap().table.load_version(), 1);
}

#[test]
fn parameter_combinations_are_capped() {
    let registry = Registry::new(vec![positions_dataset(50)]);
    let context: ConnectionContext = Arc::new(());
    for seed in 0..4 {
        let mut p = BTreeMap::new();
        p.insert("seed".to_owned(), seed.to_string());
        registry.open("positions", &p, &context).unwrap();
    }
    let mut p = BTreeMap::new();
    p.insert("seed".to_owned(), "99".to_owned());
    let err = registry.open("positions", &p, &context).unwrap_err();
    assert!(err.contains("limit"), "{err}");
}

#[test]
fn empty_sources_open_as_empty_tables() {
    let registry = Registry::new(vec![positions_dataset(0)]);
    let context: ConnectionContext = Arc::new(());
    let table = registry
        .open("positions", &BTreeMap::new(), &context)
        .unwrap();
    assert_eq!(table.read().unwrap().table.num_rows(), 0);
}

async fn export(body: String) -> (axum::http::StatusCode, axum::http::HeaderMap, Vec<u8>) {
    use tower::ServiceExt;
    let registry = Arc::new(Registry::new(vec![positions_dataset(2_000)]));
    let response = router(registry)
        .oneshot(
            axum::http::Request::post("/export")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

fn form(layout: &str, format: &str) -> String {
    let encode = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    format!("layout={}&format={format}", encode(layout))
}

#[tokio::test]
async fn the_export_endpoint_downloads_a_workbook() {
    let layout =
        r#"{"dataset":"positions","group_by":["fund"],"measures":[{"aggregate":"count_all"}]}"#;
    let (status, headers, body) = export(form(layout, "xlsx")).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .contains("spreadsheetml")
    );
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"positions-")
            && disposition.ends_with(".xlsx\""),
        "{disposition}"
    );
    assert_eq!(&body[..2], b"PK", "an xlsx file is a zip");
}

#[tokio::test]
async fn the_export_endpoint_downloads_csv_with_every_row() {
    let (status, headers, body) = export(form(
        r#"{"dataset":"positions","columns":["position_id","fund"]}"#,
        "csv",
    ))
    .await;
    assert_eq!(status, 200);
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/csv")
    );
    let text = String::from_utf8(body).unwrap();
    let text = text.trim_start_matches('\u{feff}');
    assert_eq!(text.lines().next(), Some("position_id,fund"));
    assert_eq!(text.lines().count(), 2_001);
}

#[tokio::test]
async fn bad_exports_say_why() {
    let (status, _, body) = export(form(r#"{"dataset":"positions"}"#, "pdf")).await;
    assert_eq!(status, 400);
    assert!(String::from_utf8_lossy(&body).contains("xlsx or csv"));
    let (status, _, body) = export(form(r#"{"group_by":["fund"]}"#, "xlsx")).await;
    assert_eq!(status, 400);
    assert!(String::from_utf8_lossy(&body).contains("dataset"));
    let (status, _, _) = export(form(r#"{"dataset":"nothing"}"#, "xlsx")).await;
    assert_eq!(status, 404);
    let (status, _, body) = export(form(
        r#"{"dataset":"positions","group_by":["no_such_column"]}"#,
        "xlsx",
    ))
    .await;
    assert_eq!(status, 422);
    assert!(String::from_utf8_lossy(&body).contains("no_such_column"));
}

#[tokio::test]
async fn a_refusal_for_a_page_posts_its_reason_to_the_parent_window() {
    let body = format!(
        "{}&token=abc",
        form(
            r#"{"dataset":"positions","group_by":["</script>"]}"#,
            "xlsx"
        )
    );
    let (status, headers, body) = export(body).await;
    assert_eq!(status, 422);
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let page = String::from_utf8(body).unwrap();
    assert!(page.contains("parent.postMessage("), "{page}");
    assert!(page.contains(r#""source":"parallax-export""#), "{page}");
    assert!(page.contains(r#""token":"abc""#), "{page}");
    // The reason cannot end the script early.
    assert_eq!(page.matches("</script>").count(), 1, "{page}");
}

/// Answers at once the first time and slowly afterwards, as a source under load.
struct SlowAfterFirst {
    fetches: std::sync::atomic::AtomicUsize,
}

impl parallax_server::Source for SlowAfterFirst {
    fn fetch(&self, _: &BTreeMap<String, String>) -> Result<parallax_server::Fetched, String> {
        if self
            .fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            > 0
        {
            std::thread::sleep(Duration::from_millis(1_500));
        }
        let batch = arrow_array::RecordBatch::try_new(
            Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
                "x",
                arrow_schema::DataType::Int64,
                false,
            )])),
            vec![Arc::new(arrow_array::Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        Ok(parallax_server::Fetched {
            schema: batch.schema(),
            batches: vec![batch],
            as_of_micros: 1,
        })
    }
}

#[test]
fn opening_a_table_does_not_wait_for_its_reload() {
    let registry = Arc::new(Registry::new(vec![Dataset {
        name: "slow".into(),
        parameters: Vec::new(),
        source: Box::new(SlowAfterFirst {
            fetches: std::sync::atomic::AtomicUsize::new(0),
        }),
        primary_key: None,
        duplicates: DuplicatePolicy::Error,
        refresh: RefreshPolicy::default(),
        resolve: None,
        max_tables: 4,
    }]));
    let context = parallax_session::no_context();
    let first = registry.open("slow", &BTreeMap::new(), &context).unwrap();
    let reloading = Arc::clone(&registry);
    let reload = std::thread::spawn(move || reloading.invalidate("slow"));
    std::thread::sleep(Duration::from_millis(200));
    let started = std::time::Instant::now();
    let again = registry.open("slow", &BTreeMap::new(), &context).unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the open waited {:?} for the reload",
        started.elapsed()
    );
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(reload.join().unwrap(), Some(1));
}
