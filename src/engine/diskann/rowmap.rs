//! \[DESIGN\] The row map: which node holds a (vector, row id) entry, for
//! `rows`, `remove` and an insert of an entry already held. A B-link tree
//! in the same file (`engine/tree.rs`) keyed by an 8-byte hash of the
//! stored vector then the row id, both big-endian, so a vector's rows are
//! one range. The vector itself would pass a B-link tree's key limit. Its
//! value is the node number. The node's stored vector is compared, so a
//! hash shared by two vectors of one row only moves the later to the next
//! hash, at most `PROBES` of them.

use std::ops::Bound;

use crate::engine::tree::Direction;
use crate::error::{Code, Result, SqlError};
use crate::hash::xxhash::xxh3_64;

use super::DiskAnn;
use super::node::{corrupt, u32_at};

/// Hashes tried for one row's vectors before a collision is refused.
const PROBES: u64 = 4;
/// Bytes of a key: hash, then row id.
const KEY: usize = 16;
/// The bytes of the hash in a key.
const HASH: usize = 8;

/// A range of row map keys.
type Range<'a> = (Bound<&'a [u8]>, Bound<&'a [u8]>);

/// Where an entry is, or where it would go.
pub(super) enum Found {
    /// The node holding it.
    Node(u32),
    /// Not held: the key a new node's entry takes.
    Absent([u8; KEY]),
}

/// The key of `hash` and `row`.
pub(super) fn key(hash: u64, row: u64) -> [u8; KEY] {
    let mut k = [0_u8; KEY];
    let (h, r) = k.split_at_mut(HASH);
    h.copy_from_slice(&hash.to_be_bytes());
    r.copy_from_slice(&row.to_be_bytes());
    k
}

/// The row id of a key.
fn row_of(k: &[u8]) -> u64 {
    let mut b = [0_u8; HASH];
    b.iter_mut().zip(k.iter().skip(HASH)).for_each(|(d, s)| *d = *s);
    u64::from_be_bytes(b)
}

/// The hash of stored vector bytes.
pub(super) fn hash_of(stored: &[u8]) -> u64 {
    xxh3_64(stored)
}

impl DiskAnn {
    /// The node holding (`stored`, `row`), or the key it would take.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if `PROBES` vectors of one row share a hash; the
    /// buffer pool's.
    pub(super) fn find(&self, stored: &[u8], row: u64) -> Result<Found> {
        let first = hash_of(stored);
        for probe in 0..PROBES {
            let k = key(first.wrapping_add(probe), row);
            let Some(value) = self.inner.rowmap.get(&k)? else { return Ok(Found::Absent(k)) };
            let node = u32_at(&value, 0);
            if self.holds(node, stored)? {
                return Ok(Found::Node(node));
            }
        }
        Err(SqlError::new(Code::Unsupported, "[EXTENSION] too many vectors of one row share a DiskANN hash"))
    }

    /// Whether node `node` stores `stored`.
    fn holds(&self, node: u32, stored: &[u8]) -> Result<bool> {
        let record = self.record(node)?;
        Ok(self.inner.shape.stored_vector(&record)? == stored)
    }

    /// Every (row, node) whose node stores `stored`.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub(super) fn entries_of(&self, stored: &[u8]) -> Result<Vec<(u64, u32)>> {
        let first = hash_of(stored);
        let (low, high) = (key(first, 0), key(first.wrapping_add(PROBES), 0));
        let mut found = Vec::new();
        let ranges: Vec<Range<'_>> = if high > low {
            vec![(Bound::Included(&low[..]), Bound::Excluded(&high[..]))]
        } else {
            vec![(Bound::Included(&low[..]), Bound::Unbounded), (Bound::Unbounded, Bound::Excluded(&high[..]))]
        };
        for (from, to) in ranges {
            for entry in self.inner.rowmap.scan(from, to, Direction::Forward) {
                let (k, value) = entry?;
                let node = u32_at(&value, 0);
                if self.holds(node, stored)? {
                    found.push((row_of(&k), node));
                }
            }
        }
        Ok(found)
    }

    /// Records node `node` under key `k`; whether it was absent.
    pub(super) fn map(&self, k: &[u8; KEY], node: u32) -> Result<bool> {
        self.inner.rowmap.insert(k, &node.to_le_bytes())
    }

    /// Takes key `k` out if it still names node `node`.
    pub(super) fn unmap(&self, k: &[u8; KEY], node: u32) -> Result<bool> {
        self.inner.rowmap.delete_if(k, &node.to_le_bytes())
    }

    /// The row map key of node `node` holding `stored` for `row`, by
    /// probing as `find` does.
    pub(super) fn key_of(&self, stored: &[u8], row: u64, node: u32) -> Result<Option<[u8; KEY]>> {
        let first = hash_of(stored);
        for probe in 0..PROBES {
            let k = key(first.wrapping_add(probe), row);
            let Some(value) = self.inner.rowmap.get(&k)? else { return Ok(None) };
            if u32_at(&value, 0) == node {
                return Ok(Some(k));
            }
        }
        Err(corrupt("a node's row map entry is past its probes"))
    }
}
