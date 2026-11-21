//! [ISO/IEC 9075-2:2023 §7.15 GR3, GR4; §10.10 GR1e ii 1] Where a batch's
//! rows begin a window partition or a peer group (RULES.md D560): of their
//! normal keys, the partitioning's then the ordering's, each row's bytes
//! compared with the row's before it by `Kernels::boundaries`, of the
//! partitioning's bytes for partitions and of all for peers; the values
//! compared only where a string was cut short in its key, or where the
//! keys have no normal form. A batch's first row is compared with the
//! last of the batch before: by bytes if both are of the sort's one
//! layout, else by their values.

use crate::exec::sort::{RunKeys, alike_cells};
use crate::kernels::Kernels;
use crate::logical::SortKey;
use crate::storage::{Batch, Column};

/// The last row read of a window's input: a batch of it alone, and its
/// key if of the sort's one layout.
pub(in crate::exec::window) struct Last {
    pub(in crate::exec::window) row: Batch,
    pub(in crate::exec::window) key: Option<Vec<u8>>,
}

impl Last {
    /// The last row of `batch`, its keys `keys`.
    pub(in crate::exec::window) fn of(batch: &Batch, keys: Option<&RunKeys>) -> Option<Last> {
        let at = batch.rows.checked_sub(1)?;
        let picks = [Some(at)];
        let columns: Vec<Column> = batch.columns.iter().map(|c| c.gathered(&picks)).collect();
        let key =
            keys.filter(|k| k.shared).and_then(|k| k.bytes.get(at.saturating_mul(k.width)..at.saturating_add(1).saturating_mul(k.width)));
        Some(Last { row: Batch { columns, rows: 1 }, key: key.map(<[u8]>::to_vec) })
    }
}

/// The keys of a window: its partitioning's, then those and its ordering's.
pub(in crate::exec::window) type Sorted<'a> = (&'a [SortKey], &'a [SortKey]);

/// Of each row of `batch`, whether it begins a partition and whether it
/// begins a peer group, a partition's first among them, of its keys
/// `keys` if they have a normal form, the row before it `last`.
pub(in crate::exec::window) fn edges(
    (batch, keys): (&Batch, Option<&RunKeys>),
    last: Option<&Last>,
    (partition, both): Sorted<'_>,
    kernels: &dyn Kernels,
) -> (Vec<bool>, Vec<bool>) {
    let n = batch.rows;
    let (mut parts, mut peers) = (vec![false; n], vec![false; n]);
    for (r, (part, peer)) in parts.iter_mut().zip(peers.iter_mut()).enumerate().skip(1) {
        (*part, *peer) = match keys {
            Some(_) => (false, false),
            None => apart((batch, r.saturating_sub(1)), (batch, r), (partition, both)),
        };
    }
    if let Some(k) = keys {
        let split = partition.len().checked_sub(1).and_then(|p| k.ends.get(p)).copied().unwrap_or(0);
        let mut words = Vec::new();
        kernels.boundaries(&k.bytes, k.width, (0, split), &mut words);
        set_bits(&words, &mut parts);
        words.clear();
        kernels.boundaries(&k.bytes, k.width, (0, k.width), &mut words);
        set_bits(&words, &mut peers);
        if k.truncates {
            recheck(batch, (&mut parts, &mut peers), (partition, both));
        }
    }
    if let Some(first) = parts.first_mut().zip(peers.first_mut()) {
        let (part, peer) = match last {
            None => (true, true),
            Some(last) => first_apart(last, (batch, keys), (partition, both)),
        };
        (*first.0, *first.1) = (part, peer);
    }
    for (part, peer) in parts.iter().zip(peers.iter_mut()) {
        *peer |= *part;
    }
    (parts, peers)
}

/// Sets each of `flags` whose bit is set in `words`, a mask's.
fn set_bits(words: &[u64], flags: &mut [bool]) {
    for (r, flag) in flags.iter_mut().enumerate() {
        let word = words.get(r.checked_div(64).unwrap_or_default()).copied().unwrap_or_default();
        let bit = u32::try_from(r.checked_rem(64).unwrap_or_default()).unwrap_or_default();
        *flag |= word.checked_shr(bit).unwrap_or_default() & 1 == 1;
    }
}

/// Of rows whose partitioning's bytes are equal, of a key with a string
/// cut short, whether they begin a partition or a peer group, by their
/// values: equal bytes may hide a difference past a string's cut, while
/// bytes that differ always tell values apart.
fn recheck(batch: &Batch, (parts, peers): (&mut [bool], &mut [bool]), sorted: Sorted<'_>) {
    for (r, (part, peer)) in parts.iter_mut().zip(peers.iter_mut()).enumerate().skip(1) {
        if !*part {
            (*part, *peer) = apart((batch, r.saturating_sub(1)), (batch, r), sorted);
        }
    }
}

/// Whether row `b` begins a partition after row `a`, and whether a peer
/// group, by their values.
fn apart(a: (&Batch, usize), b: (&Batch, usize), (partition, both): Sorted<'_>) -> (bool, bool) {
    if !alike_cells(partition, a, b) {
        return (true, true);
    }
    (false, !alike_cells(both, a, b))
}

/// Whether the first row of `batch` begins a partition after `last`, and
/// whether a peer group.
fn first_apart(last: &Last, (batch, keys): (&Batch, Option<&RunKeys>), sorted: Sorted<'_>) -> (bool, bool) {
    let shared = keys.filter(|k| k.shared && !k.truncates).zip(last.key.as_ref());
    if let Some((k, before)) = shared {
        let split = sorted.0.len().checked_sub(1).and_then(|p| k.ends.get(p)).copied().unwrap_or(0);
        let first = k.bytes.get(..k.width).unwrap_or_default();
        let part = first.get(..split) != before.get(..split);
        return (part, part || first != before.as_slice());
    }
    apart((&last.row, 0), (batch, 0), sorted)
}
