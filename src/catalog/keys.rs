//! \[EXTENSION\] An index's key, of a table's row (RULES.md D60). An index on a
//! collated column keys that column by its sort key, so a key is not the
//! row's values: every path from a row or values to a key is here, so none
//! can disagree with the index.

use super::{IndexDesc, TableDesc};
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
}

impl KeyShape {
    /// Whether a key column is by a sort key.
    pub fn is_collated(&self) -> bool {
        !self.collations.is_empty()
    }

    /// The key of `row`, if it has every key column.
    pub fn key_of(&self, row: &Row) -> Option<Key> {
        Some(keyed(project(row, &self.columns)?, &self.collations))
    }
}

/// The key of `values`, of the key columns in order, by `collations`.
fn keyed(values: Row, collations: &[Option<CollationRef>]) -> Key {
    if collations.is_empty() {
        return Key(values);
    }
    let key = |(v, c): (Value, &Option<CollationRef>)| c.as_ref().map_or_else(|| v.clone(), |c| c.sort_key(&v));
    Key(values.into_iter().zip(collations).map(key).collect())
}

impl IndexDesc {
    /// Its key's shape, for reads.
    pub fn key_shape(&self) -> KeyShape {
        KeyShape { columns: self.columns.clone(), collations: self.collations.clone() }
    }

    /// Whether a key column is by a sort key.
    pub fn is_collated(&self) -> bool {
        !self.collations.is_empty()
    }

    /// The key of `row`, if it has every key column.
    pub fn key_of(&self, row: &Row) -> Option<Key> {
        let values = project(row, &self.columns)?;
        Some(self.key_values(values))
    }

    /// The key of `values`, of the key columns in order.
    pub fn key_values(&self, values: Row) -> Key {
        keyed(values, &self.collations)
    }

    /// The types of the key, of `table`: those of its columns, but of
    /// BINARY VARYING where a column is by a sort key.
    pub fn key_types(&self, table: &TableDesc) -> Option<Vec<DataType>> {
        let mut types = table.types(&self.columns)?;
        for (t, c) in types.iter_mut().zip(&self.collations) {
            if c.is_some() {
                *t = SORT_KEY;
            }
        }
        Some(types)
    }
}
