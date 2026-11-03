//! The HTTP connector: pulls a dataset from an existing REST GET endpoint.
//!
//! The request asks for an Arrow IPC stream and accepts JSON, which goes through
//! the shared JSON converter. Dataset parameters fill `{name}` placeholders in the
//! URL and the rest become query parameters, so a table is still cached by
//! dataset and parameters. A load is one response; pages fetched one after
//! another could come from different moments.
//!
//! Credentials are the host's: header values in configuration may name an
//! environment variable as `env:NAME`, read at each fetch, and a host embedding
//! the server can supply a function that returns headers. The library has no
//! authentication code of its own.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::json::{JsonField, parse_records, to_batch};
use crate::{Fetched, Source, fill, now_micros, percent_encode};

/// The Arrow IPC stream media type, registered with IANA.
pub const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

/// The default ceiling on a response's size, so a runaway endpoint cannot
/// exhaust memory. A JSON body costs several times its size once parsed.
const DEFAULT_MAX_BYTES: u64 = 2 << 30;

fn default_max_bytes() -> u64 {
    DEFAULT_MAX_BYTES
}

/// Headers a host adds to each request, such as a service credential.
pub type Credential = Arc<dyn Fn() -> Result<Vec<(String, String)>, String> + Send + Sync>;

fn default_timeout() -> u64 {
    120
}

/// How to call an endpoint.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    /// The URL. `{name}` is replaced by the dataset parameter `name`.
    pub url: String,
    /// Query parameters added to every request.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    /// Headers added to every request. A value `env:NAME` is read from the
    /// environment variable `NAME` at each fetch.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// For JSON responses, the dotted path to the array of records; the top level
    /// when absent.
    #[serde(default)]
    pub records: Option<String>,
    /// For JSON responses, the columns and their types.
    #[serde(default)]
    pub columns: Vec<JsonField>,
    /// A column that names a record in error messages.
    #[serde(default)]
    pub key: Option<String>,
    /// How long one request may take.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// The largest response read, in bytes; 2 GiB unless set.
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
}

/// A dataset pulled from a REST GET endpoint.
pub struct Http {
    config: HttpConfig,
    agent: ureq::Agent,
    credential: Option<Credential>,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("url", &self.config.url)
            .finish_non_exhaustive()
    }
}

impl Http {
    /// A connector for `config`.
    #[must_use]
    pub fn new(config: HttpConfig) -> Self {
        // Internal endpoints are signed by the company's own authority, which the
        // operating system trusts, so certificates are checked against its store.
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(config.timeout_seconds)))
            .http_status_as_error(false)
            .tls_config(tls)
            .build()
            .new_agent();
        Self {
            config,
            agent,
            credential: None,
        }
    }

    /// Adds the host's credential headers to every request.
    #[must_use]
    pub fn with_credential(mut self, credential: Credential) -> Self {
        self.credential = Some(credential);
        self
    }

    /// The URL for `parameters`: placeholders filled, the rest as query parameters.
    fn url(&self, parameters: &BTreeMap<String, String>) -> Result<String, String> {
        let filled = fill(&self.config.url, parameters, |v| Ok(percent_encode(v)))?;
        let (base, used) = (filled.text, filled.used);
        let mut query: Vec<String> = self
            .config
            .query
            .iter()
            .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
            .collect();
        query.extend(
            parameters
                .iter()
                .filter(|(k, _)| !used.contains(k.as_str()))
                .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v))),
        );
        Ok(if query.is_empty() {
            base
        } else {
            let joiner = if base.contains('?') { '&' } else { '?' };
            format!("{base}{joiner}{}", query.join("&"))
        })
    }

    fn headers(&self) -> Result<Vec<(String, String)>, String> {
        let mut headers = Vec::new();
        for (name, value) in &self.config.headers {
            let value = match value.strip_prefix("env:") {
                Some(variable) => std::env::var(variable).map_err(|_| {
                    format!("the environment variable {variable} for header {name} is not set")
                })?,
                None => value.clone(),
            };
            headers.push((name.clone(), value));
        }
        if let Some(credential) = &self.credential {
            headers.extend(credential()?);
        }
        Ok(headers)
    }
}

impl Source for Http {
    fn fetch(&self, parameters: &BTreeMap<String, String>) -> Result<Fetched, String> {
        let url = self.url(parameters)?;
        let mut request = self
            .agent
            .get(&url)
            .header("Accept", format!("{ARROW_STREAM}, application/json;q=0.9"));
        for (name, value) in self.headers()? {
            request = request.header(name, value);
        }
        let response = request
            .call()
            .map_err(|e| format!("GET {}: {e}", self.config.url))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut body = response.into_body();
        let mut reader = body.with_config().limit(self.config.max_bytes).reader();
        if !status.is_success() {
            let mut text = String::new();
            let _ = reader.by_ref().take(2_000).read_to_string(&mut text);
            return Err(format!(
                "GET {} answered {status}: {}",
                self.config.url,
                text.trim()
            ));
        }
        let as_of_micros = now_micros();
        if content_type.starts_with(ARROW_STREAM) {
            let stream = arrow_ipc::reader::StreamReader::try_new(reader, None)
                .map_err(|e| format!("reading the Arrow stream: {e}"))?;
            let schema = stream.schema();
            let batches = stream
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("reading the Arrow stream: {e}"))?;
            return Ok(Fetched {
                schema,
                batches,
                as_of_micros,
            });
        }
        if !content_type.starts_with("application/json") {
            return Err(format!(
                "GET {} answered {content_type:?}; expected {ARROW_STREAM} or application/json",
                self.config.url
            ));
        }
        if self.config.columns.is_empty() {
            return Err(format!(
                "GET {} answered JSON, so the dataset must declare its columns",
                self.config.url
            ));
        }
        let mut text = String::new();
        reader
            .read_to_string(&mut text)
            .map_err(|e| format!("reading the response: {e}"))?;
        let records = parse_records(&text, self.config.records.as_deref())?;
        let batch = to_batch(&self.config.columns, &records, self.config.key.as_deref())?;
        Ok(Fetched {
            schema: batch.schema(),
            batches: vec![batch],
            as_of_micros,
        })
    }
}
