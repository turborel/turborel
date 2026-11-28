//! \[DESIGN\] A morsel's groups made by batch (RULES.md D293, D354, D356):
//! its batches' rows grouped as a grouping by batch groups them
//! (`groups.rs`), in the executor's table (`exec/table.rs`), each batch's
//! keys hashed a vector at a time and each set function's states fed in
//! one loop, no row made, a run of a column kept as runs taken whole; held whole, as a grouping run in parallel charges
//! what each morsel's groups hold to its grant once they are made
//! (`parallel/grouped.rs`).
//!
//! \[DESIGN\] (RULES.md D1260, D1313) A morsel whose rows a table would
//! not reduce is not grouped here at all. Each row is made a group of its
//! own, with no group looked for and no slot taken, and the partition
//! that merges the morsel finds each key's group as it merges any
//! morsel's. The rows are written once where their partition's run lies
//! (`table/made/apart.rs`): the morsel's batches are held and hashed, the
//! rows of each partition counted, and then each column is written at its
//! size, every row's value at its place.

use super::super::{Batches, Read, fed, lanes};
use super::coded::{Memo, grouped, hash_of};
use crate::error::Result;
use crate::exec::table::{GroupTable, Key, Seed, hash_lanes, hash_rows};
use crate::exec::{Runner, broken};
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::storage::sel::nth;
use crate::storage::{Batch, Sel};

/// The groups of the rows of `batches`, one morsel's, as `read` reads them,
/// keyed by hashes of `seed`, each its first row numbered from `first` in
/// the order the rows come, its set functions `aggs` fed and not finished;
/// their warnings heard by `runner`. The table is made with room for
/// `room` groups, as many as the morsel is expected to make. If `apart`
/// is given, every row is a group of its own, none looked for (RULES.md
/// D1260), the groups written by partition among as many partitions as
/// it says, by the lanes of the key it says, or by whole keys of none
/// (RULES.md D1313).
///
/// # Errors
///
/// As reading the batches, or a set function, fails.
pub(in crate::exec) fn morsel_groups(
    read: &Read,
    aggs: &[AggCall],
    batches: Batches,
    (runner, first, seed): (&Runner, u64, &Seed),
    (room, apart): (usize, Option<(usize, &[usize])>),
) -> Result<GroupTable> {
    if read.groups.is_empty()
        && let Some(states) = writing(aggs)
    {
        return one_group(read, (aggs, states), batches, (runner, first, seed));
    }
    if let Some(parted) = apart {
        return rows_apart(read, aggs, batches, (runner, first, seed), parted);
    }
    let mut table = GroupTable::with_room(read.groups.len(), aggs, seed, room);
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

/// \[DESIGN\] (RULES.md D1260, D1313) The rows of `batches`, one
/// morsel's, each a group of its own in a table with no slots, its first
/// row its own number from `first`, its set functions fed its one row.
/// The batches are held until each row's place among its partition's is
/// known, of `parts` partitions by the key's `lanes` or by whole keys,
/// so each key, hash and state is written once, where the morsel hands
/// it over. No group is looked for, so a key the morsel holds twice is
/// two groups, which the partition's table merges.
///
/// # Errors
///
/// As reading the batches, or a set function, fails.
fn rows_apart(
    read: &Read,
    aggs: &[AggCall],
    batches: Batches,
    (runner, first, seed): (&Runner, u64, &Seed),
    (parts, by_lanes): (usize, &[usize]),
) -> Result<GroupTable> {
    let held = batches.collect::<Result<Vec<_>>>()?;
    let (mut hashes, mut by, mut one) = (Vec::new(), Vec::new(), Vec::new());
    let at = read.groups.as_slice();
    for (batch, rows) in &held {
        let rows = rows.as_ref().map(Sel::as_slice);
        hash_rows(batch, (at, rows), (seed, runner.kernels()), &mut one);
        hashes.extend_from_slice(&one);
        if !by_lanes.is_empty() {
            hash_lanes(batch, (at, by_lanes, rows), (seed, runner.kernels()), &mut one);
            by.extend_from_slice(&one);
        }
    }
    let by = (!by_lanes.is_empty()).then_some(by.as_slice());
    let mut table = GroupTable::placed((held.as_slice(), at), (aggs, seed), (hashes.as_slice(), by, first), (parts, by_lanes));
    let fed: Vec<_> = held.iter().map(|(batch, rows)| lanes(&read.taken, batch, rows.as_ref().map(Sel::as_slice))).collect();
    table.feed_placed(&held, (&read.taken, &fed), (&mut Vec::new(), runner))?;
    Ok(table)
}

/// The accumulators of `aggs`, if each writes its state, which the one
/// group of a morsel without grouping columns is merged by. A plan runs
/// in parallel only set functions that write theirs (D53), so this holds
/// of every morsel planned.
fn writing(aggs: &[AggCall]) -> Option<Vec<Box<dyn Accumulator>>> {
    let states: Vec<Box<dyn Accumulator>> = aggs.iter().map(|a| a.func.0.accumulator(a.result.clone())).collect();
    states.iter().all(|state| state.write_state(&mut Vec::new())).then_some(states)
}

/// \[DESIGN\] (RULES.md D1170) The one group of the rows of `batches`, a
/// morsel's, of no grouping columns: `states` fed each batch's vectors as
/// a set function of no grouping is fed (`batch.rs`), no row hashed nor
/// taken by a group, then written into the table's one group, made of
/// the first row, numbered `first`. A morsel of no rows makes none.
fn one_group(
    read: &Read,
    (aggs, mut states): (&[AggCall], Vec<Box<dyn Accumulator>>),
    batches: Batches,
    (runner, first, seed): (&Runner, u64, &Seed),
) -> Result<GroupTable> {
    let mut table = GroupTable::with_room(0, aggs, seed, 1);
    let (mut hashes, mut taken) = (Vec::new(), 0_usize);
    for kept in batches {
        let (batch, rows) = kept?;
        let rows = rows.as_ref().map(Sel::as_slice);
        let count = rows.map_or(batch.rows, <[u32]>::len);
        if count == 0 {
            continue;
        }
        if table.len() == 0 {
            let r = nth(rows, 0);
            let hash = hash_of(&batch, (&[], r), (seed, runner.kernels()), &mut hashes);
            table.found_or_made(hash, Key::Row(&batch, &[], r), first);
        }
        fed(&batch, (&read.taken, runner), rows, &mut states)?;
        taken = taken.saturating_add(count);
    }
    if table.len() > 0 {
        let written: Option<Vec<Vec<u8>>> = states.iter().map(|state| written_state(state.as_ref())).collect();
        let written = written.ok_or_else(|| broken("a set function of no grouping wrote no state"))?;
        table.merge_written(0, (first, &written))?;
    }
    table.taken = taken;
    Ok(table)
}

/// The state `state` writes, if it writes one.
fn written_state(state: &dyn Accumulator) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    state.write_state(&mut out).then_some(out)
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
