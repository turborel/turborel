//! \[EXTENSION\] A table constraint added [ISO/IEC 9075:1992 §11.16] or
//! dropped [§11.17] by ALTER TABLE: its descriptor and, for a unique
//! constraint, its index; the table's rows and storage stay as they are.

use std::sync::Arc;

use super::{Cause, Made};
use crate::ast::{DropBehavior, TableConstraint, TableName};
use crate::binder::qualify;
use crate::catalog::{Catalog, ConstraintKind, IndexDesc, QualifiedName};
use crate::db::State;
use crate::db::constraints::{Described, Within};
use crate::db::ddl::Dropped;
use crate::db::schema::constraint_index_name;
use crate::error::{Code, Result, SqlError};
use crate::index::Index;
use crate::txn::{Concurrency, Timestamp, Txn};

/// A Syntax Rule violation.
fn rule(message: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, message)
}

impl State {
    /// [ISO/IEC 9075:1992 §11.16] Adds the constraint `c`, its names of
    /// schema `names`, to base table `qn`, with the index of a unique one:
    /// `made`'s, if given; else made
    /// and filled with the rows transaction `tx` sees, or, without one
    /// (making the change again from the log), its table's.
    pub(in crate::db) fn add_constraint(
        &mut self,
        (qn, names): (&QualifiedName, &str),
        c: &TableConstraint,
        (made, tx): (Option<&Made>, Option<&Txn>),
    ) -> Result<Made> {
        let mut desc = self.base_table(qn)?;
        let table_columns = desc.columns.clone();
        let mut added = Described::new((qn, names, Within::Session), &table_columns, self.all_constraint_names());
        added.reserve(c.name.iter());
        added.table_constraint(c)?;
        let (constraints, pending) = (added.constraints, added.pending);
        self.constraint_schemas(qn, (&constraints, &pending))?;
        let unique = constraints.iter().find_map(|k| {
            if let ConstraintKind::Unique { columns, period, .. } = &k.kind {
                Some(desc.key_columns(columns, period.as_deref()))
            } else {
                None
            }
        });
        desc.constraints.extend(constraints);
        crate::db::constraints::periods::keyed_periods(&desc).and_then(|()| self.check_connections(&desc, &[]))?;
        for p in &pending {
            let resolved = self.resolve_foreign_key(names, &desc, p)?;
            desc.constraints.push(resolved);
        }
        Described::check_unique(&desc)?;
        desc.refresh_not_null();
        self.inherited_nullability(&mut desc, &[]);
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(desc.clone());
        self.catalog = Arc::from(catalog);
        self.renullified(qn);
        let mut built = Made::default();
        if let Some(columns) = unique {
            let name = (1..=self.catalog.indexes(qn).len().saturating_add(1))
                .map(|k| constraint_index_name(qn, k))
                .find(|n| self.catalog.index(n).is_none())
                .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no name is free for a UNIQUE constraint of {qn}")))?;
            let structure = match made.and_then(|m| m.indexes.first()) {
                Some((_, structure)) => Arc::clone(structure),
                None => self.unique_index(qn, &name, &columns, tx)?,
            };
            self.indexes.insert(name.clone(), Arc::clone(&structure));
            // Of what the table's columns and its new constraint make it:
            // its key's collations, and whether it holds nulls.
            let ix = self.with_derived(self.constraint_index(qn, &name, columns), &desc)?;
            let mut with_index = self.catalog.duplicate();
            with_index.add_index(ix);
            self.catalog = Arc::from(with_index);
            built.indexes.push((name, structure));
        }
        let mut checked = desc;
        self.bind_checks(&mut checked)?;
        let mut with_checks = self.catalog.duplicate();
        with_checks.replace_table(checked);
        self.catalog = Arc::from(with_checks);
        Ok(built)
    }

    /// The descriptor of index `name` of a UNIQUE constraint on `columns`
    /// of `qn`.
    fn constraint_index(&self, qn: &QualifiedName, name: &str, columns: Vec<usize>) -> IndexDesc {
        let method = self.constraint_method().to_owned();
        IndexDesc {
            name: name.to_owned(),
            table: qn.clone(),
            columns,
            method,
            unique: true,
            options: Vec::new(),
            collations: Vec::new(),
            nulls: false,
            expressions: Vec::new(),
        }
    }

