//! \[DESIGN\] A key's values read as they are taken: a page of its bucket's
//! chain at a time, so a key of more values than memory holds is read
//! whole, as a spill area's table is probed by a key most of a hash join's
//! right rows have (RULES.md D38), and an index's key of many rows is
//! read (`index/hash.rs`). Split from engine/hash.rs (RULES.md R-SIZE-2).
//!
//! The table's shape is held while they are read, as a scan's is
//! (`hash/scan.rs`): no split or merge moves entries between pages, or
//! frees a page, meanwhile. A writer adds an entry to the chain's last
//! page, or a page after it, and a remove takes one from its page alone,
//! so an entry there throughout is given once; one added or removed
//! meanwhile may or may not be.

use std::collections::VecDeque;
use std::sync::Arc;

use super::bucket::{self, fragment};
use super::{HashTable, hash};
use crate::disk::pager::PageId;
use crate::error::Result;

impl HashTable {
    /// The values of `key`, in the order added, each page of its bucket's
    /// chain read as the values before it are taken; the table's shape
    /// not changing until what this gives is dropped.
    pub fn values(self: &Arc<Self>, key: &[u8]) -> impl Iterator<Item = Result<Vec<u8>>> + Send + 'static {
        let held = Arc::clone(self).held();
        let (key, h) = (key.to_vec(), hash(key));
        let mut at: Option<Result<PageId>> = Some(held.0.head(held.0.current().bucket(h)));
        let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
        std::iter::from_fn(move || {
            loop {
                if let Some(value) = pending.pop_front() {
                    return Some(Ok(value));
                }
                let page = match at.take()? {
                    Ok(page) => page,
                    Err(e) => return Some(Err(e)),
                };
                let read = held
                    .0
                    .pool
                    .read(held.0.file, page.0)
                    .and_then(|content| Ok((bucket::next(&content)?, bucket::find(&content, &key, fragment(h))?)));
                match read {
                    Ok((next, values)) => {
                        at = next.map(Ok);
                        pending.extend(values);
                    }
                    Err(e) => return Some(Err(e)),
                }
            }
        })
    }
}
