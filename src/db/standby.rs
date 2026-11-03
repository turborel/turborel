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
//!   until it is opened again, in place (`Standby::reopen`), which
//!   recovers it from its last checkpoint alone, and it follows again
//!   (`standby/reopen.rs`).
//!
//! The changes it does not take are those it would write to its log, as a
//! failed sync stops them (RULES.md D25): a row change, the commit of a
//! transaction that wrote, a definition, a checkpoint; reads and rollbacks
//! go on.
//!
//! **Continuous replay.** A group commits a log record by record, so the
//! records a following database is given end anywhere: within a
//! transaction, whose commit comes in a later call. Following, it holds
//! the records of each transaction not yet ended, each transaction's in a
//! work area's bytes (MEMORY.WORK_BYTES) and the rest in a spill area
//! (`txn/followed.rs`), and replays them with the record that ends it,
//! merged in position order as they are read back (`standby/merge.rs`);
//! the others at once, as recovery replays them wherever they stand: a
//! definition made outside the transactions of sessions, a setting, a
//! user, an index, a checkpoint's. So each call replays what recovery of
//! the whole log so far would make of it, with the transactions left open
//! undone.
//!
//! **Ending open transactions.** A view change keeps records that had not
//! committed ([VRR §4.2]: "Operations that had not committed might also
//! survive"), and so the first records of a transaction whose commit
//! record its primary, deposed, never appends. Promoted, the database
//! ends each transaction it holds open, an abort record of it appended and
//! synced to its new log before it serves, so each replica following that
//! log ends it too.
//!
//! **Export and import.** A replica the log alone no longer brings up to
//! date, behind what the others retain of it, takes the primary's state:
//! its export (`Standby::export`, `db/export.rs`: its last checkpoint's
//! image and its durable files), which replication moves without decoding
//! it, and which the replica installs (`Standby::import`,
//! `standby/import.rs`): its files replaced, the image recorded as its
//! catalog's, and the database opened again in place from it, following
//! from the position the image holds the log through.

mod import;
mod merge;
mod reopen;

use std::collections::BTreeMap;
use std::sync::Arc;

use self::merge::{Merged, Stream};
pub(in crate::db) use self::reopen::Opening;
use super::export::StateSource;
use super::recovery::Definitions;
use super::{Database, State, api};
use crate::diagnostics::Event;
use crate::error::{Result, SqlError};
use crate::log::record::{Change, Record};
use crate::log::{Log, Lsn, sealed};
use crate::spill::ListSpace;
use crate::sync::lock_builtin;
use crate::txn::{Followed, FollowedRecords, Following, Held, NO_TRANSACTION, TxId};

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
    /// Opens it again in place, deposed or following, from its files: its
    /// state recovered from its last checkpoint alone, as the records of
    /// its replica's log after it may not be committed; it then follows,
    /// the same handle, its jobs, settings and sinks kept, sessions begun
    /// before refused (`Database::open_following`). Through where it holds
    /// the log: the records after, once committed, are given it again.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was not opened from its files, or serves; as
    /// `Database::open_following`.
    fn reopen(&self) -> Result<Lsn>;
    /// \[EXTENSION\] An export of it, as it is now: its last checkpoint's
    /// image and its durable files, for a replica behind to install
    /// (`Database::export`). While the export lives, it takes no
    /// checkpoint.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was not opened from its files, or recorded no
    /// checkpoint; -915 if a file cannot be read.
    fn export(&self) -> Result<Box<dyn StateSource>>;
    /// \[EXTENSION\] Installs `source`, an export of the primary's
    /// database, in place of its own state, deposed or following: its
    /// durable files replaced by the export's, the export's image recorded
    /// as its catalog's, and itself opened again from it, following.
    /// Through where it then holds the log: the position the image holds
    /// every record before.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was not opened from its files, or serves; -915
    /// if a file cannot be written; as `Database::open_following`.
    fn import(&self, source: Box<dyn StateSource>) -> Result<Lsn>;
    /// \[EXTENSION\] Retires it: its replica was removed from the group
    /// (RULES.md D57, reconfiguration), so it serves nothing more, reads
    /// neither (SQLCODE -917), until the operator stops the server.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn retire(&self) -> Result<()>;
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
        | Change::Reconfiguration { .. }
        | Change::Checkpoint
        | Change::Stale { .. } => false,
    };
    if of_transaction && tx != NO_TRANSACTION { Scope::Open(tx) } else { Scope::Now }
}

