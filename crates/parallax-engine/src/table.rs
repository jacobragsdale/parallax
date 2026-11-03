//! Tables: columns addressed by stable slots, loaded and updated as diffs.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::{Field, Schema, SchemaRef};
use rustc_hash::FxHashMap;

use crate::bitmap::Bitmap;
use crate::column::Column;
use crate::error::LoadError;
use crate::types::{ColumnType, Key};

/// What to do when a load repeats a primary key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DuplicatePolicy {
    /// Refuse the load.
    #[default]
    Error,
    /// Keep the last row with each key.
    LastWins,
}

/// How a table is keyed.
#[derive(Clone, Debug, Default)]
pub struct TableOptions {
    /// The primary key column, if any. Loads into a keyed table become updates,
    /// inserts and deletes by key. A table without a key is replaced wholesale.
    pub primary_key: Option<String>,
    /// What to do with repeated keys.
    pub duplicates: DuplicatePolicy,
}

/// The old images of rows that a change removed or overwrote, in the table's schema.
#[derive(Clone, Debug)]
pub struct RemovedRows {
    /// One column per table column, one row per removed or overwritten row.
    pub columns: Vec<Column>,
    /// Each row's insertion sequence number.
    pub seq: Vec<u64>,
}

impl RemovedRows {
    /// The number of rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seq.len()
    }

    /// Returns `true` if no row was removed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seq.is_empty()
    }
}

/// What a load or update changed. Views apply it to stay current: they remove the
/// old images and add the current values of `added`.
#[derive(Clone, Debug)]
pub struct ChangeSet {
    /// The table's load version after the change.
    pub load_version: u64,
    /// Old images of deleted rows and of the previous values of updated rows.
    pub removed: RemovedRows,
    /// Slots holding inserted rows and the new values of updated rows.
    pub added: Vec<usize>,
}

/// A summary of a load, for the view's info panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Rows in the load.
    pub rows: usize,
    /// Rows whose key was new.
    pub inserted: usize,
    /// Rows whose key existed with different values.
    pub updated: usize,
    /// Rows whose key existed with the same values.
    pub unchanged: usize,
    /// Rows deleted because their key was not in the load.
    pub deleted: usize,
    /// Per column, how many NaNs became nulls.
    pub nans_to_null: Vec<(String, usize)>,
    /// The table's load version after the load.
    pub load_version: u64,
}

/// Columns staged for a load, with how many NaNs became nulls per column.
type Staged = (Vec<Column>, Vec<(String, usize)>);

/// A table.
#[derive(Clone, Debug)]
pub struct Table {
    schema: SchemaRef,
    columns: Vec<Column>,
    live: Bitmap,
    seq: Vec<u64>,
    free: Vec<usize>,
    key_column: Option<usize>,
    index: FxHashMap<Key, usize>,
    options: TableOptions,
    next_seq: u64,
    load_version: u64,
}

impl Table {
    /// Creates an empty table.
    ///
    /// # Errors
    ///
    /// Returns an error if a column type is unsupported or the primary key is not a
    /// column.
    pub fn new(schema: &Schema, options: TableOptions) -> Result<Self, LoadError> {
        let mut fields = Vec::with_capacity(schema.fields().len());
        let mut columns = Vec::with_capacity(schema.fields().len());
        for field in schema.fields() {
            let ty = ColumnType::from_arrow(field.data_type()).ok_or_else(|| {
                LoadError::UnsupportedType {
                    column: field.name().clone(),
                    data_type: field.data_type().clone(),
                }
            })?;
            fields.push(Field::new(field.name(), ty.to_arrow(), true));
            columns.push(Column::new(ty));
        }
        let key_column = match &options.primary_key {
            Some(name) => Some(
                schema
                    .index_of(name)
                    .map_err(|_| LoadError::UnknownKeyColumn(name.clone()))?,
            ),
            None => None,
        };
        Ok(Self {
            schema: Arc::new(Schema::new(fields)),
            columns,
            live: Bitmap::new(),
            seq: Vec::new(),
            free: Vec::new(),
            key_column,
            index: FxHashMap::default(),
            options,
            next_seq: 0,
            load_version: 0,
        })
    }

    /// The table's schema, with every column nullable.
    #[must_use]
    pub fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// The number of live rows.
    #[must_use]
    pub fn num_rows(&self) -> usize {
        self.live.count_ones()
    }

