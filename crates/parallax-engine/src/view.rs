//! Views: filtered, grouped, pivoted and sorted results of a table, returned as Arrow
//! windows.
//!
//! A grouped view keeps a tree of nodes. The root is the grand total, each level
//! below it groups by one more column, and every node's state is fed directly by base
//! rows, so no total is ever computed from other totals. Column pivots keep one cell
//! per node and column key next to the node's own total.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int32Array, Int64Array,
    RecordBatch, StringArray, TimestampMicrosecondArray, UInt8Array,
};
use arrow_schema::{DataType, Field, Schema};
use rustc_hash::FxHashMap;

use crate::aggregate::{
    Aggregate, CompiledMeasure, Input, Measure, MeasureValue, ShowAs, State, finish, pow10,
};
use crate::bignum::ratio_to_f64;
use crate::column::Column;
use crate::error::ConfigError;
use crate::filter::{CompiledFilter, Filter};
use crate::format::{self, Format, Formatter};
use crate::table::{ChangeSet, RemovedRows, Table};
use crate::types::{ColumnType, Key, Scalar};

/// A sort on one column of a flat view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sort {
    /// The column.
    pub column: String,
    /// Largest first. Nulls sort last either way.
    pub descending: bool,
}

/// What orders the children of each group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupOrder {
    /// The group's own value.
    Key,
    /// The value of a measure, by index.
    Measure(usize),
}

/// How groups are ordered among their siblings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupSort {
    /// What to order by.
    pub by: GroupOrder,
    /// Largest first. Nulls sort last either way.
    pub descending: bool,
}

impl Default for GroupSort {
    fn default() -> Self {
        Self {
            by: GroupOrder::Key,
            descending: false,
        }
    }
}

/// A comparison for aggregate filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
}

/// An aggregate filter: groups whose measure fails it are hidden. Totals still
/// include their rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Having {
    /// The measure tested, by index.
    pub measure: usize,
    /// The comparison.
    pub op: Comparison,
    /// The value compared with.
    pub value: f64,
}

/// Keep the `n` largest or smallest groups among each set of siblings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TopN {
    /// How many groups to keep.
    pub n: usize,
    /// The measure ranked, by index.
    pub measure: usize,
    /// Keep the largest values rather than the smallest.
    pub largest: bool,
    /// Show the rest as one "Other" row, aggregated from their base rows.
    pub other: bool,
}

/// What a view computes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewConfig {
    /// Columns of a flat view, in order. Empty means every column.
    pub columns: Vec<String>,
    /// Row pivots, outermost first. Empty, with no measures, makes a flat view.
    pub group_by: Vec<String>,
    /// Column pivots.
    pub split_by: Vec<String>,
    /// Measures of a grouped view.
    pub measures: Vec<Measure>,
    /// Row filters.
    pub filters: Vec<Filter>,
    /// Sorts of a flat view.
    pub sort: Vec<Sort>,
    /// Ordering of groups.
    pub group_sort: GroupSort,
    /// Aggregate filters.
    pub having: Vec<Having>,
    /// Top-N per set of siblings.
    pub top_n: Option<TopN>,
    /// Levels expanded by default: 0 shows the grand total and the first level, 1
    /// also shows the second level, and so on.
    pub expand_depth: usize,
    /// Display formats by source column. A measure without its own format takes its
    /// column's, except counts and variances.
    pub formats: BTreeMap<String, Format>,
}

impl ViewConfig {
    fn is_grouped(&self) -> bool {
        !self.group_by.is_empty() || !self.split_by.is_empty() || !self.measures.is_empty()
    }
}

/// A computed value of a grouped view, for callers and tests.
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    /// No value.
    Null,
    /// A count.
    Count(i64),
    /// A double.
    Float(f64),
    /// An unscaled decimal at the measure's scale.
    Decimal(i128),
    /// A source-column value, from minimum, maximum, first or last.
    Value(Scalar),
    /// The exact result exceeds 38 digits.
    Overflow,
    /// The group mixes units.
    Mixed,
    /// The result is undefined, such as a percentage of zero.
    Undefined,
}

/// One node of a grouped view and its values.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupCell {
    /// The group values from the outermost level down; empty for the grand total.
    pub path: Vec<Scalar>,
    /// The column-pivot values, or `None` for the row total.
    pub split: Option<Vec<Scalar>>,
    /// Rows that passed the filters.
    pub rows: i64,
    /// One value per measure.
    pub values: Vec<CellValue>,
}

/// A window of a view, ready to send.
#[derive(Clone, Debug)]
pub struct Frame {
    /// The rows and columns asked for.
    pub batch: RecordBatch,
    /// Visible rows in the whole view.
    pub total_rows: usize,
    /// Data columns in the whole view.
    pub total_columns: usize,
    /// The view configuration's version.
    pub config_version: u64,
    /// The table load the frame reflects.
    pub load_version: u64,
}

/// Field metadata on frames. Every field carries a role, so a client finds each
/// column by role and index rather than by name: names are for people and can
/// repeat, for instance when two measures are the same.
pub mod meta {
    /// The field's role: one of the constants in [`role`](super::role).
    pub const ROLE: &str = "parallax.role";
    /// For data-column roles, the column's position within the frame.
    pub const INDEX: &str = "parallax.index";
    /// For group keys, the level, from 0 for the outermost.
    pub const LEVEL: &str = "parallax.level";
    /// For values, the header text, which names the display zone of instants.
    pub const TITLE: &str = "parallax.title";
    /// For values of grouped views, the position of the measure in the layout.
    pub const MEASURE: &str = "parallax.measure";
    /// For values under column pivots, the pivot values as a JSON array.
    pub const SPLIT: &str = "parallax.split";
}

/// The roles a frame field can have.
pub mod role {
    /// A flat view row's insertion sequence.
    pub const SEQ: &str = "seq";
    /// A grouped row's depth.
    pub const DEPTH: &str = "depth";
    /// A grouped row's kind: total, group, leaf or other.
    pub const KIND: &str = "kind";
    /// Whether a grouped row is expanded, null for rows that cannot expand.
    pub const EXPANDED: &str = "expanded";
    /// A grouped row's display label.
    pub const LABEL: &str = "label";
    /// A group key at one level.
    pub const GROUP: &str = "group";
    /// A data column's values.
    pub const VALUE: &str = "value";
    /// A data column's cell flags.
    pub const FLAG: &str = "flag";
    /// A data column's display text.
    pub const TEXT: &str = "text";
    /// A data column's text for copying, with every digit, for timestamps.
    pub const COPY: &str = "copy";
}

/// Cell flags carried next to each value column.
pub mod flag {
    /// The value is valid.
    pub const OK: u8 = 0;
    /// The exact result exceeds 38 decimal digits.
    pub const OVERFLOW: u8 = 1;
    /// The group mixes units.
    pub const MIXED: u8 = 2;
    /// The value is undefined.
    pub const UNDEFINED: u8 = 3;
}

/// Where a row is read from: the table, or the old images in a change set.
#[derive(Clone, Copy)]
enum Source<'a> {
    Table(&'a Table),
    Removed(&'a Table, &'a RemovedRows),
}

impl<'a> Source<'a> {
    fn column(&self, c: usize) -> &'a Column {
        match self {
            Self::Table(t) => t.column(c),
            Self::Removed(_, r) => &r.columns[c],
        }
    }

    /// The key of `row` in the table's dictionary domain.
    fn key(&self, c: usize, row: usize) -> Key {
        match self {
            Self::Table(t) => t.column(c).key(row),
            Self::Removed(t, r) => {
                let key = r.columns[c].key(row);
                match r.columns[c].text(key) {
                    Some(text) => Key::Str(
                        t.column(c)
                            .dictionary()
                            .and_then(|d| d.code(text))
                            .expect("removed text was interned in the table"),
                    ),
                    None => key,
                }
            }
        }
    }

    fn seq(&self, row: usize) -> u64 {
        match self {
            Self::Table(t) => t.seq(row),
            Self::Removed(_, r) => r.seq[row],
        }
    }
}

