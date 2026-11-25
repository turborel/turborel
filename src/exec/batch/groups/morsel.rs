//! \[DESIGN\] A morsel's groups made by batch (RULES.md D293, D354, D356):
//! its batches' rows grouped as a grouping by batch groups them
//! (`groups.rs`), in the executor's table (`exec/table.rs`), each batch's
//! keys hashed a vector at a time and each set function's states fed in
//! one loop, no row made, a run of a column kept as runs taken whole; held whole, as a grouping run in parallel charges
//! what each morsel's groups hold to its grant once they are made
//! (`parallel/grouped.rs`).

use super::super::{Batches, Read, lanes};
use super::coded::{Memo, grouped};
use crate::error::Result;
use crate::exec::Runner;
use crate::exec::table::{GroupTable, Key, Seed, hash_rows};
use crate::logical::AggCall;
use crate::storage::sel::nth;
use crate::storage::{Batch, Sel};

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
    let (mut hashes, mut scratch, mut memo) = (Vec::new(), Vec::new(), Memo::default());
    let mut number = first;
    for kept in batches {
        let (batch, rows) = kept?;
        let rows = rows.as_ref().map(Sel::as_slice);
        let count = rows.map_or(batch.rows, <[u32]>::len);
        // Of keys all codes of dictionaries, by their codes (`coded.rs`).
        let at = (&batch, read.groups.as_slice(), rows);
        if !grouped((&mut table, &mut memo), at, (number, seed, runner.kernels()), &mut hashes) {
            hashed(&mut table, at, (number, seed, runner), &mut hashes);
        }
        let lanes = lanes(&read.taken, &batch, rows);
        table.feed(&batch, (&read.taken, &lanes), (&mut scratch, runner))?;
        number = number.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
    Ok(table)
}

/// Groups the rows `rows`, or all, of `batch` by its columns `at` into
/// `table`, each row's key hashed of `seed` a vector at a time, its group
/// found or made, its first row numbered from `number`.
fn hashed(
    table: &mut GroupTable,
    (batch, at, rows): (&Batch, &[usize], Option<&[u32]>),
    (number, seed, runner): (u64, &Seed, &Runner),
    hashes: &mut Vec<u64>,
) {
    hash_rows(batch, (at, rows), (seed, runner.kernels()), hashes);
    // Of a grouping by one column of runs, a row of the run of the row
    // before it goes to that row's group, no key compared (`run_ends`).
    let mut ends = super::run_ends(batch, at, rows).into_iter().flatten();
    let (mut boundary, mut g) = (0_usize, 0_u32);
    for (n, &hash) in hashes.iter().enumerate() {
        let r = nth(rows, n);
        if n >= boundary {
            boundary = ends.next().unwrap_or(n.saturating_add(1));
            let first_row = number.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
            g = table.found_or_made(hash, Key::Row(batch, at, r), first_row);
        }
        table.take(g, (n, r));
    }
}
