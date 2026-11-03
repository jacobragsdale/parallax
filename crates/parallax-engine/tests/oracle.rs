//! Differential tests: random views checked against DuckDB on the same data.
//!
//! DuckDB groups and filters the rows. Counts, decimal sums, minimums, maximums and
//! first/last values must match exactly. Floating-point sums must match an exactly
//! rounded reference computed from the very values DuckDB grouped, bit for bit.
//! DuckDB's own floating-point means, variances and percentiles are not exactly
//! rounded, so those compare within a relative tolerance.

mod common;

use std::collections::HashMap;

use arrow_array::cast::AsArray;
use arrow_array::types::Float64Type;
use arrow_array::{Array, RecordBatch};
use common::{Picker, duck, engine_table, fsum, number_at, positions, query, scalar_at};
use duckdb::Connection;
use parallax_datagen::calendar::from_epoch_days;
use parallax_engine::view::{GroupOrder, GroupSort, Sort, TopN};
use parallax_engine::{
    Aggregate, CellValue, ColumnType, Filter, FilterOp, GroupCell, Measure, Scalar, ShowAs, Table,
    View, ViewConfig,
};

const GROUPS: &[&str] = &[
    "fund",
    "strategy",
    "portfolio_manager",
    "asset_class",
    "sector",
    "industry",
    "country",
    "region",
    "currency",
    "long_short",
    "credit_rating",
    "ticker",
];
const SPLITS: &[&str] = &["asset_class", "long_short", "region"];
const DECIMALS: &[&str] = &[
    "quantity",
    "price_local",
    "market_value_usd",
    "daily_pnl_usd",
    "unrealized_pnl_usd",
];
const FLOATS: &[&str] = &[
    "weight_in_fund",
    "beta",
    "volatility_30d",
    "var_95_1d_usd",
    "duration",
];

fn ident(c: &str) -> String {
    format!("\"{c}\"")
}

fn scale_of(table: &Table, column: &str) -> i8 {
    match table
        .column(table.column_index(column).unwrap())
        .column_type()
    {
        ColumnType::Decimal { scale, .. } => *scale,
        _ => 0,
    }
}

fn literal(table: &Table, column: &str, value: &Scalar) -> String {
    match value {
        Scalar::Utf8(s) => format!("'{}'", s.replace('\'', "''")),
        Scalar::Int64(i) => i.to_string(),
        Scalar::Float64(f) => format!("'{f:?}'::DOUBLE"),
        Scalar::Decimal(d) => {
            let scale = scale_of(table, column) as u32;
            let sign = if *d < 0 { "-" } else { "" };
            let m = d.unsigned_abs();
            let p = 10u128.pow(scale);
            if scale == 0 {
                format!("{sign}{m}")
            } else {
                format!("{sign}{}.{:0width$}", m / p, m % p, width = scale as usize)
            }
        }
        Scalar::Date32(d) => format!("DATE '{}'", from_epoch_days(*d)),
        Scalar::Boolean(b) => b.to_string(),
        other => panic!("no literal for {other:?}"),
    }
}

fn filter_sql(table: &Table, f: &Filter) -> String {
    let c = ident(&f.column);
    let lit = |v: &Scalar| literal(table, &f.column, v);
    let text = matches!(
        table
            .column(table.column_index(&f.column).unwrap())
            .column_type(),
        ColumnType::Utf8
    );
    // Text ranges follow the engine's collation: case-insensitive, then bytes.
    let text_range = |op: &str, v: &Scalar| {
        let l = lit(v);
        let strict = op.trim_end_matches('=');
        format!("(lower({c}) {strict} lower({l}) OR (lower({c}) = lower({l}) AND {c} {op} {l}))")
    };
    match &f.op {
        FilterOp::Eq(v) => format!("{c} = {}", lit(v)),
        FilterOp::Ne(v) => format!("{c} <> {}", lit(v)),
        FilterOp::Lt(v) if text => text_range("<", v),
        FilterOp::Gt(v) if text => text_range(">", v),
        FilterOp::Lt(v) => format!("{c} < {}", lit(v)),
        FilterOp::Le(v) => format!("{c} <= {}", lit(v)),
        FilterOp::Gt(v) => format!("{c} > {}", lit(v)),
        FilterOp::Ge(v) => format!("{c} >= {}", lit(v)),
        FilterOp::In(vs) => {
            let listed: Vec<String> = vs.iter().filter(|v| **v != Scalar::Null).map(lit).collect();
            let values = if listed.is_empty() {
                "FALSE".to_owned()
            } else {
                format!("{c} IN ({})", listed.join(", "))
            };
            if vs.contains(&Scalar::Null) {
                format!("({values} OR {c} IS NULL)")
            } else {
                values
            }
        }
        FilterOp::NotIn(vs) => {
            format!(
                "{c} NOT IN ({})",
                vs.iter().map(lit).collect::<Vec<_>>().join(", ")
            )
        }
        FilterOp::IsNull => format!("{c} IS NULL"),
        FilterOp::IsNotNull => format!("{c} IS NOT NULL"),
        FilterOp::Contains(s) => {
            format!("contains(lower({c}), lower('{}'))", s.replace('\'', "''"))
        }
        FilterOp::StartsWith(s) => {
            format!(
                "starts_with(lower({c}), lower('{}'))",
                s.replace('\'', "''")
            )
        }
    }
}

