//! SQL Server and Oracle Database through ODBC.
//!
//! `arrow-odbc` fetches column batches rather than rows. Three adjustments keep
//! loads exact:
//!
//! - **Declared types.** A column named in `types` is fetched as text and parsed
//!   into its declared type exactly. Oracle's `NUMBER` without a precision is a
//!   floating decimal of up to 38 digits, which Oracle's driver converts through
//!   a double even when asked for text, so such a column must be selected as
//!   `TO_CHAR(col, 'TM9', 'NLS_NUMERIC_CHARACTERS=''.,''')` and declared. On
//!   Oracle, any other double column refuses the load unless `doubles` lists it.
//! - **Zoneless times.** SQL Server `datetime` and `datetime2`, and Oracle
//!   `DATE`, carry no zone. Each such column is read in its declared zone, New
//!   York unless `zones` names another, and a time in the spring gap, or in the
//!   repeated autumn hour without a declared choice, refuses the load. Columns in
//!   `wall_clock` stay as written. Digits below the microsecond refuse the load
//!   rather than being cut; cast such columns to `datetime2(6)` in the query.
//! - **JSON columns.** SQL Server's `json` type and JSON in `nvarchar(max)` arrive
//!   as text. A column named in `json` is parsed by the shared converter into the
//!   fields it declares, which replace it.
//!
//! Statements and text go through ODBC's UTF-16 calls, so non-ASCII text is right
//! whatever the machine's locale.
//!
//! The connection string may name environment variables as `${NAME}`, read at
//! each fetch, so passwords stay out of configuration files. Query parameters
//! are bound in the order `bind` lists them, as text.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use arrow_array::cast::AsArray;
use arrow_array::types::{
    TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
    TimestampSecondType,
};
use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_odbc::odbc_api::{ConnectionOptions, Environment, IntoParameter};
use arrow_odbc::{OdbcReaderBuilder, TextEncoding};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use serde::Deserialize;

use crate::json::{
    Ambiguous, DEFAULT_SOURCE_ZONE, FieldType, JsonField, instants_from_wall_clock, to_batch,
    typed_from_text,
};
use crate::{Fetched, Source, expand_environment, now_micros};

/// The most bytes one transfer buffer may use.
const MAX_BATCH_BYTES: usize = 128 << 20;

/// The longest text fetched from columns of unbounded length, such as
/// `nvarchar(max)`; longer values refuse the load rather than being cut.
const MAX_TEXT: usize = 1 << 20;

/// The zone of a zoneless time column.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneRule {
    /// The IANA zone the column's times were written in.
    pub zone: String,
    /// What to do with a time in the repeated autumn hour.
    #[serde(default)]
    pub ambiguous: Ambiguous,
}

/// A JSON text column and the fields read from it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonColumn {
    /// The text column holding JSON documents.
    pub column: String,
    /// The fields read from each document, which replace the column.
    pub fields: Vec<JsonField>,
}

/// How to read a dataset through ODBC.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OdbcConfig {
    /// The ODBC connection string. `${NAME}` is replaced by the environment
    /// variable `NAME` at each fetch.
    pub connection: String,
    /// The query.
    pub query: String,
    /// Dataset parameters bound to the query's `?` markers, in order.
    #[serde(default)]
    pub bind: Vec<String>,
    /// Columns fetched as text and parsed exactly into a declared type.
    #[serde(default)]
    pub types: BTreeMap<String, FieldType>,
    /// Zones of zoneless time columns; the rest are read in New York.
    #[serde(default)]
    pub zones: BTreeMap<String, ZoneRule>,
    /// Zoneless time columns kept as wall-clock times.
    #[serde(default)]
    pub wall_clock: Vec<String>,
    /// JSON text columns and the fields read from them.
    #[serde(default)]
    pub json: Vec<JsonColumn>,
    /// Oracle columns that really are binary doubles. Oracle's driver reports a
    /// `NUMBER` without a precision as a double, which would round it, so on
    /// Oracle every double column must be listed here, declared as a float, or
    /// selected through `TO_CHAR` and declared.
    #[serde(default)]
    pub doubles: Vec<String>,
}

/// A dataset read through ODBC.
#[derive(Clone, Debug)]
pub struct Odbc {
    config: OdbcConfig,
}

impl Odbc {
    /// A connector for `config`.
    #[must_use]
    pub fn new(config: OdbcConfig) -> Self {
        Self { config }
    }
}

/// The process's ODBC environment, which drivers expect to be created once.
fn environment() -> Result<&'static Environment, String> {
    static ENVIRONMENT: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| Environment::new().map_err(|e| format!("starting ODBC: {e}")))
        .as_ref()
        .map_err(Clone::clone)
}

