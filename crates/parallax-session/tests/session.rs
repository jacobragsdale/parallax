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