fn where_clause(table: &Table, filters: &[Filter]) -> String {
    if filters.is_empty() {
        String::new()
    } else {
        format!(
            " WHERE {}",
            filters
                .iter()
                .map(|f| filter_sql(table, f))
                .collect::<Vec<_>>()
                .join(" AND ")
        )
    }
}

fn measure_sql(m: &Measure) -> String {
    let c = m.column.as_deref().map(ident).unwrap_or_default();
    match &m.aggregate {
        Aggregate::Count => format!("count({c})"),
        Aggregate::CountAll => "count(*)".into(),
        Aggregate::Sum => format!("sum({c})"),
        Aggregate::Mean => format!("avg({c})"),
        Aggregate::DistinctCount => format!("count(DISTINCT {c})"),
        Aggregate::Min => format!("min({c})"),
        Aggregate::Max => format!("max({c})"),
        Aggregate::Variance => format!("var_samp({c})"),
        Aggregate::StdDev => format!("stddev_samp({c})"),
        // DuckDB rounds a decimal's interpolated percentile to the column's scale;
        // the engine keeps PERCENTILE.INC's interpolated value as a double.
        Aggregate::Median => format!("quantile_cont({c}::DOUBLE, 0.5)"),
        Aggregate::Percentile(p) => format!("quantile_cont({c}::DOUBLE, {p})"),
        Aggregate::First => format!("arg_min({c}, __seq) FILTER (WHERE {c} IS NOT NULL)"),
        Aggregate::Last => format!("arg_max({c}, __seq) FILTER (WHERE {c} IS NOT NULL)"),
        Aggregate::WeightedMean => {
            let w = ident(m.weight.as_deref().unwrap());
            format!(
                "sum({c} * {w}) FILTER (WHERE {c} IS NOT NULL AND {w} IS NOT NULL) / sum({w}) FILTER (WHERE {c} IS NOT NULL AND {w} IS NOT NULL)"
            )
        }
    }
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return a.is_nan() && b.is_nan();
    }
    if a.is_infinite() || b.is_infinite() {
        return a == b;
    }
    (a - b).abs() <= tolerance * a.abs().max(b.abs()).max(1e-300) || (a - b).abs() < 1e-300
}

fn ours_number(v: &CellValue, scale: i8) -> Option<f64> {
    match v {
        CellValue::Null => None,
        CellValue::Count(c) => Some(*c as f64),
        CellValue::Float(f) => Some(*f),
        CellValue::Undefined => Some(f64::NAN),
        CellValue::Decimal(d) => Some(*d as f64 / 10f64.powi(i32::from(scale))),
        CellValue::Value(Scalar::Decimal(d)) => Some(*d as f64 / 10f64.powi(i32::from(scale))),
        CellValue::Value(Scalar::Float64(f)) => Some(*f),
        CellValue::Value(Scalar::Int64(i)) => Some(*i as f64),
        other => panic!("not a number: {other:?}"),
    }
}

