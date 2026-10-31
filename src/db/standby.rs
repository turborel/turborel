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
//! **Continuous replay.** A group commits a log record by record, so the
//! records a following database is given end anywhere: within a
//! transaction, whose commit comes in a later call. Following, it holds
//! the records of each transaction not yet ended, and replays them with
//! the record that ends it; the others at once, as recovery replays them
//! wherever they stand: a definition made outside the transactions of
//! sessions, a setting, a user, an index, a checkpoint's. So each call
//! replays what recovery of the whole log so far would make of it, with
//! the transactions left open undone.
//!
//! **Ending open transactions.** A view change keeps records that had not
//! committed ([VRR §4.2]: "Operations that had not committed might also
//! survive"), and so the first records of a transaction whose commit
//! record its primary, deposed, never appends. Promoted, the database
//! ends each transaction it holds open, an abort record of it appended and
//! synced to its new log before it serves, so each replica following that
//! log ends it too.
//!
//! The export and import of a database, by which state transfer brings up
//! a replica the log alone no longer can, are not yet part of the seam.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::recovery::Definitions;
use super::{Database, State, api};
use crate::error::Result;
use crate::log::record::{Change, Record};
use crate::log::{Log, Lsn};
use crate::sync::lock_builtin;
use crate::txn::{Held, NO_TRANSACTION, TxId};

/// \[DESIGN\] A database as a replica of a group sees it (RULES.md D57):
/// see the module's documentation.
pub trait Standby: Send + Sync {
    /// Applies `records`, each a position and the bytes a log holds there,
    /// in position order, by the database's one replay; from then on it
    /// follows, taking no change until it is promoted. The records follow
    /// those it was given before: the log from its start, or from where
    /// its last checkpoint or replay left it. The records of a transaction
    /// not yet ended are held, and replayed with the record that ends it,
    /// in this call or a later one. Calls take turns: one driver makes
    /// them. The position of the last record taken, 0 if none was.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was deposed; the replay's (SQLCODE -916 for a
    /// malformed record), none of the records then taken.
    fn replay(&self, records: &mut dyn Iterator<Item = Result<(Lsn, Vec<u8>)>>) -> Result<Lsn>;
    /// Serves as primary, writing to `log` from the next transaction on:
    /// it takes changes again. Each transaction of the log it followed
    /// that is not ended is ended first: rolled back, its abort record
    /// appended to `log` and synced.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was deposed; `log`'s, appending or syncing an
    /// abort record: it then still follows, the transactions still open.
    fn promote(&self, log: Arc<dyn Log>) -> Result<()>;
    /// Stops serving: it takes no change until it is opened again. Its
    /// commits in doubt stay so (RULES.md D25).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn depose(&self) -> Result<()>;
}

/// How following a log treats a record, as recovery's replay does: at
/// once, held with its transaction's, or ending its transaction.
enum Scope {
    /// Replayed at once: what recovery makes wherever it stands.
    Now,
    /// Held until its transaction ends: what recovery makes of a
    /// transaction only as it ends.
    Open(TxId),
    /// Ends its transaction: replayed with the records held of it.
    Ends(TxId),
}

/// The scope of `record`. Each kind is matched without a wildcard, as in
/// the replay (RULES.md D57), so a kind added does not compile until it is
/// placed.
fn scope(record: &Record) -> Scope {
    let tx = record.tx;
    let of_transaction = match &record.change {
        Change::Commit { .. } | Change::Abort => return Scope::Ends(tx),
        // A session's `<schema definition>` is made as its transaction
        // commits; outside one, at once (`Database::definition_at`).
        Change::Insert { .. }
        | Change::Update { .. }
        | Change::Delete { .. }
        | Change::StatementUndone { .. }
        | Change::Drop { .. }
        | Change::AlterTable { .. }
        | Change::Defined { .. }
        | Change::Schema { .. } => true,
        Change::CreateIndex { .. }
        | Change::DropIndex { .. }
        | Change::AlterIndex { .. }
        | Change::Setting { .. }
        | Change::User { .. }
        | Change::Owner { .. }
        | Change::Checkpoint
        | Change::Stale { .. } => false,
    };
    if of_transaction && tx != NO_TRANSACTION { Scope::Open(tx) } else { Scope::Now }
}

impl Standby for Database {
    fn replay(&self, records: &mut dyn Iterator<Item = Result<(Lsn, Vec<u8>)>>) -> Result<Lsn> {
        let tx = Arc::clone(&self.snapshot()?.tx);
        refuse_deposed(tx.held(), "replays a log")?;
        let (encodings, types) = self.openers()?;
        let mut taken = Vec::new();
        for read in records {
            let (at, bytes) = read?;
            taken.push((at, crate::log::sealed::open(&bytes, &encodings, &types)?));
        }
        tx.hold(Some(Held::Following));
        let last = taken.last().map_or(Lsn(0), |&(at, _)| at);
        let mut now = Vec::new();
        {
            let mut followed = lock_builtin(&tx.followed);
            for (at, record) in taken {
                match scope(&record) {
                    Scope::Now => now.push((at, record)),
                    Scope::Open(id) => followed.entry(id).or_default().push((at, record)),
                    Scope::Ends(id) => {
                        now.extend(followed.remove(&id).unwrap_or_default());
                        now.push((at, record));
                    }
                }
            }
        }
        self.replay_now(now)?;
        Ok(last)
    }

    fn promote(&self, log: Arc<dyn Log>) -> Result<()> {
        let tx = Arc::clone(&self.snapshot()?.tx);
        refuse_deposed(tx.held(), "is promoted")?;
        let open: Vec<TxId> = lock_builtin(&tx.followed).keys().copied().collect();
        let mut ends = Vec::new();
        for &id in &open {
            let record = Record { tx: id, change: Change::Abort };
            ends.push((log.append(&record.encode())?, record));
        }
        if let Some(&(last, _)) = ends.last() {
            log.sync(last)?;
        }
        let held: BTreeMap<TxId, Vec<(Lsn, Record)>> = {
            let mut followed = lock_builtin(&tx.followed);
            open.iter().filter_map(|id| followed.remove_entry(id)).collect()
        };
        ends.extend(held.into_values().flatten());
        self.replay_now(ends)?;
        self.set_log(log)?;
        tx.hold(None);
        Ok(())
    }

    fn depose(&self) -> Result<()> {
        self.snapshot()?.tx.hold(Some(Held::Deposed));
        Ok(())
    }
}

impl Database {
    /// Replays `records`, in position order, by the one replay; then makes
    /// the index entries of the rows it touched, as index entries are not
    /// logged, and applies the settings it made.
    fn replay_now(&self, mut records: Vec<(Lsn, Record)>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        records.sort_by_key(|&(at, _)| at);
        let (_, mut touched) = self.replay_records(&mut records.into_iter().map(Ok), Definitions::All)?;
        self.change(|st: &mut State| st.mend_indexes(&mut touched, |_| true))?;
        self.apply_settings()
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
