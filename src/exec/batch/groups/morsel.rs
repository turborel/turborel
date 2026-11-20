//! \[DESIGN\] A morsel's groups made by batch (RULES.md D293, D354, D356):
//! its batches' rows grouped as a grouping by batch groups them
//! (`groups.rs`), in the executor's table (`exec/table.rs`), each batch's
//! keys hashed a vector at a time and each set function's states fed in
//! one loop, no row made; held whole, as a grouping run in parallel charges
//! what each morsel's groups hold to its grant once they are made
//! (`parallel/grouped.rs`).

use super::super::{Batches, Read, lanes};
use crate::error::Result;
use crate::exec::Runner;
use crate::exec::table::{GroupTable, Key, Seed, hash_rows};
use crate::logical::AggCall;
use crate::storage::Sel;
use crate::storage::sel::nth;

/// The groups of the rows of `batches`, one morsel's, as `read` reads them,
/// keyed by hashes of `seed`, each its first row numbered from `first` in
/// the order the rows come, its set functions `aggs` fed and not finished;
/// their warnings heard by `runner`.
///
/// # Errors
///
/// As reading the batches, or a set function, fails.
pub(in crate::exec) fn morsel_groups(
    read: &Read,
    aggs: &[AggCall],
    batches: Batches,
    (runner, first, seed): (&Runner, u64, &Seed),
) -> Result<GroupTable> {
    let mut table = GroupTable::new(read.groups.len(), aggs, seed, usize::MAX);
    let (mut hashes, mut scratch) = (Vec::new(), Vec::new());
    let mut number = first;
    for kept in batches {
        let (batch, rows) = kept?;
        let rows = rows.as_ref().map(Sel::as_slice);
        let count = rows.map_or(batch.rows, <[u32]>::len);
        hash_rows(&batch, (&read.groups, rows), (seed, runner.kernels()), &mut hashes);
        for (n, &hash) in hashes.iter().enumerate() {
            let r = nth(rows, n);
            let first_row = number.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
            let g = table.found_or_made(hash, Key::Row(&batch, &read.groups, r), first_row);
            table.take(g, (n, r));
        }
        let lanes = lanes(&read.taken, &batch, rows);
        table.feed(&batch, (&read.taken, &lanes), (&mut scratch, runner))?;
        number = number.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
    Ok(table)
}
