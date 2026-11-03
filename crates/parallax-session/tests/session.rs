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

/// The value and display-text columns of a frame, paired by their index metadata.
fn text_columns(
    batch: &arrow_array::RecordBatch,
) -> Vec<(String, arrow_array::ArrayRef, Vec<String>)> {
    use arrow_array::Array;
    let schema = batch.schema();
    let role = |i: usize| {
        schema
            .field(i)
            .metadata()
            .get("parallax.role")
            .cloned()
            .unwrap_or_default()
    };
    let index = |i: usize| schema.field(i).metadata().get("parallax.index").cloned();
    (0..schema.fields().len())
        .filter(|&i| role(i) == "value")
        .map(|i| {
            let text = (0..schema.fields().len())
                .find(|&j| role(j) == "text" && index(j) == index(i))
                .expect("every value has display text");
            let text = arrow_array::cast::AsArray::as_string::<i32>(batch.column(text).as_ref());
            assert_eq!(text.len(), batch.column(i).len());
            let title = schema.field(i).metadata()["parallax.title"].clone();
            (
                title,
                batch.column(i).clone(),
                text.iter().map(|t| t.unwrap().to_owned()).collect(),
            )
        })
        .collect()
}

#[test]
fn frames_carry_display_text_in_the_layouts_formats() {
    use arrow_array::Array;
    let messages = decoded();
    let frames: Vec<_> = messages
        .iter()
        .filter_map(|m| match m.body.as_ref().unwrap() {
            Body::Frame(f) => Some(f.clone()),
            _ => None,
        })
        .collect();
    let pivot = decode_batches(&frames[0].batch).unwrap().remove(0);
    let columns = text_columns(&pivot);
    assert!(!columns.is_empty());
    let mut seen = [false; 4];
    for (title, values, text) in &columns {
        for (row, t) in text.iter().enumerate() {
            if values.is_null(row) {
                continue;
            }
            if title.ends_with("% of total") {
                seen[0] = true;
                assert!(t.ends_with('%'), "{title}: {t}");
            } else if title.ends_with("sum(market_value_usd)") {
                seen[1] = true;
                // Millions, one decimal, negatives in parentheses.
                let body = t.trim_start_matches('(').trim_end_matches(')');
                let number = body
                    .strip_suffix('M')
                    .unwrap_or_else(|| panic!("{title}: {t}"));
                assert_eq!(
                    number.split('.').nth(1).map(str::len),
                    Some(1),
                    "{title}: {t}"
                );
                assert!(!t.contains('-'), "{title}: {t}");
            } else if title.ends_with("mean(beta)") && !t.contains('∞') {
                seen[2] = true;
                assert_eq!(t.split('.').nth(1).map(str::len), Some(3), "{title}: {t}");
            } else if title.ends_with("count(*)") {
                seen[3] = true;
                assert!(!t.contains('.'), "{title}: {t}");
            }
        }
    }
    assert_eq!(seen, [true; 4], "every measure was checked");
    // Column pivots are titled by their values in display form.
    assert!(
        columns
            .iter()
            .any(|(title, _, _)| title.starts_with("Long | ")),
        "{:?}",
        columns.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
    // The drill-through is a flat view; its dates show as dates.
    let flat = decode_batches(&frames[3].batch).unwrap().remove(0);
    let (_, _, dates) = text_columns(&flat)
        .into_iter()
        .find(|(title, _, _)| title == "as_of_date")
        .unwrap();
    assert!(
        dates
            .iter()
            .all(|d| d.len() == 10 && d.as_bytes()[4] == b'-'),
        "{dates:?}"
    );
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
pub const TRANSCRIPT_FINGERPRINT: u64 = 7_641_691_102_802_052_240;

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

mod requests {
    use std::sync::{Arc, RwLock};

    use parallax_datagen::Dataset;
    use parallax_datagen::positions::{Positions, PositionsOptions};
    use parallax_engine::{Table, TableOptions};
    use parallax_protocol::ipc::decode_batches;
    use parallax_protocol::v1::{
        ClientMessage, FetchWindow, OpenView, SetExpanded, SetViewport, UpdateView,
        client_message::Body as C, server_message::Body,
    };
    use parallax_session::{MAX_FETCH_CELLS, MemoryCatalog, Session, TableState};

    fn session() -> Session {
        session_of(3_000)
    }

    fn session_of(rows: u64) -> Session {
        let batch = Positions::new(PositionsOptions {
            rows,
            seed: 5,
            securities: 200,
            ..PositionsOptions::default()
        })
        .unwrap()
        .batch(0..rows);
        let mut table = Table::new(&batch.schema(), TableOptions::default()).unwrap();
        table.load(&[batch]).unwrap();
        let catalog = Arc::new(MemoryCatalog::new());
        catalog.insert(
            "positions",
            Arc::new(RwLock::new(TableState::new(table, 1, 1))),
        );
        Session::new(catalog)
    }

    fn ask(session: &mut Session, request_id: u64, body: C) -> Vec<(u64, Body)> {
        session
            .handle(ClientMessage {
                request_id,
                body: Some(body),
            })
            .into_iter()
            .map(|m| (m.request_id, m.body.unwrap()))
            .collect()
    }

    fn open(session: &mut Session, layout: &str) -> Vec<(u64, Body)> {
        ask(
            session,
            1,
            C::OpenView(OpenView {
                view_id: 1,
                dataset: "positions".into(),
                parameters: Default::default(),
                layout_json: layout.into(),
            }),
        )
    }

    fn frame_rows(messages: &[(u64, Body)]) -> Option<(u64, usize)> {
        messages.iter().find_map(|(id, b)| match b {
            Body::Frame(f) => Some((*id, decode_batches(&f.batch).unwrap()[0].num_rows())),
            _ => None,
        })
    }

    #[test]
    fn a_fetched_window_does_not_move_the_viewport() {
        let mut s = session();
        open(&mut s, r#"{"columns":["position_id","fund"]}"#);
        let viewport = C::SetViewport(SetViewport {
            view_id: 1,
            first_row: 0,
            row_count: 10,
            first_column: 0,
            column_count: 2,
        });
        assert_eq!(frame_rows(&ask(&mut s, 2, viewport)), Some((2, 10)));
        let fetched = ask(
            &mut s,
            3,
            C::FetchWindow(FetchWindow {
                view_id: 1,
                first_row: 100,
                row_count: 500,
                first_column: 0,
                column_count: 2,
            }),
        );
        assert_eq!(frame_rows(&fetched), Some((3, 500)));
        // An update still answers with the viewport's ten rows.
        let updated = ask(
            &mut s,
            4,
            C::UpdateView(UpdateView {
                view_id: 1,
                layout_json: r#"{"columns":["position_id","fund"],"formats":{"position_id":{"thousands":false}}}"#.into(),
                config_version: 1,
            }),
        );
        assert_eq!(frame_rows(&updated), Some((4, 10)));
    }

    #[test]
    fn fetches_are_capped_after_clamping_to_the_view() {
        // 30,000 rows by 40 columns is 1.2 million cells.
        let mut s = session_of(30_000);
        open(&mut s, "{}");
        let fetch = |first_row: u64, row_count: u64| {
            C::FetchWindow(FetchWindow {
                view_id: 1,
                first_row,
                row_count,
                first_column: 0,
                column_count: u32::MAX,
            })
        };
        let rows = MAX_FETCH_CELLS / 40;
        assert!(matches!(
            ask(&mut s, 2, fetch(0, rows))[0].1,
            Body::Frame(_)
        ));
        let Body::Error(e) = &ask(&mut s, 3, fetch(0, rows + 1))[0].1 else {
            panic!("refused");
        };
        assert!(e.message.contains("cells"), "{}", e.message);
        // Past the end, the window is clamped first, so a huge count is fine.
        assert!(matches!(
            ask(&mut s, 4, fetch(29_000, u64::MAX))[0].1,
            Body::Frame(_)
        ));
    }

    #[test]
    fn a_format_change_keeps_expanded_groups() {
        let mut s = session();
        let layout = r#"{"group_by":["fund","strategy"],"measures":[{"column":"market_value_usd","aggregate":"sum"}]}"#;
        open(&mut s, layout);
        ask(
            &mut s,
            2,
            C::SetViewport(SetViewport {
                view_id: 1,
                first_row: 0,
                row_count: 100,
                first_column: 0,
                column_count: 5,
            }),
        );
        let collapsed = ask(
            &mut s,
            3,
            C::SetExpanded(SetExpanded {
                view_id: 1,
                path_json: "[\"Atlas Global Macro\"]".into(),
                expanded: false,
            }),
        );
        let (_, rows_collapsed) = frame_rows(&collapsed).unwrap();
        // The same layout with the collapse recorded and a new format: no rebuild,
        // so the group stays collapsed and the row count is unchanged.
        let reformatted = ask(
            &mut s,
            4,
            C::UpdateView(UpdateView {
                view_id: 1,
                layout_json: r#"{"group_by":["fund","strategy"],"measures":[{"column":"market_value_usd","aggregate":"sum","format":{"abbreviate":"millions"}}],"expanded":[{"path":["Atlas Global Macro"],"expanded":false}]}"#.into(),
                config_version: 1,
            }),
        );
        let (_, rows_after) = frame_rows(&reformatted).unwrap();
        assert_eq!(rows_after, rows_collapsed);
        let Body::Frame(f) = &reformatted[0].1 else {
            panic!("a frame");
        };
        let batch = &decode_batches(&f.batch).unwrap()[0];
        let text = batch.column_by_name("sum(market_value_usd)#text").unwrap();
        let text = arrow_array::cast::AsArray::as_string::<i32>(text.as_ref());
        assert!(text.value(0).ends_with('M'), "{}", text.value(0));
    }

    #[test]
    fn a_layout_for_another_dataset_is_refused() {
        let mut s = session();
        let answer = open(&mut s, r#"{"dataset":"trades"}"#);
        let Body::Error(e) = &answer[0].1 else {
            panic!("an error, got {answer:?}");
        };
        assert!(e.message.contains("trades"), "{}", e.message);
        assert!(matches!(
            open(&mut s, r#"{"dataset":"positions"}"#)[0].1,
            Body::ViewOpened(_)
        ));
    }
}