#[derive(Clone, Debug)]
struct Cell {
    rows: i64,
    states: Vec<State>,
    units: Vec<Option<FxHashMap<Key, u32>>>,
}

impl Cell {
    fn new(measures: &[CompiledMeasure], leaf: bool) -> Self {
        Self {
            rows: 0,
            states: measures.iter().map(|m| m.empty(leaf)).collect(),
            units: measures
                .iter()
                .map(|m| m.unit.map(|_| FxHashMap::default()))
                .collect(),
        }
    }

    fn apply(&mut self, measures: &[CompiledMeasure], inputs: &[Input], sign: i64) {
        self.rows += sign;
        for (i, m) in measures.iter().enumerate() {
            self.states[i].apply(m, &inputs[i], sign);
            if let (Some(units), Some(unit)) = (&mut self.units[i], inputs[i].unit)
                && (inputs[i].key != Key::Null || m.aggregate == Aggregate::CountAll)
            {
                let entry = units.entry(unit).or_insert(0);
                if sign > 0 {
                    *entry += 1;
                } else {
                    *entry -= 1;
                    if *entry == 0 {
                        units.remove(&unit);
                    }
                }
            }
        }
    }

    fn merge(&mut self, other: &Self) {
        self.rows += other.rows;
        for (a, b) in self.states.iter_mut().zip(&other.states) {
            a.merge(b);
        }
        for (a, b) in self.units.iter_mut().zip(&other.units) {
            if let (Some(a), Some(b)) = (a, b) {
                for (&k, &c) in b {
                    *a.entry(k).or_insert(0) += c;
                }
            }
        }
    }
}

const NO_PARENT: u32 = u32::MAX;
const ROW_TOTAL: u32 = u32::MAX;

#[derive(Clone, Debug)]
struct Node {
    key: Key,
    depth: u32,
    parent: u32,
    children: FxHashMap<Key, u32>,
    total: Cell,
    split: FxHashMap<u32, Cell>,
}

#[derive(Clone, Debug)]
enum Visible {
    Node(u32),
    Other { parent: u32, members: Vec<u32> },
}

#[derive(Clone, Debug)]
struct Grouped {
    measures: Vec<CompiledMeasure>,
    group_columns: Vec<usize>,
    split_columns: Vec<usize>,
    nodes: Vec<Option<Node>>,
    free: Vec<u32>,
    split_ids: FxHashMap<Box<[Key]>, u32>,
    split_keys: Vec<Box<[Key]>>,
    split_rows: Vec<i64>,
    expanded: Vec<(Vec<Scalar>, bool)>,
    visible: Option<Vec<Visible>>,
    columns: Option<Vec<(u32, usize)>>,
    /// Values of lazy measures above the leaves, by (node, split, measure). Cleared
    /// whenever the view changes.
    lazy_values: RefCell<FxHashMap<(u32, u32, usize), MeasureValue>>,
    scratch_inputs: Vec<Input>,
    scratch_keys: Vec<Key>,
    scratch_path: Vec<u32>,
}

#[derive(Clone, Debug)]
struct Flat {
    columns: Vec<usize>,
    sort: Vec<(usize, bool)>,
    order: Vec<usize>,
}

#[derive(Clone, Debug)]
enum Kind {
    Flat(Flat),
    Grouped(Box<Grouped>),
}

/// A view of a table.
#[derive(Clone, Debug)]
pub struct View {
    config: ViewConfig,
    filters: Vec<CompiledFilter>,
    formatters: Formatters,
    kind: Kind,
    config_version: u64,
    load_version: u64,
}

/// The checked formats of a view: one per table column and one per measure.
#[derive(Clone, Debug)]
struct Formatters {
    columns: Vec<Formatter>,
    measures: Vec<Formatter>,
}

impl Formatters {
    fn new(table: &Table, config: &ViewConfig) -> Result<Self, ConfigError> {
        let compile = |column: &str, format: &Format| {
            Formatter::new(format).map_err(|e| ConfigError::Format {
                column: column.to_owned(),
                reason: e.to_string(),
            })
        };
        let mut by_index: HashMap<usize, Formatter> = HashMap::new();
        for (name, format) in &config.formats {
            by_index.insert(column_index(table, name)?, compile(name, format)?);
        }
        let default = Formatter::default();
        let columns = (0..table.schema().fields().len())
            .map(|c| by_index.remove(&c).unwrap_or_else(|| default.clone()))
            .collect();
        let measures = config
            .measures
            .iter()
            .map(|m| {
                let format =
                    measure_format(m, m.column.as_ref().and_then(|c| config.formats.get(c)));
                compile(&m.label(), &format)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { columns, measures })
    }
}

/// A configuration with its display formats removed, for comparing what is
/// computed.
fn without_formats(config: &ViewConfig) -> ViewConfig {
    let mut stripped = config.clone();
    stripped.formats.clear();
    for m in &mut stripped.measures {
        m.format = None;
    }
    stripped
}

/// The format a measure shows with: its own, else a percentage for shares, the
/// type's default for counts and variances, and otherwise its column's format.
fn measure_format(measure: &Measure, column: Option<&Format>) -> Format {
    if let Some(f) = &measure.format {
        return f.clone();
    }
    if measure.show_as != ShowAs::Value {
        return Format {
            percent: true,
            ..Format::default()
        };
    }
    match measure.aggregate {
        Aggregate::Count | Aggregate::CountAll | Aggregate::DistinctCount | Aggregate::Variance => {
            Format {
                zone: column.and_then(|c| c.zone.clone()),
                ..Format::default()
            }
        }
        _ => column.cloned().unwrap_or_default(),
    }
}

/// The most data columns a column pivot may produce.
pub const MAX_COLUMNS: usize = 10_000;

fn column_index(table: &Table, name: &str) -> Result<usize, ConfigError> {
    table
        .column_index(name)
        .ok_or_else(|| ConfigError::UnknownColumn(name.to_owned()))
}

fn compile_measure(table: &Table, m: &Measure) -> Result<CompiledMeasure, ConfigError> {
    let column = m
        .column
        .as_deref()
        .map(|c| column_index(table, c))
        .transpose()?;
    let column_type = column.map(|c| table.column(c).column_type().clone());
    if m.aggregate != Aggregate::CountAll && column.is_none() {
        return Err(ConfigError::UnknownColumn("(none)".into()));
    }
    if m.aggregate.needs_numeric() && !column_type.as_ref().is_some_and(ColumnType::is_numeric) {
        return Err(ConfigError::AggregateType {
            aggregate: m.aggregate.name(),
            column: m.column.clone().unwrap_or_default(),
            data_type: format!("{column_type:?}"),
        });
    }
    if let Aggregate::Percentile(p) = m.aggregate
        && !(0.0..=1.0).contains(&p)
    {
        return Err(ConfigError::Percentile(p.to_string()));
    }
    let weight = m
        .weight
        .as_deref()
        .map(|c| column_index(table, c))
        .transpose()?;
    let weight_type = weight.map(|c| table.column(c).column_type().clone());
    if m.aggregate == Aggregate::WeightedMean
        && !weight_type.as_ref().is_some_and(ColumnType::is_numeric)
    {
        return Err(ConfigError::AggregateType {
            aggregate: "wmean weight".into(),
            column: m.weight.clone().unwrap_or_default(),
            data_type: format!("{weight_type:?}"),
        });
    }
    let unit = m
        .unit_column
        .as_deref()
        .map(|c| column_index(table, c))
        .transpose()?;
    Ok(CompiledMeasure {
        aggregate: m.aggregate.clone(),
        column,
        column_type,
        weight,
        weight_type,
        unit,
        show_as: m.show_as,
    })
}

impl View {
    /// Builds a view of `table`.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration names unknown columns, applies an
    /// aggregate to the wrong type, or refers to a measure that does not exist.
    pub fn new(table: &Table, config: ViewConfig) -> Result<Self, ConfigError> {
        let filters = config
            .filters
            .iter()
            .map(|f| CompiledFilter::compile(table, f))
            .collect::<Result<Vec<_>, _>>()?;
        let formatters = Formatters::new(table, &config)?;
        let kind = if config.is_grouped() {
            let measures = config
                .measures
                .iter()
                .map(|m| compile_measure(table, m))
                .collect::<Result<Vec<_>, _>>()?;
            let n = measures.len();
            let check = |i: usize| {
                if i < n {
                    Ok(())
                } else {
                    Err(ConfigError::UnknownMeasure(i))
                }
            };
            if let GroupOrder::Measure(i) = config.group_sort.by {
                check(i)?;
            }
            for h in &config.having {
                check(h.measure)?;
            }
            if let Some(t) = &config.top_n {
                check(t.measure)?;
            }
            Kind::Grouped(Box::new(Grouped {
                measures,
                group_columns: config
                    .group_by
                    .iter()
                    .map(|c| column_index(table, c))
                    .collect::<Result<_, _>>()?,
                split_columns: config
                    .split_by
                    .iter()
                    .map(|c| column_index(table, c))
                    .collect::<Result<_, _>>()?,
                nodes: Vec::new(),
                free: Vec::new(),
                split_ids: FxHashMap::default(),
                split_keys: Vec::new(),
                split_rows: Vec::new(),
                expanded: Vec::new(),
                visible: None,
                columns: None,
                lazy_values: RefCell::default(),
                scratch_inputs: Vec::new(),
                scratch_keys: Vec::new(),
                scratch_path: Vec::new(),
            }))
        } else {
            let columns = if config.columns.is_empty() {
                (0..table.schema().fields().len()).collect()
            } else {
                config
                    .columns
                    .iter()
                    .map(|c| column_index(table, c))
                    .collect::<Result<_, _>>()?
            };
            let sort = config
                .sort
                .iter()
                .map(|s| Ok((column_index(table, &s.column)?, s.descending)))
                .collect::<Result<_, ConfigError>>()?;
            Kind::Flat(Flat {
                columns,
                sort,
                order: Vec::new(),
            })
        };
        let mut view = Self {
            config,
            filters,
            formatters,
            kind,
            config_version: 1,
            load_version: table.load_version(),
        };
        view.check_column_limit(table)?;
        view.rebuild(table);
        Ok(view)
    }

