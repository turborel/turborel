//! \[EXTENSION\] Declared local temporary tables [ISO/IEC 9075:1992 §13.11]:
//! a session's own, named MODULE.T, in no schema of the catalog.
//!
//! A session is a module, so the definitions its statements see hold its
//! declared tables in a schema MODULE of their own, owned by the session's
//! authorization identifier, which holds every privilege on them [GR3];
//! each has an instance, as every temporary table does
//! (`db/temporary.rs`). No other session sees them, the log keeps nothing
//! of them, and they go with the session [GR4]. They are in no view of the
//! Information Schema [GR5] (`State::without_declared`).

use std::slice;
use std::sync::{Arc, Weak};

use super::constraints::Within;
use super::schema::constraint_index_name;
use super::{Completion, Session, State, rule};
use crate::ast::{PrivilegeDefinition, RevokeStatement, SchemaStatement, TableDefinition};
use crate::catalog::{Catalog, IndexDesc, QualifiedName, TableDesc, TemporaryKind};
use crate::error::{At, Result};
use crate::lexer::kw;
use crate::sync::lock_builtin;

impl Session {
    /// [ISO/IEC 9075:1992 §13.11] DECLARE LOCAL TEMPORARY TABLE `t`, on the
    /// definitions `st`: declared for the rest of the session.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the session declared a table of its name already
    /// [SR1], or for a rule its definition breaks [§11.3–§11.9]; -104 for a
    /// privilege it lacks.
    pub(super) fn declare_table(&self, st: &Arc<State>, t: &TableDefinition) -> Result<Completion> {
        let qn = QualifiedName { auth: kw::MODULE.to_owned(), ident: t.name.ident.clone() };
        if st.catalog.table(&qn).is_some() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §13.11 SR1] {qn} is declared already")));
        }
        let mut next = State::clone(st);
        // [§5.4 SR6, SR8b] Its names without a schema's are of the session's
        // default schema; MODULE.T names a declared table.
        let names = self.default_schema();
        let (mut desc, pending) = next.describe_table((kw::MODULE, &names, self.now.get().zone, Within::Declared), t, &[], &[])?;
        let mut staged = next.catalog.duplicate();
        in_module(&mut *staged, &self.auth(), vec![desc.clone()]);
        next.catalog = Arc::from(staged);
        next.complete_tables(&names, slice::from_mut(&mut desc), slice::from_ref(&pending))?;
        let indexes = indexes_of(&desc, st.constraint_method());
        // [ISO/IEC 9075:1992 §11.5 GR1b v] a default placed padded.
        let padding = super::padding::of_table(&desc, t);
        let mut declared = lock_builtin(&self.temporaries.declared);
        declared.tables.push((desc, indexes));
        declared.made = None;
        Ok(Completion::Success.warned(padding))
    }

    /// The rules of a declared table `statement` would break, checked on
    /// `st`, the definitions with the session's declared tables, before it
    /// is made on the published ones, which have none: no privilege is
    /// granted or revoked on one [ISO/IEC 9075:1992 §10.3 SR1], no view
    /// names one [§11.19 SR5], and a table refers to one only as SQL-92
    /// lets it [§11.8 SR4, §11.9 SR3]. `object` is what it is of, its names
    /// of schema `schema`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule it breaks, or another of the view's or the
    /// table's.
    pub(super) fn refuse_declared(&self, st: &State, statement: &SchemaStatement, (object, schema): (&QualifiedName, &str)) -> Result<()> {
        if lock_builtin(&self.temporaries.declared).tables.is_empty() {
            return Ok(());
        }
        match statement {
            SchemaStatement::Grant(PrivilegeDefinition { table, .. }) | SchemaStatement::Revoke(RevokeStatement { table, .. }) => {
                st.object(object).map(|_| ()).or_at(table.at)
            }
            SchemaStatement::View(v) => st.describe_view((&object.auth, schema), v).map(|_| ()).or_at(v.name.at),
            // [§11.8 SR4, §11.9 SR3] which tables its constraints refer to.
            SchemaStatement::Table(t) => State::clone(st).table_described((schema, self.now.get().zone), t, None).map(|_| ()),
            SchemaStatement::Domain(_) | SchemaStatement::Assertion(_) | SchemaStatement::AlterDomain(_) => Ok(()),
        }
    }

    /// `st`, with the session's declared local temporary tables in its
    /// schema MODULE: the catalog made with them the call before, if made
    /// of the same catalog for the same authorization identifier, so a
    /// plan kept by the catalog's identity holds (RULES.md D45).
    pub(super) fn with_declared(&self, st: Arc<State>) -> Arc<State> {
        let mut declared = lock_builtin(&self.temporaries.declared);
        if declared.tables.is_empty() {
            return st;
        }
        let auth = self.auth().clone();
        let under = Arc::downgrade(&st.catalog);
        let made = declared.made.as_ref().filter(|m| Weak::ptr_eq(&m.under, &under) && m.auth == auth).map(|m| Arc::clone(&m.catalog));
        let catalog = made.unwrap_or_else(|| {
            let mut catalog = st.catalog.duplicate();
            in_module(&mut *catalog, &auth, declared.tables.iter().map(|(desc, _)| desc.clone()).collect());
            for (_, indexes) in &declared.tables {
                indexes.iter().cloned().for_each(|ix| catalog.add_index(ix));
            }
            let catalog: Arc<dyn Catalog> = Arc::from(catalog);
            declared.made = Some(Made { under, auth, catalog: Arc::clone(&catalog) });
            catalog
        });
        let mut copy = State::clone(&st);
        copy.catalog = catalog;
        Arc::new(copy)
    }
}

