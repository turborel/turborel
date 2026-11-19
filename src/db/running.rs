//! The transaction a call of the session runs, and its end within an SQL
//! control statement NOT ATOMIC run directly [ISO/IEC 9075-4:2006 draft
//! §4.10.4; ISO/IEC 9075-2:2023 §17.7, §17.8].
//!
//! A call records the transaction it runs on this thread, so that another
//! transaction never waits on this thread for its locks (`txn/locks.rs`).
//! A COMMIT or ROLLBACK of a control statement ends that transaction
//! within the call, and the statements after it run in a new one, begun as
//! the first of them needs one (`routines/program/host.rs`): so the
//! session, not the call, records the transaction running.

use std::sync::Arc;

use super::{Call, Session, State};
use crate::error::Result;
use crate::txn::Txn;

impl Session {
    /// Records that its running call runs transaction `tx` on this thread,
    /// in place of any other.
    pub(super) fn runs(&self, tx: &Arc<Txn>) {
        tx.runs_on(Some(std::thread::current().id()));
        if let Some(before) = self.running.replace(Some(Arc::clone(tx)))
            && !Arc::ptr_eq(&before, tx)
        {
            before.runs_on(None);
        }
    }

    /// Records that its call runs no transaction any more.
    pub(super) fn ran(&self) {
        if let Some(tx) = self.running.take() {
            tx.runs_on(None);
        }
    }

    /// The state a call of it runs against now: the published state, with
    /// the definitions its transaction changed, its temporary tables and
    /// its own settings.
    ///
    /// # Errors
    ///
    /// SQLCODE -917 if the database is retired; a definition its
    /// transaction changed no longer applies.
    pub(super) fn current_state(&self) -> Result<Arc<State>> {
        let retired = |st: Arc<State>| match st.tx.held() {
            Some(crate::txn::Held::Retired) => Err(crate::txn::Held::Retired.refusal()),
            Some(crate::txn::Held::Following | crate::txn::Held::Deposed) | None => Ok(st),
        };
        let published = self.db.refuse_earlier(self.service).and_then(|()| self.db.snapshot()).and_then(retired)?;
        Ok(self.with_own(self.with_temporaries(self.definitions(published)?)))
    }

    /// Its current transaction, if one is active.
    pub(super) fn current_transaction(&self) -> Option<Arc<Txn>> {
        self.inner.borrow().tx.clone()
    }

    /// A transaction begun for the statement next of its running call,
    /// against `st`, if none is active [X3.135-1986 §7.3 GR2a].
    ///
    /// # Errors
    ///
    /// A definition the statement reads changed since the transaction
    /// began.
    pub(super) fn begun(&self, st: &State) -> Result<Arc<Txn>> {
        let tx = self.ensure_transaction(st);
        self.runs(&tx);
        tx.begin_statement(|| !self.cursors_open())?;
        Ok(tx)
    }

    /// [ISO/IEC 9075-2:2023 §17.7, §17.8] COMMIT if `commit`, else
    /// ROLLBACK, run by an SQL control statement run directly: its
    /// transaction ended within it, every cursor of it not holdable closed
    /// [§17.7 GR3], and none running until the next statement begins one.
    ///
    /// # Errors
    ///
    /// As `commit`'s or `rollback`'s.
    pub(super) fn end_within(&self, st: &Arc<State>, commit: bool) -> Result<()> {
        let ended = if commit { self.commit_in(st) } else { self.rollback_in(st) };
        self.ran();
        self.db.noted(ended)
    }
}

/// As a call ends, the session is no longer busy, and its statement is timed.
impl Drop for Call<'_> {
    fn drop(&mut self) {
        self.session.ran();
        self.session.busy.set(false);
        if let Some(timed) = self.timed.take() {
            timed.done(&self.session.db);
        }
    }
}
