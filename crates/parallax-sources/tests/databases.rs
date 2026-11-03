//! The database connectors against real servers. Each test runs when its
//! environment variable names a server, as CI's service containers do, and
//! passes without checking anything otherwise:
//!
//! - `PARALLAX_TEST_MSSQL`: an ODBC connection string for SQL Server
//! - `PARALLAX_TEST_ORACLE`: an ODBC connection string for Oracle Database
//! - `PARALLAX_TEST_MONGODB`: a MongoDB connection string

#![allow(missing_docs)]
#![cfg(any(feature = "odbc", feature = "mongodb"))]

use std::collections::BTreeMap;

fn server(variable: &str) -> Option<String> {
    let value = std::env::var(variable).ok().filter(|v| !v.is_empty());
    if value.is_none() {
        eprintln!("{variable} is not set; skipping");
    }
    value
}

#[cfg(feature = "mongodb")]
mod mongo {
    use arrow_array::Array;
    use arrow_array::cast::AsArray;
    use arrow_array::types::{Decimal128Type, Float64Type, Int64Type, TimestampMicrosecondType};
    use mongodb::bson::{Bson, DateTime, Decimal128, doc, oid::ObjectId};
    use parallax_sources::Source;
    use parallax_sources::json::{FieldType, JsonField};
    use parallax_sources::mongo::{Mongo, MongoConfig};

    use super::{BTreeMap, server};

    fn field(name: &str, path: &str, ty: &str) -> JsonField {
        let mut f = JsonField::new(name, FieldType::try_from(ty.to_owned()).unwrap());
        f.path = Some(path.to_owned());
        f
    }

    fn connector_uri() -> String {
        std::env::var("PARALLAX_TEST_MONGODB").unwrap()
    }

