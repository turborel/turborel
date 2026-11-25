//! \[DESIGN\] A file opened when the process has no descriptor left
//! (RULES.md D1136). FILES.MAX_OPEN bounds the database's files, but the
//! process holds others besides, its connections' sockets among them, and
//! the system's limit may be lower than the setting. A file the system
//! will not open for want of a descriptor (SQLCODE -918 from
//! `OsFileSystem`) is opened as one past FILES.MAX_OPEN is. A file of the
//! database's not in use is closed for it and the open tried again, or,
//! while every file open is in use, tried again after one is done. Only when none but
//! the one asking holds a descriptor is the error returned, as no file of
//! the database's can give one up.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use super::{LOOK_AGAIN, Registry};
use crate::error::{Code, Result, SqlError};

/// Who asks for a descriptor that is no file's: a directory opened to be
/// synced, which holds it only for that, and was admitted for none.
pub(super) const NO_FILE: u64 = u64::MAX;

impl Registry {
    /// What `open` gives, for `asking`, a file admitted already or
    /// `NO_FILE`. While the process has no descriptor left, it is tried
    /// again after a file of the database's gives one up. The failures of
    /// syncs of files closed for it are returned too, for the caller to
    /// tell once it holds no lock.
    pub(super) fn opened<T>(&self, asking: u64, open: impl Fn() -> Result<T>) -> (Result<T>, Vec<SqlError>) {
        // The descriptors held but the one asking's.
        let own = usize::from(asking != NO_FILE);
        let mut failed = Vec::new();
        loop {
            let done = open();
            let spent = done.as_ref().err().is_some_and(|e| e.code == Code::Resources);
            if !spent || self.open.load(Ordering::Acquire) <= own {
                return (done, failed);
            }
            if !self.evict(asking, &mut failed) {
                let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
                drop(self.done.wait_timeout(waiting, LOOK_AGAIN).unwrap_or_else(PoisonError::into_inner));
            }
        }
    }
}
