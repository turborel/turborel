//! \[EXTENSION\] Deferred constraints [ISO/IEC 9075:1992 §10.6, §14.2,
//! §14.3]: SET CONSTRAINTS, which sets the constraint modes of the
//! session's transaction, or of its next if none is active, and checks the
//! constraints it makes immediate [§10.6 GR3]; and COMMIT, which first
//! checks every constraint still deferred, as if SET CONSTRAINTS ALL
//! IMMEDIATE ran [§14.3 GR4], and rolls back if one is not satisfied
//! [GR5a]. The checks wait in the session (`integrity::Deferral`).

use std::collections::HashMap;
use std::sync::Arc;

use super::integrity::{Check, Cursored, Effects, mode_name};
use super::{Call, Completion, Session, State, rule};
use crate::ast::TableName;
use crate::binder::qualify;
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::txn::Txn;

impl Session {
    /// What a statement of this session does, nothing yet: run under its
    /// authorization identifier, with its constraint modes.
    pub(super) fn effects(&self) -> Effects<'_> {
        Effects::new(&self.auth, &self.deferral, Cursored { rows: self.cursor_changes(), conflict: &self.conflict })
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
            self.check_deferred(st, &tx, chosen.as_deref())?;
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
    /// transaction `tx`: done with if they hold, else kept.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, for the first that does not hold.
    fn check_deferred(&self, st: &Arc<State>, tx: &Txn, named: Option<&[QualifiedName]>) -> Result<()> {
        let pending = std::mem::take(&mut self.deferral.borrow_mut().pending);
        let (now, later): (Vec<Check>, Vec<Check>) =
            pending.into_iter().partition(|c| named.is_none_or(|n| st.constraint_of(c).is_some_and(|d| n.contains(mode_name(d)))));
        let mut bound = HashMap::new();
        // [ISO/IEC 9075:1992 §4.28] serializable at every level.
        let checked = tx.check_reads(true).and_then(|()| now.iter().try_for_each(|check| st.check(tx, check, &mut bound)));
        let checked = checked.and(tx.check_reads(false));
        let mut deferral = self.deferral.borrow_mut();
        deferral.pending = later;
        if checked.is_err() {
            deferral.pending.extend(now);
        }
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
        self.check_deferred(call.state(), tx, None).map_err(|e| {
            SqlError::new(Code::CommitFailed, format!("[ISO/IEC 9075:1992 §14.3 GR5a] the transaction is rolled back: {}", e.described()))
                .with_state(SqlState::TRANSACTION_ROLLBACK_INTEGRITY)
        })
    }
}
