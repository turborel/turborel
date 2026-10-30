//! \[DESIGN\] Scans of a linear hash table (`engine/hash.rs`), a bucket at
//! a time, so a scan holds no more than one bucket's entries.
//!
//! * **Buckets**, exactly: a scan of every entry stops the shape changing
//!   while it lives, so each entry held throughout is in one bucket it
//!   reads, once. It waits for a change of shape under way to end; one due
//!   meanwhile is left to the writer after it, as when another is under
//!   way, the chains growing meanwhile. Writers go on: a bucket is read
//!   under its stripe's lock, released before the next is read.
//! * **Keys**, for statistics: the shape may change meanwhile, so a split
//!   may move an entry past the scan, or before it.
//!
//! Split from hash.rs (RULES.md R-SIZE-2).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{Entry, HashTable};
use crate::error::Result;
use crate::sync::lock_builtin;

/// A scan under way: the shape is not changed until it is dropped.
struct Scanning(Arc<HashTable>);

impl Drop for Scanning {
    fn drop(&mut self) {
        self.0.scans.fetch_sub(1, Ordering::SeqCst);
    }
}

impl HashTable {
    /// Whether a scan of every bucket is under way, so the shape must not
    /// change.
    pub(super) fn scanned(&self) -> bool {
        self.scans.load(Ordering::SeqCst) > 0
    }

    /// The entries of bucket `b`, in the order added, read under its
    /// stripe's lock.
    fn bucket(&self, b: u64) -> Result<Vec<Entry>> {
        let _locked = self.lock(&[b]);
        Ok(self.chain(self.head(b)?)?.1.into_iter().map(|(_, e)| e).collect())
    }

    /// Every entry, a bucket at a time, each bucket's in the order added;
    /// the shape not changing until what it gives is dropped. Entries
    /// added or removed meanwhile may or may not be given.
    pub fn scan(self: Arc<Self>) -> impl Iterator<Item = Result<Vec<(Vec<u8>, Vec<u8>)>>> + Send {
        self.scans.fetch_add(1, Ordering::SeqCst);
        // A change of shape begun before holds the directory to its end.
        drop(lock_builtin(&self.directory));
        let scanning = Scanning(self);
        let mut b = 0_u64;
        std::iter::from_fn(move || {
            let table = &scanning.0;
            if b >= table.current().buckets() {
                return None;
            }
            let read = table.bucket(b);
            b = b.saturating_add(1);
            Some(read)
        })
    }

    /// Every entry's key, bucket by bucket, each bucket read under its
    /// stripe's lock alone, so writers go on meanwhile: for statistics, as
    /// a bucket split during the scan may move an entry past it, or before.
    pub fn keys(&self) -> impl Iterator<Item = Result<Vec<u8>>> + '_ {
        let (mut b, mut pending) = (0_u64, Vec::new().into_iter());
        std::iter::from_fn(move || {
            loop {
                if let Some(key) = pending.next() {
                    return Some(Ok(key));
                }
                if b >= self.current().buckets() {
                    return None;
                }
                let read = self.bucket(b);
                b = b.saturating_add(1);
                match read {
                    Ok(entries) => pending = entries.into_iter().map(|(key, _)| key).collect::<Vec<_>>().into_iter(),
                    Err(e) => return Some(Err(e)),
                }
            }
        })
    }
}
