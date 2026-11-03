//! A copy of a table answers every view exactly as the original does.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use arrow_array::cast::AsArray;
use arrow_array::types::Int64Type;
use arrow_array::{BooleanArray, RecordBatch};
use parallax_datagen::Dataset;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_engine::TableOptions;
use parallax_protocol::ipc::decode_batches;
use parallax_protocol::v1::{
    ClientMessage, DistinctValues, OpenView, ServerMessage, SetViewport, client_message,
    server_message::Body,
};
use parallax_session::{MemoryCatalog, Session, TableState};

fn positions(seed: u64) -> RecordBatch {
    Positions::new(PositionsOptions {
        rows: 3_000,
        seed,
        securities: 300,
        hostile: true,
        ..PositionsOptions::default()
    })
    .unwrap()
    .batch(0..3_000)
}

/// Keeps the rows whose position id `keep` accepts.
fn rows_where(batch: &RecordBatch, keep: impl Fn(i64) -> bool) -> RecordBatch {
    let ids = batch
        .column_by_name("position_id")
        .unwrap()
        .as_primitive::<Int64Type>();
    let mask: BooleanArray = ids.iter().map(|id| Some(keep(id.unwrap()))).collect();
    arrow_select::filter::filter_record_batch(batch, &mask).unwrap()
}

/// A table whose slots are no longer in insertion order: a third of the rows are
/// deleted, then come back as new rows in freed slots. Without a key, every
/// reload replaces every row, refilling the freed slots from the last.
fn reloaded_with(primary_key: Option<&str>) -> TableState {
    let first = positions(7);
    let mut state = TableState::from_batches(
        &first.schema(),
        std::slice::from_ref(&first),
        TableOptions {
            primary_key: primary_key.map(str::to_owned),
            ..TableOptions::default()
        },
        1_000,
        900,
    )
    .unwrap();
    state
        .reload(&[rows_where(&positions(8), |id| id % 3 != 0)], 2_000, 1_900)
        .unwrap();
    state.reload(&[positions(9)], 3_000, 2_900).unwrap();
    state.record_failure("the source timed out".into(), 3_500);
    state
}

fn reloaded() -> TableState {
    reloaded_with(Some("position_id"))
}

fn message(request_id: u64, body: client_message::Body) -> Vec<u8> {
    use prost::Message;
    ClientMessage {
        request_id,
        body: Some(body),
    }
    .encode_to_vec()
}

/// Opens `layout` on `state` and returns every reply, with the whole view in one
/// frame and the distinct values of `fund`.
fn answers(state: TableState, layout: &str) -> Vec<ServerMessage> {
    use prost::Message;
    let catalog = Arc::new(MemoryCatalog::new());
    catalog.insert("positions", Arc::new(RwLock::new(state)));
    let mut session = Session::new(catalog);
    use client_message::Body as B;
    let mut replies = Vec::new();
    for bytes in [
        message(
            1,
            B::OpenView(OpenView {
                view_id: 1,
                dataset: "positions".into(),
                parameters: HashMap::new(),
                layout_json: layout.into(),
            }),
        ),
        message(
            2,
            B::SetViewport(SetViewport {
                view_id: 1,
                first_row: 0,
                row_count: 10_000,
                first_column: 0,
                column_count: 200,
            }),
        ),
        message(
            3,
            B::DistinctValues(DistinctValues {
                view_id: 1,
                column: "fund".into(),
                search: String::new(),
                limit: 100,
            }),
        ),
    ] {
        replies.extend(
            session
                .handle_bytes(&bytes)
                .iter()
                .map(|b| ServerMessage::decode(b.as_slice()).unwrap()),
        );
    }
    replies
}

/// A reply in a form that compares exactly: every message as its debug text,
/// and a frame's columns as Arrow arrays, without the insertion-sequence column,
/// since the copy numbers its rows afresh in the same order.
#[derive(Debug, PartialEq)]
enum Reply {
    Frame(String, Vec<(arrow_schema::Field, arrow_array::ArrayRef)>),
    Other(String),
}

