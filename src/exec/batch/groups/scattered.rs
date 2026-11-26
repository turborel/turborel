//! \[DESIGN\] (RULES.md D1119) The rows of a morsel whose keys are nearly
//! all apart grouped as groups of their own (`table/scattered.rs`). A
//! morsel's first batch is grouped as any is. If it made more groups
//! than half its rows, its keys are taken to be apart, and the rest of
//! its batches are hashed a vector at a time and each row made a group,
//! none looked for. The partitions then find each row's group, as they
//! found each group of the morsel's table, and give what a grouping of
//! the rows gives.

use crate::exec::Runner;
use crate::exec::table::{GroupTable, Key, Seed, hash_rows};
use crate::storage::Batch;
use crate::storage::sel::nth;

/// Whether a morsel whose first batch of `rows` rows made `groups`
/// groups has keys apart, so its other rows are made groups of their own.
pub(super) fn apart(groups: usize, rows: usize) -> bool {
    groups.saturating_mul(2) > rows
}

/// Makes each of the rows `rows`, or all, of `batch` a group of its own
/// in `table`, of its key of the columns `at` hashed of `seed` a vector
/// at a time, its first row numbered from `number`.
pub(super) fn scattered(
    table: &mut GroupTable,
    (batch, at, rows): (&Batch, &[usize], Option<&[u32]>),
    (number, seed, runner): (u64, &Seed, &Runner),
    hashes: &mut Vec<u64>,
) {
    hash_rows(batch, (at, rows), (seed, runner.kernels()), hashes);
    for (n, &hash) in hashes.iter().enumerate() {
        let r = nth(rows, n);
        let first = number.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        let g = table.scattered(hash, Key::Row(batch, at, r), first);
        table.take(g, (n, r));
    }
}
