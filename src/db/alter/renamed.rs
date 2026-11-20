//! \[EXTENSION\] ALTER TABLE … RENAME TO …: a base table given a new name
//! in its schema (RULES.md D402). SQL:2023 renames no table [ISO/IEC
//! 9075-2:2023 §11.10], so this is an extension of the `<alter table
//! action>`s, as the RENAME COLUMN of `db/alter/rename.rs` is.
//!
//! The new name is one no table, view or property graph of the schema has,
//! as CREATE TABLE requires of its name [ISO/IEC 9075:1992 §11.3 SR2].
//! What refers to the table by its descriptor is changed with it: its
//! indexes, its privileges, its triggers, a foreign key of it or of
//! another table that references it, and its own CHECKs' horizons. Its
//! identity column's generator keeps the name it was given, so its
//! counter goes on (RULES.md D63). Its constraints and indexes keep their
//! names.
//!
//! What names it in a text read again, a view's query, a CHECK, an
//! assertion, a trigger's action, a routine's body, a property graph, a
//! generation expression or a key part, is not rewritten, as RENAME COLUMN
//! rewrites none (RULES.md D83); while one does, the rename is refused
//! (`renamed/texts.rs`). A trigger's own text names its subject table in
//! its ON clause, which is written again of the new name.
//!
//! The log's changes of a table's rows name it, so its rows are copied to
//! a new generation of its storage under the new name, by the
//! transaction's own writes, as ADD COLUMN copies them (`db/ddl.rs`); the
//! old name is retired as a table dropped is, and the generation given is
//! logged with the action, so a replay gives the same.

mod texts;

use std::sync::Arc;

use super::{Made, Reshape};
use crate::ast::{AlterAction, TableName};
use crate::catalog::{ConstraintKind, Horizon, IndexDesc, PrivilegeDesc, QualifiedName, TableDesc};
use crate::db::ddl::Dropped;
use crate::db::{State, api, rule};
use crate::error::{At, Result};
use crate::parser::{GENERATION, Parser};

/// The name table `table` has once `action` is done: its new name, of
/// its schema unless written of one, if `action` renames it.
pub(in crate::db) fn renamed_to(table: &QualifiedName, action: &AlterAction) -> QualifiedName {
    let AlterAction::RenameTable { to, .. } = action else { return table.clone() };
    QualifiedName { auth: to.auth.clone().unwrap_or_else(|| table.auth.clone()), ident: to.ident.clone() }
}

/// The tables the ALTER TABLE action logged as `action` changes, of table
/// `table`: it, and its new name if it is renamed.
pub(in crate::db) fn renamed_by(table: &QualifiedName, action: &str) -> Vec<QualifiedName> {
    let parsed = Parser::for_stored(action).and_then(|mut p| p.alter_table_action());
    match parsed {
        Ok(a @ AlterAction::RenameTable { .. }) => vec![table.clone(), renamed_to(table, &a)],
        Ok(_) | Err(_) => vec![table.clone()],
    }
}

/// \[DESIGN\] The text the log keeps of `action`, if it renames `table`:
/// its new name and the generation `next` gave its storage.
pub(in crate::db) fn logged_rename(next: &State, table: &QualifiedName, action: &AlterAction) -> Option<String> {
    let AlterAction::RenameTable { .. } = action else { return None };
    let to = renamed_to(table, action);
    let generation = next.generation_of(&to);
    Some(format!("RENAME TO {}.{} {GENERATION} {generation}", super::rename::quoted(&to.auth), super::rename::quoted(&to.ident)))
}

/// `horizon` with table `from` named `to`.
fn renamed_horizon(horizon: &Horizon, (from, to): (&QualifiedName, &QualifiedName)) -> Horizon {
    let widths = horizon.widths.iter().map(|(t, w)| (if t == from { to.clone() } else { t.clone() }, *w)).collect();
    Horizon { widths, ..horizon.clone() }
}

/// `desc`'s constraints with table `from` named `to`: a foreign key's
/// referenced table, and a CHECK's horizon.
fn renamed_constraints(desc: &mut TableDesc, (from, to): (&QualifiedName, &QualifiedName)) {
    for c in &mut desc.constraints {
        match &mut c.kind {
            ConstraintKind::ForeignKey(fk) => {
                if fk.table == *from {
                    fk.table = to.clone();
                }
            }
            ConstraintKind::Check { horizon, .. } => *horizon = renamed_horizon(horizon, (from, to)),
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::Connection(_) => {}
        }
    }
}

