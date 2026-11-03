//! MongoDB collections, through the official driver and the shared JSON
//! converter.
//!
//! Documents are turned into JSON without losing anything the converter checks:
//! `Decimal128` becomes a number with every digit, dates become RFC 3339 instants,
//! object ids become their hex text, and a non-finite double is accepted only by
//! number columns. Types the converter has no column for, such as binary data,
//! become objects, which a declared field refuses by name.
//!
//! The filter is MongoDB extended JSON with `{name}` placeholders for dataset
//! parameters, filled as JSON strings, so a parameter cannot change the filter's
//! shape. Only the declared fields are fetched.

use std::collections::BTreeMap;

use mongodb::bson::{Bson, Document, doc};
use serde::Deserialize;
use serde_json::Value;

use crate::json::{JsonField, non_finite, to_batch};
use crate::{Fetched, Source, expand_environment, fill, now_micros};

/// How to read a dataset from MongoDB.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MongoConfig {
    /// The connection string. `${NAME}` is replaced by the environment variable
    /// `NAME` at each fetch.
    pub uri: String,
    /// The database.
    pub database: String,
    /// The collection.
    pub collection: String,
    /// The filter, as extended JSON with `{name}` placeholders.
    #[serde(default)]
    pub filter: Option<String>,
    /// The fields read from each document.
    pub columns: Vec<JsonField>,
    /// A field that names a document in error messages, such as `_id`.
    #[serde(default)]
    pub key: Option<String>,
}

/// A dataset read from a MongoDB collection.
#[derive(Clone, Debug)]
pub struct Mongo {
    config: MongoConfig,
}

impl Mongo {
    /// A connector for `config`.
    #[must_use]
    pub fn new(config: MongoConfig) -> Self {
        Self { config }
    }

    /// The filter for `parameters`, with each value filled in as a JSON string.
    fn filter(&self, parameters: &BTreeMap<String, String>) -> Result<Document, String> {
        let Some(template) = &self.config.filter else {
            return Ok(Document::new());
        };
        let filled = fill(template, parameters, |v| {
            let quoted = serde_json::to_string(v).map_err(|e| e.to_string())?;
            // The template already has the quotes around the placeholder.
            Ok(quoted[1..quoted.len() - 1].to_owned())
        })?;
        let value: Value = serde_json::from_str(&filled.text)
            .map_err(|e| format!("the filter is not JSON: {e}"))?;
        let Value::Object(map) = value else {
            return Err("the filter must be a JSON object".to_owned());
        };
        Document::try_from(map).map_err(|e| format!("the filter is not extended JSON: {e}"))
    }
}

/// A BSON value as the JSON the converter reads.
fn to_json(value: Bson) -> Value {
    match value {
        Bson::Double(v) if v.is_finite() => {
            serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
        }
        Bson::Double(v) => non_finite(v),
        Bson::Int32(v) => v.into(),
        Bson::Int64(v) => v.into(),
        // Every digit survives: with exact numbers on, the text is the number.
        Bson::Decimal128(d) => {
            serde_json::from_str::<Value>(&d.to_string()).unwrap_or_else(|_| non_finite(f64::NAN))
        }
        Bson::String(s) => Value::String(s),
        Bson::Boolean(b) => Value::Bool(b),
        Bson::Null | Bson::Undefined => Value::Null,
        Bson::DateTime(dt) => dt
            .try_to_rfc3339_string()
            .map_or(Value::Null, Value::String),
        Bson::ObjectId(id) => Value::String(id.to_hex()),
        Bson::Document(d) => Value::Object(d.into_iter().map(|(k, v)| (k, to_json(v))).collect()),
        Bson::Array(items) => Value::Array(items.into_iter().map(to_json).collect()),
        other => {
            let mut unsupported = serde_json::Map::new();
            unsupported.insert(
                "unsupported BSON type".to_owned(),
                Value::String(format!("{:?}", other.element_type())),
            );
            Value::Object(unsupported)
        }
    }
}

impl Source for Mongo {
    fn fetch(&self, parameters: &BTreeMap<String, String>) -> Result<Fetched, String> {
        let filter = self.filter(parameters)?;
        let client = mongodb::sync::Client::with_uri_str(expand_environment(&self.config.uri)?)
            .map_err(|e| format!("connecting: {e}"))?;
        let collection = client
            .database(&self.config.database)
            .collection::<Document>(&self.config.collection);
        let mut projection = doc! {};
        for field in &self.config.columns {
            projection.insert(field.path.clone().unwrap_or_else(|| field.name.clone()), 1);
        }
        if let Some(key) = &self.config.key {
            projection.insert(key.clone(), 1);
        }
        let as_of_micros = now_micros();
        let cursor = collection
            .find(filter)
            .projection(projection)
            .run()
            .map_err(|e| format!("querying {}: {e}", self.config.collection))?;
        let documents = cursor
            .map(|d| {
                d.map(|d| to_json(Bson::Document(d)))
                    .map_err(|e| format!("reading {}: {e}", self.config.collection))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let batch = to_batch(&self.config.columns, &documents, self.config.key.as_deref())?;
        Ok(Fetched {
            schema: batch.schema(),
            batches: vec![batch],
            as_of_micros,
        })
    }
}
