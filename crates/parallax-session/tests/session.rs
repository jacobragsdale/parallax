//! The protocol conversation, checked message by message.

use parallax_protocol::ipc::decode_batches;
use parallax_protocol::v1::{ServerMessage, server_message::Body};
use parallax_session::testing::{fingerprint, transcript};
use prost::Message;

fn decoded() -> Vec<ServerMessage> {
    transcript()
        .iter()
        .map(|b| ServerMessage::decode(b.as_slice()).unwrap())
        .collect()
}

#[test]
fn the_conversation_answers_every_request() {
    let messages = decoded();
    let kinds: Vec<&str> = messages
        .iter()
        .map(|m| match m.body.as_ref().unwrap() {
            Body::Welcome(_) => "welcome",
            Body::ViewOpened(_) => "opened",
            Body::Frame(_) => "frame",
            Body::Error(_) => "error",
            Body::TableStatus(_) => "status",
            Body::DistinctValues(_) => "distinct",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "welcome", "opened", "frame", "frame", "frame", "distinct", "opened", "frame",
            // The reload: each open view gets its status, then a pushed frame.
            "status", "frame", "status", "frame", "error"
        ]
    );
}

#[test]
fn frames_carry_versions_and_decode() {
    let messages = decoded();
    let frames: Vec<_> = messages
        .iter()
        .filter_map(|m| match m.body.as_ref().unwrap() {
            Body::Frame(f) => Some(f.clone()),
            _ => None,
        })
        .collect();
    // Before the update the pivot runs config version 0; after it, version 2.
    assert_eq!(frames[0].config_version, 0);
    assert_eq!(frames[2].config_version, 2);
    assert!(
        frames
            .iter()
            .take(4)
            .all(|f| f.load_version == 1 && !f.pushed)
    );
    let pushed: Vec<_> = frames.iter().filter(|f| f.pushed).collect();
    assert_eq!(pushed.len(), 2);
    assert!(
        pushed
            .iter()
            .all(|f| f.load_version == 2 && f.source_as_of_micros == 1_900)
    );
    for f in &frames {
        let batches = decode_batches(&f.batch).unwrap();
        assert_eq!(batches.len(), 1);
        assert!(batches[0].num_rows() as u64 <= f.total_rows);
    }
    // The pivot shows the grand total first.
    let first = decode_batches(&frames[0].batch).unwrap().remove(0);
    let kind = first.column_by_name("__kind").unwrap();
    let kind = arrow_array::cast::AsArray::as_string::<i32>(kind.as_ref());
    assert_eq!(kind.value(0), "total");
}

