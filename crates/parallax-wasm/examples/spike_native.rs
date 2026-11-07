//! Spike 0.3: the native half of the browser comparison.
//!
//! Generates the same positions the measurement page generates, with the same
//! function, loads them as the worker's `loadTable` does, replays the protocol
//! messages the page recorded, and writes the replies, so that they can be
//! compared byte for byte with the replies the browser's worker sent. It also
//! writes native exports of the given layouts, to compare with the browser's.
//!
//! ```sh
//! cargo run --release -p parallax-wasm --features datagen --example spike_native -- \
//!     ROWS OUT_DIR [--messages FILE] [--export NAME=LAYOUT_JSON ...]
//! ```
//!
//! Messages and replies are length-prefixed, a little-endian `u32` before each.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use parallax_engine::format::Formatter;
use parallax_engine::{Table, TableOptions};
use parallax_export::{ExportSource, ReadTable, write_csv, write_xlsx};
use parallax_protocol::ipc::decode_stream;
use parallax_protocol::layout::Layout;
use parallax_protocol::v1::{ServerMessage, server_message};
use parallax_session::{MemoryCatalog, Session, SharedTable, TableState, read_table};
use prost::Message;

/// The page's fixed load times.
const NOW_MICROS: i64 = 1_761_840_000_000_000;
const AS_OF_MICROS: i64 = 1_761_836_400_000_000;

struct Shared(SharedTable);
struct Guard<'a>(std::sync::RwLockReadGuard<'a, TableState>);

impl std::ops::Deref for Guard<'_> {
    type Target = Table;
    fn deref(&self) -> &Table {
        &self.0.table
    }
}

impl ReadTable for Shared {
    type Guard<'a> = Guard<'a>;
    fn read(&self) -> Result<Guard<'_>, String> {
        read_table(&self.0).map(Guard)
    }
}

fn split(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        out.push(&bytes[at + 4..at + 4 + len]);
        at += 4 + len;
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rows: f64 = args[0].parse().expect("rows");
    let out = PathBuf::from(&args[1]);
    std::fs::create_dir_all(&out).unwrap();
    let mut messages = None;
    let mut exports = Vec::new();
    let mut rest = args[2..].iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--messages" => messages = Some(PathBuf::from(rest.next().unwrap())),
            "--export" => {
                let (name, layout) = rest.next().unwrap().split_once('=').unwrap();
                exports.push((name.to_owned(), layout.to_owned()));
            }
            other => panic!("unknown argument {other}"),
        }
    }

    let started = Instant::now();
    let bytes =
        parallax_wasm::generate_positions(rows, 42.0).unwrap_or_else(|_| panic!("generate"));
    println!(
        "generated {rows} rows: {} bytes in {:.2} s",
        bytes.len(),
        started.elapsed().as_secs_f64()
    );
    std::fs::write(out.join("positions.arrows"), &bytes).unwrap();

    // As `Worker::load_table` does.
    let started = Instant::now();
    let (schema, batches) = decode_stream(&bytes).unwrap();
    let state = TableState::from_batches(
        &schema,
        &batches,
        TableOptions {
            primary_key: Some("position_id".into()),
            ..TableOptions::default()
        },
        NOW_MICROS,
        AS_OF_MICROS,
    )
    .unwrap();
    drop(batches);
    println!("loaded in {:.2} s", started.elapsed().as_secs_f64());
    let shared: SharedTable = Arc::new(RwLock::new(state));
    let catalog = Arc::new(MemoryCatalog::new());
    catalog.insert("positions", Arc::clone(&shared));

    if let Some(path) = messages {
        let recorded = std::fs::read(path).unwrap();
        let mut session = Session::new(catalog.clone());
        let mut replies = Vec::new();
        let mut frames = 0;
        for message in split(&recorded) {
            let started = Instant::now();
            let answered = session.handle_bytes(message);
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            for reply in answered {
                if let Ok(ServerMessage {
                    body: Some(server_message::Body::Frame(frame)),
                    ..
                }) = ServerMessage::decode(reply.as_slice())
                {
                    std::fs::write(out.join(format!("frame-{frames}.arrows")), &frame.batch)
                        .unwrap();
                    println!(
                        "frame {frames}: view {} rows {} of {} columns {} ({ms:.1} ms)",
                        frame.view_id, frame.first_row, frame.total_rows, frame.total_columns
                    );
                    frames += 1;
                }
                replies.extend_from_slice(&u32::try_from(reply.len()).unwrap().to_le_bytes());
                replies.extend_from_slice(&reply);
            }
        }
        std::fs::write(out.join("native-replies.bin"), &replies).unwrap();
    }

    for (name, layout_json) in exports {
        let layout = Layout::from_json(&layout_json).unwrap();
        let as_of = Formatter::default().timestamp(AS_OF_MICROS, true);
        let source = ExportSource {
            dataset: layout.dataset.clone(),
            as_of: format!("{as_of} New York"),
            load_version: None,
        };
        let table = Shared(Arc::clone(&shared));
        let started = Instant::now();
        let mut file = std::fs::File::create(out.join(format!("native-{name}.xlsx"))).unwrap();
        let summary = write_xlsx(&table, &layout, &source, &mut file).unwrap();
        println!(
            "native xlsx {name}: {} rows x {} columns in {:.2} s",
            summary.rows,
            summary.columns,
            started.elapsed().as_secs_f64()
        );
        let file = std::fs::File::create(out.join(format!("native-{name}.csv"))).unwrap();
        write_csv(&table, &layout, &source, std::io::BufWriter::new(file)).unwrap();
    }
}
