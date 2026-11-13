//! \[DESIGN\] The generated columns whose generation expressions name a
//! domain, by a CAST to it, or a collation a definition made, by a
//! `<collate clause>` [ISO/IEC 9075-2:2023 §11.40 SR2a, §12.7 GR12h,
//! GR12z].
//!
//! DROP DOMAIN with RESTRICT refuses one, as its descriptor references the
//! domain \[§11.40 SR2a\]. The standard loses a column whose generation
//! expression loses its required privileges \[§12.7 GR12h iv\], which name
//! only EXECUTE, so a domain or collation dropped, or USAGE on it revoked,
//! would leave an expression that no longer binds. Such a column is lost
//! too: refused with RESTRICT, as what a revoke abandons is \[ISO/IEC
//! 9075:1992 §11.37 SR18\], and with CASCADE dropped first, as ALTER TABLE
//! DROP COLUMN CASCADE drops a lost column \[GR12z\], each logged apart.
//! The statement's own change then finds none, and refuses if it does.

use super::super::collations::collates;
use super::super::domains::casts_to;
use super::super::{Session, State, api, rule};
use crate::ast::{AlterAction, DropBehavior, PrivilegeObject, RevokeStatement};
use crate::catalog::{Generation, QualifiedName};
use crate::error::Result;
use crate::log::record::DropTarget;
use crate::txn::Txn;

/// A generated column, with its table.
pub(in crate::db) type Column = (QualifiedName, String);

/// What a generation expression names, besides its table's columns.
#[derive(Clone, Copy)]
pub(in crate::db) enum Named<'a> {
    /// A domain, by a CAST to it.
    Domain(&'a QualifiedName),
    /// A collation a definition made, by a `<collate clause>`.
    Collation(&'a QualifiedName),
}

impl Named<'_> {
    /// Whether generation expression `g` names it.
    fn in_expression(self, g: &Generation) -> bool {
        match self {
            Named::Domain(qn) => casts_to(&g.text, &g.names, qn),
            Named::Collation(qn) => collates(&g.text, qn),
        }
    }

    /// It, as a refusal names it.
    fn described(self) -> String {
        match self {
            Named::Domain(qn) => format!("domain {qn}"),
            Named::Collation(qn) => format!("collation {qn}"),
        }
    }
}

impl State {
    /// The generated columns of the tables of a schema `within` takes whose
    /// generation expressions name `named`.
    pub(in crate::db) fn generations_naming(&self, named: Named<'_>, within: &dyn Fn(&str) -> bool) -> Vec<Column> {
        let mut columns = Vec::new();
        for t in self.catalog.tables().into_iter().filter(|t| within(&t.name.auth)) {
            let naming = t.columns.iter().filter(|c| c.generation().is_some_and(|g| named.in_expression(g)));
            columns.extend(naming.map(|c| (t.name.clone(), c.name.clone())));
        }
        columns
    }

    /// That no generation expression names `named`, which goes: each that
    /// did was dropped first (`Session::dropping_generations`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one does.
    pub(in crate::db) fn generations_gone(&self, named: Named<'_>) -> Result<()> {
        match self.generations_naming(named, &|_| true).first() {
            Some((t, c)) => Err(api(format!("[DESIGN] generated column {t}.{c} names {}, and was not dropped first", named.described()))),
            None => Ok(()),
        }
    }