/// Microseconds of a zoneless timestamp column of any unit, refusing digits
/// below the microsecond.
fn wall_clock_micros(column: &str, array: &ArrayRef) -> Result<Vec<Option<i64>>, String> {
    let DataType::Timestamp(unit, _) = array.data_type() else {
        return Err(format!("{column} is not a timestamp"));
    };
    (0..array.len())
        .map(|i| {
            if array.is_null(i) {
                return Ok(None);
            }
            let micros = match unit {
                TimeUnit::Second => array.as_primitive::<TimestampSecondType>().value(i).checked_mul(1_000_000),
                TimeUnit::Millisecond => array.as_primitive::<TimestampMillisecondType>().value(i).checked_mul(1_000),
                TimeUnit::Microsecond => Some(array.as_primitive::<TimestampMicrosecondType>().value(i)),
                TimeUnit::Nanosecond => {
                    let nanos = array.as_primitive::<TimestampNanosecondType>().value(i);
                    if nanos % 1_000 != 0 {
                        return Err(format!("row {i}, {column}: has digits below the microsecond; cast it to datetime2(6) in the query"));
                    }
                    Some(nanos / 1_000)
                }
            };
            micros.map(Some).ok_or_else(|| format!("row {i}, {column}: out of range"))
        })
        .collect()
}

