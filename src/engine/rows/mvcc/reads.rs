//! What a transaction read of a table, for its commit's check (RULES.md
//! D29, D38): rows, conditions, or the whole table, which past
//! TRANSACTIONS.TRACKED_ROWS they become; and what a transaction changed,
//! which another's reads are checked against. Split from engine/rows/mvcc.rs
//! (RULES.md R-SIZE-2).

use std::cmp::Ordering;

use crate::hash::murmur::MurmurSet;
use crate::spill::row_bytes;
use crate::storage::{Conjunction, Row, RowId};
use crate::value::{Value, compare};

/// What a transaction read.
pub(in crate::engine::rows) enum Read {
    /// A row, by its id.
    Row(RowId),
    /// The rows a scan or an index chose by conditions.
    Condition(Conjunction),
    /// Every row: a scan.
    All,
}

/// What a transaction has read of a table: past the rows tracked, the
/// whole table.
#[derive(Default)]
pub(super) struct Reads {
    pub(super) rows: MurmurSet<RowId>,
    pub(super) conditions: Vec<Conjunction>,
    pub(super) all: bool,
}

impl Reads {
    /// Adds `read`, then counts every row read if more than `tracked` rows
    /// or conditions are: a read of more than it read, which may refuse a
    /// commit it need not, never one it must.
    pub(super) fn add(&mut self, read: Read, tracked: usize) {
        match read {
            Read::Row(id) => {
                self.rows.insert(id);
            }
            Read::Condition(condition) => self.conditions.push(condition),
            Read::All => self.all = true,
        }
        if self.rows.len() > tracked || self.conditions.len() > tracked {
            *self = Reads { all: true, ..Reads::default() };
        }
    }

    /// Whether it read anything.
    pub(super) fn any(&self) -> bool {
        self.all || !self.rows.is_empty() || !self.conditions.is_empty()
    }
}

/// The rows of a validated transaction's changes, the versions it made and
/// those before them, which a condition another read is checked against:
/// the rows while their bytes are within the bytes held, then each
/// column's least and greatest value, which a condition may match more
/// often than a row does.
pub(super) enum Changed {
    Rows(Vec<Row>, usize),
    Bounds(Vec<Option<(Value, Value)>>),
}

impl Default for Changed {
    fn default() -> Changed {
        Changed::Rows(Vec::new(), 0)
    }
}

impl Changed {
    /// Adds `row`, the rows held then made bounds if they pass `held`
    /// bytes.
    pub(super) fn add(&mut self, row: Row, held: usize) {
        match self {
            Changed::Rows(rows, bytes) => {
                *bytes = bytes.saturating_add(row_bytes(&row));
                rows.push(row);
                if *bytes > held {
                    let mut bounds = Vec::new();
                    for kept in rows.drain(..) {
                        widen(&mut bounds, &kept);
                    }
                    *self = Changed::Bounds(bounds);
                }
            }
            Changed::Bounds(bounds) => widen(bounds, &row),
        }
    }

    /// Whether one of `conditions` matches a row of it, or may.
    pub(super) fn matched_by(&self, conditions: &[Conjunction]) -> bool {
        match self {
            Changed::Rows(rows, _) => rows.iter().any(|row| conditions.iter().any(|c| c.matches(row))),
            Changed::Bounds(bounds) => conditions.iter().any(|c| c.may_match(bounds)),
        }
    }
}

/// `bounds` widened to hold each value of `row` not null.
fn widen(bounds: &mut Vec<Option<(Value, Value)>>, row: &Row) {
    if bounds.len() < row.len() {
        bounds.resize(row.len(), None);
    }
    for (bound, v) in bounds.iter_mut().zip(row) {
        if v.is_null() {
            continue;
        }
        match bound {
            Some((least, greatest)) => {
                if compare(v, least) == Some(Ordering::Less) {
                    *least = v.clone();
                }
                if compare(v, greatest) == Some(Ordering::Greater) {
                    *greatest = v.clone();
                }
            }
            None => *bound = Some((v.clone(), v.clone())),
        }
    }
}
