//! \[DESIGN\] The standby seam (RULES.md D57): what replication asks of a
//! database whose log a group of replicas keeps. Replication orders and
//! moves records it never decodes; the database applies them, by its one
//! replay (`db/recovery.rs`), and is told when to serve as primary and
//! when to stop. The calls come from replication's driver, never back from
//! a plug-in (RULES.md R-ERR-4).
//!
//! As a replica stands, so does its database:
//! * **Serving**, as every database does unless told otherwise: it takes
//!   changes, which it writes to its log.
//! * **Following** a log another replica writes, from the first records it
//!   replays until it is promoted: it takes no change (SQLCODE -917).
//! * **Deposed**, once it stops serving as primary: it takes no change
//!   until it is opened again, which recovers it from its last checkpoint
//!   and its log.
//!
//! The changes it does not take are those it would write to its log, as a
//! failed sync stops them (RULES.md D25): a row change, the commit of a
//! transaction that wrote, a definition, a checkpoint; reads and rollbacks
//! go on.
//!
//! The export and import of a database, by which state transfer brings up
//! a replica the log alone no longer can, are not yet part of the seam.

use std::sync::Arc;

use super::recovery::Definitions;
use super::{Database, State, api};
use crate::error::Result;
use crate::log::{Log, Lsn};
use crate::txn::Held;

/// \[DESIGN\] A database as a replica of a group sees it (RULES.md D57):
/// see the module's documentation.
pub trait Standby: Send + Sync {
    /// Applies `records`, each a position and the bytes a log holds there,
    /// in position order, by the database's one replay; from then on it
    /// follows, taking no change until it is promoted. The records follow
    /// those it holds: the log from its start, or from where its last
    /// checkpoint or replay left it. Each call replays as recovery does: a
    /// transaction whose records end without its commit or rollback is
    /// undone, so a call is given whole transactions. The position of the
    /// last record applied, 0 if none was.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was deposed; the replay's (SQLCODE -916 for a
    /// malformed record).
    fn replay(&self, records: &mut dyn Iterator<Item = Result<(Lsn, Vec<u8>)>>) -> Result<Lsn>;
    /// Serves as primary, writing to `log` from the next transaction on:
    /// it takes changes again.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was deposed.
    fn promote(&self, log: Arc<dyn Log>) -> Result<()>;
    /// Stops serving: it takes no change until it is opened again. Its
    /// commits in doubt stay so (RULES.md D25).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn depose(&self) -> Result<()>;
}

impl Standby for Database {
    fn replay(&self, records: &mut dyn Iterator<Item = Result<(Lsn, Vec<u8>)>>) -> Result<Lsn> {
        let tx = Arc::clone(&self.snapshot()?.tx);
        refuse_deposed(tx.held(), "replays a log")?;
        tx.hold(Some(Held::Following));
        let mut last = Lsn(0);
        let (_, mut touched) = {
            let mut noted = records.inspect(|read| {
                if let Ok((at, _)) = read {
                    last = *at;
                }
            });
            Database::replay(self, &mut noted, Definitions::All)?
        };
        // Index entries are not logged: those of the rows replayed are made.
        self.change(|st: &mut State| st.mend_indexes(&mut touched, |_| true))?;
        self.apply_settings()?;
        Ok(last)
    }

    fn promote(&self, log: Arc<dyn Log>) -> Result<()> {
        let tx = Arc::clone(&self.snapshot()?.tx);
        refuse_deposed(tx.held(), "is promoted")?;
        self.set_log(log)?;
        tx.hold(None);
        Ok(())
    }

    fn depose(&self) -> Result<()> {
        self.snapshot()?.tx.hold(Some(Held::Deposed));
        Ok(())
    }
}

/// Refuses what a deposed database is asked to do, `what`: it is opened
/// again first.
fn refuse_deposed(held: Option<Held>, what: &str) -> Result<()> {
    match held {
        Some(Held::Deposed) => Err(api(format!("[DESIGN] RULES.md D57: a deposed database is opened again before it {what}"))),
        Some(Held::Following) | None => Ok(()),
    }
}