    #[test]
    fn documents_load_exactly() {
        let Some(uri) = server("PARALLAX_TEST_MONGODB") else {
            return;
        };
        let client = mongodb::sync::Client::with_uri_str(&uri).unwrap();
        let collection = client
            .database("parallax_test")
            .collection::<mongodb::bson::Document>("positions");
        collection.drop().run().unwrap();
        let id = ObjectId::new();
        let notional: Decimal128 = "12345678901234567.89".parse().unwrap();
        collection
            .insert_many([
                doc! { "_id": id, "fund": "Atlas", "qty": 9_007_199_254_740_993_i64, "notional": notional,
                       "risk": { "beta": 1.25 }, "as_of": DateTime::from_millis(1_793_511_000_123), "tag": "x" },
                doc! { "_id": ObjectId::new(), "fund": "Cobalt", "qty": 5_i32, "notional": Bson::Null,
                       "risk": { "beta": f64::NAN }, "as_of": DateTime::from_millis(0) },
                doc! { "_id": ObjectId::new(), "fund": "Excluded", "qty": 1_i32 },
            ])
            .run()
            .unwrap();
        let connector = Mongo::new(MongoConfig {
            uri,
            database: "parallax_test".into(),
            collection: "positions".into(),
            filter: Some(r#"{"fund": {"$in": ["{first}", "Cobalt"]}}"#.into()),
            columns: vec![
                field("id", "_id", "text"),
                field("fund", "fund", "text"),
                field("qty", "qty", "integer"),
                field("notional", "notional", "decimal(20, 2)"),
                field("beta", "risk.beta", "float"),
                field("as_of", "as_of", "timestamp"),
                field("tag", "tag", "text"),
            ],
            key: Some("_id".into()),
        });
        let parameters: BTreeMap<String, String> =
            [("first".to_owned(), "Atlas".to_owned())].into();
        let fetched = connector.fetch(&parameters).unwrap();
        let batch = &fetched.batches[0];
        assert_eq!(batch.num_rows(), 2, "the filter keeps Atlas and Cobalt");
        let row = (0..2)
            .find(|&r| batch.column(1).as_string::<i32>().value(r) == "Atlas")
            .unwrap();
        assert_eq!(batch.column(0).as_string::<i32>().value(row), id.to_hex());
        assert_eq!(
            batch.column(2).as_primitive::<Int64Type>().value(row),
            9_007_199_254_740_993
        );
        assert_eq!(
            batch.column(3).as_primitive::<Decimal128Type>().value(row),
            1_234_567_890_123_456_789
        );
        assert_eq!(
            batch.column(4).as_primitive::<Float64Type>().value(row),
            1.25
        );
        assert_eq!(
            batch
                .column(5)
                .as_primitive::<TimestampMicrosecondType>()
                .value(row),
            1_793_511_000_123_000
        );
        let other = 1 - row;
        assert!(batch.column(3).is_null(other));
        assert!(
            batch
                .column(4)
                .as_primitive::<Float64Type>()
                .value(other)
                .is_nan()
        );
        assert!(batch.column(6).is_null(other), "a missing field is null");
        // Infinite decimals keep their sign in a float column; dates past year
        // 9999 refuse the load rather than turning into null.
        let edge = client
            .database("parallax_test")
            .collection::<mongodb::bson::Document>("edges");
        edge.drop().run().unwrap();
        edge.insert_one(doc! { "x": "Infinity".parse::<Decimal128>().unwrap(), "y": "-Infinity".parse::<Decimal128>().unwrap() }).run().unwrap();
        let floats = Mongo::new(MongoConfig {
            uri: connector_uri(),
            database: "parallax_test".into(),
            collection: "edges".into(),
            filter: None,
            columns: vec![field("x", "x", "float"), field("y", "y", "float")],
            key: None,
        });
        let b = &floats.fetch(&BTreeMap::new()).unwrap().batches[0];
        assert_eq!(
            b.column(0).as_primitive::<Float64Type>().value(0),
            f64::INFINITY
        );
        assert_eq!(
            b.column(1).as_primitive::<Float64Type>().value(0),
            f64::NEG_INFINITY
        );
        edge.insert_one(
            doc! { "x": 1.0, "y": 1.0, "when": DateTime::from_millis(300_000_000_000_000) },
        )
        .run()
        .unwrap();
        let dated = Mongo::new(MongoConfig {
            uri: connector_uri(),
            database: "parallax_test".into(),
            collection: "edges".into(),
            filter: Some(r#"{"when": {"$exists": true}}"#.into()),
            columns: vec![field("when", "when", "timestamp")],
            key: None,
        });
        assert!(
            dated
                .fetch(&BTreeMap::new())
                .unwrap_err()
                .contains("year 9999")
        );
        // A filter value cannot change the filter's shape.
        let hostile: BTreeMap<String, String> =
            [("first".to_owned(), r#"", "$where": "1"#.to_owned())].into();
        assert_eq!(connector.fetch(&hostile).unwrap().batches[0].num_rows(), 1);
    }
}

#[cfg(feature = "odbc")]
mod odbc {
    use arrow_array::Array;
    use arrow_array::cast::AsArray;
    use arrow_array::types::{
        Date32Type, Decimal128Type, Float64Type, Int64Type, TimestampMicrosecondType,
    };
    use arrow_odbc::odbc_api::{ConnectionOptions, Environment};
    use parallax_sources::Source;
    use parallax_sources::json::{Ambiguous, FieldType, JsonField};
    use parallax_sources::odbc::{JsonColumn, Odbc, OdbcConfig, ZoneRule};

    use super::{BTreeMap, server};

    fn run(connection: &str, statements: &[&str]) {
        let environment = Environment::new().unwrap();
        let conn = environment
            .connect_with_connection_string(connection, ConnectionOptions::default())
            .unwrap();
        for statement in statements {
            // Dropping a table that does not exist yet is fine.
            if let Err(e) = conn.execute(statement, (), None) {
                assert!(
                    statement.starts_with("DROP") || statement.starts_with("BEGIN"),
                    "{statement}: {e}"
                );
            }
        }
    }

    fn config(connection: String, query: &str) -> OdbcConfig {
        OdbcConfig {
            connection,
            query: query.into(),
            bind: Vec::new(),
            types: BTreeMap::new(),
            zones: BTreeMap::new(),
            wall_clock: Vec::new(),
            json: Vec::new(),
            doubles: Vec::new(),
        }
    }

    #[test]
    fn sql_server_loads_exactly_and_reads_wall_clock_times_in_new_york() {
        let Some(connection) = server("PARALLAX_TEST_MSSQL") else {
            return;
        };
        run(
            &connection,
            &[
                "DROP TABLE IF EXISTS parallax_positions",
                "CREATE TABLE parallax_positions (id BIGINT, fund NVARCHAR(50), mv DECIMAL(20, 2), px FLOAT, on_day DATE, \
                 traded DATETIME2(6), stamped DATETIMEOFFSET(6), attrs NVARCHAR(MAX))",
                "INSERT INTO parallax_positions VALUES \
                 (9007199254740993, N'Atlas', 123456789012345678.90, 0.1, '2026-11-02', '2026-11-01 01:30:00', '2026-11-01 05:30:00 +00:00', N'{\"beta\": 1.25, \"desk\": \"Rates\"}'), \
                 (2, N'Zürich', -0.05, NULL, NULL, '2026-06-01 09:30:00.123456', NULL, NULL)",
            ],
        );
        let mut c = config(
            connection.clone(),
            "SELECT id, fund, mv, px, on_day, traded, stamped, attrs FROM parallax_positions WHERE fund <> ? ORDER BY id",
        );
        c.bind = vec!["exclude".into()];
        c.types.insert("stamped".into(), FieldType::Timestamp);
        c.zones.insert(
            "traded".into(),
            ZoneRule {
                zone: "America/New_York".into(),
                ambiguous: Ambiguous::Earlier,
            },
        );
        c.json.push(JsonColumn {
            column: "attrs".into(),
            fields: vec![
                JsonField::new("beta", FieldType::Float),
                JsonField::new("desk", FieldType::Text),
            ],
        });
        let parameters: BTreeMap<String, String> =
            [("exclude".to_owned(), "nobody".to_owned())].into();
        let fetched = Odbc::new(c.clone()).fetch(&parameters).unwrap();
        let b = arrow_select::concat::concat_batches(&fetched.schema, &fetched.batches).unwrap();
        let col = |name: &str| b.column_by_name(name).unwrap().clone();
        assert_eq!(
            col("id").as_primitive::<Int64Type>().value(1),
            9_007_199_254_740_993
        );
        assert_eq!(col("fund").as_string::<i32>().value(0), "Zürich");
        assert_eq!(
            col("mv").as_primitive::<Decimal128Type>().value(1),
            12_345_678_901_234_567_890
        );
        assert_eq!(col("px").as_primitive::<Float64Type>().value(1), 0.1);
        assert_eq!(col("on_day").as_primitive::<Date32Type>().value(1), 20_759);
        // 01:30 on 2026-11-01 happened twice in New York; the column takes the first.
        assert_eq!(
            col("traded")
                .as_primitive::<TimestampMicrosecondType>()
                .value(1),
            1_793_511_000_000_000
        );
        assert_eq!(
            col("stamped")
                .as_primitive::<TimestampMicrosecondType>()
                .value(1),
            1_793_511_000_000_000
        );
        assert_eq!(col("beta").as_primitive::<Float64Type>().value(1), 1.25);
        assert_eq!(col("desk").as_string::<i32>().value(1), "Rates");
        assert!(col("beta").is_null(0));
        // A setting that names a column the query does not return is refused.
        let mut typo = c.clone();
        typo.wall_clock.push("traded_at".into());
        assert!(
            Odbc::new(typo)
                .fetch(&parameters)
                .unwrap_err()
                .contains("does not return")
        );
        // Without a declared choice, the repeated hour refuses the load.
        c.zones.clear();
        let error = Odbc::new(c).fetch(&parameters).unwrap_err();
        assert!(error.contains("repeats"), "{error}");
    }

    #[test]
    fn oracle_numbers_without_precision_load_exactly() {
        let Some(connection) = server("PARALLAX_TEST_ORACLE") else {
            return;
        };
        run(
            &connection,
            &[
                "BEGIN EXECUTE IMMEDIATE 'DROP TABLE parallax_lots'; EXCEPTION WHEN OTHERS THEN NULL; END;",
                "CREATE TABLE parallax_lots (id NUMBER(19), qty NUMBER, note VARCHAR2(20), opened DATE)",
                "INSERT INTO parallax_lots VALUES (1, 12345678901234567890.123456789, '', TO_DATE('2026-03-08 03:30:00', 'YYYY-MM-DD HH24:MI:SS'))",
                // TO_CHAR writes this as -.25, without the leading zero.
                "INSERT INTO parallax_lots VALUES (2, -0.25, 'short', TO_DATE('2026-03-08 03:30:00', 'YYYY-MM-DD HH24:MI:SS'))",
            ],
        );
        // Undeclared, Oracle's driver reports the NUMBER as a double, which would round it.
        let undeclared = Odbc::new(config(
            connection.clone(),
            "SELECT id, qty FROM parallax_lots",
        ))
        .fetch(&BTreeMap::new());
        assert!(
            undeclared
                .as_ref()
                .is_err_and(|e| e.contains("as a double")),
            "{undeclared:?}"
        );
        // Declaring it is not enough: the driver still converts through a double,
        // so the refusal says to select it through TO_CHAR.
        let mut declared_only = config(connection.clone(), "SELECT id, qty FROM parallax_lots");
        declared_only
            .types
            .insert("QTY".into(), FieldType::Decimal(38, 10));
        let refused = Odbc::new(declared_only).fetch(&BTreeMap::new());
        assert!(
            refused.as_ref().is_err_and(|e| e.contains("TO_CHAR(QTY")),
            "{refused:?}"
        );
        let mut c = config(
            connection,
            "SELECT id, TO_CHAR(qty, 'TM9', 'NLS_NUMERIC_CHARACTERS=''.,''') AS qty, note, opened FROM parallax_lots ORDER BY id",
        );
        // Oracle returns column names in upper case.
        c.types.insert("QTY".into(), FieldType::Decimal(38, 10));
        let fetched = Odbc::new(c).fetch(&BTreeMap::new()).unwrap();
        let b = &fetched.batches[0];
        let qty = b
            .column_by_name("QTY")
            .unwrap()
            .as_primitive::<Decimal128Type>();
        assert_eq!(qty.value(0), 123_456_789_012_345_678_901_234_567_890);
        assert_eq!(qty.value(1), -2_500_000_000);
        // Oracle stores the empty string as null.
        assert!(b.column_by_name("NOTE").unwrap().is_null(0));
        // 03:30 EDT on 2026-03-08 is 07:30 UTC.
        assert_eq!(
            b.column_by_name("OPENED")
                .unwrap()
                .as_primitive::<TimestampMicrosecondType>()
                .value(0),
            1_772_955_000_000_000
        );
    }
}
