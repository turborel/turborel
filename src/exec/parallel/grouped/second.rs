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
//! `regrouped.rs` feeds them a row at a time. Its reader finishes the
//! groups, or, of a second grouping that crosses the first's partitions,
//! writes their states for the statement's thread to merge (RULES.md
//! D1219).
//!
//! \[DESIGN\] (RULES.md D1280) A set function of the second that merges
//! the parts of one of the first (RULES.md D341) takes that function's
//! column of states from the first's table as it is, where the column
//! holds counts or exact totals (`table/merged.rs`). Its own states are
//! a column of the same kind, and the first's are added to them slot by
//! slot. No part is made a value. A part kept as accumulators is given
//! as its value and merged by an accumulator, as before.

use crate::error::Result;
use crate::exec::Runner;
use crate::exec::batch::{Read, lanes};
use crate::exec::table::{GroupTable, Key, Seed, hash_rows};
use crate::logical::AggCall;
use crate::storage::Row;

/// [X3.135-1986 §5.22 GR2] The groups `read` and `aggs` make of `first`,
/// a partition's groups of the first grouping in the order of their
/// first rows, keyed by hashes of `seed`: each numbered by its first
/// group of the first, in that order, in their table, for its reader to
/// finish or to write.
///
/// # Errors
///
/// As a set function fails.
pub(super) fn regrouped(first: GroupTable, (read, aggs): (&Read, &[AggCall]), (runner, seed): (&Runner, &Seed)) -> Result<GroupTable> {
    let from = first.merged_by(&read.taken, aggs);
    let (batch, firsts, parts) = first.into_parts(&from)?;
    let mut table = GroupTable::merging(read.groups.len(), aggs, seed, &parts);
    let mut hashes = Vec::new();
    hash_rows(&batch, (&read.groups, None), (seed, runner.kernels()), &mut hashes);
    for (n, (&hash, &number)) in hashes.iter().zip(&firsts).enumerate() {
        let g = table.found_or_made(hash, Key::Row(&batch, &read.groups, n), number);
        table.take(g, (n, n));
    }
    let fed = lanes(&read.taken, &batch, None);
    table.feed_parts(&batch, (&read.taken, &fed), &parts, (&mut Row::new(), runner))?;
    Ok(table)
}
