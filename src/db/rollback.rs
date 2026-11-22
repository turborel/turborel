//! Rolling back a session's transaction, and ending the session's last
//! one [X3.135-1986 §7.1 GR2, §8.9].

use super::area::commands;
use super::{Session, State};
use crate::error::{Result, SqlState};
use crate::txn::SessionEnd;

impl Session {
    /// [X3.135-1986 §8.9] ROLLBACK WORK.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if a change could not be undone; the transaction still ends,
    /// and the tables concerned are damaged (RULES.md D19). SQLCODE -910 if
    /// called back from a plug-in.
    pub fn rollback(&self) -> Result<()> {
        let call = self.enter()?;
        self.emptied((commands::ROLLBACK_WORK, commands::UNRECOGNIZED));
        self.diagnosed(self.db.noted(self.rollback_in(call.state())), |()| SqlState::SUCCESSFUL_COMPLETION)
    }

    /// [X3.135-1986 §7.1 GR2, ISO/IEC 9075:1992 §20.1 GR2a] Ends the
    /// session's transaction after its last statement: committed or rolled
    /// back as SESSION.COMMIT_AT_END says, and rolled back if a constraint is not
    /// satisfied; its failure returned, where dropping the session would
    /// lose it. A server calls it when its client terminates the session,
    /// which then has no transaction left to end when it is dropped.
    ///
    /// # Errors
    ///
    /// As `commit`'s, or `rollback`'s, as SESSION.COMMIT_AT_END says.
    pub fn end(&self) -> Result<()> {
        let ending = self.session_end(self.enter()?.state());
        match ending {
            SessionEnd::Commit => self.commit(),
            SessionEnd::Rollback => self.rollback(),
        }
    }

    pub(super) fn rollback_in(&self, st: &State) -> Result<()> {
        // The changes of definitions are forgotten; the published ones have
        // the storage of every table its changes are undone in.
        let ending = self.end_transaction();
        let id = ending.as_ref().map(|tx| tx.id);
        let ended = match (ending, self.take_staged()) {
            (Some(tx), None) => st.rollback(&tx),
            (Some(tx), Some(staged)) => {
                staged.discard();
                self.db.snapshot().and_then(|published| published.rollback(&tx))
            }
            (None, _) => return Ok(()),
        };
        // [X3.135-1986 §8.9] Ended, every change undone or not (RULES.md D19).
        self.transaction_ended(false);
        self.locators_ended(false);
        self.temporaries_ended(st, id);
        self.db.checkpoint_if_due();
        self.db.maintain_if_due();
        ended
    }
}
