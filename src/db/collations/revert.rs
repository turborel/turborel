//! \[EXTENSION\] [ISO/IEC 9075-2:2023 §11.44 GR2, §12.7 GR12ad, §11.2 GR5]
//! The columns of a collation reverted to the default, as DROP COLLATION,
//! REVOKE USAGE with CASCADE and DROP SCHEMA with CASCADE revert them: each
//! table's rows copied, as ALTER TABLE copies them, so its indexes are in
//! the order of the collation it ends with (RULES.md D60).
//!
//! A table whose foreign key references a table reverted after it, or not
//! reverted at all, is checked again once all are, so a key holds by the
//! collation the referenced columns end with. The reverts need no Access
//! Rule, as the statements the standard effectively executes run without
//! one.

use super::super::ddl::validate_rows;
use super::super::{Session, State};
use crate::ast::{AlterAction, DropBehavior, PrivilegeObject, SchemaStatement};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::error::{At, Result};
use crate::parser::REVERT_COLLATION;
use crate::system::INFORMATION_SCHEMA;
use crate::txn::Txn;

impl State {
    /// The tables with a column of collation `qn`, but those of schema
    /// `except`, in the order of their names.
    pub(in crate::db) fn tables_of_collation(&self, qn: &QualifiedName, except: Option<&str>) -> Vec<QualifiedName> {
        let of = |c: &ColumnDesc| c.collation.as_ref() == Some(qn);
        let tables = self.catalog.tables().into_iter().filter(|t| except != Some(t.name.auth.as_str()) && t.columns.iter().any(of));
        tables.map(|t| t.name.clone()).collect()
    }

    /// The collations made in schema `auth`.
    pub(in crate::db) fn schema_collations(&self, auth: &str) -> Vec<QualifiedName> {
        self.catalog.collations().into_iter().filter(|c| c.name.auth == auth).map(|c| c.name.clone()).collect()
    }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §11.44 GR2] The columns of collation `qn` of
    /// `tables` made of the character set's, each table's rows copied
    /// (`AlterAction::RevertCollation`), in the order given, and the tables
    /// referencing them checked again. If one fails after
    /// another is done, the transaction only rolls back: the one done keeps
    /// its new definition, its copied rows undone.
    pub(in crate::db) fn revert_columns(&self, tx: &Txn, qn: &QualifiedName, tables: &[QualifiedName]) -> Result<()> {
        let action = AlterAction::RevertCollation(qn.clone());
        let text = format!("{REVERT_COLLATION} {}.{}", qn.auth, qn.ident);
        let mut done = 0_usize;
        let mut revert = || {
            for table in tables {
                let fresh = self.definitions(self.db.snapshot()?)?;
                let temporary = fresh.base_table(table)?.temporary;
                self.alter_base_table(&fresh, tx, (table, &action, &text), temporary)?;
                done = done.saturating_add(1);
            }
            self.recheck_referencing(tx, tables)
        };
        let reverted = revert();
        if reverted.is_err() && done > 0 {
            tx.doom("a collation's columns were reverted on a table before one failed");
        }
        reverted
    }

    /// Each table whose foreign key references one of `reverted`, the
    /// tables reverted in that order, and that was not reverted after it:
    /// its rows checked again, as a key holds by the collation the
    /// referenced columns end with. A temporary table references only
    /// temporary ones [ISO/IEC 9075:1992 §11.8 SR4], reverted with it.
    fn recheck_referencing(&self, tx: &Txn, reverted: &[QualifiedName]) -> Result<()> {
        let next = self.definitions(self.db.snapshot()?)?;
        let at = |t: &QualifiedName| reverted.iter().position(|r| r == t);
        for t in next.catalog.tables().into_iter().filter(|t| t.temporary.is_none()) {
            let before = |parent: &QualifiedName| *parent != t.name && at(parent).is_some_and(|p| at(&t.name).is_none_or(|c| c < p));
            if t.foreign_keys().any(|(_, fk)| before(&fk.table)) {
                validate_rows(&next, tx, &t.name, self.effects(&next))?;
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §12.7 GR12ad] Of REVOKE USAGE on a collation
    /// with CASCADE, `statement`, the columns of the tables of each schema
    /// left without it reverted, before the revoke is made; nothing for
    /// another statement. The collation's name is held exclusive already.
    ///
    /// # Errors
    ///
    /// The revoke's own, before a row is copied; as `revert_columns`.
    pub(in crate::db) fn revert_impacted(&self, tx: &Txn, statement: &SchemaStatement) -> Result<()> {
        let SchemaStatement::Revoke(r) = statement else { return Ok(()) };
        if r.object != PrivilegeObject::Collation || r.behavior != DropBehavior::Cascade {
            return Ok(());
        }
        let fresh = self.definitions(self.db.snapshot()?)?;
        let (qn, tables) = fresh.impacted_tables(&self.auth(), r).or_at(r.table.at)?;
        if tables.is_empty() {
            return Ok(());
        }
        self.revert_columns(tx, &qn, &tables).or_at(r.table.at)?;
        // Refused once its columns are reverted: the transaction only
        // rolls back.
        let after = self.definitions(self.db.snapshot()?)?;
        let made = State::clone(&after).revoke((&self.auth(), &self.default_schema()), r).map(|_| ());
        if made.is_err() {
            tx.doom("a REVOKE failed with a collation's columns reverted");
        }
        made
    }
}

/// The collation made that the column `action` adds to table `qn` is of,
/// as `next` has it.
pub(in crate::db) fn added_collation(next: &State, qn: &QualifiedName, action: &AlterAction) -> Option<QualifiedName> {
    let AlterAction::AddColumn(def) = action else { return None };
    let column = next.catalog.table(qn)?.columns.iter().find(|c| c.name == def.name)?;
    column.collation.clone().filter(is_made)
}

/// Whether collation `name` is one a definition made, not a built-in.
pub(in crate::db) fn is_made(name: &QualifiedName) -> bool {
    name.auth != INFORMATION_SCHEMA
}