    /// Refuses column pivots that would produce more than [`MAX_COLUMNS`] data
    /// columns, stopping as soon as the limit is passed.
    fn check_column_limit(&self, table: &Table) -> Result<(), ConfigError> {
        let Kind::Grouped(g) = &self.kind else {
            return Ok(());
        };
        if g.split_columns.is_empty() {
            return Ok(());
        }
        let per_key = g.measures.len().max(1);
        let limit = MAX_COLUMNS / per_key;
        let mut seen: rustc_hash::FxHashSet<Box<[Key]>> = rustc_hash::FxHashSet::default();
        let mut keys = Vec::with_capacity(g.split_columns.len());
        for slot in table.live_slots() {
            if !self.passes(Source::Table(table), slot) {
                continue;
            }
            keys.clear();
            keys.extend(g.split_columns.iter().map(|&c| table.column(c).key(slot)));
            if !seen.contains(keys.as_slice()) {
                seen.insert(keys.as_slice().into());
                if seen.len() > limit {
                    return Err(ConfigError::TooManyColumns(
                        (seen.len() + 1) * per_key,
                        MAX_COLUMNS,
                    ));
                }
            }
        }
        Ok(())
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &ViewConfig {
        &self.config
    }

    /// Switches to `config` without recomputing anything, if it differs from the
    /// current configuration only in display formats. Returns `false`, changing
    /// nothing, when it differs in anything else.
    ///
    /// # Errors
    ///
    /// Returns an error if a new format is invalid; the view is then unchanged.
    pub fn reformat(&mut self, table: &Table, config: &ViewConfig) -> Result<bool, ConfigError> {
        if without_formats(&self.config) != without_formats(config) {
            return Ok(false);
        }
        self.formatters = Formatters::new(table, config)?;
        self.config = config.clone();
        Ok(true)
    }

    /// The configuration version, which every frame carries.
    #[must_use]
    pub fn config_version(&self) -> u64 {
        self.config_version
    }

    /// The table load this view reflects.
    #[must_use]
    pub fn load_version(&self) -> u64 {
        self.load_version
    }

    fn passes(&self, source: Source<'_>, row: usize) -> bool {
        self.filters
            .iter()
            .all(|f| f.passes(source.column(f.column), row))
    }

    /// Recomputes the view from the table.
    pub fn rebuild(&mut self, table: &Table) {
        self.load_version = table.load_version();
        let filters = std::mem::take(&mut self.filters);
        let passes = |slot: usize| {
            filters
                .iter()
                .all(|f| f.passes(table.column(f.column), slot))
        };
        match &mut self.kind {
            Kind::Flat(flat) => {
                flat.order = table.live_slots().filter(|&s| passes(s)).collect();
                sort_flat(table, flat);
            }
            Kind::Grouped(g) => {
                g.nodes.clear();
                g.free.clear();
                g.split_ids.clear();
                g.split_keys.clear();
                g.split_rows.clear();
                g.alloc(Key::Null, 0, NO_PARENT);
                let source = Source::Table(table);
                for slot in table.live_slots() {
                    if passes(slot) {
                        g.apply_row(source, slot, 1);
                    }
                }
                g.invalidate();
            }
        }
        self.filters = filters;
    }

    /// Applies a table change so that the view equals one rebuilt on the new table.
    pub fn apply(&mut self, table: &Table, change: &ChangeSet) {
        self.load_version = change.load_version;
        if matches!(self.kind, Kind::Flat(_)) {
            self.rebuild(table);
            return;
        }
        let removed = Source::Removed(table, &change.removed);
        let current = Source::Table(table);
        let pass_removed: Vec<usize> = (0..change.removed.len())
            .filter(|&r| self.passes(removed, r))
            .collect();
        let pass_added: Vec<usize> = change
            .added
            .iter()
            .copied()
            .filter(|&s| self.passes(current, s))
            .collect();
        let Kind::Grouped(g) = &mut self.kind else {
            unreachable!()
        };
        for r in pass_removed {
            g.apply_row(removed, r, -1);
        }
        for s in pass_added {
            g.apply_row(current, s, 1);
        }
        g.invalidate();
    }

    /// Expands or collapses the group at `path`. The setting is kept by value, so it
    /// survives refreshes and re-sorts.
    pub fn set_expanded(&mut self, path: Vec<Scalar>, expanded: bool) {
        if let Kind::Grouped(g) = &mut self.kind {
            g.expanded.retain(|(p, _)| *p != path);
            g.expanded.push((path, expanded));
            g.visible = None;
        }
    }

    /// The number of visible rows.
    pub fn num_rows(&mut self, table: &Table) -> usize {
        match &mut self.kind {
            Kind::Flat(flat) => flat.order.len(),
            Kind::Grouped(g) => g.visible(table, &self.config).len(),
        }
    }

    /// The number of data columns.
    pub fn num_columns(&mut self, table: &Table) -> usize {
        match &mut self.kind {
            Kind::Flat(flat) => flat.columns.len(),
            Kind::Grouped(g) => g.data_columns(table).len(),
        }
    }

    /// Returns rows `rows` and data columns `columns` of the view.
    pub fn window(&mut self, table: &Table, rows: Range<usize>, columns: Range<usize>) -> Frame {
        let (batch, total_rows, total_columns) = match &mut self.kind {
            Kind::Flat(flat) => flat_window(table, &self.formatters, flat, rows, columns),
            Kind::Grouped(g) => g.window(table, &self.config, &self.formatters, rows, columns),
        };
        Frame {
            batch,
            total_rows,
            total_columns,
            config_version: self.config_version,
            load_version: self.load_version,
        }
    }

    /// Returns the group values of visible row `row` of a grouped view, from the
    /// outermost level down: empty for the grand total, `None` for an Other row, a
    /// row out of range, or a flat view.
    pub fn row_path(&mut self, table: &Table, row: usize) -> Option<Vec<Scalar>> {
        let Kind::Grouped(g) = &mut self.kind else {
            return None;
        };
        match g.visible(table, &self.config).get(row)? {
            Visible::Node(id) => {
                let id = *id;
                Some(g.path(table, id))
            }
            Visible::Other { .. } => None,
        }
    }

    /// Returns a flat view configuration of the base rows under visible row `row`:
    /// the view's filters plus one equality filter per group level.
    pub fn drill_through(&mut self, table: &Table, row: usize) -> Option<ViewConfig> {
        let path = self.row_path(table, row)?;
        let mut filters = self.config.filters.clone();
        for (column, value) in self.config.group_by.iter().zip(path) {
            let op = match value {
                Scalar::Null => crate::filter::FilterOp::IsNull,
                value => crate::filter::FilterOp::Eq(value),
            };
            filters.push(Filter::new(column, op));
        }
        Some(ViewConfig {
            filters,
            formats: self.config.formats.clone(),
            ..ViewConfig::default()
        })
    }

    /// Returns every node of a grouped view with its values, including collapsed and
    /// hidden ones, for checks and exports. Empty for a flat view.
    pub fn cells(&mut self, table: &Table) -> Vec<GroupCell> {
        let Kind::Grouped(g) = &mut self.kind else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for id in 0..g.nodes.len() as u32 {
            let Some(node) = &g.nodes[id as usize] else {
                continue;
            };
            let path = g.path(table, id);
            let rows = node.total.rows;
            let mut splits: Vec<(u32, i64)> =
                node.split.iter().map(|(&s, c)| (s, c.rows)).collect();
            splits.sort_unstable();
            out.push(GroupCell {
                path: path.clone(),
                split: None,
                rows,
                values: (0..g.measures.len())
                    .map(|m| g.cell_value(table, id, ROW_TOTAL, m))
                    .collect(),
            });
            for (s, rows) in splits {
                out.push(GroupCell {
                    path: path.clone(),
                    split: Some(g.split_scalars(table, s)),
                    rows,
                    values: (0..g.measures.len())
                        .map(|m| g.cell_value(table, id, s, m))
                        .collect(),
                });
            }
        }
        out
    }
}

fn sort_flat(table: &Table, flat: &mut Flat) {
    let key_column = table.key_column();
    let cmp = |&a: &usize, &b: &usize| {
        for &(c, descending) in &flat.sort {
            let column = table.column(c);
            let (ka, kb) = (column.key(a), column.key(b));
            let ordering = match (ka, kb) {
                (Key::Null, Key::Null) => Ordering::Equal,
                (Key::Null, _) => Ordering::Greater,
                (_, Key::Null) => Ordering::Less,
                _ if descending => column.cmp_keys(kb, ka),
                _ => column.cmp_keys(ka, kb),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        match key_column {
            Some(k) => table
                .column(k)
                .cmp_keys(table.column(k).key(a), table.column(k).key(b)),
            None => table.seq(a).cmp(&table.seq(b)),
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::slice::ParallelSliceMut;
        let mut order = std::mem::take(&mut flat.order);
        order.par_sort_unstable_by(cmp);
        flat.order = order;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let mut order = std::mem::take(&mut flat.order);
        order.sort_unstable_by(cmp);
        flat.order = order;
    }
}

fn flat_window(
    table: &Table,
    formatters: &Formatters,
    flat: &Flat,
    rows: Range<usize>,
    columns: Range<usize>,
) -> (RecordBatch, usize, usize) {
    let start = rows.start.min(flat.order.len());
    let end = rows.end.min(flat.order.len()).max(start);
    let slots = &flat.order[start..end];
    let cols = columns.start.min(flat.columns.len())..columns.end.min(flat.columns.len());
    let mut out = FrameBuilder::default();
    out.push(
        role_field("__seq", DataType::Int64, false, role::SEQ, &[]),
        Arc::new(Int64Array::from_iter_values(
            slots.iter().map(|&s| table.seq(s) as i64),
        )),
    );
    for (index, &c) in flat.columns[cols].iter().enumerate() {
        let field = table.schema().field(c);
        let formatter = &formatters.columns[c];
        out.data_column(
            index,
            field,
            field.name().clone(),
            table.column(c).to_arrow(slots),
            None,
            formatter,
            None,
            None,
        );
    }
    (out.finish(), flat.order.len(), flat.columns.len())
}

/// A field with a role and other frame metadata.
fn role_field(
    name: &str,
    data_type: DataType,
    nullable: bool,
    role: &str,
    extra: &[(&str, String)],
) -> Field {
    let mut metadata = HashMap::from([(meta::ROLE.to_owned(), role.to_owned())]);
    for (k, v) in extra {
        metadata.insert((*k).to_owned(), v.clone());
    }
    Field::new(name, data_type, nullable).with_metadata(metadata)
}

/// Collects a frame's fields and arrays.
#[derive(Default)]
struct FrameBuilder {
    fields: Vec<Field>,
    arrays: Vec<ArrayRef>,
}

impl FrameBuilder {
    fn push(&mut self, field: Field, array: ArrayRef) {
        self.fields.push(field);
        self.arrays.push(array);
    }

    /// Adds a data column: its values, its flags if it has any, its display text,
    /// and for timestamps its full-precision text. `source` is the table field of a
    /// flat view's column, whose nullability and metadata the values keep, or a new
    /// field for a measure, whose position `measure` is.
    #[allow(clippy::too_many_arguments)]
    fn data_column(
        &mut self,
        index: usize,
        source: &Field,
        title: String,
        values: ArrayRef,
        flags: Option<Vec<u8>>,
        formatter: &Formatter,
        measure: Option<usize>,
        split: Option<String>,
    ) {
        let name = source.name().as_str();
        let data_type = values.data_type().clone();
        let title = match &data_type {
            DataType::Timestamp(_, Some(_)) => format!("{title} ({})", formatter.zone_name()),
            _ => title,
        };
        let at = [(meta::INDEX, index.to_string())];
        let mut text = formatter.texts(&values);
        if let Some(flags) = &flags {
            for (t, &f) in text.iter_mut().zip(flags) {
                match f {
                    flag::OVERFLOW => format::OVERFLOW_TEXT.clone_into(t),
                    flag::MIXED => format::MIXED_TEXT.clone_into(t),
                    flag::UNDEFINED => format::UNDEFINED_TEXT.clone_into(t),
                    _ => {}
                }
            }
        }
        let copy = matches!(data_type, DataType::Timestamp(..))
            .then(|| formatter.timestamps_full(&values));
        let mut metadata = source.metadata().clone();
        metadata.insert(meta::ROLE.to_owned(), role::VALUE.to_owned());
        metadata.insert(meta::INDEX.to_owned(), index.to_string());
        metadata.insert(meta::TITLE.to_owned(), title);
        if let Some(m) = measure {
            metadata.insert(meta::MEASURE.to_owned(), m.to_string());
        }
        if let Some(split) = split {
            metadata.insert(meta::SPLIT.to_owned(), split);
        }
        self.push(
            Field::new(name, data_type, source.is_nullable()).with_metadata(metadata),
            values,
        );
        if let Some(flags) = flags {
            self.push(
                role_field(
                    &format!("{name}#flag"),
                    DataType::UInt8,
                    false,
                    role::FLAG,
                    &at,
                ),
                Arc::new(UInt8Array::from(flags)),
            );
        }
        self.push(
            role_field(
                &format!("{name}#text"),
                DataType::Utf8,
                false,
                role::TEXT,
                &at,
            ),
            Arc::new(StringArray::from(text)),
        );
        if let Some(copy) = copy {
            self.push(
                role_field(
                    &format!("{name}#copy"),
                    DataType::Utf8,
                    false,
                    role::COPY,
                    &at,
                ),
                Arc::new(StringArray::from(copy)),
            );
        }
    }

    fn finish(self) -> RecordBatch {
        RecordBatch::try_new(Arc::new(Schema::new(self.fields)), self.arrays)
            .expect("consistent window")
    }
}

fn to_f64(value: &MeasureValue, scale: i8) -> Option<f64> {
    match value {
        MeasureValue::Count(c) => Some(*c as f64),
        MeasureValue::Float(f) => Some(*f),
        MeasureValue::Decimal(d) => Some(ratio_to_f64(*d, &[pow10(scale)])),
        MeasureValue::Key(k) => match k.0 {
            Key::Int(i) => Some(i as f64),
            Key::Float(bits) => Some(f64::from_bits(bits)),
            Key::Decimal(d) => Some(ratio_to_f64(d, &[pow10(scale)])),
            _ => None,
        },
        _ => None,
    }
}

impl Grouped {
    fn levels(&self) -> usize {
        self.group_columns.len()
    }

    fn alloc(&mut self, key: Key, depth: u32, parent: u32) -> u32 {
        let leaf = depth as usize == self.levels();
        let node = Node {
            key,
            depth,
            parent,
            children: FxHashMap::default(),
            total: Cell::new(&self.measures, leaf),
            split: FxHashMap::default(),
        };
        match self.free.pop() {
            Some(id) => {
                self.nodes[id as usize] = Some(node);
                id
            }
            None => {
                self.nodes.push(Some(node));
                (self.nodes.len() - 1) as u32
            }
        }
    }

    fn node(&self, id: u32) -> &Node {
        self.nodes[id as usize].as_ref().expect("live node")
    }

    fn node_mut(&mut self, id: u32) -> &mut Node {
        self.nodes[id as usize].as_mut().expect("live node")
    }

    fn invalidate(&mut self) {
        self.visible = None;
        self.columns = None;
        self.lazy_values.get_mut().clear();
    }

    fn fill_inputs(&self, source: Source<'_>, row: usize, out: &mut Vec<Input>) {
        let seq = source.seq(row);
        out.clear();
        out.extend(
            self.measures
                .iter()
                .map(|m| {
                    let (key, float, exact) = match m.column {
                        Some(c) => {
                            let column = source.column(c);
                            (source.key(c, row), column.f64(row), column.exact(row))
                        }
                        None => (Key::Null, None, None),
                    };
                    let (weight_float, weight_exact) = match m.weight {
                        Some(w) => {
                            let column = source.column(w);
                            (column.f64(row), column.exact(row))
                        }
                        None => (None, None),
                    };
                    Input {
                        key,
                        float,
                        exact: if matches!(m.column_type, Some(ColumnType::Float64)) {
                            None
                        } else {
                            exact
                        },
                        weight_float: if matches!(m.weight_type, Some(ColumnType::Float64)) {
                            weight_float
                        } else {
                            None
                        },
                        weight_exact: if matches!(m.weight_type, Some(ColumnType::Float64)) {
                            None
                        } else {
                            weight_exact
                        },
                        unit: m.unit.map(|u| source.key(u, row)),
                        seq,
                    }
                })
                .map(|mut input| {
                    if input.exact.is_some() {
                        input.float = None;
                    }
                    input
                }),
        );
    }

    fn apply_row(&mut self, source: Source<'_>, row: usize, sign: i64) {
        let mut inputs = std::mem::take(&mut self.scratch_inputs);
        self.fill_inputs(source, row, &mut inputs);
        let split = if self.split_columns.is_empty() {
            None
        } else {
            let mut keys = std::mem::take(&mut self.scratch_keys);
            keys.clear();
            keys.extend(self.split_columns.iter().map(|&c| source.key(c, row)));
            let id = match self.split_ids.get(keys.as_slice()) {
                Some(&id) => id,
                None => {
                    let id = self.split_keys.len() as u32;
                    let boxed: Box<[Key]> = keys.as_slice().into();
                    self.split_ids.insert(boxed.clone(), id);
                    self.split_keys.push(boxed);
                    self.split_rows.push(0);
                    id
                }
            };
            self.scratch_keys = keys;
            self.split_rows[id as usize] += sign;
            Some(id)
        };
        let levels = self.levels();
        let mut path = std::mem::take(&mut self.scratch_path);
        path.clear();
        let mut node = 0u32;
        path.push(node);
        for level in 0..levels {
            let key = source.key(self.group_columns[level], row);
            let child = match self.node(node).children.get(&key) {
                Some(&c) => c,
                None => {
                    let c = self.alloc(key, level as u32 + 1, node);
                    self.node_mut(node).children.insert(key, c);
                    c
                }
            };
            path.push(child);
            node = child;
        }
        for &id in &path {
            let leaf = self.node(id).depth as usize == levels;
            let measures = std::mem::take(&mut self.measures);
            let n = self.node_mut(id);
            n.total.apply(&measures, &inputs, sign);
            if let Some(s) = split {
                let cell = n
                    .split
                    .entry(s)
                    .or_insert_with(|| Cell::new(&measures, leaf));
                cell.apply(&measures, &inputs, sign);
                if cell.rows == 0 {
                    n.split.remove(&s);
                }
            }
            self.measures = measures;
        }
        if sign < 0 {
            for &id in path.iter().skip(1).rev() {
                if self.node(id).total.rows == 0 {
                    let (parent, key) = (self.node(id).parent, self.node(id).key);
                    self.node_mut(parent).children.remove(&key);
                    self.nodes[id as usize] = None;
                    self.free.push(id);
                }
            }
        }
        self.scratch_path = path;
        self.scratch_inputs = inputs;
    }

    fn path(&self, table: &Table, id: u32) -> Vec<Scalar> {
        let mut keys = Vec::new();
        let mut node = id;
        while self.node(node).parent != NO_PARENT {
            keys.push((self.node(node).depth, self.node(node).key));
            node = self.node(node).parent;
        }
        keys.reverse();
        keys.into_iter()
            .map(|(depth, key)| {
                table
                    .column(self.group_columns[depth as usize - 1])
                    .key_to_scalar(key)
            })
            .collect()
    }

    fn split_scalars(&self, table: &Table, split: u32) -> Vec<Scalar> {
        self.split_keys[split as usize]
            .iter()
            .zip(&self.split_columns)
            .map(|(&k, &c)| table.column(c).key_to_scalar(k))
            .collect()
    }

    fn cell(&self, id: u32, split: u32) -> Option<&Cell> {
        let node = self.node(id);
        if split == ROW_TOTAL {
            Some(&node.total)
        } else {
            node.split.get(&split)
        }
    }

    /// Merges the leaf cells under `ids` into one leaf-level cell.
    fn merged_leaves(&self, ids: &[u32], split: u32) -> Cell {
        let mut out = Cell::new(&self.measures, true);
        let mut stack: Vec<u32> = ids.to_vec();
        while let Some(id) = stack.pop() {
            let node = self.node(id);
            if node.depth as usize == self.levels() {
                if let Some(cell) = self.cell(id, split) {
                    out.merge(cell);
                }
            } else {
                stack.extend(node.children.values().copied());
            }
        }
        out
    }

    fn measure_value(
        &self,
        table: &Table,
        cell: &Cell,
        lazy: Option<&Cell>,
        m: usize,
    ) -> MeasureValue {
        let measure = &self.measures[m];
        if let Some(units) = &cell.units[m]
            && units.len() > 1
            && !matches!(
                measure.aggregate,
                Aggregate::Count | Aggregate::CountAll | Aggregate::DistinctCount
            )
        {
            return MeasureValue::Mixed;
        }
        let column = measure.column.map(|c| table.column(c));
        let cmp = |a: Key, b: Key| column.expect("lazy measures have a column").cmp_keys(a, b);
        let state = match (&cell.states[m], lazy) {
            (State::Lazy, Some(merged)) => &merged.states[m],
            (state, _) => state,
        };
        finish(measure, state, &cmp)
    }

    fn empty_value(&self, m: usize) -> MeasureValue {
        match self.measures[m].aggregate {
            Aggregate::Count | Aggregate::CountAll | Aggregate::DistinctCount => {
                MeasureValue::Count(0)
            }
            _ => MeasureValue::Null,
        }
    }

    fn raw_value(&self, table: &Table, ids: &[u32], split: u32, m: usize) -> MeasureValue {
        if let [id] = ids {
            let Some(cell) = self.cell(*id, split) else {
                return self.empty_value(m);
            };
            if cell.rows == 0 {
                return self.empty_value(m);
            }
            if !matches!(cell.states[m], State::Lazy) {
                return self.measure_value(table, cell, None, m);
            }
            if let Some(value) = self.lazy_values.borrow().get(&(*id, split, m)) {
                return value.clone();
            }
            let merged = self.merged_leaves(ids, split);
            let value = self.measure_value(table, cell, Some(&merged), m);
            self.lazy_values
                .borrow_mut()
                .insert((*id, split, m), value.clone());
            return value;
        }
        // An Other row: merge the cells of every member.
        let mut combined: Option<Cell> = None;
        for &id in ids {
            if let Some(c) = self.cell(id, split) {
                match &mut combined {
                    Some(acc) => acc.merge(c),
                    None => combined = Some(c.clone()),
                }
            }
        }
        let Some(cell) = combined else {
            return self.empty_value(m);
        };
        if cell.rows == 0 {
            return self.empty_value(m);
        }
        let lazy = matches!(cell.states[m], State::Lazy).then(|| self.merged_leaves(ids, split));
        self.measure_value(table, &cell, lazy.as_ref(), m)
    }

    fn shown(&self, table: &Table, ids: &[u32], split: u32, m: usize) -> CellValue {
        let measure = &self.measures[m];
        let value = self.raw_value(table, ids, split, m);
        let base = match measure.show_as {
            ShowAs::Value => return to_cell(table, measure, value),
            ShowAs::PercentOfGrandTotal => self.raw_value(table, &[0], ROW_TOTAL, m),
            ShowAs::PercentOfColumnTotal => self.raw_value(table, &[0], split, m),
            ShowAs::PercentOfRowTotal => self.raw_value(table, ids, ROW_TOTAL, m),
            ShowAs::PercentOfParent => {
                let parent = self.node(ids[0]).parent;
                if parent == NO_PARENT {
                    value.clone()
                } else {
                    self.raw_value(table, &[parent], split, m)
                }
            }
        };
        match (&value, &base) {
            (MeasureValue::Mixed, _) | (_, MeasureValue::Mixed) => CellValue::Mixed,
            (MeasureValue::Overflow, _) | (_, MeasureValue::Overflow) => CellValue::Overflow,
            (MeasureValue::Null, _) => CellValue::Null,
            _ => match (
                to_f64(&value, measure.scale()),
                to_f64(&base, measure.scale()),
            ) {
                (Some(v), Some(b)) if b != 0.0 && b.is_finite() => CellValue::Float(v / b),
                (Some(_), _) => CellValue::Undefined,
                _ => CellValue::Null,
            },
        }
    }

    fn cell_value(&self, table: &Table, id: u32, split: u32, m: usize) -> CellValue {
        self.shown(table, &[id], split, m)
    }

    fn numeric(&self, table: &Table, ids: &[u32], m: usize) -> Option<f64> {
        match self.shown(table, ids, ROW_TOTAL, m) {
            CellValue::Count(c) => Some(c as f64),
            CellValue::Float(f) if !f.is_nan() => Some(f),
            CellValue::Decimal(d) => Some(ratio_to_f64(d, &[pow10(self.measures[m].scale())])),
            CellValue::Value(Scalar::Int64(i)) => Some(i as f64),
            CellValue::Value(Scalar::Float64(f)) => Some(f),
            CellValue::Value(Scalar::Decimal(d)) => {
                Some(ratio_to_f64(d, &[pow10(self.measures[m].scale())]))
            }
            _ => None,
        }
    }

    fn is_expanded(&self, table: &Table, id: u32, config: &ViewConfig) -> bool {
        let node = self.node(id);
        if node.parent == NO_PARENT {
            return true;
        }
        if node.depth as usize == self.levels() {
            return false;
        }
        if !self.expanded.is_empty() {
            let path = self.path(table, id);
            if let Some((_, e)) = self.expanded.iter().rev().find(|(p, _)| *p == path) {
                return *e;
            }
        }
        (node.depth as usize) <= config.expand_depth
    }

    fn children_in_order(
        &self,
        table: &Table,
        id: u32,
        config: &ViewConfig,
    ) -> (Vec<u32>, Vec<u32>) {
        let node = self.node(id);
        let level = node.depth as usize;
        let column = table.column(self.group_columns[level]);
        let mut children: Vec<u32> = node.children.values().copied().collect();
        let mut keyed: Vec<(u32, Option<f64>)> = children.iter().map(|&c| (c, None)).collect();
        if let GroupOrder::Measure(m) = config.group_sort.by {
            for entry in &mut keyed {
                entry.1 = self.numeric(table, &[entry.0], m);
            }
        }
        let by_key = |a: u32, b: u32| column.cmp_keys(self.node(a).key, self.node(b).key);
        keyed.sort_by(|a, b| {
            let primary = match config.group_sort.by {
                GroupOrder::Key => {
                    let o = by_key(a.0, b.0);
                    match (self.node(a.0).key, self.node(b.0).key) {
                        (Key::Null, _) | (_, Key::Null) => o,
                        _ if config.group_sort.descending => o.reverse(),
                        _ => o,
                    }
                }
                GroupOrder::Measure(_) => match (a.1, b.1) {
                    (None, None) => Ordering::Equal,
                    (None, _) => Ordering::Greater,
                    (_, None) => Ordering::Less,
                    (Some(x), Some(y)) if config.group_sort.descending => y.total_cmp(&x),
                    (Some(x), Some(y)) => x.total_cmp(&y),
                },
            };
            primary.then_with(|| by_key(a.0, b.0))
        });
        children = keyed.into_iter().map(|(c, _)| c).collect();
        children.retain(|&c| {
            config.having.iter().all(|h| {
                self.numeric(table, &[c], h.measure)
                    .is_some_and(|v| match h.op {
                        Comparison::Gt => v > h.value,
                        Comparison::Ge => v >= h.value,
                        Comparison::Lt => v < h.value,
                        Comparison::Le => v <= h.value,
                        Comparison::Eq => v == h.value,
                        Comparison::Ne => v != h.value,
                    })
            })
        });
        let mut rest = Vec::new();
        if let Some(top) = config.top_n
            && children.len() > top.n
        {
            let mut ranked: Vec<(u32, Option<f64>)> = children
                .iter()
                .map(|&c| (c, self.numeric(table, &[c], top.measure)))
                .collect();
            ranked.sort_by(|a, b| match (a.1, b.1) {
                (None, None) => by_key(a.0, b.0),
                (None, _) => Ordering::Greater,
                (_, None) => Ordering::Less,
                (Some(x), Some(y)) => {
                    let o = if top.largest {
                        y.total_cmp(&x)
                    } else {
                        x.total_cmp(&y)
                    };
                    o.then_with(|| by_key(a.0, b.0))
                }
            });
            let keep: rustc_hash::FxHashSet<u32> = ranked.iter().take(top.n).map(|e| e.0).collect();
            rest = children
                .iter()
                .copied()
                .filter(|c| !keep.contains(c))
                .collect();
            children.retain(|c| keep.contains(c));
            if !top.other {
                rest.clear();
            }
        }
        (children, rest)
    }

    fn visible(&mut self, table: &Table, config: &ViewConfig) -> &[Visible] {
        if self.visible.is_none() {
            let mut out = vec![Visible::Node(0)];
            // Depth-first, children in display order.
            let mut ordered: Vec<Visible> = Vec::new();
            fn walk(
                g: &Grouped,
                table: &Table,
                config: &ViewConfig,
                id: u32,
                out: &mut Vec<Visible>,
            ) {
                if g.node(id).depth as usize == g.levels() || !g.is_expanded(table, id, config) {
                    return;
                }
                let (children, rest) = g.children_in_order(table, id, config);
                for c in children {
                    out.push(Visible::Node(c));
                    walk(g, table, config, c, out);
                }
                if !rest.is_empty() {
                    out.push(Visible::Other {
                        parent: id,
                        members: rest,
                    });
                }
            }
            walk(self, table, config, 0, &mut ordered);
            out.extend(ordered);
            self.visible = Some(out);
        }
        self.visible.as_deref().expect("computed")
    }

    fn data_columns(&mut self, table: &Table) -> &[(u32, usize)] {
        if self.columns.is_none() {
            let mut splits: Vec<u32> = (0..self.split_keys.len() as u32)
                .filter(|&s| self.split_rows[s as usize] > 0)
                .collect();
            splits.sort_by(|&a, &b| {
                for (i, &c) in self.split_columns.iter().enumerate() {
                    let o = table.column(c).cmp_keys(
                        self.split_keys[a as usize][i],
                        self.split_keys[b as usize][i],
                    );
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                Ordering::Equal
            });
            let m = self.measures.len();
            // Changes after the view opened can add column keys; keep within the limit.
            splits.truncate(MAX_COLUMNS / m.max(1) - 1);
            let mut columns: Vec<(u32, usize)> = splits
                .iter()
                .flat_map(|&s| (0..m).map(move |i| (s, i)))
                .collect();
            columns.extend((0..m).map(|i| (ROW_TOTAL, i)));
            self.columns = Some(columns);
        }
        self.columns.as_deref().expect("computed")
    }

    fn window(
        &mut self,
        table: &Table,
        config: &ViewConfig,
        formatters: &Formatters,
        rows: Range<usize>,
        columns: Range<usize>,
    ) -> (RecordBatch, usize, usize) {
        let total_rows = self.visible(table, config).len();
        let start = rows.start.min(total_rows);
        let end = rows.end.min(total_rows).max(start);
        let shown: Vec<Visible> = self.visible(table, config)[start..end].to_vec();
        let total_columns = self.data_columns(table).len();
        let cols = columns.start.min(total_columns)..columns.end.min(total_columns);
        let data_columns: Vec<(u32, usize)> = self.data_columns(table)[cols].to_vec();
        let levels = self.levels();

        let mut out = FrameBuilder::default();
        let ids_of = |v: &Visible| -> Vec<u32> {
            match v {
                Visible::Node(id) => vec![*id],
                Visible::Other { members, .. } => members.clone(),
            }
        };
        let depth_of = |v: &Visible| match v {
            Visible::Node(id) => self.node(*id).depth as i32,
            Visible::Other { parent, .. } => self.node(*parent).depth as i32 + 1,
        };
        out.push(
            role_field("__depth", DataType::Int32, false, role::DEPTH, &[]),
            Arc::new(Int32Array::from_iter_values(shown.iter().map(depth_of))),
        );
        out.push(
            role_field("__kind", DataType::Utf8, false, role::KIND, &[]),
            Arc::new(StringArray::from_iter_values(shown.iter().map(
                |v| match v {
                    Visible::Other { .. } => "other",
                    Visible::Node(0) => "total",
                    Visible::Node(id) if self.node(*id).depth as usize == levels => "leaf",
                    Visible::Node(_) => "group",
                },
            ))),
        );
        out.push(
            role_field("__expanded", DataType::Boolean, true, role::EXPANDED, &[]),
            Arc::new(
                shown
                    .iter()
                    .map(|v| match v {
                        Visible::Node(id)
                            if *id != 0 && (self.node(*id).depth as usize) < levels =>
                        {
                            Some(self.is_expanded(table, *id, config))
                        }
                        _ => None,
                    })
                    .collect::<BooleanArray>(),
            ),
        );
        out.push(
            role_field("__label", DataType::Utf8, false, role::LABEL, &[]),
            Arc::new(StringArray::from_iter_values(shown.iter().map(
                |v| match v {
                    Visible::Other { .. } => "Other".to_owned(),
                    Visible::Node(0) => "Total".to_owned(),
                    Visible::Node(id) => {
                        let node = self.node(*id);
                        let c = self.group_columns[node.depth as usize - 1];
                        display_label(
                            table,
                            formatters,
                            c,
                            &table.column(c).key_to_scalar(node.key),
                        )
                    }
                },
            ))),
        );
        for (level, &c) in self.group_columns.iter().enumerate() {
            let column = table.column(c);
            let keys: Vec<Key> = shown
                .iter()
                .map(|v| {
                    let id = match v {
                        Visible::Node(id) => *id,
                        Visible::Other { parent, .. } => *parent,
                    };
                    let mut node = id;
                    while node != NO_PARENT && self.node(node).depth as usize > level + 1 {
                        node = self.node(node).parent;
                    }
                    if node != NO_PARENT && self.node(node).depth as usize == level + 1 {
                        self.node(node).key
                    } else {
                        Key::Null
                    }
                })
                .collect();
            out.push(
                role_field(
                    table.schema().field(c).name(),
                    column.column_type().to_arrow(),
                    true,
                    role::GROUP,
                    &[(meta::LEVEL, level.to_string())],
                ),
                keys_to_arrow(column, &keys),
            );
        }
        for (index, &(split, m)) in data_columns.iter().enumerate() {
            let measure = self.measures[m].clone();
            let values: Vec<CellValue> = shown
                .iter()
                .map(|v| self.shown(table, &ids_of(v), split, m))
                .collect();
            let label = config.measures[m].label();
            let (name, title, split_json) = match split {
                ROW_TOTAL if self.split_columns.is_empty() => (label.clone(), label, None),
                ROW_TOTAL => (format!("Total | {label}"), format!("Total | {label}"), None),
                s => {
                    let scalars = self.split_scalars(table, s);
                    let mut name = String::new();
                    let mut title = String::new();
                    let json: Vec<String> = self
                        .split_columns
                        .iter()
                        .zip(&scalars)
                        .map(|(&c, v)| format::scalar_json(table.column(c).column_type(), v))
                        .collect();
                    for (&c, value) in self.split_columns.iter().zip(&scalars) {
                        let ty = table.column(c).column_type();
                        match value {
                            Scalar::Null => name.push_str("null"),
                            v => name.push_str(&quoted(&format::canonical(ty, v))),
                        }
                        name.push_str(" | ");
                        title.push_str(&display_label(table, formatters, c, value));
                        title.push_str(" | ");
                    }
                    name.push_str(&label);
                    title.push_str(&label);
                    (name, title, Some(format!("[{}]", json.join(","))))
                }
            };
            let (array, data_type) = values_to_arrow(table, &measure, &values);
            let flags = values
                .iter()
                .map(|v| match v {
                    CellValue::Overflow => flag::OVERFLOW,
                    CellValue::Mixed => flag::MIXED,
                    CellValue::Undefined => flag::UNDEFINED,
                    CellValue::Float(f) if f.is_nan() => flag::UNDEFINED,
                    _ => flag::OK,
                })
                .collect();
            out.data_column(
                index,
                &Field::new(name, data_type, true),
                title,
                array,
                Some(flags),
                &formatters.measures[m],
                Some(m),
                split_json,
            );
        }
        (out.finish(), total_rows, total_columns)
    }
}

/// A group value as a row label or column-pivot header: `(blank)` for null, and
/// otherwise the value in its column's format.
fn display_label(table: &Table, formatters: &Formatters, column: usize, value: &Scalar) -> String {
    match value {
        Scalar::Null => "(blank)".to_owned(),
        v => formatters.columns[column].scalar(table.column(column).column_type(), v),
    }
}

/// Quotes text as a JSON string does, so that column-pivot names built from values
/// never collide with each other or with the fixed `Total` names.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn to_cell(table: &Table, measure: &CompiledMeasure, value: MeasureValue) -> CellValue {
    match value {
        MeasureValue::Null => CellValue::Null,
        MeasureValue::Count(c) => CellValue::Count(c),
        MeasureValue::Float(f) if f.is_nan() => CellValue::Undefined,
        MeasureValue::Float(f) => CellValue::Float(f),
        MeasureValue::Decimal(d) => CellValue::Decimal(d),
        MeasureValue::Key(k) => CellValue::Value(
            table
                .column(measure.column.expect("key results have a column"))
                .key_to_scalar(k.0),
        ),
        MeasureValue::Overflow => CellValue::Overflow,
        MeasureValue::Mixed => CellValue::Mixed,
    }
}

fn keys_to_arrow(column: &Column, keys: &[Key]) -> ArrayRef {
    let scalars: Vec<Scalar> = keys.iter().map(|&k| column.key_to_scalar(k)).collect();
    scalars_to_arrow(column.column_type(), &scalars)
}

fn scalars_to_arrow(ty: &ColumnType, scalars: &[Scalar]) -> ArrayRef {
    match ty {
        ColumnType::Boolean => Arc::new(
            scalars
                .iter()
                .map(|s| {
                    if let Scalar::Boolean(b) = s {
                        Some(*b)
                    } else {
                        None
                    }
                })
                .collect::<BooleanArray>(),
        ),
        ColumnType::Int64 => Arc::new(
            scalars
                .iter()
                .map(|s| {
                    if let Scalar::Int64(i) = s {
                        Some(*i)
                    } else {
                        None
                    }
                })
                .collect::<Int64Array>(),
        ),
        ColumnType::Float64 => Arc::new(
            scalars
                .iter()
                .map(|s| {
                    if let Scalar::Float64(f) = s {
                        Some(*f)
                    } else {
                        None
                    }
                })
                .collect::<Float64Array>(),
        ),
        ColumnType::Decimal { precision, scale } => Arc::new(
            scalars
                .iter()
                .map(|s| {
                    if let Scalar::Decimal(d) = s {
                        Some(*d)
                    } else {
                        None
                    }
                })
                .collect::<Decimal128Array>()
                .with_precision_and_scale(*precision, *scale)
                .expect("valid decimal"),
        ),
        ColumnType::Utf8 => Arc::new(
            scalars
                .iter()
                .map(|s| {
                    if let Scalar::Utf8(t) = s {
                        Some(t.as_str())
                    } else {
                        None
                    }
                })
                .collect::<StringArray>(),
        ),
        ColumnType::Date32 => Arc::new(
            scalars
                .iter()
                .map(|s| {
                    if let Scalar::Date32(d) = s {
                        Some(*d)
                    } else {
                        None
                    }
                })
                .collect::<Date32Array>(),
        ),
        ColumnType::Timestamp { timezone } => Arc::new(
            scalars
                .iter()
                .map(|s| {
                    if let Scalar::Timestamp(t) = s {
                        Some(*t)
                    } else {
                        None
                    }
                })
                .collect::<TimestampMicrosecondArray>()
                .with_timezone_opt(timezone.clone()),
        ),
    }
}

/// Converts measure values to an Arrow array of the measure's output type.
fn values_to_arrow(
    table: &Table,
    measure: &CompiledMeasure,
    values: &[CellValue],
) -> (ArrayRef, DataType) {
    let percent = measure.show_as != ShowAs::Value;
    let floats = |values: &[CellValue]| -> ArrayRef {
        Arc::new(
            values
                .iter()
                .map(|v| match v {
                    CellValue::Float(f) if !f.is_nan() => Some(*f),
                    _ => None,
                })
                .collect::<Float64Array>(),
        )
    };
    if percent {
        return (floats(values), DataType::Float64);
    }
    match &measure.aggregate {
        Aggregate::Count | Aggregate::CountAll | Aggregate::DistinctCount => (
            Arc::new(
                values
                    .iter()
                    .map(|v| {
                        if let CellValue::Count(c) = v {
                            Some(*c)
                        } else {
                            None
                        }
                    })
                    .collect::<Int64Array>(),
            ),
            DataType::Int64,
        ),
        Aggregate::Sum if !matches!(measure.column_type, Some(ColumnType::Float64)) => {
            let scale = measure.scale();
            let array = values
                .iter()
                .map(|v| {
                    if let CellValue::Decimal(d) = v {
                        Some(*d)
                    } else {
                        None
                    }
                })
                .collect::<Decimal128Array>()
                .with_precision_and_scale(38, scale)
                .expect("valid decimal");
            (Arc::new(array), DataType::Decimal128(38, scale))
        }
        Aggregate::Min | Aggregate::Max | Aggregate::First | Aggregate::Last => {
            let ty = measure.column_type.clone().expect("source column");
            let scalars: Vec<Scalar> = values
                .iter()
                .map(|v| {
                    if let CellValue::Value(s) = v {
                        s.clone()
                    } else {
                        Scalar::Null
                    }
                })
                .collect();
            let _ = table;
            (scalars_to_arrow(&ty, &scalars), ty.to_arrow())
        }
        _ => (floats(values), DataType::Float64),
    }
}