/// Compares one engine value with DuckDB's. Returns a description of any mismatch.
fn check(
    table: &Table,
    m: &Measure,
    ours: &CellValue,
    theirs: &dyn Array,
    list: Option<&dyn Array>,
    row: usize,
) -> Result<(), String> {
    let column = m.column.as_deref();
    let scale = column.map_or(0, |c| scale_of(table, c));
    let is_float = column.is_some_and(|c| FLOATS.contains(&c));
    let mismatch = || {
        Err(format!(
            "{}: ours {ours:?}, DuckDB {:?}",
            m.label(),
            scalar_or_number(theirs, row)
        ))
    };
    match &m.aggregate {
        Aggregate::Count | Aggregate::CountAll | Aggregate::DistinctCount => {
            let expected = number_at(theirs, row).unwrap_or(0.0) as i64;
            if *ours == CellValue::Count(expected) {
                Ok(())
            } else {
                mismatch()
            }
        }
        Aggregate::Sum if !is_float => match (ours, scalar_at(theirs, row)) {
            (CellValue::Null, Scalar::Null) => Ok(()),
            (CellValue::Decimal(a), Scalar::Decimal(b)) if *a == b => Ok(()),
            _ => mismatch(),
        },
        Aggregate::Sum => {
            let values: Vec<f64> = match list {
                Some(l) if !l.is_null(row) => {
                    let l = l.as_list::<i32>().value(row);
                    l.as_primitive::<Float64Type>().iter().flatten().collect()
                }
                _ => Vec::new(),
            };
            if values.is_empty() {
                return if *ours == CellValue::Null {
                    Ok(())
                } else {
                    mismatch()
                };
            }
            let expected = fsum(&values);
            match ours {
                CellValue::Float(f) if f.to_bits() == expected.to_bits() => Ok(()),
                CellValue::Float(f) if *f == 0.0 && expected == 0.0 => Ok(()),
                CellValue::Undefined if expected.is_nan() => Ok(()),
                _ => Err(format!("{}: ours {ours:?}, exact {expected:?}", m.label())),
            }
        }
        Aggregate::Min | Aggregate::Max | Aggregate::First | Aggregate::Last => {
            let expected = scalar_at(theirs, row);
            let ok = match (ours, &expected) {
                (CellValue::Null, Scalar::Null) => true,
                (CellValue::Value(Scalar::Float64(a)), Scalar::Float64(b)) => a == b,
                (CellValue::Value(a), b) => a == b,
                _ => false,
            };
            if ok { Ok(()) } else { mismatch() }
        }
        _ => {
            let a = ours_number(ours, 0);
            let b = number_at(theirs, row);
            // Means and percentiles of decimals come out of the engine as doubles.
            let _ = scale;
            match (a, b) {
                (None, None) => Ok(()),
                (Some(a), Some(b)) if close(a, b, 1e-9) => Ok(()),
                _ => mismatch(),
            }
        }
    }
}

fn scalar_or_number(array: &dyn Array, row: usize) -> String {
    match array.data_type() {
        arrow_schema::DataType::Utf8 | arrow_schema::DataType::Utf8View => {
            format!("{:?}", scalar_at(array, row))
        }
        _ => format!("{:?}", number_at(array, row)),
    }
}

/// Checks every node of a grouped view against DuckDB, level by level.
fn verify_grouped(table: &Table, conn: &Connection, config: &ViewConfig) -> Result<(), String> {
    let mut view = View::new(table, config.clone()).map_err(|e| e.to_string())?;
    let cells = view.cells(table);
    let by_key: HashMap<String, &GroupCell> = cells
        .iter()
        .map(|c| (format!("{:?}", (&c.path, &c.split)), c))
        .collect();
    let filters = where_clause(table, &config.filters);
    let splits: Vec<Option<&String>> = if config.split_by.is_empty() {
        vec![None]
    } else {
        vec![None, Some(&config.split_by[0])]
    };
    for level in 0..=config.group_by.len() {
        for split in &splits {
            let mut keys: Vec<String> = config.group_by[..level].iter().map(|g| ident(g)).collect();
            if let Some(s) = split {
                keys.push(ident(s));
            }
            let mut select = keys.clone();
            select.push("count(*) AS __rows".into());
            for (i, m) in config.measures.iter().enumerate() {
                select.push(format!("{} AS m{i}", measure_sql(m)));
                if m.aggregate == Aggregate::Sum
                    && m.column.as_deref().is_some_and(|c| FLOATS.contains(&c))
                {
                    let c = ident(m.column.as_deref().unwrap());
                    select.push(format!("list({c}) FILTER (WHERE {c} IS NOT NULL) AS l{i}"));
                }
            }
            let group = if keys.is_empty() {
                String::new()
            } else {
                format!(" GROUP BY {}", keys.join(", "))
            };
            let sql = format!("SELECT {} FROM t{filters}{group}", select.join(", "));
            let batches = query(conn, &sql);
            let mut groups = 0;
            for batch in &batches {
                for row in 0..batch.num_rows() {
                    let rows = number_at(batch.column_by_name("__rows").unwrap().as_ref(), row)
                        .unwrap() as i64;
                    // Without GROUP BY, DuckDB returns one row even when nothing passes
                    // the filters, just as the engine always has a grand total.
                    if rows == 0 && !keys.is_empty() {
                        continue;
                    }
                    groups += 1;
                    let path: Vec<Scalar> = (0..level)
                        .map(|i| scalar_at(batch.column(i).as_ref(), row))
                        .collect();
                    let split_key =
                        split.map(|_| vec![scalar_at(batch.column(level).as_ref(), row)]);
                    let Some(cell) = by_key.get(&format!("{:?}", (&path, &split_key))) else {
                        return Err(format!("no engine cell for {path:?} {split_key:?}\n{sql}"));
                    };
                    if cell.rows != rows {
                        return Err(format!("rows {} vs {rows} at {path:?}\n{sql}", cell.rows));
                    }
                    for (i, m) in config.measures.iter().enumerate() {
                        let theirs = batch.column_by_name(&format!("m{i}")).unwrap();
                        let list = batch.column_by_name(&format!("l{i}"));
                        check(
                            table,
                            m,
                            &cell.values[i],
                            theirs.as_ref(),
                            list.map(|l| l.as_ref()),
                            row,
                        )
                        .map_err(|e| format!("{e} at {path:?} {split_key:?}\n{sql}"))?;
                    }
                }
            }
            let ours = cells
                .iter()
                .filter(|c| c.path.len() == level && c.split.is_some() == split.is_some())
                .count();
            if ours != groups {
                return Err(format!("{ours} engine groups vs {groups} in DuckDB\n{sql}"));
            }
        }
    }
    Ok(())
}