impl State {
    /// \[EXTENSION\] Base table `qn` renamed `to`, its storage of generation `generation` if the log gives it,
    /// else one no table of the name had (`made`'s, if given): what it
    /// took away, what it made, and its rows to be copied as they are.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the name is of another schema, or a table, view or
    /// property graph of the schema has it [ISO/IEC 9075:1992 §11.3 SR2];
    /// as `renamable` and `unnamed_table`; the engine's.
    pub(super) fn rename_table(
        &mut self,
        qn: &QualifiedName,
        (to, generation): (&TableName, Option<u32>),
        made: Option<&Made>,
    ) -> Result<(Dropped, Made, Option<Reshape>)> {
        let desc = self.base_table(qn)?;
        let new = renamed_to(qn, &AlterAction::RenameTable { to: to.clone(), generation });
        if new.auth != qn.auth {
            return Err(rule(format!("[EXTENSION] {qn} is renamed in its schema, {}, not in {}", qn.auth, new.auth)).or_at(to.at));
        }
        self.new_object(&new, "§11.3 SR2").or_at(to.at)?;
        self.renamable(&desc)?;
        self.unnamed_table(&desc)?;
        let mut next = desc.clone();
        next.name = new.clone();
        // A table of the name dropped had generations of its own.
        next.generation = generation.unwrap_or_else(|| self.retired.get(&new).map_or(0, |g| g.saturating_add(1)));
        renamed_constraints(&mut next, (qn, &new));
        self.rename_in_catalog(&next, qn);
        self.retired.entry(qn.clone()).and_modify(|g| *g = (*g).max(desc.generation)).or_insert(desc.generation);
        if let Some(engine) = self.placement.remove(qn) {
            self.placement.insert(new.clone(), engine);
        }
        self.locks.insert(new.clone(), self.tx.name_lock(&new));
        let storage = if let Some(storage) = made.and_then(|m| m.storage.clone()) {
            storage
        } else {
            let engine = Arc::clone(self.engines.get(&next.engine).ok_or_else(|| api(format!("[DESIGN] no engine {}", next.engine)))?);
            self.table_storage(&*engine, &next)?
        };
        let mut dropped = Dropped::default();
        dropped.storage.extend(self.storage.remove(qn));
        self.storage.insert(Arc::new(new.clone()), Arc::clone(&storage));
        let indexes = self.remake_indexes(&next, Reshape::Keep, made, &mut dropped)?;
        let mut catalog = self.catalog.duplicate();
        for (ix, _) in &indexes {
            catalog.add_index(ix.clone());
        }
        self.catalog = Arc::from(catalog);
        self.rebind_checks(&new)?;
        let built = Made { storage: Some(storage), indexes: indexes.into_iter().map(|(ix, s)| (ix.name, s)).collect(), tables: Vec::new() };
        Ok((dropped, built, Some(Reshape::Keep)))
    }

    /// The catalog with table `old` described as `next`, under its new
    /// name: its indexes, its privileges, its triggers and the foreign keys
    /// of other tables that reference it changed with it.
    fn rename_in_catalog(&mut self, next: &TableDesc, old: &QualifiedName) {
        let new = &next.name;
        let mut catalog = self.catalog.duplicate();
        catalog.remove_table(old);
        catalog.add_table(next.clone());
        for other in self.catalog.tables().into_iter().filter(|t| t.name != *old) {
            let mut changed = other.clone();
            renamed_constraints(&mut changed, (old, new));
            if changed.constraints != other.constraints {
                catalog.replace_table(changed);
            }
        }
        for ix in self.catalog.indexes(old) {
            catalog.remove_index(&ix.name);
            catalog.add_index(IndexDesc { table: new.clone(), ..ix.clone() });
        }
        let privileges: Vec<PrivilegeDesc> =
            self.catalog.privileges(old).into_iter().map(|p| PrivilegeDesc { table: new.clone(), ..p.clone() }).collect();
        catalog.revoke_all(old);
        catalog.grant(privileges);
        for t in self.catalog.triggers_on(old) {
            catalog.put_trigger(texts::renamed_trigger(t, (old, new)));
        }
        self.catalog = Arc::from(catalog);
    }
}