/// \[DESIGN\] A session's declared local temporary tables, each with its
/// indexes, and the catalog last made with them.
#[derive(Default)]
pub(super) struct Declared {
    tables: Vec<(TableDesc, Vec<IndexDesc>)>,
    made: Option<Made>,
}

/// A catalog made with a session's declared tables (`with_declared`): of
/// the catalog `under` (whose allocation the `Weak` keeps, so no other
/// catalog takes its address), its schema MODULE owned by `auth`.
struct Made {
    under: Weak<dyn Catalog>,
    auth: String,
    catalog: Arc<dyn Catalog>,
}

/// The schema MODULE of `catalog`, owned by `auth`, with `tables` in it.
fn in_module(catalog: &mut dyn Catalog, auth: &str, tables: Vec<TableDesc>) {
    if catalog.schema_exists(kw::MODULE) {
        for t in tables {
            catalog.add_table(t);
        }
    } else {
        catalog.create_schema(kw::MODULE, auth, tables);
    }
}

/// The indexes of the UNIQUE constraints of `desc`, of `method`, named as a
/// persistent table's are (`constraint_index_name`).
fn indexes_of(desc: &TableDesc, method: &str) -> Vec<IndexDesc> {
    (1..)
        .zip(desc.unique())
        .map(|(k, columns)| IndexDesc {
            name: constraint_index_name(&desc.name, k),
            table: desc.name.clone(),
            columns: columns.to_vec(),
            method: method.to_owned(),
            unique: true,
            options: Vec::new(),
        })
        .collect()
}

impl State {
    /// These definitions without the session's declared local temporary
    /// tables, as the Information Schema shows them [ISO/IEC 9075:1992
    /// §13.11 GR5]; none if they have none.
    pub(crate) fn without_declared(&self) -> Option<State> {
        if !self.catalog.tables().iter().any(|t| t.temporary.is_some_and(|t| t.kind == TemporaryKind::Declared)) {
            return None;
        }
        let mut catalog = self.catalog.duplicate();
        for desc in self.catalog.tables().into_iter().filter(|t| t.name.auth == kw::MODULE) {
            self.catalog.indexes(&desc.name).iter().for_each(|ix| catalog.remove_index(&ix.name));
            catalog.revoke_all(&desc.name);
            catalog.remove_table(&desc.name);
        }
        catalog.remove_schema(kw::MODULE);
        let mut shown = State::clone(self);
        shown.catalog = Arc::from(catalog);
        Some(shown)
    }
}
