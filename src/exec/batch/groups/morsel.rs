//! \[DESIGN\] A morsel's groups made by batch (RULES.md D293, D354): its
//! batches' rows grouped as a grouping by batch groups them (`groups.rs`),
//! each row's key made of the vectors (`put_key`), each group's rows of a
//! batch gathered in order and fed to its set functions a vector at a
//! time, no row made; held whole, as a grouping run in parallel charges
//! what each morsel's groups hold to its grant once they are written as
//! pieces (`parallel/grouped.rs`).

use super::super::{Batches, Read, fed_group, lanes, put_key};
use super::table::{Held, Table};
use super::{Grouping, fresh, gathered};
use crate::error::Result;
use crate::exec::Runner;
use crate::logical::AggCall;
use crate::storage::sel::nth;
use crate::storage::{Batch, Sel};

/// The groups of the rows of `batches`, one morsel's, as `read` reads them,
/// each with its key, its first row numbered from `first` in the order
/// the rows come, its set functions `aggs` fed and not finished, in no
/// order; their warnings heard by `runner`.
///
/// # Errors
///
/// As reading the batches, or a set function, fails.
pub(in crate::exec) fn morsel_groups(
    read: &Read,
    aggs: &[AggCall],
    batches: Batches,
    (runner, first): (&Runner, u64),
) -> Result<Vec<(Vec<u8>, Held)>> {
    let how = Grouping { read, aggs, runner, partial: false };
    let (mut table, mut key, mut scratch) = (Table::new(usize::MAX), Vec::new(), Vec::new());
    let mut number = first;
    for kept in batches {
        let (batch, rows) = kept?;
        let count = rows.as_ref().map_or(batch.rows, Sel::len);
        grouped_batch((&batch, rows.as_deref()), (&mut table, &mut key, &mut scratch), number, how)?;
        number = number.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
    Ok(table.drained())
}

/// Groups the rows `rows`, or all, of `batch`, the first numbered `first`,
/// into `table`, each key made in `key`, an expression's row in `scratch`.
fn grouped_batch(
    (batch, rows): (&Batch, Option<&[u32]>),
    (table, key, scratch): (&mut Table, &mut Vec<u8>, &mut Vec<crate::value::Value>),
    first: u64,
    how: Grouping<'_>,
) -> Result<()> {
    let lanes = lanes(&how.read.taken, batch, rows);
    let (mut groups, mut placed): (Vec<usize>, Vec<(usize, usize, usize)>) = (Vec::new(), Vec::new());
    for n in 0..rows.map_or(batch.rows, <[u32]>::len) {
        let r = nth(rows, n);
        key.clear();
        for &at in &how.read.groups {
            put_key(batch.cell(at, r), key);
        }
        let at = if let Some(at) = table.get(key) {
            at
        } else {
            let values = how.read.groups.iter().map(|&c| batch.cell(c, r).to_value()).collect();
            table.add(key, fresh(first.saturating_add(u64::try_from(n).unwrap_or(u64::MAX)), values, how))
        };
        if how.aggs.is_empty() {
            continue;
        }
        if table.slots.len() <= at {
            table.slots.resize(at.saturating_add(1), 0);
        }
        let slot = table.slots.get_mut(at).map_or(0, |slot| {
            if *slot == 0 {
                groups.push(at);
                *slot = groups.len();
            }
            slot.saturating_sub(1)
        });
        placed.push((slot, n, r));
    }
    let (given, read_rows, ends) = gathered(&placed, groups.len());
    let mut from = 0;
    for (&at, &end) in groups.iter().zip(&ends) {
        if let Some(slot) = table.slots.get_mut(at) {
            *slot = 0;
        }
        let (these, read_these) = (given.get(from..end).unwrap_or_default(), read_rows.get(from..end).unwrap_or_default());
        from = end;
        let Some((_, _, states)) = table.groups.get_mut(at) else { continue };
        fed_group(states, (&how.read.taken, scratch, how.runner), (batch, &lanes), (these, read_these))?;
    }
    Ok(())
}