fn comparable(replies: Vec<ServerMessage>) -> Vec<Reply> {
    replies
        .into_iter()
        .map(|reply| match reply.body.unwrap() {
            Body::Frame(mut frame) => {
                let batch = std::mem::take(&mut frame.batch);
                let columns = decode_batches(&batch)
                    .unwrap()
                    .iter()
                    .flat_map(|b| {
                        b.schema()
                            .fields()
                            .iter()
                            .zip(b.columns())
                            .filter(|(field, _)| {
                                field.metadata().get("parallax.role").map(String::as_str)
                                    != Some("seq")
                            })
                            .map(|(field, column)| (field.as_ref().clone(), Arc::clone(column)))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                Reply::Frame(format!("{frame:?}"), columns)
            }
            Body::Error(error) => panic!("the view failed: {}", error.message),
            other => Reply::Other(format!("{other:?}")),
        })
        .collect()
}

const LAYOUTS: &[&str] = &[
    r#"{"version":1}"#,
    r#"{"version":1,"sort":[{"column":"fund"}],"filters":[{"column":"asset_class","op":"in","values":["Equity",null]}]}"#,
    r#"{"version":1,"group_by":["fund","sector"],"split_by":["long_short"],
       "measures":[{"column":"market_value_usd","aggregate":"sum"},{"column":"beta","aggregate":"mean"},
       {"aggregate":"count_all"},{"column":"market_value_usd","aggregate":"sum","show_as":"percent_of_grand_total"}],
       "expand_depth":2,"formats":{"market_value_usd":{"abbreviate":"millions","negative":"parentheses"}}}"#,
    r#"{"version":1,"group_by":["ticker"],"measures":[{"column":"daily_pnl_usd","aggregate":"sum"}],
       "group_sort":{"by":{"measure":0},"descending":true},"top_n":{"n":25,"measure":0}}"#,
];

#[test]
fn every_view_of_a_copy_matches_the_original() {
    // Flat views break ties by the key, or without one by insertion order.
    for key in [Some("position_id"), None] {
        let mut bytes = Vec::new();
        reloaded_with(key).write_copy(&mut bytes).unwrap();
        same_views(|| reloaded_with(key), &bytes);
    }
}

fn same_views(original: impl Fn() -> TableState, bytes: &[u8]) {
    for layout in LAYOUTS {
        let original = comparable(answers(original(), layout));
        let copy = comparable(answers(TableState::read_copy(bytes).unwrap(), layout));
        assert_eq!(original.len(), copy.len(), "{layout}");
        for (a, b) in original.iter().zip(&copy) {
            if let (Reply::Frame(_, a_columns), Reply::Frame(_, b_columns)) = (a, b) {
                for ((field, a), (_, b)) in a_columns.iter().zip(b_columns) {
                    assert!(a == b, "{layout}: column {} differs", field.name());
                }
            }
            assert!(a == b, "{layout}");
        }
        assert!(
            original
                .iter()
                .any(|reply| matches!(reply, Reply::Frame(..))),
            "{layout} drew nothing"
        );
    }
}

#[test]
fn the_copy_keeps_the_originals_status() {
    let original = reloaded();
    let mut bytes = Vec::new();
    original.write_copy(&mut bytes).unwrap();
    let copy = TableState::read_copy(&bytes).unwrap();
    assert_eq!(copy.table.load_version(), original.table.load_version());
    assert_eq!(copy.table.num_rows(), original.table.num_rows());
    assert_eq!(copy.source_as_of_micros(), 2_900);
    let status = |state: TableState| {
        answers(state, r#"{"version":1}"#)
            .into_iter()
            .find_map(|reply| match reply.body.unwrap() {
                Body::ViewOpened(opened) => opened.status,
                _ => None,
            })
            .unwrap()
    };
    let (a, b) = (status(original), status(copy));
    assert_eq!(a, b);
    assert_eq!(a.refresh_error, "the source timed out");
    assert!(
        a.nans_to_null.iter().any(|c| c.count > 0),
        "the hostile data has NaNs: {a:?}"
    );
}

#[test]
fn a_stream_that_is_not_a_copy_is_refused() {
    let plain = parallax_protocol::ipc::encode_batch(&positions(7)).unwrap();
    assert!(
        TableState::read_copy(&plain)
            .unwrap_err()
            .contains("not a parallax copy")
    );
    assert!(
        TableState::read_copy(b"not arrow")
            .unwrap_err()
            .contains("not an Arrow stream")
    );
}