impl Odbc {
    /// Applies declared types, zones and JSON columns to one fetched batch.
    fn adjust(&self, batch: &RecordBatch) -> Result<RecordBatch, String> {
        let mut fields: Vec<Field> = Vec::new();
        let mut arrays: Vec<ArrayRef> = Vec::new();
        for (field, array) in batch.schema().fields().iter().zip(batch.columns()) {
            let name = field.name();
            if let Some(json) = self.config.json.iter().find(|j| &j.column == name) {
                let texts = array
                    .as_string_opt::<i32>()
                    .ok_or_else(|| format!("{name} must be text to hold JSON"))?;
                let documents: Vec<serde_json::Value> = (0..texts.len())
                    .map(|i| {
                        if texts.is_null(i) {
                            Ok(serde_json::Value::Object(serde_json::Map::new()))
                        } else {
                            serde_json::from_str(&crate::json::quote_bare_numbers(texts.value(i)))
                                .map_err(|e| format!("row {i}, {name}: malformed JSON: {e}"))
                        }
                    })
                    .collect::<Result<_, _>>()?;
                let parsed =
                    to_batch(&json.fields, &documents, None).map_err(|e| format!("{name}: {e}"))?;
                for (f, a) in parsed.schema().fields().iter().zip(parsed.columns()) {
                    fields.push(f.as_ref().clone());
                    arrays.push(Arc::clone(a));
                }
            } else if let Some(declared) = self.config.types.get(name) {
                let texts = array
                    .as_string_opt::<i32>()
                    .ok_or_else(|| format!("{name} was not fetched as text"))?;
                let mut field = JsonField::new(name, *declared);
                if let Some(rule) = self.config.zones.get(name) {
                    field.zone = Some(rule.zone.clone());
                    field.ambiguous = rule.ambiguous;
                }
                let typed = typed_from_text(&field, texts.iter())?;
                fields.push(Field::new(name, typed.data_type().clone(), true));
                arrays.push(typed);
            } else if matches!(array.data_type(), DataType::Timestamp(_, None))
                && !self.config.wall_clock.contains(name)
            {
                let rule = self.config.zones.get(name);
                let zone = rule.map_or(DEFAULT_SOURCE_ZONE, |r| r.zone.as_str());
                let ambiguous = rule.map_or(Ambiguous::Error, |r| r.ambiguous);
                let instants = instants_from_wall_clock(
                    name,
                    wall_clock_micros(name, array)?.into_iter(),
                    zone,
                    ambiguous,
                )?;
                fields.push(Field::new(
                    name,
                    instants.data_type().clone(),
                    field.is_nullable(),
                ));
                arrays.push(instants);
            } else {
                fields.push(field.as_ref().clone());
                arrays.push(Arc::clone(array));
            }
        }
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| e.to_string())
    }

    /// Refuses configuration that names columns the query does not return, which
    /// would otherwise fall back to defaults silently, and on Oracle, double
    /// columns that are neither declared nor listed as doubles.
    fn check(&self, inferred: &Schema, oracle: bool) -> Result<(), String> {
        let names: Vec<&str> = inferred
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect();
        let configured = self
            .config
            .types
            .keys()
            .map(|n| ("types", n))
            .chain(self.config.zones.keys().map(|n| ("zones", n)))
            .chain(self.config.wall_clock.iter().map(|n| ("wall_clock", n)))
            .chain(self.config.json.iter().map(|j| ("json", &j.column)))
            .chain(self.config.doubles.iter().map(|n| ("doubles", n)));
        for (setting, name) in configured {
            if !names.contains(&name.as_str()) {
                return Err(format!(
                    "{setting} names {name}, which the query does not return; its columns are {}",
                    names.join(", ")
                ));
            }
        }
        if oracle {
            for field in inferred.fields() {
                let name = field.name();
                let double = matches!(field.data_type(), DataType::Float32 | DataType::Float64);
                if !double || self.config.doubles.contains(name) {
                    continue;
                }
                // Oracle's driver converts a NUMBER without a precision through a
                // double even when it is fetched as text, so only text made by the
                // query keeps every digit.
                let to_char =
                    format!("TO_CHAR({name}, 'TM9', 'NLS_NUMERIC_CHARACTERS=''.,''') AS {name}");
                match self.config.types.get(name) {
                    Some(FieldType::Float) => {}
                    Some(_) => {
                        return Err(format!(
                            "{name} comes from Oracle as a double, which rounds a NUMBER without a precision even when fetched as text; select it as {to_char} and keep its declared type"
                        ));
                    }
                    None => {
                        return Err(format!(
                            "{name} comes from Oracle as a double, which rounds a NUMBER without a precision; select it as {to_char} and declare its type in types, such as decimal(38, 10), or list it in doubles if it is BINARY_DOUBLE"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The schema to fetch with: declared columns as text, the rest as inferred.
    fn fetch_schema(&self, inferred: &Schema) -> SchemaRef {
        Arc::new(Schema::new(
            inferred
                .fields()
                .iter()
                .map(|f| {
                    if self.config.types.contains_key(f.name())
                        || self.config.json.iter().any(|j| &j.column == f.name())
                    {
                        Field::new(f.name(), DataType::Utf8, true)
                    } else {
                        f.as_ref().clone()
                    }
                })
                .collect::<Vec<_>>(),
        ))
    }
}

impl Source for Odbc {
    fn fetch(&self, parameters: &BTreeMap<String, String>) -> Result<Fetched, String> {
        let connection_string = expand_environment(&self.config.connection, crate::odbc_value)?;
        let connection = environment()?
            .connect_with_connection_string(&connection_string, ConnectionOptions::default())
            .map_err(|e| format!("connecting: {e}"))?;
        let values: Vec<_> = self
            .config
            .bind
            .iter()
            .map(|name| {
                parameters
                    .get(name)
                    .cloned()
                    .map(IntoParameter::into_parameter)
                    .ok_or_else(|| format!("the parameter {name} is missing"))
            })
            .collect::<Result<_, _>>()?;
        let as_of_micros = now_micros();
        let mut cursor = connection
            .execute(&self.config.query, values.as_slice(), None)
            .map_err(|e| format!("running the query: {e}"))?
            .ok_or_else(|| {
                "the statement returned no result set; the query must be a SELECT".to_owned()
            })?;
        let inferred = arrow_odbc::arrow_schema_from(&mut cursor, None, false)
            .map_err(|e| format!("reading the result's columns: {e}"))?;
        let oracle = connection
            .database_management_system_name()
            .is_ok_and(|n| n.to_ascii_lowercase().contains("oracle"));
        self.check(&inferred, oracle)?;
        let reader = OdbcReaderBuilder::new()
            .with_schema(self.fetch_schema(&inferred))
            .with_max_bytes_per_batch(MAX_BATCH_BYTES)
            .with_max_text_size(MAX_TEXT)
            // UTF-16 is ODBC's one fixed encoding; narrow text depends on the
            // machine's locale, which in a container is often not UTF-8.
            .with_payload_text_encoding(TextEncoding::Utf16)
            .build(cursor)
            .map_err(|e| format!("reading the result: {e}"))?;
        let mut batches = Vec::new();
        for batch in reader {
            let batch = batch.map_err(|e| format!("reading the result: {e}"))?;
            batches.push(self.adjust(&batch)?);
        }
        let schema = match batches.first() {
            Some(b) => b.schema(),
            None => self
                .adjust(&RecordBatch::new_empty(self.fetch_schema(&inferred)))?
                .schema(),
        };
        Ok(Fetched {
            schema,
            batches,
            as_of_micros,
        })
    }
}
