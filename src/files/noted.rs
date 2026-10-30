//! \[DESIGN\] Files a database does not open, but shows (RULES.md D56): a
//! server's own, as its diagnostics log, its socket, and its TLS and key
//! files. Each is noted for as long as its note lives, with whether it is
//! open and its reads, writes and syncs as its holder tells them; they are
//! not the database's to limit.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use super::{FileInfo, Registry, Use};

/// What is noted of a file.
pub(super) struct Entry {
    path: PathBuf,
    open: AtomicBool,
    counts: [AtomicU64; 3],
}

impl Entry {
    pub(super) fn info(&self) -> FileInfo {
        let count = |u: Use| self.counts.get(u.at()).map_or(0, |c| c.load(Ordering::Relaxed));
        FileInfo {
            path: self.path.clone(),
            open: self.open.load(Ordering::Relaxed),
            reads: count(Use::Read),
            writes: count(Use::Write),
            syncs: count(Use::Sync),
            noted: true,
        }
    }
}

/// \[DESIGN\] A file noted (`Registry::note`), shown while it lives.
pub struct Noted {
    id: u64,
    entry: Arc<Entry>,
    registry: Weak<Registry>,
}

impl Registry {
    /// Notes the file at `path`, held open now if `open`, until the note is
    /// dropped.
    pub fn note(self: &Arc<Self>, path: &Path, open: bool) -> Noted {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(Entry { path: path.to_path_buf(), open: AtomicBool::new(open), counts: Default::default() });
        self.notes().insert(id, Arc::downgrade(&entry));
        Noted { id, entry, registry: Arc::downgrade(self) }
    }
}

impl Noted {
    fn count(&self, what: Use) {
        if let Some(count) = self.entry.counts.get(what.at()) {
            count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Counts a read of it.
    pub fn read(&self) {
        self.count(Use::Read);
    }

    /// Counts a write to it.
    pub fn wrote(&self) {
        self.count(Use::Write);
    }

    /// Counts a sync of it.
    pub fn synced(&self) {
        self.count(Use::Sync);
    }

    /// Notes whether it is held open now.
    pub fn set_open(&self, open: bool) {
        self.entry.open.store(open, Ordering::Relaxed);
    }
}

impl Drop for Noted {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.notes().remove(&self.id);
        }
    }
}
