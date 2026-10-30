//! \[EXTENSION\] A table constraint added [ISO/IEC 9075:1992 §11.16] or
//! dropped [§11.17] by ALTER TABLE: its descriptor and, for a unique
//! constraint, its index; the table's rows and storage stay as they are.
//! Split from db/alter.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::Made;
use crate::ast::{DropBehavior, TableConstraint, TableName};
use crate::binder::qualify;
use crate::catalog::{ConstraintKind, IndexDesc, QualifiedName};
use crate::db::State;
use crate::db::constraints::{Described, Within};
use crate::db::ddl::Dropped;
use crate::db::schema::constraint_index_name;
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key};
use crate::storage::project;
use crate::txn::{Timestamp, Txn};
use crate::value::Value;

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
        let unique = constraints
            .iter()
            .find_map(|k| if let ConstraintKind::Unique { columns, .. } = &k.kind { Some(columns.clone()) } else { None });
        desc.constraints.extend(constraints);
        for p in &pending {
            let resolved = self.resolve_foreign_key(names, &desc, p)?;
            desc.constraints.push(resolved);
        }
        Described::check_unique(&desc)?;
        desc.refresh_not_null();
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(desc.clone());
        self.catalog = Arc::from(catalog);
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
            let mut with_index = self.catalog.duplicate();
            with_index.add_index(self.constraint_index(qn, &name, columns));
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
        IndexDesc { name: name.to_owned(), table: qn.clone(), columns, method, unique: true, options: Vec::new() }
    }

    /// The index `name` of a unique constraint on `columns` of `qn`, filled
    /// with the rows `tx` sees, which are those every snapshot sees once it
    /// commits, as it holds the table's exclusive lock; or, without `tx`,
    /// with every version of its rows, unless the database is being opened
    /// from its files, which reopens or builds it (`index_structure`).
    fn unique_index(&mut self, qn: &QualifiedName, name: &str, columns: &[usize], tx: Option<&Txn>) -> Result<Arc<dyn Index>> {
        let desc = self.base_table(qn)?;
        let ix = self.constraint_index(qn, name, columns.to_vec());
        let method = Arc::clone(
            self.methods.get(self.constraint_method()).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] no constraint index method"))?,
        );
        let types = desc.types(columns).unwrap_or_default();
        let (structure, built) = self.index_structure(&*method, (&ix, &desc), &types)?;
        match tx {
            Some(tx) => {
                let all: Vec<usize> = (0..desc.columns.len()).collect();
                for read in Arc::clone(self.storage_of(qn)?).scan(tx.id, &all) {
                    let (id, row) = read?;
                    let Some(key) = project(&row, columns).filter(|k| !k.iter().any(Value::is_null)) else { continue };
                    structure.insert(Key(key), id)?;
                }
            }
            None if !built => self.fill_index(&*structure, qn, columns, Timestamp::MAX)?,
            None => {}
        }
        Ok(structure)
    }

    /// [ISO/IEC 9075:1992 §11.17] Drops the constraint `name`, its schema
    /// `implicit` if it names none [§5.4 SR8], of base table `qn`: and with CASCADE, the foreign keys that reference the columns
    /// of a unique one [SR3, GR1]; with RESTRICT, not while there are any
    /// [SR4]. A unique one's index goes with it.
    pub(in crate::db) fn drop_constraint(
        &mut self,
        (qn, implicit): (&QualifiedName, &str),
        name: &TableName,
        behavior: DropBehavior,
    ) -> Result<Dropped> {
        let mut desc = self.base_table(qn)?;
        let named = qualify(name, implicit);
        let at = desc
            .constraints
            .iter()
            .position(|c| c.name == named)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.17 SR2] {qn} has no constraint {named}")))?;
        let removed = desc.constraints.remove(at);
        let mut dropped = Dropped::default();
        let mut catalog = self.catalog.duplicate();
        if let ConstraintKind::Unique { columns, .. } = &removed.kind {
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
            for ix in self.catalog.indexes(qn).into_iter().filter(|ix| ix.unique && ix.columns == *columns) {
                catalog.remove_index(&ix.name);
                dropped.indexes.extend(self.indexes.remove(&ix.name));
            }
        }
        desc.refresh_not_null();
        catalog.replace_table(desc);
        self.catalog = Arc::from(catalog);
        Ok(dropped)
    }
}
