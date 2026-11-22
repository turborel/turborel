//! \[DESIGN\] The current snapshot of a database's definitions, as a
//! statement takes it (RULES.md D780). The snapshot is published under a
//! lock and counted; each session keeps a copy of its own of the last one
//! it took, with that count, and takes it again only once the count has
//! moved. A statement of a session then reads one shared word, written
//! only as the definitions change, and writes to no line another session's
//! statement writes: neither the lock's count of readers nor the shared
//! snapshot's count of references, which every statement of every session
//! changed twice, so that at eight threads each statement waited on lines
//! moving between cores.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{Database, State};
use crate::error::{Code, Result, fail};
use crate::sync::{read, write};

/// A session's own copy of a snapshot, and the count of the snapshots
/// published when it was taken.
pub(in crate::db) type Own = Option<(u64, Arc<State>)>;

impl Database {
    /// The current snapshot of the definitions, for one statement or
    /// cursor: SQLCODE -910 if the database is closed or a thread panicked
    /// publishing one, SQLSTATE 38T11 if a host plug-in stopped the node.
    pub(crate) fn snapshot(&self) -> Result<Arc<State>> {
        crate::db::hosted::running(self.snapshot_unstopped()?)
    }

    /// `snapshot`, whether or not a host plug-in stopped the node.
    pub(crate) fn snapshot_unstopped(&self) -> Result<Arc<State>> {
        self.refuse_closed()?;
        Ok(Arc::clone(&*read(&self.0.current)?))
    }

    /// `snapshot`, of a session whose own copy is `own`: that copy if no
    /// snapshot was published since it was taken, else a copy of the
    /// current one, kept in its place. The count is read before the
    /// snapshot, and moved after one is published, so a copy is never
    /// counted newer than it is.
    pub(in crate::db) fn snapshot_for(&self, own: &mut Own) -> Result<Arc<State>> {
        self.refuse_closed()?;
        let count = self.0.publications.load(Ordering::Acquire);
        if let Some((taken, st)) = own.as_ref()
            && *taken == count
        {
            return crate::db::hosted::running(Arc::clone(st));
        }
        let copy = Arc::new(State::clone(&**read(&self.0.current)?));
        *own = Some((count, Arc::clone(&copy)));
        crate::db::hosted::running(copy)
    }

    /// Publishes `state` as the current snapshot, and counts it.
    pub(in crate::db) fn publish(&self, state: State) -> Result<()> {
        let mut current = write(&self.0.current)?;
        *current = Arc::new(state);
        self.0.publications.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// SQLCODE -910 if it is closed.
    fn refuse_closed(&self) -> Result<()> {
        if self.0.closed.load(Ordering::Acquire) {
            return fail(Code::Api, "[DESIGN] the database is closed");
        }
        Ok(())
    }
}
