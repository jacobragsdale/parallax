//! Where parallax tables come from: the [`Source`] interface every connector
//! implements, the shared JSON converter, and the connectors.
//!
//! A source reads every row for a set of parameters in one call. The server
//! caches the result as a table, reloads it as a diff, and keeps the last good
//! load when a fetch fails.

use std::collections::BTreeMap;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;

#[cfg(feature = "http")]
pub mod http;
pub mod json;
#[cfg(feature = "parquet")]
pub mod parquet;

/// The current time in microseconds since the Unix epoch.
#[must_use]
pub fn now_micros() -> i64 {
    jiff::Timestamp::now().as_microsecond()
}

/// Rows read from a source.
#[derive(Debug)]
pub struct Fetched {
    /// The schema, known even when there are no rows.
    pub schema: SchemaRef,
    /// The rows.
    pub batches: Vec<RecordBatch>,
    /// When the source produced them, in microseconds since the Unix epoch.
    pub as_of_micros: i64,
}

/// Where a dataset's rows come from.
pub trait Source: Send + Sync {
    /// Reads every row for `parameters`.
    ///
    /// # Errors
    ///
    /// Returns a message if the source cannot be read.
    fn fetch(&self, parameters: &BTreeMap<String, String>) -> Result<Fetched, String>;
}

/// Fills `{name}` placeholders in `template` from `parameters`.
///
/// # Errors
///
/// Returns a message naming a placeholder without a parameter.
pub fn fill(
    template: &str,
    parameters: &BTreeMap<String, String>,
    escape: impl Fn(&str) -> String,
) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..]
            .find('}')
            .ok_or_else(|| format!("unclosed {{ in {template}"))?
            + open;
        let name = &rest[open + 1..close];
        let value = parameters
            .get(name)
            .ok_or_else(|| format!("the parameter {name} is missing"))?;
        out.push_str(&escape(value));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Percent-encodes text for a URL path segment or query value.
#[must_use]
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
