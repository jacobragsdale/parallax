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
#[cfg(feature = "mongodb")]
pub mod mongo;
#[cfg(feature = "odbc")]
pub mod odbc;
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

/// A filled template and the parameters it used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filled {
    /// The text with every placeholder replaced.
    pub text: String,
    /// The parameters the template named.
    pub used: std::collections::BTreeSet<String>,
}

/// Fills `{name}` placeholders in `template` from `parameters`, passing each value
/// through `escape`, which may refuse it. Every brace must start a placeholder: a
/// name of letters, digits and underscores, closed by a brace.
///
/// # Errors
///
/// Returns a message naming a malformed placeholder, a placeholder without a
/// parameter, or a value `escape` refuses.
pub fn fill(
    template: &str,
    parameters: &BTreeMap<String, String>,
    escape: impl Fn(&str) -> Result<String, String>,
) -> Result<Filled, String> {
    fill_with(template, parameters, escape, true)
}

/// Fills `{name}` placeholders in a JSON template, copying every other brace, which
/// belongs to the JSON itself.
///
/// # Errors
///
/// Returns a message naming a placeholder without a parameter, or a value
/// `escape` refuses.
pub fn fill_json(
    template: &str,
    parameters: &BTreeMap<String, String>,
    escape: impl Fn(&str) -> Result<String, String>,
) -> Result<Filled, String> {
    fill_with(template, parameters, escape, false)
}

fn fill_with(
    template: &str,
    parameters: &BTreeMap<String, String>,
    escape: impl Fn(&str) -> Result<String, String>,
    strict: bool,
) -> Result<Filled, String> {
    let is_name = |s: &str| {
        !s.is_empty()
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !s.starts_with(|c: char| c.is_ascii_digit())
    };
    let mut text = String::with_capacity(template.len());
    let mut used = std::collections::BTreeSet::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_name(&after[..close]) => {
                let name = &after[..close];
                let value = parameters
                    .get(name)
                    .ok_or_else(|| format!("the parameter {name} is missing"))?;
                text.push_str(&escape(value)?);
                used.insert(name.to_owned());
                rest = &after[close + 1..];
            }
            _ if strict => {
                let shown: String = rest[open..].chars().take(24).collect();
                return Err(format!(
                    "{shown:?} in {template} is not a placeholder; write {{name}} with letters, digits and underscores"
                ));
            }
            _ => {
                text.push('{');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    Ok(Filled { text, used })
}

/// Quotes a value for an ODBC connection string: in braces, with closing braces
/// doubled, whenever it holds a character that would end or split the value.
#[must_use]
pub fn odbc_value(value: &str) -> String {
    if value.contains([';', '{', '}']) || value.starts_with(' ') || value.ends_with(' ') {
        format!("{{{}}}", value.replace('}', "}}"))
    } else {
        value.to_owned()
    }
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

/// Reads a connection setting that may come from the environment: `env:NAME`
/// takes the whole value from the variable `NAME`, and `${NAME}` inside the text is
/// replaced by the variable's value passed through `escape`, which quotes it for
/// the setting's syntax, so a password with special characters stays one value.
///
/// # Errors
///
/// Returns a message naming a variable that is not set.
pub fn expand_environment(text: &str, escape: impl Fn(&str) -> String) -> Result<String, String> {
    expand_with(text, escape, |name| std::env::var(name).ok())
}

fn expand_with(
    text: &str,
    escape: impl Fn(&str) -> String,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let read = |name: &str| {
        lookup(name).ok_or_else(|| format!("the environment variable {name} is not set"))
    };
    if let Some(name) = text.strip_prefix("env:") {
        return read(name.trim());
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("${") {
        out.push_str(&rest[..open]);
        let close = rest[open..]
            .find('}')
            .ok_or_else(|| "an unclosed ${ in the text".to_owned())?
            + open;
        out.push_str(&escape(&read(&rest[open + 2..close])?));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_templates_keep_their_own_braces() {
        let p: BTreeMap<String, String> = [("as_of".to_owned(), "2026-11-02".to_owned())].into();
        let filled = fill_json(r#"{"date": "{as_of}", "n": {"$gt": 1}}"#, &p, |v| {
            Ok(v.to_owned())
        })
        .unwrap();
        assert_eq!(filled.text, r#"{"date": "2026-11-02", "n": {"$gt": 1}}"#);
        assert_eq!(filled.used.into_iter().collect::<Vec<_>>(), ["as_of"]);
        assert!(
            fill("/{missing}/", &p, |v| Ok(v.to_owned()))
                .unwrap_err()
                .contains("missing")
        );
        // Outside JSON, a malformed placeholder is an error, not literal text.
        for bad in ["/{as-of}/", "/{ as_of }/", "/{as_of/"] {
            assert!(
                fill(bad, &p, |v| Ok(v.to_owned()))
                    .unwrap_err()
                    .contains("not a placeholder"),
                "{bad}"
            );
        }
    }

    #[test]
    fn environment_values_are_escaped_for_their_setting() {
        let lookup = |name: &str| (name == "SECRET").then(|| "p@ss;w}rd".to_owned());
        assert_eq!(
            expand_with("PWD=${SECRET};", odbc_value, lookup).unwrap(),
            "PWD={p@ss;w}}rd};"
        );
        assert_eq!(
            expand_with("mongodb://u:${SECRET}@h", percent_encode, lookup).unwrap(),
            "mongodb://u:p%40ss%3Bw%7Drd@h"
        );
        assert_eq!(
            expand_with("env:SECRET", odbc_value, lookup).unwrap(),
            "p@ss;w}rd"
        );
        assert!(
            expand_with("${UNSET}", odbc_value, lookup)
                .unwrap_err()
                .contains("UNSET")
        );
        assert_eq!(odbc_value("plain"), "plain");
    }
}
