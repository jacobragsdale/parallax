//! A fixed protocol conversation, replayed by the native and WebAssembly test suites.
//! Both must produce the same bytes.

use std::sync::{Arc, RwLock};

use parallax_datagen::Dataset;
use parallax_datagen::positions::{Positions, PositionsOptions};
use parallax_engine::{Table, TableOptions};
use parallax_protocol::v1::{
    ClientMessage, CloseView, DistinctValues, DrillThrough, Hello, OpenView, SetExpanded,
    SetViewport, UpdateView, client_message,
};
use prost::Message;

use crate::{MemoryCatalog, Session, TableState};

fn positions(rows: u64, seed: u64) -> arrow_array::RecordBatch {
    Positions::new(PositionsOptions {
        rows,
        seed,
        securities: 400,
        hostile: true,
        ..PositionsOptions::default()
    })
    .expect("valid options")
    .batch(0..rows)
}

fn send(session: &mut Session, out: &mut Vec<Vec<u8>>, bytes: Vec<u8>) {
    out.extend(session.handle_bytes(&bytes));
}

fn message(request_id: u64, body: client_message::Body) -> Vec<u8> {
    ClientMessage {
        request_id,
        body: Some(body),
    }
    .encode_to_vec()
}

const PIVOT: &str = r#"{"version":1,"group_by":["fund","sector"],"split_by":["long_short"],
"measures":[{"column":"market_value_usd","aggregate":"sum"},
{"column":"beta","aggregate":"mean"},{"aggregate":"count_all"},
{"column":"market_value_usd","aggregate":"sum","show_as":"percent_of_grand_total"}],
"filters":[{"column":"asset_class","op":"in","values":["Equity","Bond"]}]}"#;

const RESORTED: &str = r#"{"version":1,"group_by":["fund","sector"],"split_by":["long_short"],
"measures":[{"column":"market_value_usd","aggregate":"sum"},
{"column":"beta","aggregate":"mean"},{"aggregate":"count_all"}],
"group_sort":{"by":{"measure":0},"descending":true},"expand_depth":1,
"filters":[{"column":"asset_class","op":"in","values":["Equity","Bond"]}]}"#;

/// Runs the conversation and returns every server message, encoded, in order.
///
/// # Panics
///
/// Panics if the generated data cannot be loaded, which would be a bug.
#[must_use]
pub fn transcript() -> Vec<Vec<u8>> {
    let first = positions(2_000, 7);
    let mut table = Table::new(
        &first.schema(),
        TableOptions {
            primary_key: Some("position_id".into()),
            ..TableOptions::default()
        },
    )
    .expect("schema");
    table.load(&[first]).expect("load");
    let shared = Arc::new(RwLock::new(TableState::new(table, 1_000, 900)));
    let catalog = Arc::new(MemoryCatalog::new());
    catalog.insert("positions", Arc::clone(&shared));
    let mut session = Session::new(catalog);
    let mut out = Vec::new();
    use client_message::Body as B;
    send(
        &mut session,
        &mut out,
        message(
            1,
            B::Hello(Hello {
                protocol_version: 1,
            }),
        ),
    );
    send(
        &mut session,
        &mut out,
        message(
            2,
            B::OpenView(OpenView {
                view_id: 1,
                dataset: "positions".into(),
                parameters: Default::default(),
                layout_json: PIVOT.into(),
            }),
        ),
    );
    send(
        &mut session,
        &mut out,
        message(
            3,
            B::SetViewport(SetViewport {
                view_id: 1,
                first_row: 0,
                row_count: 50,
                first_column: 0,
                column_count: 12,
            }),
        ),
    );
    let fund = shared
        .read()
        .expect("lock")
        .table
        .column(
            shared
                .read()
                .expect("lock")
                .table
                .column_index("fund")
                .expect("fund"),
        )
        .scalar(0);
    let parallax_engine::Scalar::Utf8(fund) = fund else {
        panic!("fund is text")
    };
    send(
        &mut session,
        &mut out,
        message(
            4,
            B::SetExpanded(SetExpanded {
                view_id: 1,
                path_json: serde_json::json!([fund]).to_string(),
                expanded: true,
            }),
        ),
    );
    send(
        &mut session,
        &mut out,
        message(
            5,
            B::UpdateView(UpdateView {
                view_id: 1,
                layout_json: RESORTED.into(),
                config_version: 2,
            }),
        ),
    );
    send(
        &mut session,
        &mut out,
        message(
            6,
            B::DistinctValues(DistinctValues {
                view_id: 1,
                column: "ticker".into(),
                search: "a".into(),
                limit: 25,
            }),
        ),
    );
    send(
        &mut session,
        &mut out,
        message(
            7,
            B::DrillThrough(DrillThrough {
                view_id: 1,
                row: 3,
                new_view_id: 2,
            }),
        ),
    );
    send(
        &mut session,
        &mut out,
        message(
            8,
            B::SetViewport(SetViewport {
                view_id: 2,
                first_row: 0,
                row_count: 20,
                first_column: 0,
                column_count: 40,
            }),
        ),
    );
    // A reload: different rows, applied to the open views as a diff.
    let second = positions(2_000, 8);
    shared
        .write()
        .expect("lock")
        .reload(&[second], 2_000, 1_900)
        .expect("reload");
    out.extend(session.refresh().iter().map(Message::encode_to_vec));
    send(
        &mut session,
        &mut out,
        message(9, B::CloseView(CloseView { view_id: 2 })),
    );
    // An invalid layout is answered with an error, not a panic.
    send(
        &mut session,
        &mut out,
        message(
            10,
            B::UpdateView(UpdateView {
                view_id: 1,
                layout_json: r#"{"version":1,"group_by":["no_such_column"]}"#.into(),
                config_version: 3,
            }),
        ),
    );
    out
}

/// FNV-1a over a list of messages, for comparing transcripts.
#[must_use]
pub fn fingerprint(messages: &[Vec<u8>]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for m in messages {
        for &b in (m.len() as u64).to_le_bytes().iter().chain(m) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}
