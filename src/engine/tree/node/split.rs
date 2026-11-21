//! \[DESIGN\] A full node split in two (RULES.md D31; Lehman and Yao 1981,
//! §4): where it splits, by bytes or where a run of inserts goes on
//! (`node.rs`), and the separator, suffix-truncated for a leaf. Each half
//! is built with its own prefix, what its first and last keys share with
//! its new high key.

use super::build::{Shape, built, built_size};
use super::{KeyRef, child_of, corrupt, entry_size};
use crate::disk::pager::PageId;
use crate::error::Result;

/// The halves of a split node (§4): the left half, rewritten in its page,
/// and the right half, in a new page to its right; and the separator, the
/// left half's high key, which goes to the parent with the right half's
/// page.
pub(in crate::engine::tree) struct Halves {
    pub(in crate::engine::tree) left: Vec<u8>,
    pub(in crate::engine::tree) right: Vec<u8>,
    pub(in crate::engine::tree) separator: Vec<u8>,
}

/// Splits a node of `shape` that would hold `entries`, in key order, too
/// many for its page; the right half goes to page `new`. Halves are even
/// by bytes, unless inserts run in key order through the node and `run`
/// is the index of the entry added: then the entries before it stay, and
/// it starts the right half, marked as inserted last there, so the run
/// goes on (see `node.rs`). Either way the split moves to the nearest
/// entry at which both halves fit `capacity`, each built with its own
/// prefix: a key without the prefix the node's keys share takes it from
/// the half it joins, whose keys then take their whole length.
///
/// # Errors
///
/// SQLCODE -916 if there are too few entries to split, or as `build`.
pub(in crate::engine::tree) fn split(
    shape: &Shape<'_>,
    entries: &[(KeyRef<'_>, &[u8])],
    run: Option<usize>,
    (new, capacity): (PageId, usize),
) -> Result<Halves> {
    let leaf = shape.level == 0;
    // A leaf keeps the separator, its highest key; an inner node gives it
    // up, its child becoming the right half's first.
    let most = entries.len().checked_sub(if leaf { 2 } else { 1 }).ok_or_else(|| corrupt("a node too full has too few entries"))?;
    // A run starts the right half at the entry added: in a leaf, the one
    // before it is the separator; in an inner node it goes up.
    let at_run = run.and_then(|at| at.checked_sub(1)).filter(|&before| before <= most);
    let start = at_run.unwrap_or_else(|| balance(entries).min(most));
    let middle = fitting(shape, entries, (start, most), capacity);
    let (_, value) = *entries.get(middle).ok_or_else(|| corrupt("a split is past its entries"))?;
    let separator = separator(entries, middle, leaf);
    let (left_end, right_first) = if leaf { (middle.saturating_add(1), PageId(0)) } else { (middle, child_of(value)?) };
    let left_entries = entries.get(..left_end).unwrap_or_default();
    let right_entries = entries.get(middle.saturating_add(1)..).unwrap_or_default();
    let left = built(&Shape { high: Some(&separator), right: Some(new), ..*shape }, left_entries, None, None)?;
    let right = built(&Shape { first: right_first, ..*shape }, right_entries, at_run.map(|_| 0), None)?;
    Ok(Halves { left, right, separator })
}

/// The separator of a split at entry `at`: its key, or of a leaf the
/// shortest key parting it from the next, so the levels above hold short
/// keys.
fn separator(entries: &[(KeyRef<'_>, &[u8])], at: usize, leaf: bool) -> Vec<u8> {
    let last = entries.get(at).map(|(k, _)| k.to_vec()).unwrap_or_default();
    match (leaf, entries.get(at.saturating_add(1))) {
        (true, Some((first, _))) => shortest_between(&last, &first.to_vec()).to_vec(),
        (true, None) | (false, _) => last,
    }
}

/// The bytes of the left and right halves of a split at entry `at`.
fn sizes(shape: &Shape<'_>, entries: &[(KeyRef<'_>, &[u8])], at: usize) -> (usize, usize) {
    let leaf = shape.level == 0;
    let left_end = if leaf { at.saturating_add(1) } else { at };
    let left = built_size(separator(entries, at, leaf).len(), entries.get(..left_end).unwrap_or_default());
    let right = built_size(shape.high.map_or(0, <[u8]>::len), entries.get(at.saturating_add(1)..).unwrap_or_default());
    (left, right)
}

/// The split point nearest `start`, up to `most`, at which both halves fit
/// `capacity`; `start` if none does.
fn fitting(shape: &Shape<'_>, entries: &[(KeyRef<'_>, &[u8])], (start, most): (usize, usize), capacity: usize) -> usize {
    let fits = |at: usize| {
        let (left, right) = sizes(shape, entries, at);
        left <= capacity && right <= capacity
    };
    (0..=most)
        .flat_map(|d| [start.checked_sub(d), start.checked_add(d).filter(|&at| at <= most && d > 0)])
        .flatten()
        .find(|&at| fits(at))
        .unwrap_or(start)
}

/// \[DESIGN\] Suffix truncation (Bayer and Unterauer, "Prefix B-Trees",
/// ACM TODS 2(1), 1977): the shortest key at or above `last` and below
/// `first`, which is above it: a proper prefix of `first` one byte past
/// their common prefix, if `first` is longer; `last` itself if it is a
/// prefix of `first`, or no proper prefix of `first` lies between them.
pub(in crate::engine::tree) fn shortest_between<'k>(last: &'k [u8], first: &'k [u8]) -> &'k [u8] {
    let common = last.iter().zip(first).take_while(|(a, b)| a == b).count();
    if common == last.len() {
        return last;
    }
    let prefix = first.get(..common.saturating_add(1)).unwrap_or(last);
    if prefix.len() < first.len() { prefix } else { last }
}

/// The entry at which `entries` are half their bytes.
fn balance(entries: &[(KeyRef<'_>, &[u8])]) -> usize {
    let size = |(k, v): &(KeyRef<'_>, &[u8])| entry_size(&[], v).saturating_add(k.len());
    let total = entries.iter().fold(0_usize, |n, e| n.saturating_add(size(e)));
    let half = total.checked_div(2).unwrap_or_default();
    let mut sum = 0_usize;
    for (i, entry) in entries.iter().enumerate() {
        sum = sum.saturating_add(size(entry));
        if sum >= half {
            return i;
        }
    }
    entries.len().saturating_sub(1)
}
