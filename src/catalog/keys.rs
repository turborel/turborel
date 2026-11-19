//! \[EXTENSION\] An index's key, of a table's row (RULES.md D60). An index on a
//! collated column keys that column by its sort key, so a key is not the
//! row's values: every path from a row or values to a key is here, so none
//! can disagree with the index. So is whether the index holds a key with a
//! null [ISO/IEC 9075-2:2023 §11.7]. A key part that is an expression is
//! its value of the row (`catalog/key_expressions.rs`, RULES.md
//! D92).

use super::key_expressions::{part_values, reads_of};
use super::{IndexDesc, KeyExpression, TableDesc};
use crate::error::{Code, Result, SqlError};
use crate::index::Key;
use crate::storage::{Row, project};
use crate::value::collation::{CollationRef, SORT_KEY};
use crate::value::{DataType, Value};

/// What a read needs of an index to make the key of a row: cheap to hold
/// for each probe, as the whole `IndexDesc` is not.
pub struct KeyShape {
    /// The positions, in the table, of the key's columns.
    pub columns: Vec<usize>,
    /// As `IndexDesc::collations`.
    collations: Vec<Option<CollationRef>>,
    /// As `IndexDesc::expressions`.
    expressions: Vec<KeyExpression>,
    /// The columns the key reads of a row (`IndexDesc::reads`).
    pub reads: Vec<usize>,
}

impl KeyShape {
    /// Whether a key column is by a sort key.
    pub fn is_collated(&self) -> bool {
        !self.collations.is_empty()
    }

    /// Whether a key part is an expression.
    pub fn has_expressions(&self) -> bool {
        !self.expressions.is_empty()
    }

    /// The key of `row`, if it has every key column; none if an expression
    /// raises, which no row a table holds makes it (RULES.md D92).
    pub fn key_of(&self, row: &Row) -> Option<Key> {
        let values = if self.expressions.is_empty() {
            project(row, &self.columns)?
        } else {
            part_values(&self.columns, &self.expressions, row).ok()??
        };
        Some(keyed(values, &self.collations))
    }
}

/// The key of `values`, of the key columns in order, by `collations`.
fn keyed(values: Row, collations: &[Option<CollationRef>]) -> Key {
    if collations.is_empty() {
        return Key(values);
    }
    // No index keys by a plug-in's collation, whose call may fail and may
    // not be made under the locks keys are made under (RULES.md D221,
    // R-ERR-4): one is refused as the index is defined, so this fails not.
    let key =
        |(v, c): (Value, &Option<CollationRef>)| c.as_ref().map_or_else(|| v.clone(), |c| c.sort_key(&v).unwrap_or_else(|_| v.clone()));
    Key(values.into_iter().zip(collations).map(key).collect())
}

impl IndexDesc {
    /// Its key's shape, for reads.
    pub fn key_shape(&self) -> KeyShape {
        let (columns, collations, expressions) = (self.columns.clone(), self.collations.clone(), self.expressions.clone());
        KeyShape { reads: reads_of(&columns, &expressions), columns, collations, expressions }
    }

    /// Whether a key column is by a sort key.
    pub fn is_collated(&self) -> bool {
        !self.collations.is_empty()
    }

    /// The key of `row`, if it has every key column; none if an expression
    /// raises (`computed_key`).
    pub fn key_of(&self, row: &Row) -> Option<Key> {
        if self.expressions.is_empty() {
            return Some(self.key_values(project(row, &self.columns)?));
        }
        self.computed_key(row).ok()
    }

    /// The key of `row`, as a write makes its entry.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the row lacks a key column; as
    /// `KeyExpression::value_of`, a limit of this implementation alone,
    /// which then fails the write (RULES.md D92).
    pub fn computed_key(&self, row: &Row) -> Result<Key> {
        let values = part_values(&self.columns, &self.expressions, row)?;
        let values = values.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] a row is shorter than index {}", self.name)))?;
        Ok(self.key_values(values))
    }

    /// Whether it holds an entry of `key`: of no null, or of any if it
    /// holds nulls (`IndexDesc::nulls`). Every path that adds or removes
    /// an entry asks it, so none can disagree with another.
    pub fn holds(&self, key: &Key) -> bool {
        self.nulls || !key.0.iter().any(Value::is_null)
    }

    /// The key of `row`'s entry, if it has every key column and the index
    /// holds the key (`holds`).
    pub fn entry_of(&self, row: &Row) -> Option<Key> {
        self.key_of(row).filter(|k| self.holds(k))
    }

    /// The key of `values`, of the key columns in order.
    pub fn key_values(&self, values: Row) -> Key {
        keyed(values, &self.collations)
    }

    /// The types of the key, of `table`: those of its columns, or its
    /// expressions' declared types, but of BINARY VARYING where a column is
    /// by a sort key.
    pub fn key_types(&self, table: &TableDesc) -> Option<Vec<DataType>> {
        let part_type = |(at, &c): (usize, &usize)| match self.expressions.iter().find(|e| e.part == at) {
            Some(e) => e.bound.as_ref().map(|b| b.data_type().clone()),
            None => table.columns.get(c).map(|c| c.data_type.clone()),
        };
        let mut types: Vec<DataType> = if self.expressions.is_empty() {
            table.types(&self.columns)?
        } else {
            self.columns.iter().enumerate().map(part_type).collect::<Option<_>>()?
        };
        for (t, c) in types.iter_mut().zip(&self.collations) {
            if c.is_some() {
                *t = SORT_KEY;
            }
        }
        Some(types)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.7] an index holds a
    /// key with a null only if it holds nulls, a key of none always. MC/DC
    /// of `nulls || no null`: a key of none held by an index of no nulls
    /// (no null true); one with a null not held by it (both false), held
    /// by an index of nulls (nulls true).
    #[test]
    fn an_index_holds_a_key_with_a_null_only_if_it_holds_nulls() {
        let index = |nulls: bool| IndexDesc {
            name: "I".into(),
            table: QualifiedName { auth: "A".into(), ident: "T".into() },
            columns: vec![0, 1],
            method: String::new(),
            unique: true,
            options: Vec::new(),
            collations: Vec::new(),
            nulls,
            expressions: Vec::new(),
        };
        let one = Value::Bool(true);
        let (whole, nulled) = (vec![one.clone(), one.clone()], vec![one, Value::Null]);
        assert!(index(false).entry_of(&whole).is_some(), "a key of no null");
        assert!(index(false).entry_of(&nulled).is_none(), "a null, of an index of none");
        assert!(index(true).entry_of(&nulled).is_some(), "a null, of an index of nulls");
    }
}