    /// The generated columns DROP of `target` with CASCADE drops first:
    /// those that name the domain or collation it drops, or, of a schema,
    /// those of other schemas' tables that name its domains or collations,
    /// as its own tables go with it [ISO/IEC 9075:1992 §11.2 GR1, GR3,
    /// GR5]. And the columns a type it drops loses, of a type based on it
    /// or whose generation expressions reference it [ISO/IEC 9075-2:2023
    /// §12.7 GR12h], as revoking every privilege on it loses them [§11.59
    /// GR7].
    pub(in crate::db) fn generations_dropped(&self, target: &DropTarget) -> Vec<Column> {
        match target {
            DropTarget::Domain(qn) => self.generations_naming(Named::Domain(qn), &|_| true),
            DropTarget::Type(qn) => self.columns_lost_with(std::slice::from_ref(qn), &|_| true),
            DropTarget::Collation(qn) => self.generations_naming(Named::Collation(qn), &|_| true),
            DropTarget::Schema(auth) => {
                let other = |schema: &str| schema != auth;
                let mut columns = Vec::new();
                for d in self.catalog.domains().into_iter().filter(|d| d.name.auth == *auth) {
                    columns.extend(self.generations_naming(Named::Domain(&d.name), &other));
                }
                for qn in self.schema_collations(auth) {
                    columns.extend(self.generations_naming(Named::Collation(&qn), &other));
                }
                columns.extend(self.columns_lost_with(&self.schema_user_types(auth), &other));
                columns.sort();
                columns.dedup();
                columns
            }
            DropTarget::Table(_) | DropTarget::View(_) | DropTarget::Assertion(_) | DropTarget::Sequence(_) => Vec::new(),
        }
    }

    /// The generated columns REVOKE `r` by `auth`, its names in schema
    /// `schema`, loses: of USAGE on a domain or collation, those of the
    /// tables of each schema whose owner it leaves without USAGE that name
    /// it; of a privilege on a type or its routine, the columns it loses
    /// (`State::type_columns_revoked`).
    ///
    /// # Errors
    ///
    /// As the revoke's own refusals of the object and its privileges.
    pub(in crate::db) fn generations_revoked(&self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Vec<Column>> {
        if r.object == PrivilegeObject::Domain {
            self.domain_generations_revoked((auth, schema), r)
        } else if r.object == PrivilegeObject::Collation {
            self.collation_generations_revoked(auth, r)
        } else if matches!(r.object, PrivilegeObject::Type | PrivilegeObject::SpecificRoutine | PrivilegeObject::Routine) {
            self.type_columns_revoked((auth, schema), r)
        } else {
            Ok(Vec::new())
        }
    }
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.23 SR4] That `columns`, which a statement
    /// drops as ALTER TABLE DROP COLUMN CASCADE drops each, leave each of
    /// their tables a column.
    fn none_emptied(&self, columns: &[Column]) -> Result<()> {
        for (qn, _) in columns {
            let Some(t) = self.catalog.table(qn) else { continue };
            if t.columns.iter().all(|c| columns.iter().any(|(q, n)| q == qn && *n == c.name)) {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §12.7 GR12z, §11.23 SR4] the statement drops every column of {qn}, which keeps one");
                return Err(rule(msg));
            }
        }
        Ok(())
    }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §12.7 GR12z] In transaction `tx`, each of
    /// `columns` dropped, as ALTER TABLE DROP COLUMN CASCADE drops it, each
    /// logged apart, and then `then` made. Once one is dropped, the
    /// transaction only rolls back if a later drop or `then` fails, so a
    /// table each of whose columns is lost is refused before any is
    /// dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `columns` are every column of a table [§11.23 SR4];
    /// as each drop's, or `then`'s.
    pub(in crate::db) fn dropping_generations<T>(&self, tx: &Txn, columns: &[Column], then: impl FnOnce() -> Result<T>) -> Result<T> {
        if !columns.is_empty() {
            self.definitions(self.db.snapshot()?)?.none_emptied(columns)?;
        }
        let mut dropped = false;
        let made = (|| {
            for (qn, name) in columns {
                let fresh = self.definitions(self.db.snapshot()?)?;
                let temporary = fresh.base_table(qn)?.temporary;
                let action = AlterAction::DropColumn { column: name.clone(), at: 0, behavior: DropBehavior::Cascade };
                let text = format!("DROP COLUMN \"{}\" CASCADE", name.replace('"', "\"\""));
                self.alter_base_table(&fresh, tx, (qn, &action, &text), temporary)?;
                dropped = true;
            }
            then()
        })();
        if made.is_err() && dropped {
            tx.doom("generated columns were dropped before a statement that needed them gone failed");
        }
        made
    }
}
