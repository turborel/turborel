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
//!
//! \[DESIGN\] (RULES.md D1325) Where the tables made before it followed
//! runs of one key, such a morsel hands on an entry a run, not a row. A
//! run's first row alone is hashed and written, and the entry's states
//! are those of the run's rows (`table/made/apart/runs.rs`).

use super::super::{Batches, Read, fed, lanes};
use super::coded::{Memo, grouped, hash_of};
use crate::error::Result;
use crate::exec::table::{GroupTable, Key, Runs, Seed, hash_lanes, hash_rows, narrow};
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
/// (RULES.md D1313); and, if it says so, the rows of a run of one key
/// are one such group (RULES.md D1325).
///
/// # Errors
///
/// As reading the batches, or a set function, fails.
pub(in crate::exec) fn morsel_groups(
    read: &Read,
    aggs: &[AggCall],
    batches: Batches,
    (runner, first, seed): (&Runner, u64, &Seed),
    (room, apart): (usize, Option<(usize, &[usize], bool)>),
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
        if grouped((&mut table, &mut memo), at, (number, seed, runner.kernels()), &mut hashes) {
            // Rows taken by their codes are each counted as looked up, as
            // no run is followed of them (RULES.md D1323, D1325).
            table.looked = table.looked.saturating_add(count);
        } else {
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
/// \[DESIGN\] (RULES.md D1325) If `runs`, the rows of one key that come
/// one after another in a batch (`run_ends`) are one group, an entry a
/// run. Such a batch is held with each run's first row as the rows kept
/// of it, so its keys, hashes and places are those rows', and every row
/// kept of it and the runs' ends are held beside it for the states
/// alone, which take every row of a run.
///
/// # Errors
///
/// As reading the batches, or a set function, fails.
fn rows_apart(
    read: &Read,
    aggs: &[AggCall],
    batches: Batches,
    (runner, first, seed): (&Runner, u64, &Seed),
    (parts, by_lanes, runs): (usize, &[usize], bool),
) -> Result<GroupTable> {
    let at = read.groups.as_slice();
    let ends_of = |batch: &Batch, rows: Option<&[u32]>| if runs { super::run_ends(batch, at, rows) } else { None };
    let (held, told) = Runs::held(batches.collect::<Result<Vec<_>>>()?, ends_of);
    let (mut hashes, mut by, mut one) = (Vec::new(), Vec::new(), Vec::new());
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
    let (of, how) = ((held.as_slice(), at), (parts, by_lanes));
    // An entry's first row is numbered among all the morsel's rows.
    let mut table = if told.any() {
        GroupTable::placed(of, (aggs, seed), (hashes.as_slice(), by, told.numbers(&held, first)), how)
    } else {
        GroupTable::placed(of, (aggs, seed), (hashes.as_slice(), by, (0_u64..).map(|n| first.saturating_add(n))), how)
    };
    let vectors: Vec<_> =
        held.iter().enumerate().map(|(b, (batch, heads))| lanes(&read.taken, batch, told.rows_of(b, heads.as_ref()))).collect();
    table.feed_placed((&held, &told), (&read.taken, &vectors), (&mut Vec::new(), runner))?;
    table.taken = told.rows();
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
    // Its rows are told as looked up, as they were before lookups were
    // counted apart from rows (RULES.md D1325).
    (table.taken, table.looked) = (taken, taken);
    Ok(table)
}

/// The state `state` writes, if it writes one.
fn written_state(state: &dyn Accumulator) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    state.write_state(&mut out).then_some(out)
}

/// Groups the rows `rows`, or all, of `batch` by its columns `at` into
/// `table`, each row's group found or made by its key and hash of
/// `seed`, its first row numbered from `number`.
///
/// \[DESIGN\] (RULES.md D31, D1323) Where rows of one key come one
/// after another (`run_ends`), a run's first row alone is hashed and
/// looked up, and the rows after it go to its group. Where no row
/// repeats the one before it, every row's key is hashed a vector at a
/// time, as of a batch that tells no run. The table counts the rows
/// looked up (RULES.md D1325).
fn hashed(
    table: &mut GroupTable,
    (batch, at, rows): (&Batch, &[usize], Option<&[u32]>),
    (number, seed, runner): (u64, &Seed, &Runner),
    hashes: &mut Vec<u64>,
) {
    let count = rows.map_or(batch.rows, <[u32]>::len);
    let numbered = |n: usize| number.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
    let Some(ends) = super::run_ends(batch, at, rows).filter(|ends| ends.len() < count) else {
        table.looked = table.looked.saturating_add(count);
        hash_rows(batch, (at, rows), (seed, runner.kernels()), hashes);
        for (n, &hash) in hashes.iter().enumerate() {
            let r = nth(rows, n);
            let g = table.found_or_made(hash, Key::Row(batch, at, r), numbered(n));
            table.take(g, (n, r));
        }
        return;
    };
    // Each run's first row, as its place among the rows given and in the
    // batch: the first row given, then the row after each end but the last.
    table.looked = table.looked.saturating_add(ends.len());
    let begins: Vec<usize> = std::iter::once(0).chain(ends.iter().copied()).take(ends.len()).collect();
    let firsts: Vec<u32> = begins.iter().map(|&n| narrow(nth(rows, n))).collect();
    hash_rows(batch, (at, Some(&firsts)), (seed, runner.kernels()), hashes);
    for ((&hash, &from), &end) in hashes.iter().zip(&begins).zip(&ends) {
        let g = table.found_or_made(hash, Key::Row(batch, at, nth(rows, from)), numbered(from));
        for n in from..end {
            table.take(g, (n, nth(rows, n)));
        }
    }
}
