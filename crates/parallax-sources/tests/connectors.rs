//! The HTTP and Parquet connectors against a local server and local files.

#![allow(missing_docs)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use arrow_array::cast::AsArray;
use arrow_array::types::{Decimal128Type, Int64Type};
use arrow_array::{ArrayRef, Decimal128Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parallax_sources::Source;
use parallax_sources::http::{ARROW_STREAM, Http, HttpConfig};
use parallax_sources::json::{FieldType, JsonField};
use parallax_sources::parquet::{Parquet, ParquetConfig};

fn batch() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("fund", DataType::Utf8, false),
            Field::new("mv", DataType::Decimal128(20, 2), false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
            Arc::new(StringArray::from(vec!["Atlas", "Cobalt"])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![12_345, -5])
                    .with_precision_and_scale(20, 2)
                    .unwrap(),
            ) as ArrayRef,
        ],
    )
    .unwrap()
}

fn arrow_bytes() -> Vec<u8> {
    let b = batch();
    let mut out = Vec::new();
    let mut writer = arrow_ipc::writer::StreamWriter::try_new(&mut out, &b.schema()).unwrap();
    writer.write(&b).unwrap();
    writer.finish().unwrap();
    drop(writer);
    out
}

/// What one request asked for.
#[derive(Clone, Debug, Default)]
struct Seen {
    target: String,
    headers: BTreeMap<String, String>,
}

/// Serves one canned answer per connection and records each request.
fn serve(
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut request = Seen {
                target: first
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned(),
                ..Seen::default()
            };
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    request
                        .headers
                        .insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
                }
            }
            log.lock().unwrap().push(request);
            let mut stream = stream;
            write!(stream, "HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    (format!("http://{address}"), seen)
}

fn http(url: String) -> HttpConfig {
    HttpConfig {
        url,
        query: BTreeMap::new(),
        headers: BTreeMap::new(),
        records: None,
        columns: Vec::new(),
        key: None,
        timeout_seconds: 10,
        max_bytes: 1 << 20,
    }
}

fn parameters(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn an_arrow_answer_is_read_as_sent_and_parameters_reach_the_url() {
    let (base, seen) = serve(200, ARROW_STREAM, arrow_bytes());
    let mut config = http(format!("{base}/funds/{{fund}}/positions"));
    config.query.insert("view".into(), "full".into());
    config.headers.insert("x-service".into(), "parallax".into());
    let fetched = Http::new(config)
        .fetch(&parameters(&[
            ("fund", "Atlas Global"),
            ("as_of", "2026-11-02"),
        ]))
        .unwrap();
    assert_eq!(fetched.batches, vec![batch()]);
    let request = seen.lock().unwrap()[0].clone();
    assert_eq!(
        request.target,
        "/funds/Atlas%20Global/positions?view=full&as_of=2026-11-02"
    );
    assert!(request.headers["accept"].starts_with(ARROW_STREAM));
    assert_eq!(request.headers["x-service"], "parallax");
}

#[test]
fn a_json_answer_goes_through_the_converter_exactly() {
    let body =
        br#"{"data": [{"id": 1, "mv": 12345678901234567.89}, {"id": 2, "mv": NaN}]}"#.to_vec();
    let (base, _) = serve(200, "application/json; charset=utf-8", body);
    let mut config = http(format!("{base}/positions"));
    config.records = Some("data".into());
    config.columns = vec![
        JsonField::new("id", FieldType::Integer),
        JsonField::new("mv", FieldType::Decimal(20, 2)),
    ];
    let error = Http::new(config.clone())
        .fetch(&BTreeMap::new())
        .unwrap_err();
    // NaN is not a decimal, so the load fails naming the row.
    assert!(error.contains("row 1, mv"), "{error}");
    let (base, _) = serve(
        200,
        "application/json",
        br#"{"data": [{"id": 1, "mv": 12345678901234567.89}]}"#.to_vec(),
    );
    config.url = format!("{base}/positions");
    let fetched = Http::new(config).fetch(&BTreeMap::new()).unwrap();
    let mv = fetched.batches[0]
        .column(1)
        .as_primitive::<Decimal128Type>();
    assert_eq!(mv.value(0), 1_234_567_890_123_456_789);
    assert_eq!(
        fetched.batches[0]
            .column(0)
            .as_primitive::<Int64Type>()
            .value(0),
        1
    );
}

#[test]
fn failures_say_what_the_endpoint_answered() {
    let (base, _) = serve(503, "text/plain", b"database is down".to_vec());
    let error = Http::new(http(format!("{base}/x")))
        .fetch(&BTreeMap::new())
        .unwrap_err();
    assert!(
        error.contains("503") && error.contains("database is down"),
        "{error}"
    );
    let (base, _) = serve(200, "application/json", b"[]".to_vec());
    let error = Http::new(http(format!("{base}/x")))
        .fetch(&BTreeMap::new())
        .unwrap_err();
    assert!(error.contains("declare its columns"), "{error}");
    let (base, _) = serve(200, "text/html", b"<html>".to_vec());
    assert!(
        Http::new(http(format!("{base}/x")))
            .fetch(&BTreeMap::new())
            .unwrap_err()
            .contains("text/html")
    );
}

#[test]
fn credentials_come_from_the_environment_or_the_host() {
    let (base, seen) = serve(200, ARROW_STREAM, arrow_bytes());
    let mut config = http(format!("{base}/x"));
    config.headers.insert(
        "authorization".into(),
        "env:PARALLAX_TEST_TOKEN_UNSET".into(),
    );
    assert!(
        Http::new(config.clone())
            .fetch(&BTreeMap::new())
            .unwrap_err()
            .contains("PARALLAX_TEST_TOKEN_UNSET")
    );
    config.headers.clear();
    let connector = Http::new(config).with_credential(Arc::new(|| {
        Ok(vec![("authorization".into(), "Bearer service".into())])
    }));
    connector.fetch(&BTreeMap::new()).unwrap();
    assert_eq!(
        seen.lock().unwrap()[0].headers["authorization"],
        "Bearer service"
    );
}

fn write_parquet(path: &std::path::Path, b: &RecordBatch) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = parquet::arrow::ArrowWriter::try_new(file, b.schema(), None).unwrap();
    writer.write(b).unwrap();
    writer.close().unwrap();
}