    /// The number of slots, live or free.
    #[must_use]
    pub fn num_slots(&self) -> usize {
        self.live.len()
    }

    /// Increments with every load or update, starting from zero for an empty table.
    #[must_use]
    pub fn load_version(&self) -> u64 {
        self.load_version
    }

    /// Returns the index of the named column.
    #[must_use]
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.schema.index_of(name).ok()
    }

    /// Returns a column by index.
    #[must_use]
    pub fn column(&self, index: usize) -> &Column {
        &self.columns[index]
    }

    /// The primary key column, if the table has one.
    #[must_use]
    pub fn key_column(&self) -> Option<usize> {
        self.key_column
    }

    /// Returns `true` if `slot` holds a live row.
    #[must_use]
    pub fn is_live(&self, slot: usize) -> bool {
        self.live.get(slot)
    }

    /// Iterates over live slots in slot order.
    pub fn live_slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.live.ones()
    }

    /// Returns the insertion sequence number of `slot`. Rows that are updated keep
    /// their number; inserted rows get the next one.
    #[must_use]
    pub fn seq(&self, slot: usize) -> u64 {
        self.seq[slot]
    }

    /// Replaces the table's contents with `batches`.
    ///
    /// For a keyed table the load is a diff: rows with new keys are inserted, rows
    /// whose values changed are updated in place, and rows whose keys are missing
    /// are deleted. A table without a key deletes everything and inserts the load.
    /// The whole load is validated before anything changes, so a refused load leaves
    /// the table as it was.
    ///
    /// # Errors
    ///
    /// Returns an error if the schema differs, a key is null or repeated, or a value
    /// cannot be represented without loss.
    pub fn load(&mut self, batches: &[RecordBatch]) -> Result<(LoadReport, ChangeSet), LoadError> {
        self.apply_rows(batches, true)
    }

    /// Inserts or updates rows by key, leaving other rows alone. Without a key,
    /// every row is inserted.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Table::load`].
    pub fn upsert(
        &mut self,
        batches: &[RecordBatch],
    ) -> Result<(LoadReport, ChangeSet), LoadError> {
        self.apply_rows(batches, false)
    }

    fn stage(&self, batches: &[RecordBatch]) -> Result<Staged, LoadError> {
        let mut staged: Vec<Column> = self
            .columns
            .iter()
            .map(|c| Column::new(c.ty.clone()))
            .collect();
        let mut nans = vec![0usize; staged.len()];
        for batch in batches {
            for field in batch.schema().fields() {
                if self.schema.index_of(field.name()).is_err() {
                    return Err(LoadError::UnexpectedColumn {
                        column: field.name().clone(),
                    });
                }
            }
            for (i, field) in self.schema.fields().iter().enumerate() {
                let array =
                    batch
                        .column_by_name(field.name())
                        .ok_or_else(|| LoadError::MissingColumn {
                            column: field.name().clone(),
                        })?;
                nans[i] += staged[i].extend_from_arrow(field.name(), array.as_ref())?;
            }
        }
        let report = self
            .schema
            .fields()
            .iter()
            .zip(nans)
            .filter(|(_, n)| *n > 0)
            .map(|(f, n)| (f.name().clone(), n))
            .collect();
        Ok((staged, report))
    }

    /// Returns the table-domain key for row `row` of a staged key column, or `None`
    /// if the key's text was never interned here, which makes it new.
    fn table_key(&self, staged_key: &Column, row: usize) -> Option<Key> {
        let key = staged_key.key(row);
        match key {
            Key::Str(_) => {
                let text = staged_key.text(key).expect("text key");
                let table_column = &self.columns[self.key_column.expect("keyed table")];
                table_column
                    .dictionary()
                    .and_then(|d| d.code(text))
                    .map(Key::Str)
            }
            other => Some(other),
        }
    }

    fn apply_rows(
        &mut self,
        batches: &[RecordBatch],
        replace: bool,
    ) -> Result<(LoadReport, ChangeSet), LoadError> {
        let (staged, nans_to_null) = self.stage(batches)?;
        let rows = staged.first().map_or(0, Column::len);
        let mut report = LoadReport {
            rows,
            nans_to_null,
            ..LoadReport::default()
        };

        // Decide what happens to each staged row before changing anything.
        let mut keep: Vec<usize> = (0..rows).collect();
        let mut targets: Vec<Option<usize>> = vec![None; rows];
        if let Some(k) = self.key_column {
            let key_name = self.schema.field(k).name().clone();
            let staged_key = &staged[k];
            let mut first_row: FxHashMap<Key, usize> = FxHashMap::default();
            let mut duplicates = 0;
            let mut examples = Vec::new();
            let mut last_row: FxHashMap<Key, usize> = FxHashMap::default();
            for row in 0..rows {
                let key = staged_key.key(row);
                if key == Key::Null {
                    return Err(LoadError::NullKey {
                        column: key_name,
                        row,
                    });
                }
                if first_row.insert(key, row).is_some() {
                    duplicates += 1;
                    if examples.len() < 5 {
                        examples.push(format!("{:?}", staged_key.scalar(row)));
                    }
                }
                last_row.insert(key, row);
            }
            if duplicates > 0 {
                if self.options.duplicates == DuplicatePolicy::Error {
                    return Err(LoadError::DuplicateKeys {
                        column: key_name,
                        count: duplicates,
                        examples,
                    });
                }
                keep = (0..rows)
                    .filter(|&row| last_row[&staged_key.key(row)] == row)
                    .collect();
            }
            for &row in &keep {
                targets[row] = self
                    .table_key(staged_key, row)
                    .and_then(|key| self.index.get(&key).copied());
            }
        } else if replace {
            // Without a key, a load replaces every row.
        }

        // From here on nothing can fail.
        let mut removed = RemovedRows {
            columns: self
                .columns
                .iter()
                .map(|c| Column::new(c.ty.clone()))
                .collect(),
            seq: Vec::new(),
        };
        let mut added = Vec::new();
        let mut seen = Bitmap::new();
        for _ in 0..self.live.len() {
            seen.push(false);
        }
        let mut updates = Vec::new();
        let mut inserts = Vec::new();
        for &row in &keep {
            match targets[row] {
                Some(slot) => {
                    seen.set(slot, true);
                    let changed = (0..self.columns.len())
                        .any(|c| !self.columns[c].same_value(slot, &staged[c], row));
                    if changed {
                        updates.push((slot, row));
                    } else {
                        report.unchanged += 1;
                    }
                }
                None => inserts.push(row),
            }
        }
        let deletes: Vec<usize> = if replace {
            self.live.ones().filter(|&slot| !seen.get(slot)).collect()
        } else {
            Vec::new()
        };

        for &slot in updates.iter().map(|(slot, _)| slot).chain(&deletes) {
            for (c, column) in self.columns.iter().enumerate() {
                removed.columns[c].push_from(column, slot);
            }
            removed.seq.push(self.seq[slot]);
        }
        for &(slot, row) in &updates {
            for (c, column) in self.columns.iter_mut().enumerate() {
                column.set_from(slot, &staged[c], row);
            }
            added.push(slot);
        }
        for &slot in &deletes {
            self.live.set(slot, false);
            if let Some(k) = self.key_column {
                let key = self.columns[k].key(slot);
                self.index.remove(&key);
            }
            self.free.push(slot);
        }
        for &row in &inserts {
            let slot = match self.free.pop() {
                Some(slot) => {
                    for (c, column) in self.columns.iter_mut().enumerate() {
                        column.set_from(slot, &staged[c], row);
                    }
                    self.live.set(slot, true);
                    self.seq[slot] = self.next_seq;
                    slot
                }
                None => {
                    for (c, column) in self.columns.iter_mut().enumerate() {
                        column.push_from(&staged[c], row);
                    }
                    self.live.push(true);
                    self.seq.push(self.next_seq);
                    self.live.len() - 1
                }
            };
            self.next_seq += 1;
            if let Some(k) = self.key_column {
                let key = self.columns[k].key(slot);
                self.index.insert(key, slot);
            }
            added.push(slot);
        }
        for column in &mut self.columns {
            column.refresh_ranks();
        }
        for column in &mut removed.columns {
            column.refresh_ranks();
        }
        self.load_version += 1;
        report.inserted = inserts.len();
        report.updated = updates.len();
        report.deleted = deletes.len();
        report.load_version = self.load_version;
        Ok((
            report,
            ChangeSet {
                load_version: self.load_version,
                removed,
                added,
            },
        ))
    }
}
