//! A server table's copy, loaded into the browser worker.

use std::collections::HashMap;

use parallax_datagen::Dataset;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_engine::TableOptions;
use parallax_protocol::v1::{
    ClientMessage, OpenView, ServerMessage, SetViewport, client_message, server_message::Body,
};
use parallax_session::TableState;
use parallax_wasm::Worker;
use prost::Message;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::wasm_bindgen_test;

fn copy() -> Vec<u8> {
    let batch = Positions::new(PositionsOptions {
        rows: 500,
        seed: 3,
        securities: 50,
        ..PositionsOptions::default()
    })
    .unwrap()
    .batch(0..500);
    let state = TableState::from_batches(
        &batch.schema(),
        &[batch],
        TableOptions::default(),
        1_000,
        900,
    )
    .unwrap();
    let mut bytes = Vec::new();
    state.write_copy(&mut bytes).unwrap();
    bytes
}

fn send(worker: &mut Worker, request_id: u64, body: client_message::Body) -> Vec<Body> {
    let bytes = ClientMessage {
        request_id,
        body: Some(body),
    }
    .encode_to_vec();
    worker
        .handle(&bytes)
        .iter()
        .map(|reply| {
            let reply: js_sys::Uint8Array = reply.dyn_into().unwrap();
            ServerMessage::decode(reply.to_vec().as_slice())
                .unwrap()
                .body
                .unwrap()
        })
        .collect()
}

fn open(worker: &mut Worker, view_id: u32, parameters: &[(&str, &str)]) -> Vec<Body> {
    let parameters: HashMap<String, String> = parameters
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let mut replies = send(
        worker,
        u64::from(view_id) * 2,
        client_message::Body::OpenView(OpenView {
            view_id,
            dataset: "positions".into(),
            parameters,
            layout_json:
                r#"{"version":1,"group_by":["fund"],"measures":[{"aggregate":"count_all"}]}"#.into(),
        }),
    );
    replies.extend(send(
        worker,
        u64::from(view_id) * 2 + 1,
        client_message::Body::SetViewport(SetViewport {
            view_id,
            first_row: 0,
            row_count: 50,
            first_column: 0,
            column_count: 10,
        }),
    ));
    replies
}

#[wasm_bindgen_test]
fn a_copy_opens_for_its_parameters_only() {
    let mut worker = Worker::new();
    let rows = worker
        .load_copy("positions", r#"{"as_of":"2026-11-02"}"#, &copy())
        .unwrap();
    assert_eq!(rows, 500);
    let replies = open(&mut worker, 1, &[("as_of", "2026-11-02")]);
    assert!(
        replies.iter().any(|r| matches!(r, Body::Frame(_))),
        "{replies:?}"
    );
    let opened = replies.iter().find_map(|r| match r {
        Body::ViewOpened(o) => o.status.clone(),
        _ => None,
    });
    assert_eq!(opened.unwrap().loaded_at_micros, 1_000);
    let other = open(&mut worker, 2, &[("as_of", "2026-11-03")]);
    assert!(
        other.iter().any(
            |r| matches!(r, Body::Error(e) if e.message.contains("not loaded for these parameters"))
        ),
        "{other:?}"
    );
}

#[wasm_bindgen_test]
fn bad_parameters_and_bytes_are_refused() {
    let mut worker = Worker::new();
    assert!(worker.load_copy("positions", "[1]", &copy()).is_err());
    assert!(worker.load_copy("positions", "{}", b"not arrow").is_err());
}