#[test]
fn drill_through_opens_a_flat_view_of_the_group() {
    let messages = decoded();
    let opened: Vec<_> = messages
        .iter()
        .filter_map(|m| match m.body.as_ref().unwrap() {
            Body::ViewOpened(v) => Some(v.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(opened[1].view_id, 2);
    let layout: serde_json::Value = serde_json::from_str(&opened[1].layout_json).unwrap();
    let filters = layout["filters"].as_array().unwrap();
    // The view's own filter plus one equality filter per group level.
    assert_eq!(filters.len(), 3, "{layout}");
    assert_eq!(filters[1]["column"], "fund");
    assert_eq!(filters[2]["column"], "sector");
}

#[test]
fn distinct_values_respect_search_and_limit() {
    let messages = decoded();
    let result = messages
        .iter()
        .find_map(|m| match m.body.as_ref().unwrap() {
            Body::DistinctValues(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    let batch = decode_batches(&result.values).unwrap().remove(0);
    assert!(batch.num_rows() <= 25);
    let tickers = arrow_array::cast::AsArray::as_string::<i32>(batch.column(0).as_ref());
    for t in tickers.iter().flatten() {
        assert!(t.to_lowercase().contains('a'), "{t}");
    }
}

#[test]
fn errors_name_the_problem() {
    let messages = decoded();
    let Body::Error(e) = messages.last().unwrap().body.as_ref().unwrap() else {
        panic!("last message is an error");
    };
    assert!(e.message.contains("no_such_column"), "{}", e.message);
}

/// The WebAssembly test suite checks the same value, so both builds must answer
/// byte for byte the same.
pub const TRANSCRIPT_FINGERPRINT: u64 = 11_268_558_718_760_388_314;

#[test]
fn transcript_fingerprint_is_stable() {
    assert_eq!(fingerprint(&transcript()), TRANSCRIPT_FINGERPRINT);
}

mod races {
    use std::sync::{Arc, RwLock};

    use arrow_array::cast::AsArray;
    use arrow_array::types::Int64Type;
    use parallax_datagen::Dataset;
    use parallax_datagen::positions::{Positions, PositionsOptions};
    use parallax_engine::{Table, TableOptions, View, ViewConfig};
    use parallax_protocol::ipc::decode_batches;
    use parallax_protocol::v1::{
        ClientMessage, DrillThrough, OpenView, SetViewport, client_message, server_message::Body,
    };
    use parallax_session::{MemoryCatalog, Session, TableState};

    fn positions(rows: u64, seed: u64) -> arrow_array::RecordBatch {
        Positions::new(PositionsOptions {
            rows,
            seed,
            securities: 300,
            hostile: true,
            ..PositionsOptions::default()
        })
        .unwrap()
        .batch(0..rows)
    }

    fn setup(rows: u64) -> (Arc<RwLock<TableState>>, Session) {
        let batch = positions(rows, 21);
        let mut table = Table::new(
            &batch.schema(),
            TableOptions {
                primary_key: Some("position_id".into()),
                ..TableOptions::default()
            },
        )
        .unwrap();
        table.load(&[batch]).unwrap();
        let shared = Arc::new(RwLock::new(TableState::new(table, 1, 1)));
        let catalog = Arc::new(MemoryCatalog::new());
        catalog.insert("positions", Arc::clone(&shared));
        (shared, Session::new(catalog))
    }

    fn ask(session: &mut Session, body: client_message::Body) -> Vec<Body> {
        session
            .handle(ClientMessage {
                request_id: 1,
                body: Some(body),
            })
            .into_iter()
            .map(|m| m.body.unwrap())
            .collect()
    }

    fn viewport(view_id: u32) -> client_message::Body {
        client_message::Body::SetViewport(SetViewport {
            view_id,
            first_row: 0,
            row_count: 200,
            first_column: 0,
            column_count: 50,
        })
    }

    #[test]
    fn a_viewport_after_an_unseen_reload_reflects_the_new_load() {
        let (shared, mut session) = setup(1_000);
        let layout = r#"{"version":1,"sort":[{"column":"market_value_usd","descending":true}]}"#;
        ask(
            &mut session,
            client_message::Body::OpenView(OpenView {
                view_id: 1,
                dataset: "positions".into(),
                parameters: Default::default(),
                layout_json: layout.into(),
            }),
        );
        ask(&mut session, viewport(1));
        // A reload the session has not been told about: fewer rows, reused slots.
        shared
            .write()
            .unwrap()
            .reload(&[positions(600, 22)], 2, 2)
            .unwrap();
        let replies = ask(&mut session, viewport(1));
        let Body::Frame(frame) = &replies[0] else {
            panic!("{replies:?}")
        };
        assert_eq!(frame.load_version, 2);
        assert_eq!(frame.total_rows, 600);
        // The rows are exactly those of a view built fresh on the new table.
        let state = shared.read().unwrap();
        let mut fresh = View::new(
            &state.table,
            ViewConfig {
                sort: vec![parallax_engine::view::Sort {
                    column: "market_value_usd".into(),
                    descending: true,
                }],
                ..ViewConfig::default()
            },
        )
        .unwrap();
        let expected = fresh.window(&state.table, 0..200, 0..50).batch;
        let got = decode_batches(&frame.batch).unwrap().remove(0);
        let ids = |b: &arrow_array::RecordBatch| -> Vec<i64> {
            b.column_by_name("position_id")
                .unwrap()
                .as_primitive::<Int64Type>()
                .values()
                .to_vec()
        };
        assert_eq!(ids(&got), ids(&expected));
    }

    #[test]
    fn drilling_into_an_infinite_float_group_works() {
        use arrow_array::{Float64Array, Int64Array, RecordBatch};
        let batch = RecordBatch::try_from_iter([
            ("id", Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5])) as _),
            (
                "x",
                Arc::new(Float64Array::from(vec![
                    f64::INFINITY,
                    1.0,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    2.0,
                ])) as _,
            ),
        ])
        .unwrap();
        let mut table = Table::new(&batch.schema(), TableOptions::default()).unwrap();
        table.load(&[batch]).unwrap();
        let catalog = Arc::new(MemoryCatalog::new());
        catalog.insert("t", Arc::new(RwLock::new(TableState::new(table, 1, 1))));
        let mut session = Session::new(catalog);
        let layout = r#"{"version":1,"group_by":["x"],"measures":[{"aggregate":"count_all"}],
            "group_sort":{"by":"key","descending":true}}"#;
        ask(
            &mut session,
            client_message::Body::OpenView(OpenView {
                view_id: 1,
                dataset: "t".into(),
                parameters: Default::default(),
                layout_json: layout.into(),
            }),
        );
        // Descending key order puts +inf first, right after the grand total.
        let replies = ask(
            &mut session,
            client_message::Body::DrillThrough(DrillThrough {
                view_id: 1,
                row: 1,
                new_view_id: 2,
            }),
        );
        let Body::ViewOpened(opened) = &replies[0] else {
            panic!("drill-through failed: {replies:?}")
        };
        assert!(
            opened.layout_json.contains(r#""value":"inf""#),
            "{}",
            opened.layout_json
        );
        let replies = ask(&mut session, viewport(2));
        let Body::Frame(frame) = &replies[0] else {
            panic!("{replies:?}")
        };
        assert_eq!(frame.total_rows, 2, "the two rows with x = +inf");
    }
}
