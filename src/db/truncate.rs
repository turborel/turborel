//! [ISO/IEC 9075-2:2023 §14.10] TRUNCATE TABLE: every row of a base table
//! deleted, and, with RESTART IDENTITY, its identity column's sequence
//! generator restarted at its start value.
//!
//! \[DESIGN\] A persistent table's rows go at once: the table is given a new
//! generation of its storage, empty, as ALTER TABLE gives it one, staged,
//! logged and published with its transaction (`db/alter/truncate.rs`,
//! `db/ddl.rs`). No row is deleted one by one, logged or taken from an
//! index. Its exclusive lock is held to the end of the transaction, so no
//! other writes it meanwhile, and a statement or transaction that began
//! before the truncation committed is refused at its next use of the table
//! (SQLCODE -911), as after ALTER TABLE: the changes it disallows that
//! another transaction would make meanwhile are implementation-defined
//! [GR4, IA234]. The read that tells whether the table had a row is checked
//! at commit at every isolation level, so a row another transaction
//! committed after this one's snapshot refuses the commit rather than going
//! unseen.
//!
//! A temporary table's rows are each session's (RULES.md D49): the
//! session's own are deleted, as a searched DELETE of no condition deletes
//! them, and with RESTART IDENTITY the generator is restarted, as the ALTER
//! TABLE that GR8 effectively executes restarts it.
//!
//! A cursor of the transaction that reads the table refuses the statement
//! [GR2, GR3], so none sees the rows go, nor is any row it changed deleted
//! [GR6]. No row deleted is no data [GR7]. Each CHECK with a subquery, and
//! each assertion, that reads the table is checked whole
//! (`db/integrity/rewritten.rs`).

use std::sync::Arc;

use super::write::delete_searched;
use super::{Completion, Session, State, StatementContext};
use crate::ast::{AlterAction, TableName};
use crate::binder::{Context, WriteKind, bind_write, qualify};
use crate::catalog::{QualifiedName, TableDesc, TemporaryKind};
use crate::error::{At, Code, Result, SqlError, SqlState, fail};
use crate::params::Params;
use crate::parser::TRUNCATE;
use crate::txn::Txn;

/// A Syntax Rule of §14.10 broken (SQLCODE -103).
fn rule(message: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, message)
}

impl State {
    /// [ISO/IEC 9075-2:2023 §14.10 SR2–SR4] The descriptor of table `qn`,
    /// which TRUNCATE TABLE names.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no schema includes it, it is a view, or a foreign
    /// key references it, its own among them.
    pub(super) fn truncated(&self, qn: &QualifiedName) -> Result<TableDesc> {
        let Some(desc) = self.catalog.table(qn) else {
            if self.catalog.view(qn).is_some() {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §14.10 SR3] {qn} is a view, not a base table")));
            }
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §14.10 SR2] there is no table {qn}")));
        };
        if desc.temporary.is_some_and(|t| t.kind == TemporaryKind::Declared) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §14.10 SR2] {qn} is a declared local temporary table, of no schema: DELETE FROM {qn} deletes its rows"
            )));
        }
        for t in self.catalog.tables() {
            if let Some((name, _)) = t.foreign_keys().find(|(_, fk)| fk.table == *qn) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §14.10 SR4] foreign key {name} of {} references {qn}, so its rows are not truncated",
                    t.name
                )));
            }
        }
        Ok(desc.clone())
    }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §14.10] TRUNCATE TABLE `table`, with RESTART
    /// IDENTITY if `restart`, in `cx`, `params` those of the statement.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if the session does not own the table's schema [AR2];
    /// -103 for a Syntax Rule it breaks; -501, SQLSTATE 36002, if a cursor
    /// of the transaction reads the table [GR2, GR3]; -201 for a constraint
    /// that reads the table broken; -911 if another transaction holds it.
    pub(super) fn truncate_table(
        &self,
        cx: StatementContext<'_>,
        (table, restart): (&TableName, bool),
        params: &Params,
    ) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        let qn = qualify(table, &self.default_schema());
        // [AR1, AR2] only the owner of its schema truncates a table.
        if !self.owns_schema(st, &qn.auth) {
            return fail(Code::Privilege, format!("[ISO/IEC 9075-2:2023 §14.10 AR2] {qn} is in a schema {} does not own", self.auth()));
        }
        let desc = st.truncated(&qn).or_at(table.at)?;
        if self.cursor_reads(&qn) {
            let msg = format!("[ISO/IEC 9075-2:2023 §14.10 GR2, GR3] a cursor of this transaction reads {qn}; close it first");
            return Err(SqlError::new(Code::Cursor, msg).with_state(SqlState::CURSOR_SENSITIVITY_FAILED));
        }
        let action = AlterAction::Truncate { restart };
        let text = if restart { format!("{TRUNCATE} RESTART IDENTITY") } else { TRUNCATE.to_owned() };
        if desc.temporary.is_none() {
            self.prepare_change(st, tx, &qn)?;
            let had = had_rows(st, tx, &qn)?;
            self.alter_base_table(st, tx, (&qn, &action, &text), None)?;
            return Ok(if had { Completion::Success } else { Completion::NoData });
        }
        let w = self.write_bound(st, params, None, |b: Context<'_>| bind_write(b, WriteKind::DeleteSearched, table, &[], None))?;
        let deleted = delete_searched(cx, w, self.effects(st))?;
        // [GR8] of a table with an identity column alone.
        if restart && desc.columns.iter().any(|c| c.identity().is_some()) {
            self.alter_base_table(st, tx, (&qn, &action, &text), desc.temporary)?;
        }
        Ok(if deleted == 0 { Completion::NoData } else { Completion::Success })
    }
}

/// Whether transaction `tx` sees a row of persistent table `table` in
/// `st`, read as the whole table, and checked at commit at every isolation
/// level [ISO/IEC 9075:1992 §4.28]: a row another transaction commits
/// meanwhile, which the truncation would take unseen, refuses the commit.
///
/// # Errors
///
/// The read's.
fn had_rows(st: &Arc<State>, tx: &Txn, table: &QualifiedName) -> Result<bool> {
    tx.check_reads(true)?;
    let read = || -> Result<bool> {
        st.read_lock(tx, table)?;
        let first = Arc::clone(st.storage_of(table)?).scan(tx.id, &[0]).next();
        Ok(first.transpose()?.is_some())
    };
    let had = read();
    let unchecked = tx.check_reads(false);
    let had = had?;
    unchecked.map(|()| had)
}
