//! \[DESIGN\] A key's values read as they are taken: a page of its bucket's
//! chain at a time, so a key of more values than memory holds is read
//! whole, as a spill area's table is probed by a key most of a hash join's
//! right rows have (RULES.md D38). Split from engine/hash.rs (RULES.md
//! R-SIZE-2).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::bucket::{self, fragment};
use super::{HashTable, hash};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};

/// The error of a table whose shape changed while a key's values were read.
fn changed() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a hash table changed while a key's values were read")
}

impl HashTable {
    /// The values of `key`, in the order added, each page of its bucket's
    /// chain read as the values before it are taken. For a table nothing
    /// is added to or removed from while they are read: one whose shape
    /// changes meanwhile fails the read (SQLCODE -910), as the entries it
    /// has still to read may have moved.
    pub fn values(self: &Arc<Self>, key: &[u8]) -> impl Iterator<Item = Result<Vec<u8>>> + Send + 'static {
        let (table, key) = (Arc::clone(self), key.to_vec());
        let h = hash(&key);
        let mut before = None;
        let mut at: Option<PageId> = None;
        let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
        let mut failed = false;
        std::iter::from_fn(move || {
            loop {
                if let Some(value) = pending.pop_front() {
                    return Some(Ok(value));
                }
                if failed {
                    return None;
                }
                let read = table.next_page(h, (&key, &mut before), &mut at)?;
                match read {
                    Ok(values) => pending.extend(values),
                    Err(e) => {
                        failed = true;
                        return Some(Err(e));
                    }
                }
            }
        })
    }

    /// The values of `key`, of hash `h`, on the page `at` of its chain, the
    /// next page made `at`; the first page if `before`, the version they
    /// are read in, is none yet. None past the last page.
    fn next_page(&self, h: u64, (key, before): (&[u8], &mut Option<u64>), at: &mut Option<PageId>) -> Option<Result<Vec<Vec<u8>>>> {
        let version = self.version.load(Ordering::Acquire);
        let page = match *before {
            None => {
                if version & 1 == 1 {
                    return Some(Err(changed()));
                }
                *before = Some(version);
                match self.head(self.current().bucket(h)) {
                    Ok(head) => head,
                    Err(e) => return Some(Err(e)),
                }
            }
            Some(was) => {
                if was != version {
                    return Some(Err(changed()));
                }
                (*at)?
            }
        };
        let read = self.pool.read(self.file, page.0).and_then(|content| {
            *at = bucket::next(&content)?;
            bucket::find(&content, key, fragment(h))
        });
        Some(read)
    }
}
