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
