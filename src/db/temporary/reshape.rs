//! [ISO/IEC 9075:1992 §11.10, §11.11 GR2, §11.15 GR4] ALTER TABLE of a
//! temporary table while sessions' instances of it hold rows (RULES.md
//! D49). Split from db/temporary.rs (RULES.md R-SIZE-2).
//!
//! A column added or dropped gives each instance that holds rows a
//! successor of the new generation, its rows copied there by the altering
//! transaction, each with the column added, of its default [§11.11 GR2],
//! or removed [§11.15 GR4], as `db/ddl.rs` copies a persistent table's;
//! the table's constraints are checked of each session's rows, at once, as
//! no other transaction writes them meanwhile (`Uses::alter`). Each
//! session takes its successor once the transaction commits
//! (`Uses::published`).

use std::cell::RefCell;
use std::sync::Arc;

use super::Instance;
use crate::ast::AlterAction;
use crate::catalog::{ConstraintKind, IndexDesc, QualifiedName, TableDesc};
use crate::db::alter::Reshape;
use crate::db::ddl::copy_rows;
use crate::db::integrity::{Cursored, Deferral, Effects};
use crate::db::{Session, State, api};
use crate::error::{Code, Result, SqlError};
use crate::storage::TableStorage;
use crate::txn::Txn;

/// Whether a constraint of `desc` reads a table other than it: a foreign
/// key to another, or a CHECK whose subqueries read one.
fn reads_another(desc: &TableDesc) -> bool {
    desc.constraints.iter().any(|c| match &c.kind {
        ConstraintKind::ForeignKey(fk) => fk.table != desc.name,
        ConstraintKind::Check { reads, .. } => reads.iter().any(|t| *t != desc.name),
        ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } => false,
    })
}

impl Session {
    /// [ISO/IEC 9075:1992 §11.10] Transaction `tx`'s ALTER TABLE `action`
    /// of temporary table `name`, its rows to change as `reshape` says,
    /// from `st` to `next`: each instance's rows, of every session, copied
    /// to a successor.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if another session's instance holds rows and a
    /// constraint of the table reads another table, which only that session
    /// sees its own instance of; -201 if a row breaks a constraint; the
    /// copy's.
    pub(in crate::db) fn reshape_instances(
        &self,
        (st, next): (&Arc<State>, &Arc<State>),
        tx: &Txn,
        (name, action, reshape): (&QualifiedName, &AlterAction, Reshape),
    ) -> Result<()> {
        let was = st.catalog.table(name).ok_or_else(|| api(format!("[DESIGN] no table {name}")))?;
        let desc = next.catalog.table(name).cloned().ok_or_else(|| api(format!("[DESIGN] no table {name}")))?;
        let indexes: Vec<IndexDesc> = next.catalog.indexes(name).into_iter().cloned().collect();
        let own = Arc::downgrade(&self.temporaries.made);
        let delete_rows = desc.temporary.is_some_and(|t| t.delete_rows);
        let mut made = Vec::new();
        for (session, source) in st.temporaries.instances_of(name, was.generation) {
            let mine = session.ptr_eq(&own);
            // Another session's rows of ON COMMIT DELETE ROWS are gone as
            // its transaction ended, which none has open.
            if source.rows.made().is_none() || (delete_rows && !mine) {
                continue;
            }
            if !mine && reads_another(&desc) && source.rows.row_count() > 0 {
                return Err(SqlError::new(
                    Code::Conflict,
                    format!(
                        "[DESIGN] another session's instance of temporary table {name} has rows, and a constraint of it reads another table, whose rows that session alone sees (RULES.md D49)"
                    ),
                ));
            }
            let successor = Instance::of(&desc, (&st.memory, &st.temporaries));
            successor.rows.add_empty(&indexes);
            self.copy_instance((&source.on(st, name, false), &successor.on(next, name, true)), tx, (name, action, reshape), mine)?;
            made.push((session, successor, mine));
        }
        // Each made once every copy is: a refusal leaves the successors an
        // earlier ALTER TABLE of the transaction made.
        for (session, successor, mine) in made {
            st.temporaries.succeed(name, &session, successor.clone());
            if mine {
                crate::sync::lock_builtin(&self.temporaries.made).insert((name.clone(), desc.generation), successor);
            }
        }
        Ok(())
    }

    /// Copies the rows of `from` to `to`, as `copy_rows` does, in
    /// transaction `tx`: the session's own, `mine`, checked as its
    /// statements are; another's, at once, none deferred to this session's
    /// commit, which reads its own instances.
    fn copy_instance(
        &self,
        (from, to): (&Arc<State>, &Arc<State>),
        tx: &Txn,
        (name, action, reshape): (&QualifiedName, &AlterAction, Reshape),
        mine: bool,
    ) -> Result<()> {
        if mine {
            return copy_rows(from, to, tx, name, (action, reshape, self.effects(to)));
        }
        let deferral = RefCell::new(Deferral { all: Some(false), ..Deferral::default() });
        let conflict = RefCell::new(None);
        let cursors = Cursored { rows: Vec::new(), conflict: &conflict };
        let mut effects = Effects::new(&self.auth(), &deferral, cursors, to.list_space());
        effects.now = self.now.get();
        copy_rows(from, to, tx, name, (action, reshape, effects))
    }
}