fn value_of(table: &Table, column: &str, picker: &mut Picker) -> Scalar {
    let c = table.column(table.column_index(column).unwrap());
    let slots: Vec<usize> = table.live_slots().collect();
    for _ in 0..50 {
        let s = c.scalar(slots[picker.below(slots.len())]);
        if s != Scalar::Null {
            return s;
        }
    }
    Scalar::Null
}

fn random_filter(table: &Table, picker: &mut Picker) -> Option<Filter> {
    let numeric = picker.chance(40);
    if numeric {
        let columns = if picker.chance(50) { DECIMALS } else { FLOATS };
        let column = *picker.pick(columns);
        let v = value_of(table, column, picker);
        if v == Scalar::Null {
            return None;
        }
        let op = match picker.below(4) {
            0 => FilterOp::Gt(v),
            1 => FilterOp::Lt(v),
            2 => FilterOp::Ge(v),
            _ => FilterOp::Le(v),
        };
        return Some(Filter::new(column, op));
    }
    let column = *picker.pick(GROUPS);
    let v = value_of(table, column, picker);
    let op = match picker.below(9) {
        0 if v != Scalar::Null => FilterOp::Eq(v),
        1 if v != Scalar::Null => FilterOp::Ne(v),
        // Either value may be null, which an "in" list matches as blank.
        2 => FilterOp::In(vec![v, value_of(table, column, picker)]),
        3 if v != Scalar::Null => FilterOp::NotIn(vec![v]),
        4 => FilterOp::IsNull,
        5 => FilterOp::IsNotNull,
        6 => FilterOp::Contains(picker.pick(&["a", "E", "in", "ou", "x"]).to_string()),
        7 if v != Scalar::Null => FilterOp::Lt(v),
        8 => FilterOp::StartsWith(picker.pick(&["a", "C", "Me", "u"]).to_string()),
        _ => return None,
    };
    Some(Filter::new(column, op))
}

fn random_measure(picker: &mut Picker) -> Measure {
    let numeric = [
        Aggregate::Sum,
        Aggregate::Mean,
        Aggregate::Count,
        Aggregate::DistinctCount,
        Aggregate::Min,
        Aggregate::Max,
        Aggregate::Variance,
        Aggregate::StdDev,
        Aggregate::Median,
        Aggregate::Percentile(0.9),
        Aggregate::First,
        Aggregate::Last,
    ];
    match picker.below(10) {
        0 => Measure::count_all(),
        1 => Measure::new(
            picker
                .pick(&[
                    Aggregate::DistinctCount,
                    Aggregate::First,
                    Aggregate::Last,
                    Aggregate::Count,
                ])
                .clone(),
            picker.pick(&["ticker", "sector", "credit_rating"]),
        ),
        _ => {
            let columns = if picker.chance(50) { DECIMALS } else { FLOATS };
            let column = *picker.pick(columns);
            Measure::new(picker.pick(&numeric).clone(), column)
        }
    }
}

