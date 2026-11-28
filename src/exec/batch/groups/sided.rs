//! \[DESIGN\] (RULES.md D1324) A morsel's side groups: the batches the
//! morsel's first grouping of a pair groups by its whole key, grouped
//! here by the second grouping's columns alone, each plain set function
//! fed its rows. The batches are grouped as a morsel's are
//! (`morsel.rs`): keys all codes of dictionaries by their codes, a
//! column stored as runs a run at a time with no key looked up, any
//! other hashed a vector at a time.

use super::super::{Kept, Read, lanes};
use super::coded::{Memo, grouped};
use super::morsel::hashed;
use crate::error::Result;
use crate::exec::Runner;
use crate::exec::table::{GroupTable, Seed};
use crate::logical::AggCall;
use crate::storage::Sel;

/// The groups of the rows of `held`, a morsel's batches, by the columns
/// `read` names, keyed by hashes of `seed`, each of `aggs` fed as `read`
/// says it takes a row and none finished; warnings heard by `runner`.
/// Every group is numbered `first`: a side group gives no row of its
/// own, so its number is read nowhere.
///
/// # Errors
///
/// As a set function, or an expression it takes, fails.
pub(in crate::exec) fn side_groups(
    read: &Read,
    aggs: &[AggCall],
    held: &[Kept],
    (runner, first, seed): (&Runner, u64, &Seed),
) -> Result<GroupTable> {
    let mut table = GroupTable::with_room(read.groups.len(), aggs, seed, 0);
    let (mut hashes, mut scratch, mut memo) = (Vec::new(), Vec::new(), Memo::default());
    for (batch, rows) in held {
        let rows = rows.as_ref().map(Sel::as_slice);
        let at = (batch, read.groups.as_slice(), rows);
        if !grouped((&mut table, &mut memo), at, (first, seed, runner.kernels()), &mut hashes) {
            hashed(&mut table, at, (first, seed, runner), &mut hashes);
        }
        let fed = lanes(&read.taken, batch, rows);
        table.feed(batch, (&read.taken, &fed), (&mut scratch, runner))?;
    }
    Ok(table)
}