#[test]
fn parquet_is_read_through_a_cache_that_follows_changes() {
    let share = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::create_dir(share.path().join("2026-11-02")).unwrap();
    let file = share.path().join("2026-11-02").join("positions.parquet");
    write_parquet(&file, &batch());
    let connector = Parquet::new(ParquetConfig {
        path: format!("{}/{{as_of}}/positions.parquet", share.path().display()),
        cache_dir: Some(cache.path().to_owned()),
    });
    let at = parameters(&[("as_of", "2026-11-02")]);
    let fetched = connector.fetch(&at).unwrap();
    assert_eq!(
        fetched
            .batches
            .iter()
            .map(RecordBatch::num_rows)
            .sum::<usize>(),
        2
    );
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 1);
    // The cache answers while the share is unchanged.
    connector.fetch(&at).unwrap();
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 1);
    // A new file replaces the cached copy.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let bigger =
        arrow_select::concat::concat_batches(&batch().schema(), &[batch(), batch()]).unwrap();
    write_parquet(&file, &bigger);
    let fetched = connector.fetch(&at).unwrap();
    assert_eq!(
        fetched
            .batches
            .iter()
            .map(RecordBatch::num_rows)
            .sum::<usize>(),
        4
    );
    assert_eq!(
        std::fs::read_dir(cache.path()).unwrap().count(),
        1,
        "the old copy is removed"
    );
}

#[test]
fn parameters_cannot_leave_the_directory() {
    let connector = Parquet::new(ParquetConfig {
        path: "/data/{as_of}/positions.parquet".into(),
        cache_dir: None,
    });
    for bad in ["../etc", "a/b", ".hidden", ""] {
        let error = connector.fetch(&parameters(&[("as_of", bad)])).unwrap_err();
        assert!(
            error.contains("cannot be used in a file path"),
            "{bad}: {error}"
        );
    }
}

#[test]
fn a_response_larger_than_the_limit_is_refused() {
    let (base, _) = serve(200, "application/json", vec![b' '; 4_096]);
    let mut config = http(format!("{base}/x"));
    config.columns = vec![JsonField::new("id", FieldType::Integer)];
    config.max_bytes = 1_024;
    let error = Http::new(config).fetch(&BTreeMap::new()).unwrap_err();
    assert!(
        error.to_lowercase().contains("limit") || error.contains("reading the response"),
        "{error}"
    );
}

#[test]
fn parameters_the_path_does_not_use_may_hold_anything() {
    let share = tempfile::tempdir().unwrap();
    let file = share.path().join("positions.parquet");
    write_parquet(&file, &batch());
    let connector = Parquet::new(ParquetConfig {
        path: file.display().to_string(),
        cache_dir: None,
    });
    connector
        .fetch(&parameters(&[("user", "CORP\\jdoe"), ("note", "")]))
        .unwrap();
}