fn random_config(table: &Table, picker: &mut Picker) -> ViewConfig {
    let levels = picker.below(4);
    let group_by: Vec<String> = picker
        .sample(GROUPS, levels)
        .into_iter()
        .map(String::from)
        .collect();
    let split_by = if picker.chance(30) {
        vec![picker.pick(SPLITS).to_string()]
    } else {
        Vec::new()
    };
    let measures = (0..1 + picker.below(4))
        .map(|_| random_measure(picker))
        .collect();
    let filters = (0..picker.below(3))
        .filter_map(|_| random_filter(table, picker))
        .collect();
    ViewConfig {
        group_by,
        split_by,
        measures,
        filters,
        ..ViewConfig::default()
    }
}

fn run_grouped(hostile: bool, seed: u64, configs: usize) {
    let batch = positions(4_000, 300, hostile, seed);
    let table = engine_table(&batch);
    let conn = duck(&batch);
    let mut picker = Picker::new(seed);
    let mut failures = Vec::new();
    for i in 0..configs {
        let config = random_config(&table, &mut picker);
        if let Err(e) = verify_grouped(&table, &conn, &config) {
            failures.push(format!("config {i}: {config:?}\n{e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {configs} configs failed:\n{}",
        failures.len(),
        failures[..failures.len().min(3)].join("\n\n")
    );
}

#[test]
fn grouped_views_match_duckdb() {
    run_grouped(false, 1, 150);
}

#[test]
fn grouped_views_match_duckdb_on_hostile_data() {
    run_grouped(true, 2, 150);
}

fn order_sql(table: &Table, sorts: &[Sort]) -> String {
    let mut keys = Vec::new();
    for s in sorts {
        let c = ident(&s.column);
        let dir = if s.descending { " DESC" } else { "" };
        let text = matches!(
            table
                .column(table.column_index(&s.column).unwrap())
                .column_type(),
            ColumnType::Utf8
        );
        if text {
            keys.push(format!("lower({c}){dir} NULLS LAST"));
        }
        keys.push(format!("{c}{dir} NULLS LAST"));
    }
    keys.push("position_id".into());
    keys.join(", ")
}

fn run_flat(hostile: bool, seed: u64, configs: usize) {
    let batch = positions(3_000, 300, hostile, seed);
    let table = engine_table(&batch);
    let conn = duck(&batch);
    let mut picker = Picker::new(seed + 100);
    let sortable: Vec<&str> = GROUPS
        .iter()
        .chain(DECIMALS)
        .chain(FLOATS)
        .copied()
        .collect();
    for i in 0..configs {
        let filters: Vec<Filter> = (0..picker.below(3))
            .filter_map(|_| random_filter(&table, &mut picker))
            .collect();
        let count = 1 + picker.below(2);
        let sort: Vec<Sort> = picker
            .sample(&sortable, count)
            .into_iter()
            .map(|c| Sort {
                column: c.to_owned(),
                descending: picker.chance(50),
            })
            .collect();
        let config = ViewConfig {
            filters: filters.clone(),
            sort: sort.clone(),
            ..ViewConfig::default()
        };
        let mut view = View::new(&table, config).unwrap();
        let n = view.num_rows(&table);
        let frame = view.window(&table, 0..n, 0..0);
        let ours: Vec<i64> = frame
            .batch
            .column(0)
            .as_primitive::<arrow_array::types::Int64Type>()
            .values()
            .to_vec();
        let sql = format!(
            "SELECT __seq FROM t{} ORDER BY {}",
            where_clause(&table, &filters),
            order_sql(&table, &sort)
        );
        let theirs: Vec<i64> = query(&conn, &sql)
            .iter()
            .flat_map(|b: &RecordBatch| {
                b.column(0)
                    .as_primitive::<arrow_array::types::Int64Type>()
                    .values()
                    .to_vec()
            })
            .collect();
        assert_eq!(ours, theirs, "flat config {i}\n{sql}");
    }
}

#[test]
fn flat_views_match_duckdb_order() {
    run_flat(false, 3, 60);
}

#[test]
fn flat_views_match_duckdb_order_on_hostile_data() {
    run_flat(true, 4, 60);
}

#[test]
fn mixed_units_are_flagged_not_summed() {
    let batch = positions(4_000, 300, false, 5);
    let table = engine_table(&batch);
    let conn = duck(&batch);
    let config = ViewConfig {
        group_by: vec!["fund".into(), "country".into()],
        measures: vec![
            Measure::new(Aggregate::Sum, "market_value_local").with_unit_column("currency"),
        ],
        ..ViewConfig::default()
    };
    let mut view = View::new(&table, config).unwrap();
    let cells = view.cells(&table);
    let sql = "SELECT fund, country, count(DISTINCT currency) AS units, sum(market_value_local) AS s FROM t GROUP BY fund, country";
    let mut checked = 0;
    for batch in query(&conn, sql) {
        for row in 0..batch.num_rows() {
            let path = vec![
                scalar_at(batch.column(0).as_ref(), row),
                scalar_at(batch.column(1).as_ref(), row),
            ];
            let cell = cells
                .iter()
                .find(|c| c.path == path && c.split.is_none())
                .unwrap();
            let units = number_at(batch.column(2).as_ref(), row).unwrap();
            if units > 1.0 {
                assert_eq!(cell.values[0], CellValue::Mixed, "{path:?}");
            } else {
                assert_eq!(
                    cell.values[0],
                    CellValue::Decimal(match scalar_at(batch.column(3).as_ref(), row) {
                        Scalar::Decimal(d) => d,
                        other => panic!("{other:?}"),
                    })
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 10);
    // Fund level mixes currencies almost everywhere; the grand total certainly does.
    let total = cells
        .iter()
        .find(|c| c.path.is_empty() && c.split.is_none())
        .unwrap();
    assert_eq!(total.values[0], CellValue::Mixed);
}

#[test]
fn percent_of_total_and_top_n_with_other() {
    let batch = positions(4_000, 300, false, 6);
    let table = engine_table(&batch);
    let conn = duck(&batch);
    let config = ViewConfig {
        group_by: vec!["sector".into()],
        measures: vec![
            Measure::new(Aggregate::Sum, "market_value_usd"),
            Measure::new(Aggregate::Sum, "market_value_usd").show_as(ShowAs::PercentOfGrandTotal),
        ],
        group_sort: GroupSort {
            by: GroupOrder::Measure(0),
            descending: true,
        },
        top_n: Some(TopN {
            n: 3,
            measure: 0,
            largest: true,
            other: true,
        }),
        ..ViewConfig::default()
    };
    let mut view = View::new(&table, config).unwrap();
    let rows = view.num_rows(&table);
    assert_eq!(rows, 1 + 3 + 1, "total, three sectors, other");
    let frame = view.window(&table, 0..rows, 0..2);
    let kinds: Vec<&str> = frame
        .batch
        .column_by_name("__kind")
        .unwrap()
        .as_string::<i32>()
        .iter()
        .flatten()
        .collect();
    assert_eq!(kinds, vec!["total", "leaf", "leaf", "leaf", "other"]);
    let sectors: Vec<Option<&str>> = frame
        .batch
        .column_by_name("sector")
        .unwrap()
        .as_string::<i32>()
        .iter()
        .collect();
    let expected = query(
        &conn,
        "SELECT sector, sum(market_value_usd) s FROM t GROUP BY sector ORDER BY s DESC LIMIT 3",
    );
    let top: Vec<Scalar> = (0..3)
        .map(|i| scalar_at(expected[0].column(0).as_ref(), i))
        .collect();
    for (i, t) in top.iter().enumerate() {
        assert_eq!(
            sectors[i + 1]
                .map(|s| Scalar::Utf8(s.to_owned()))
                .unwrap_or(Scalar::Null),
            *t
        );
    }
    // The Other row is aggregated from the base rows of the remaining sectors.
    let in_list = top
        .iter()
        .map(|s| match s {
            Scalar::Utf8(t) => format!("'{}'", t.replace('\'', "''")),
            _ => "NULL".into(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let other = query(
        &conn,
        &format!(
            "SELECT sum(market_value_usd) FROM t WHERE sector NOT IN ({in_list}) OR sector IS NULL"
        ),
    );
    let sums = frame
        .batch
        .column_by_name("sum(market_value_usd)")
        .unwrap()
        .as_primitive::<arrow_array::types::Decimal128Type>();
    assert_eq!(
        Scalar::Decimal(sums.value(4)),
        scalar_at(other[0].column(0).as_ref(), 0)
    );
    let percent = frame
        .batch
        .column_by_name("sum(market_value_usd) % of total")
        .unwrap()
        .as_primitive::<Float64Type>();
    assert_eq!(percent.value(0), 1.0);
    let grand = sums.value(0) as f64;
    for i in 1..5 {
        assert!(close(percent.value(i), sums.value(i) as f64 / grand, 1e-12));
    }
}