impl Standby for Database {
    fn replay(&self, records: &mut dyn Iterator<Item = Result<(Lsn, Vec<u8>)>>) -> Result<Lsn> {
        let (tx, space) = {
            let st = self.snapshot()?;
            (Arc::clone(&st.tx), st.list_space())
        };
        refuse_deposed(tx.held(), "replays a log")?;
        // Taking turns with a checkpoint of what it follows.
        let turn = tx.replaying.take("a replay")?;
        let (encodings, types) = self.openers()?;
        let mut taken = Vec::new();
        for read in records {
            let (at, bytes) = read?;
            let record = sealed::open(&bytes, &encodings, &types)?;
            taken.push((at, bytes, record));
        }
        tx.hold(Some(Held::Following));
        let last = taken.last().map_or(Lsn(0), |&(at, _, _)| at);
        // Taken out while records are held, as holding them may write to a
        // spill area, a plug-in (RULES.md R-ERR-4); calls take turns.
        let mut followed = std::mem::take(&mut *lock_builtin(&tx.followed));
        let image = lock_builtin(&tx.following).image;
        let sorted = sort_out(taken, &mut followed, (&space, image));
        *lock_builtin(&tx.followed) = followed;
        let mut following = lock_builtin(&tx.following);
        *following = Following { replayed: following.replayed.max(last), ..*following };
        drop(following);
        if let Some(e) = sorted.failed {
            self.emit(&Event::Failed(e));
        }
        self.replay_now(sorted.now, sorted.ended)?;
        drop(turn);
        self.checkpoint_if_due();
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
        let held: Vec<FollowedRecords> = {
            let mut followed = lock_builtin(&tx.followed);
            open.iter().filter_map(|id| followed.remove(id)).collect()
        };
        self.replay_now(ends, held)?;
        self.set_log(log)?;
        tx.hold(None);
        Ok(())
    }

    fn depose(&self) -> Result<()> {
        self.snapshot()?.tx.hold(Some(Held::Deposed));
        Ok(())
    }

    fn reopen(&self) -> Result<Lsn> {
        self.reopen_in_place()
    }

    fn export(&self) -> Result<Box<dyn StateSource>> {
        Database::export(self)
    }

    fn import(&self, source: Box<dyn StateSource>) -> Result<Lsn> {
        self.import_state(source)
    }

    fn retire(&self) -> Result<()> {
        self.snapshot()?.tx.hold(Some(Held::Retired));
        Ok(())
    }
}

/// What a call to `replay` does with the records it took: replays at
/// once, or holds; and the first failure to spill those it holds.
struct Sorted {
    /// The records replayed at once, in position order.
    now: Vec<(Lsn, Record)>,
    /// The records held of each transaction those end.
    ended: Vec<FollowedRecords>,
    /// The spill area's failure: the records are held all the same, in
    /// memory.
    failed: Option<SqlError>,
}

/// Sorts out `taken`, each a position, its bytes and its record: each
/// replayed at once, held in `followed` with its transaction's, spilling
/// in `space`, or ending its transaction, whose records held are taken. A
/// record before `image`, the position the checkpoint it was opened from
/// holds every record before, is applied already, but held with a
/// transaction it holds again (`db/standby/reopen.rs`).
fn sort_out(
    taken: Vec<(Lsn, Vec<u8>, Record)>,
    followed: &mut BTreeMap<TxId, FollowedRecords>,
    (space, image): (&ListSpace, Lsn),
) -> Sorted {
    let mut sorted = Sorted { now: Vec::new(), ended: Vec::new(), failed: None };
    for (at, bytes, record) in taken {
        let applied = at < image;
        match scope(&record) {
            Scope::Now => {
                if !applied {
                    sorted.now.push((at, record));
                }
            }
            Scope::Open(id) => {
                let held = followed.entry(id).or_insert_with(|| FollowedRecords::new(space.clone()));
                if let Err(e) = held.push(Followed { at, bytes }) {
                    sorted.failed.get_or_insert(e);
                }
            }
            Scope::Ends(id) => {
                let ended = followed.remove(&id);
                if !applied {
                    sorted.ended.extend(ended);
                    sorted.now.push((at, record));
                }
            }
        }
    }
    sorted
}

impl Database {
    /// Replays `now` and the records `ended` hold, merged in position
    /// order, by the one replay; then makes the index entries of the rows
    /// it touched, as index entries are not logged, and applies the
    /// settings it made.
    fn replay_now(&self, now: Vec<(Lsn, Record)>, ended: Vec<FollowedRecords>) -> Result<()> {
        if now.is_empty() && ended.is_empty() {
            return Ok(());
        }
        let (encodings, types) = self.openers()?;
        let open = |held: Result<Followed>| held.and_then(|f| Ok((f.at, sealed::open(&f.bytes, &encodings, &types)?)));
        let mut streams: Vec<Stream<'_>> = vec![Box::new(now.into_iter().map(Ok))];
        streams.extend(ended.into_iter().map(|held| -> Stream<'_> { Box::new(held.into_records().map(open)) }));
        let (_, mut touched) = self.replay_records(&mut Merged::new(streams), Definitions::All)?;
        // Counted toward its next checkpoint, as the changes it would log.
        let changes = u64::try_from(touched.len()).unwrap_or(u64::MAX);
        self.snapshot()?.tx.logged.fetch_add(changes, std::sync::atomic::Ordering::Relaxed);
        self.change(|st: &mut State| st.mend_indexes(&mut touched, |_| true))?;
        self.apply_settings()
    }
}

/// Refuses what a deposed database is asked to do, `what`: it is opened
/// again first.
fn refuse_deposed(held: Option<Held>, what: &str) -> Result<()> {
    match held {
        Some(Held::Deposed) => Err(api(format!("[DESIGN] RULES.md D57: a deposed database is opened again before it {what}"))),
        Some(Held::Retired) => Err(api(format!("[DESIGN] RULES.md D57: a retired database, its replica removed, never {what}"))),
        Some(Held::Following) | None => Ok(()),
    }
}
