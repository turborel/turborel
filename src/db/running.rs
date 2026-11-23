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
        let published = self.db.refuse_earlier(self.service).and_then(|()| self.own_snapshot()).and_then(retired)?;
        Ok(self.with_injected(self.with_own(self.with_temporaries(self.definitions(published)?))))
    }

    /// The published snapshot, as its own copy holds it (RULES.md D780).
    ///
    /// # Errors
    ///
    /// As `Database::snapshot`.
    pub(super) fn own_snapshot(&self) -> Result<Arc<State>> {
        self.db.snapshot_for(&mut self.published.borrow_mut())
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
        self.begin_in(st, &tx, false)?;
        Ok(tx)
    }

    /// Begins the statement next of `tx`, of `st` (`Txn::begin_statement`):
    /// a load under way in it ended as it begins, unless the statement
    /// `continues` it (RULES.md D532).
    ///
    /// # Errors
    ///
    /// As `Txn::begin_statement` and `State::end_load`.
    pub(super) fn begin_in(&self, st: &State, tx: &Txn, continues: bool) -> Result<()> {
        if tx.begin_statement(|| !self.cursors_open())? && !continues {
            st.end_load(tx)?;
        }
        Ok(())
    }

    /// \[DESIGN\] `result`, told once every commit what the statement saw
    /// past its snapshot rests on is durable (RULES.md D441): of `tx`, its
    /// transaction, and of the one active now, should an SQL control
    /// statement have begun another.
    ///
    /// # Errors
    ///
    /// `result`'s; SQLCODE -922 in its place if a failed sync of the log
    /// left such a commit in doubt.
    pub(super) fn replied<T>(&self, tx: &Txn, result: Result<T>) -> Result<T> {
        let result = tx.replied(result);
        match self.current_transaction() {
            Some(now) => now.replied(result),
            None => result,
        }
    }

    /// \[DESIGN\] RULES.md D846: `result`, a statement's completion in
    /// `tx`, told at once if its caller tells it only once the transaction
    /// has ended (`tell_at_end`): the end's own reply then waits for what
    /// it rests on, after the transaction's locks are released. A failure
    /// is told as `replied` tells it.
    ///
    /// # Errors
    ///
    /// As `replied`.
    pub(super) fn completed<T>(&self, tx: &Txn, result: Result<T>) -> Result<T> {
        if self.told_at_end.get() && result.is_ok() {
            return result;
        }
        self.replied(tx, result)
    }

    /// \[DESIGN\] RULES.md D846: whether its caller tells the completion
    /// of each statement it runs from now on, but of one that opens a
    /// cursor, only once the transaction has ended, as a server's request
    /// that ends its transaction with its statement does. Then such a
    /// statement returns with no wait for the commits it read past its
    /// snapshot to be durable, while it holds its row locks. The commit
    /// that ends its transaction returns once its own commit is durable,
    /// and so every commit before it; a rollback, or a commit refused,
    /// once those it rests on are. A caller that tells a completion
    /// before the transaction ends calls `settle_told` first.
    pub fn tell_at_end(&self, at_end: bool) {
        self.told_at_end.set(at_end);
    }

    /// \[DESIGN\] RULES.md D846: returns once every commit what its open
    /// transaction did rests on is durable, as a statement's completion
    /// would have waited had `tell_at_end` not been set.
    ///
    /// # Errors
    ///
    /// SQLCODE -922 if a failed sync of the log left such a commit in
    /// doubt; SQLCODE -910 if called back from a plug-in.
    pub fn settle_told(&self) -> Result<()> {
        let _call = self.enter()?;
        match self.current_transaction() {
            Some(tx) => tx.replied(Ok(())),
            None => Ok(()),
        }
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
        if let Some(mut timed) = self.timed.take() {
            timed.rows(u64::try_from(self.session.rows_affected()).unwrap_or(u64::MAX));
            timed.done(&self.session.db, &self.session.stop);
        }
    }
}
