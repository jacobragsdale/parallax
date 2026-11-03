//! Chart views in a session: updates, refusals and refreshes.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use parallax_datagen::Dataset;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_engine::TableOptions;
use parallax_protocol::v1::{
    ClientMessage, OpenView, ServerMessage, SetChartWindow, SetViewport, UpdateView,
    client_message, server_message::Body,
};
use parallax_session::{MemoryCatalog, Session, SharedTable, TableState};
use prost::Message;

fn positions(seed: u64) -> arrow_array::RecordBatch {
    Positions::new(PositionsOptions {
        rows: 1_000,
        seed,
        securities: 100,
        ..PositionsOptions::default()
    })
    .unwrap()
    .batch(0..1_000)
}

fn session() -> (Session, SharedTable) {
    let first = positions(1);
    let state = TableState::from_batches(
        &first.schema(),
        &[first],
        TableOptions {
            primary_key: Some("position_id".into()),
            ..TableOptions::default()
        },
        1_000,
        900,
    )
    .unwrap();
    let table = Arc::new(RwLock::new(state));
    let catalog = Arc::new(MemoryCatalog::new());
    catalog.insert("positions", Arc::clone(&table));
    (Session::new(catalog), table)
}

fn send(session: &mut Session, request_id: u64, body: client_message::Body) -> Vec<Body> {
    let bytes = ClientMessage {
        request_id,
        body: Some(body),
    }
    .encode_to_vec();
    session
        .handle_bytes(&bytes)
        .iter()
        .map(|b| ServerMessage::decode(b.as_slice()).unwrap().body.unwrap())
        .collect()
}

const HISTOGRAM: &str = r#"{"chart":{"kind":"histogram","x":"beta"}}"#;

fn open_chart(session: &mut Session) {
    send(
        session,
        1,
        client_message::Body::OpenView(OpenView {
            view_id: 1,
            dataset: "positions".into(),
            parameters: HashMap::new(),
            layout_json: HISTOGRAM.into(),
        }),
    );
    let replies = send(
        session,
        2,
        client_message::Body::SetChartWindow(SetChartWindow {
            view_id: 1,
            width: 400,
            height: 200,
            ..SetChartWindow::default()
        }),
    );
    assert!(
        matches!(replies.as_slice(), [Body::ChartFrame(_)]),
        "{replies:?}"
    );
}

fn update(session: &mut Session, request_id: u64, layout: &str) -> Vec<Body> {
    send(
        session,
        request_id,
        client_message::Body::UpdateView(UpdateView {
            view_id: 1,
            layout_json: layout.into(),
            config_version: request_id,
        }),
    )
}

#[test]
fn an_update_answers_with_the_chart_redrawn() {
    let (mut session, _) = session();
    open_chart(&mut session);
    // A display-only change keeps the reduction and still answers with a frame.
    let replies = update(
        &mut session,
        3,
        r#"{"chart":{"kind":"histogram","x":"beta","log_y":true},"formats":{"beta":{"decimals":3}}}"#,
    );
    let [Body::ChartFrame(frame)] = replies.as_slice() else {
        panic!("{replies:?}")
    };
    assert_eq!(frame.config_version, 3);
    // A change of column reduces again.
    let replies = update(
        &mut session,
        4,
        r#"{"chart":{"kind":"histogram","x":"market_value_usd"}}"#,
    );
    let [Body::ChartFrame(frame)] = replies.as_slice() else {
        panic!("{replies:?}")
    };
    assert_eq!(frame.series[0].column, "market_value_usd");
}

#[test]
fn a_chart_refuses_a_viewport_and_refreshes_without_errors() {
    let (mut session, table) = session();
    open_chart(&mut session);
    let replies = send(
        &mut session,
        3,
        client_message::Body::SetViewport(SetViewport {
            view_id: 1,
            first_row: 0,
            row_count: 10,
            first_column: 0,
            column_count: 5,
        }),
    );
    assert!(matches!(replies.as_slice(), [Body::Error(e)] if e.message.contains("is a chart")));
    table
        .write()
        .unwrap()
        .reload(&[positions(2)], 2_000, 1_900)
        .unwrap();
    let pushed: Vec<Body> = session
        .refresh()
        .into_iter()
        .map(|m| m.body.unwrap())
        .collect();
    assert!(
        matches!(pushed.as_slice(), [Body::TableStatus(_), Body::ChartFrame(f)] if f.pushed && f.load_version == 2),
        "{pushed:?}"
    );
    // A grid view turned into a chart forgets its viewport too.
    let replies = update(
        &mut session,
        4,
        r#"{"group_by":["fund"],"measures":[{"aggregate":"count_all"}]}"#,
    );
    assert!(replies.is_empty(), "no viewport yet: {replies:?}");
    send(
        &mut session,
        5,
        client_message::Body::SetViewport(SetViewport {
            view_id: 1,
            first_row: 0,
            row_count: 10,
            first_column: 0,
            column_count: 5,
        }),
    );
    // The new chart has no window yet, and no grid frame or error either.
    let replies = update(&mut session, 6, HISTOGRAM);
    assert!(replies.is_empty(), "{replies:?}");
    let replies = send(
        &mut session,
        7,
        client_message::Body::SetChartWindow(SetChartWindow {
            view_id: 1,
            width: 400,
            height: 200,
            ..SetChartWindow::default()
        }),
    );
    assert!(
        matches!(replies.as_slice(), [Body::ChartFrame(_)]),
        "{replies:?}"
    );
    table
        .write()
        .unwrap()
        .reload(&[positions(3)], 3_000, 2_900)
        .unwrap();
    let pushed = session.refresh();
    assert!(
        pushed
            .iter()
            .all(|m| !matches!(m.body, Some(Body::Error(_)))),
        "{pushed:?}"
    );
}