    /// The index `name` of a unique constraint on `columns` of `qn`, filled
    /// with the rows `tx` sees, which are those every snapshot sees once it
    /// commits, as it holds the table's exclusive lock; or, without `tx`,
    /// with every version of its rows, unless the database is being opened
    /// from its files, which reopens or builds it (`index_structure`).
    /// Empty, of a temporary table.
    fn unique_index(&mut self, qn: &QualifiedName, name: &str, columns: &[usize], tx: Option<&Txn>) -> Result<Arc<dyn Index>> {
        let desc = self.base_table(qn)?;
        let ix = self.with_derived(self.constraint_index(qn, name, columns.to_vec()), &desc)?;
        let method = Arc::clone(
            self.methods.get(self.constraint_method()).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] no constraint index method"))?,
        );
        let types = ix.key_types(&desc).unwrap_or_default();
        let (structure, built) = self.index_structure(&*method, (&ix, &desc), &types)?;
        // [DESIGN] A temporary table's own storage holds no row: each
        // session's instance makes its entries of its own rows (RULES.md
        // D49), which `storage_of` gives in a session's definitions.
        if desc.temporary.is_some() {
            return Ok(structure);
        }
        match tx {
            Some(tx) => self.fill_for(tx, qn, (&*structure, &ix))?,
            None if !built => self.fill_index(&*structure, &ix, Timestamp::MAX)?,
            None => {}
        }
        Ok(structure)
    }

    /// Fills `index`, of a unique constraint `tx` adds to `qn`. Of an
    /// engine keeping versions, with every committed version a reader may
    /// still see, as collection takes back an entry of each (RULES.md
    /// D447), and with each row `tx` itself changed as it left it; else
    /// with the rows `tx` sees.
    fn fill_for(&self, tx: &Txn, qn: &QualifiedName, (index, ix): (&dyn Index, &IndexDesc)) -> Result<()> {
        // Joined first: its engine reads only for a transaction begun in
        // it, though none of the statement's has read it.
        self.read_lock(tx, qn)?;
        let storage = Arc::clone(self.storage_of(qn)?);
        let versions = matches!(storage.capabilities().concurrency, Concurrency::Engine { .. });
        if versions {
            self.fill_index(index, ix, Timestamp::MAX)?;
            if !tx.written_tables().contains(qn) {
                return Ok(());
            }
        }
        let all: Vec<usize> = (0..self.base_table(qn)?.columns.len()).collect();
        for read in Arc::clone(&storage).scan(tx.id, &all) {
            let (id, row) = read?;
            // A committed version, counted above, unless `tx` changed it.
            if versions && storage.newest(tx.id, id)?.stamp != 0 {
                continue;
            }
            let Some(key) = ix.entry_of(&row) else { continue };
            index.insert(key, id)?;
        }
        Ok(())
    }

    /// The unique index of `qn` of key `key`, a unique constraint's,
    /// removed from `catalog` and noted in `dropped`.
    fn drop_key_index(&mut self, (qn, key): (&QualifiedName, &[usize]), catalog: &mut dyn Catalog, dropped: &mut Dropped) {
        for ix in self.catalog.indexes(qn).into_iter().filter(|ix| ix.unique && ix.columns == key) {
            catalog.remove_index(&ix.name);
            dropped.indexes.extend(self.indexes.remove(&ix.name));
        }
    }

    /// [ISO/IEC 9075:1992 §11.17] Drops the constraint `name`, its schema
    /// `implicit` if it names none [§5.4 SR8], of base table `qn`. With
    /// CASCADE the foreign keys that reference the columns of a unique one
    /// go with it [SR3, GR1], and so do the constraints, views, assertions
    /// and triggers that depend on it [ISO/IEC 9075-2:2023 §11.26 GR1, GR3
    /// to GR5]. With RESTRICT it is refused while there are any [SR4;
    /// ISO/IEC 9075-2:2023 §11.26 SR13]. A unique one's index goes with it.
    pub(in crate::db) fn drop_constraint(
        &mut self,
        (qn, implicit): (&QualifiedName, &str),
        name: &TableName,
        behavior: DropBehavior,
    ) -> Result<Dropped> {
        let mut desc = self.base_table(qn)?;
        // The definitions as they are, of which what depends on it is told.
        let before = self.clone();
        let named = qualify(name, implicit);
        let at = desc
            .constraints
            .iter()
            .position(|c| c.name == named)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.17 SR2] {qn} has no constraint {named}")))?;
        let removed = crate::db::constraints::periods::removed_constraint(&mut desc, at)?;
        let mut dropped = Dropped::default();
        let mut catalog = self.catalog.duplicate();
        if let ConstraintKind::Unique { columns, period, .. } = &removed.kind {
            let dependent = |kind: &ConstraintKind| {
                if let ConstraintKind::ForeignKey(fk) = kind {
                    fk.table == *qn && fk.referenced.len() == columns.len() && fk.referenced.iter().all(|c| columns.contains(c))
                } else {
                    false
                }
            };
            for table in self.catalog.tables() {
                let mut other = if table.name == *qn { desc.clone() } else { table.clone() };
                let first = other.constraints.iter().find(|c| dependent(&c.kind));
                if let (Some(rc), DropBehavior::Restrict) = (first, behavior) {
                    return Err(rule(format!(
                        "[ISO/IEC 9075:1992 §11.17 SR4] FOREIGN KEY {} of {} depends on {named}, so it is dropped only with CASCADE",
                        rc.name, other.name
                    )));
                }
                other.constraints.retain(|c| !dependent(&c.kind));
                if table.name == *qn {
                    desc = other;
                } else if other.constraints != table.constraints {
                    catalog.replace_table(other);
                }
            }
            self.drop_key_index((qn, &desc.key_columns(columns, period.as_deref())), &mut *catalog, &mut dropped);
        }
        desc.refresh_not_null();
        self.inherited_nullability(&mut desc, &[]);
        catalog.replace_table(desc);
        self.catalog = Arc::from(catalog);
        self.renullified(qn);
        // [ISO/IEC 9075-2:2023 §11.26 SR13a, SR13c, SR13e, GR1, GR3, GR4]
        // the constraints, views and assertions that bind by it;
        // [SR13f, GR5] the triggers; [SR13d, GR2] the routines.
        let cascade = behavior == DropBehavior::Cascade;
        dropped.absorb(self.drop_constraint_dependents(&before, qn, cascade)?);
        dropped.absorb(self.drop_dependent_triggers(&before, (Cause::Constraint, qn), cascade)?);
        dropped.absorb(self.drop_dependent_routines(&before, (Cause::Constraint, qn), cascade)?);
        Ok(dropped)
    }
}
