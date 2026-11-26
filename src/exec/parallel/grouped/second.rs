//! \[DESIGN\] (RULES.md D1123) The second grouping of a pair made in the
//! executor's table (`exec/table.rs`), as the first is. A partition's
//! groups of the first grouping are given as one batch, a group a row,
//! its keys' lanes and its set functions' results the columns
//! (`table/given.rs`), in the order of their first rows. The second
//! grouping reads that batch as a grouping by batch reads a scan's: its
//! keys hashed a vector at a time, each row's group found or made by its
//! key in the vectors, and each set function's states fed the rows in one
//! loop. No row of values is made of a group of the first, no key's bytes,
//! and no accumulator a group where a column holds the states. A group of
//! the second is numbered by its first group of the first, and each set
//! function takes its values in the first's order, as
//! `regrouped.rs` feeds them a row at a time.

use crate::error::Result;
use crate::exec::Runner;
use crate::exec::batch::{Read, lanes};
use crate::exec::table::{GroupTable, Key, Seed, hash_rows};
use crate::logical::AggCall;
use crate::storage::Row;

/// [X3.135-1986 §5.22 GR2] The groups `read` and `aggs` make of `first`,
/// a partition's groups of the first grouping in the order of their
/// first rows, keyed by hashes of `seed`: each numbered by its first
/// group of the first, in that order, finished.
///
/// # Errors
///
/// As a set function fails.
pub(super) fn regrouped(first: GroupTable, (read, aggs): (&Read, &[AggCall]), (runner, seed): (&Runner, &Seed)) -> Result<Vec<(u64, Row)>> {
    let (batch, firsts) = first.into_batch()?;
    let mut table = GroupTable::new(read.groups.len(), aggs, seed, usize::MAX);
    let mut hashes = Vec::new();
    hash_rows(&batch, (&read.groups, None), (seed, runner.kernels()), &mut hashes);
    for (n, (&hash, &number)) in hashes.iter().zip(&firsts).enumerate() {
        let g = table.found_or_made(hash, Key::Row(&batch, &read.groups, n), number);
        table.take(g, (n, n));
    }
    let fed = lanes(&read.taken, &batch, None);
    table.feed(&batch, (&read.taken, &fed), (&mut Row::new(), runner))?;
    table.finished()
}
