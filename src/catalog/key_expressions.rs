//! \[EXTENSION\] The key parts of an index that are expressions (RULES.md
//! D92). Each is kept as written, with its place in the key, and
//! bound over its table's row whenever the index enters a catalog
//! (`State::with_derived`): as it is created, as the database opens and as
//! its table is altered, so a column moved is read where it is now. Its
//! place in `IndexDesc::columns` holds `EXPRESSION_PART`, no column's
//! position, so a path that reads the key's columns alone finds none
//! there and passes the index by.

use std::sync::Arc;

use super::{IndexDesc, TableDesc};
use crate::error::{Code, Result, SqlError};
use crate::index::expression::BoundKey;
use crate::storage::Row;
use crate::value::Value;

/// The position `IndexDesc::columns` holds of a key part that is an
/// expression: of no column a table can have.
pub const EXPRESSION_PART: usize = usize::MAX;

/// A key part that is an expression.
#[derive(Debug, Clone)]
pub struct KeyExpression {
    /// Its place in the key.
    pub part: usize,
    /// The `<value expression>`, as written.
    pub text: String,
    /// It bound over its table's row (`index/expression.rs`). Derived, not
    /// stored; none until bound.
    pub bound: Option<Arc<BoundKey>>,
}

impl KeyExpression {
    /// The key part at `part` of the expression `text`, not yet bound.
    pub fn unbound(part: usize, text: String) -> KeyExpression {
        KeyExpression { part, text, bound: None }
    }

    /// Its value of `row`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it is not bound; as `BoundKey::value_of`.
    pub fn value_of(&self, row: &Row) -> Result<Value> {
        let bound = self.bound.as_ref().ok_or_else(|| {
            SqlError::new(Code::Api, format!("[DESIGN] the key expression {} is not bound to its table's row", self.text))
        })?;
        bound.value_of(row)
    }
}

/// The values of the key parts of `row`, a key of `columns`, those at the
/// places of `expressions` theirs: none if the row lacks a key column.
///
/// # Errors
///
/// As `KeyExpression::value_of`.
pub(super) fn part_values(columns: &[usize], expressions: &[KeyExpression], row: &Row) -> Result<Option<Row>> {
    let mut values = Vec::with_capacity(columns.len());
    for (at, &column) in columns.iter().enumerate() {
        if let Some(e) = expressions.iter().find(|e| e.part == at) {
            values.push(e.value_of(row)?);
            continue;
        }
        let Some(value) = row.get(column) else { return Ok(None) };
        values.push(value.clone());
    }
    Ok(Some(values))
}

/// The columns a key of `columns` and `expressions` reads of a row, in
/// order, each once.
pub(super) fn reads_of(columns: &[usize], expressions: &[KeyExpression]) -> Vec<usize> {
    let mut reads: Vec<usize> = columns.iter().copied().filter(|&c| c != EXPRESSION_PART).collect();
    reads.extend(expressions.iter().filter_map(|e| e.bound.as_ref()).flat_map(|b| b.reads().iter().copied()));
    reads.sort_unstable();
    reads.dedup();
    reads
}

impl IndexDesc {
    /// Whether a key part is an expression.
    pub fn has_expressions(&self) -> bool {
        !self.expressions.is_empty()
    }

    /// The columns its key reads of a row: its key columns, and those its
    /// expressions read, in order, each once.
    pub fn reads(&self) -> Vec<usize> {
        if self.expressions.is_empty() {
            return self.columns.clone();
        }
        reads_of(&self.columns, &self.expressions)
    }

    /// Each key part's expression as written, with its place in the key.
    pub fn texts(&self) -> Vec<(usize, String)> {
        self.expressions.iter().map(|e| (e.part, e.text.clone())).collect()
    }

    /// Each key part as shown, of `table`: its column's name, or its
    /// expression as written, in parentheses.
    pub fn part_names(&self, table: &TableDesc) -> Vec<String> {
        let shown = |(at, &c): (usize, &usize)| match self.expressions.iter().find(|e| e.part == at) {
            Some(e) => Some(format!("({})", e.text)),
            None => table.columns.get(c).map(|c| c.name.clone()),
        };
        self.columns.iter().enumerate().filter_map(shown).collect()
    }

    /// Whether `other` has the same key parts: columns, and expressions as
    /// written.
    pub fn same_parts(&self, other: &IndexDesc) -> bool {
        self.columns == other.columns && self.texts() == other.texts()
    }

    /// Whether `old` and `new`, two versions of a row, hold the same value,
    /// as written out, in every column its key reads, so they have one
    /// key: its expressions are deterministic.
    pub fn unchanged(&self, old: &Row, new: &Row) -> bool {
        self.reads().iter().all(|&c| match (old.get(c), new.get(c)) {
            (Some(a), Some(b)) => format!("{a:?}") == format!("{b:?}"),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        })
    }
}
