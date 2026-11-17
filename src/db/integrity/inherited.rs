//! [ISO/IEC 9075-2:2023 §4.17.9, §4.25] The constraints of a table of a
//! typed table hierarchy. Each table keeps its own rows, and a row of a
//! proper subtable has its superrow in each proper supertable, of the
//! subtable's first columns (`binder/hierarchy.rs`). So every constraint of
//! a supertable is one of each subtable too [§4.25.1], checked of the
//! subtable's rows at the same column positions; and a key or a referenced
//! row is looked for in every table of the family of the table whose
//! constraint it is (RULES.md D81).

use crate::catalog::{ConstraintDesc, ForeignKey, QualifiedName, TableDesc, proper_subtables, proper_supertables};
use crate::db::State;

impl State {
    /// The constraints of `table`: its own, then each proper supertable's,
    /// the nearest first.
    pub(super) fn constraints_of(&self, table: &QualifiedName) -> Vec<&ConstraintDesc> {
        let own = self.catalog.table(table).into_iter();
        let supers = proper_supertables(&*self.catalog, table);
        let tables = own.chain(supers.iter().filter_map(|s| self.catalog.table(s)));
        tables.flat_map(|d| d.constraints.iter()).collect()
    }

    /// The foreign keys of `table`, its own and its proper supertables',
    /// each with its name.
    pub(super) fn foreign_keys_of(&self, table: &QualifiedName) -> Vec<(&QualifiedName, &ForeignKey)> {
        let own = self.catalog.table(table).into_iter();
        let supers = proper_supertables(&*self.catalog, table);
        let tables = own.chain(supers.iter().filter_map(|s| self.catalog.table(s)));
        tables.flat_map(TableDesc::foreign_keys).collect()
    }

    /// `table` and its proper subtables: the tables whose rows are its
    /// rows, as superrows of a subtable's.
    pub(super) fn family(&self, table: &QualifiedName) -> Vec<QualifiedName> {
        let mut all = vec![table.clone()];
        if self.catalog.table(table).is_some_and(|d| d.typed.is_some()) {
            all.extend(proper_subtables(&*self.catalog, table));
        }
        all
    }

    /// Whether a change to a row of `table` is a change to a row of
    /// `referenced`: it is the table, or a proper supertable of it, whose
    /// row is its superrow.
    pub(super) fn reaches(&self, table: &QualifiedName, referenced: &QualifiedName) -> bool {
        table == referenced || proper_supertables(&*self.catalog, table).contains(referenced)
    }
}
