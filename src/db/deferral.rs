//! \[EXTENSION\] Deferred constraints [ISO/IEC 9075:1992 §10.6, §14.2,
//! §14.3]: SET CONSTRAINTS, which sets the constraint modes of the
//! session's transaction, or of its next if none is active, and checks the
//! constraints it makes immediate [§10.6 GR3]; and COMMIT, which first
//! checks every constraint still deferred, as if SET CONSTRAINTS ALL
//! IMMEDIATE ran [§14.3 GR4], and rolls back if one is not satisfied
//! [GR5a]. The checks wait in the session (`integrity::Deferral`).

use std::collections::HashSet;
use std::sync::Arc;

use super::integrity::{Check, Checking, Cursored, Effects, mode_name};
use super::{Call, Completion, Session, State, rule};
use crate::ast::TableName;
use crate::binder::qualify;
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::HeldRows;
use crate::txn::Txn;

/// Which waiting checks are made: those of the constraints named, or of
/// all; and which are dropped first.
type Chosen<'a> = (Option<&'a [QualifiedName]>, &'a dyn Fn(&Check) -> bool);

impl Session {
    /// What a statement of this session does, nothing yet: run under its
    /// authorization identifier, with its constraint modes, held in the
    /// work areas of `st` (RULES.md D38).
    pub(super) fn effects(&self, st: &State) -> Effects<'_> {
        let cursors = Cursored { rows: self.cursor_changes(), conflict: &self.conflict };
        let mut effects = Effects::new(&self.auth(), &self.deferral, cursors, st.list_space());
        effects.now = self.now.get();
        effects
    }

    /// [ISO/IEC 9075:1992 §14.2] SET CONSTRAINTS: the constraints `names`,
    /// or every deferrable one if none, made deferred or, if not
    /// `deferred`, immediate, and then checked [§10.6 GR3].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a name is not a constraint's [SR1], or of one not
    /// deferrable [SR2]; -201, SQLSTATE 23000, if a constraint made
    /// immediate is not satisfied, the modes left as they were.
    pub(super) fn set_constraints(&self, call: &Call<'_>, names: Option<&[TableName]>, deferred: bool) -> Result<Completion> {
        let st = call.state();
        let chosen = names.map(|list| list.iter().map(|n| self.deferrable(st, n)).collect::<Result<Vec<_>>>()).transpose()?;
        let tx = self.inner.borrow().tx.clone();
        if let (false, Some(tx)) = (deferred, tx) {
            call.runs(&tx);
            tx.begin_statement(|| !self.cursors_open())?;
            self.check_deferred(st, &tx, (chosen.as_deref(), &|_| false))?;
        }
        let mut deferral = self.deferral.borrow_mut();
        match chosen {
            // [GR2a, GR3a] ALL: every deferrable constraint.
            None => {
                deferral.all = Some(deferred);
                deferral.named.clear();
            }
            // [GR2b, GR3b]
            Some(each) => deferral.named.extend(each.into_iter().map(|n| (n, deferred))),
        }
        Ok(Completion::Success)
    }

    /// [§14.2 SR1, SR2] The constraint `name` names, which is deferrable.
    fn deferrable(&self, st: &State, name: &TableName) -> Result<QualifiedName> {
        let qn = qualify(name, &self.default_schema());
        // Of a table, a domain [§11.21 GR5], or an assertion.
        let tables = st.catalog.tables().into_iter().flat_map(|t| t.constraints.iter());
        let domains = st.catalog.domains().into_iter().flat_map(|d| d.constraints.iter());
        let found = tables.chain(domains).chain(st.catalog.assertions()).find(|c| c.name == qn).map(|c| c.attributes);
        let Some(attributes) = found else {
            return Err(rule(format!("[ISO/IEC 9075:1992 §14.2 SR1] no constraint is named {qn}")).or_at(name.at));
        };
        if !attributes.deferrable {
            return Err(rule(format!("[ISO/IEC 9075:1992 §14.2 SR2] constraint {qn} is not DEFERRABLE")).or_at(name.at));
        }
        Ok(qn)
    }

    /// The waiting checks of the constraints `named`, or of all, made, in
    /// transaction `tx`: done with if they hold, else kept; those `gone`
    /// says dropped first. Each check over all a constraint reads waits
    /// once (`Deferral::wholes`), so it is made once [ISO/IEC 9075:1992
    /// §4.10.1]. The checks are read back from the list they wait in, which
    /// is kept until they are done with, so none is lost if one cannot be
    /// read.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, for the first that does not hold; the
    /// spill area's.
    fn check_deferred(&self, st: &Arc<State>, tx: &Txn, (named, gone): Chosen<'_>) -> Result<()> {
        let (mut now, mut later) = (HeldRows::new(st.list_space()), HeldRows::new(st.list_space()));
        let (mut checked_now, mut waiting) = (HashSet::new(), HashSet::new());
        self.deferral.borrow_mut().pending.visit(&mut |c| {
            if gone(c) {
                Ok(())
            } else if named.is_none_or(|n| st.constraint_of(c).is_some_and(|d| n.contains(mode_name(d)))) {
                checked_now.extend(c.whole());
                now.push(c.clone())
            } else {
                waiting.extend(c.whole());
                later.push(c.clone())
            }
        })?;
        let mut checking = Checking::default();
        // [ISO/IEC 9075:1992 §4.28] serializable at every level.
        let checked =
            tx.check_reads(true).and_then(|()| now.visit(&mut |check| st.check(tx, check, &mut checking))).and_then(|()| checking.finish());
        let checked = checked.and(tx.check_reads(false));
        if checked.is_err() {
            later.append(now);
            waiting.extend(checked_now);
        }
        let mut deferral = self.deferral.borrow_mut();
        (deferral.pending, deferral.wholes) = (later, waiting);
        checked
    }

    /// [ISO/IEC 9075:1992 §14.3 GR4, GR5a] Before COMMIT ends transaction
    /// `tx`: every constraint checked as if SET CONSTRAINTS ALL IMMEDIATE
    /// ran.
    ///
    /// # Errors
    ///
    /// SQLCODE -913, SQLSTATE 40002 (transaction rollback: integrity
    /// constraint violation), if one is not satisfied: the caller rolls
    /// back.
    pub(super) fn check_at_commit(&self, call: &Call<'_>, tx: &Arc<Txn>) -> Result<()> {
        call.runs(tx);
        let st = call.state();
        // [§14.3 GR3] The rows of a temporary table of ON COMMIT DELETE ROWS
        // are deleted first, so what they would break holds; and no table
        // that keeps its rows refers to one [§11.8 SR5, §11.9 SR4].
        let deleted = |c: &Check| c.table().and_then(|t| st.catalog.table(t)).is_some_and(|d| d.temporary.is_some_and(|t| t.delete_rows));
        self.check_deferred(st, tx, (None, &deleted)).map_err(|e| {
            SqlError::new(Code::CommitFailed, format!("[ISO/IEC 9075:1992 §14.3 GR5a] the transaction is rolled back: {}", e.described()))
                .with_state(SqlState::TRANSACTION_ROLLBACK_INTEGRITY)
        })
    }
}
