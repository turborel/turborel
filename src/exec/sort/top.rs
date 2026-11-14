//! [ISO/IEC 9075-2:2023 §7.17 GR5, GR7] \[DESIGN\] A sort of which only
//! the first rows are asked for, by FETCH FIRST and OFFSET (`exec/limit.rs`):
//! as the rows come, only the first `most` in order are held, in a heap
//! whose greatest is the one to give up first, ties going to the row that
//! came first, as the sort's do; of WITH TIES, the rows that are peers of
//! the greatest held are held too, as long as it is. So it holds about
//! `most` rows however many it reads, and gives what the whole sort would
//! give first. Once those it holds outgrow the work area, they and the rest
//! are sorted as a whole sort is, spilling (RULES.md D38).

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use super::super::work::Work;
use super::super::{RowIter, broken};
use super::{Sorter, compare_rows};
use crate::error::Result;
use crate::logical::{Most, SortKey};
use crate::spill::row_bytes;
use crate::storage::Row;

/// A row held, the place it came in, and the keys it is ordered by.
struct Held {
    row: Row,
    place: usize,
    keys: Arc<[SortKey]>,
}

impl Ord for Held {
    fn cmp(&self, other: &Held) -> Ordering {
        compare_rows(&self.keys, &self.row, &other.row).then(self.place.cmp(&other.place))
    }
}

impl PartialOrd for Held {
    fn partial_cmp(&self, other: &Held) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Held {
    fn eq(&self, other: &Held) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Held {}

/// The rows held: the first `most` in order, and of WITH TIES the peers
/// of the greatest of them that came after it, and their bytes.
struct Top {
    keys: Arc<[SortKey]>,
    most: Most,
    heap: BinaryHeap<Held>,
    peers: Vec<Held>,
    bytes: usize,
}

impl Top {
    /// Takes `held` among the first, if it is, giving up the greatest so
    /// far for it; else, of WITH TIES, among the peers if it is one.
    fn add(&mut self, held: Held) {
        if self.heap.len() < self.most.rows {
            self.hold(held);
            return;
        }
        let Some(greatest) = self.heap.peek() else { return };
        match (compare_rows(&self.keys, &held.row, &greatest.row), self.most.ties) {
            (Ordering::Less, _) => {
                self.hold(held);
                let Some(given_up) = self.heap.pop() else { return };
                self.after(given_up);
            }
            (Ordering::Equal, true) => {
                self.bytes = self.bytes.saturating_add(row_bytes(&held.row));
                self.peers.push(held);
            }
            (Ordering::Equal, false) | (Ordering::Greater, _) => {}
        }
    }

    fn hold(&mut self, held: Held) {
        self.bytes = self.bytes.saturating_add(row_bytes(&held.row));
        self.heap.push(held);
    }

    /// `given_up`, once the greatest held: a peer of the greatest now, of
    /// WITH TIES, else let go, with the peers, if the greatest is no more
    /// their peer.
    fn after(&mut self, given_up: Held) {
        let peer = self.most.ties && self.heap.peek().is_some_and(|g| compare_rows(&self.keys, &given_up.row, &g.row).is_eq());
        if peer {
            self.peers.push(given_up);
            return;
        }
        self.bytes = self.bytes.saturating_sub(row_bytes(&given_up.row));
        for gone in self.peers.drain(..) {
            self.bytes = self.bytes.saturating_sub(row_bytes(&gone.row));
        }
    }

    /// Every row held, in the order they came.
    fn in_order_come(self) -> Vec<Held> {
        let mut all = self.heap.into_vec();
        all.extend(self.peers);
        all.sort_by_key(|h| h.place);
        all
    }
}

/// The first `most` rows of `input` sorted by `keys`, in a work area of
/// `work`, and of WITH TIES the peers of the last after it; of more, the
/// rest after them, sorted, once those held outgrow `work`.
///
/// # Errors
///
/// The input's, a row of no key column's, or a spill's.
pub(super) fn top(keys: &[SortKey], mut input: RowIter, most: Most, work: &mut Work) -> Result<RowIter> {
    let keys: Arc<[SortKey]> = keys.into();
    let mut top = Top { keys: Arc::clone(&keys), most, heap: BinaryHeap::new(), peers: Vec::new(), bytes: 0 };
    let mut place = 0_usize;
    while let Some(row) = input.next() {
        let row = row?;
        if let Some(k) = keys.iter().find(|k| row.get(k.column).is_none()) {
            return Err(broken(&format!("sort column {} is not in the rows sorted", k.column)));
        }
        top.add(Held { row, place, keys: Arc::clone(&keys) });
        place = place.saturating_add(1);
        if top.bytes > work.limit() {
            return whole(top, input, work);
        }
    }
    let mut all = top.in_order_come();
    all.sort();
    Ok(Box::new(all.into_iter().map(|h| Ok(h.row))))
}

/// The rows `top` holds, then the rest of `input`, sorted as a whole sort
/// sorts them: those held in the order they came, so equal rows keep it.
/// A row given up already is past the first, so not needed.
fn whole(top: Top, input: RowIter, work: &mut Work) -> Result<RowIter> {
    let mut sorter = Sorter::new(top.keys.to_vec());
    for held in top.in_order_come() {
        sorter.push(held.row, work)?;
    }
    for row in input {
        sorter.push(row?, work)?;
    }
    sorter.sorted(work)
}

// Unit tests: the first rows held against a whole sort's, of ONLY and WITH
// TIES, and once they outgrow the work area (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
