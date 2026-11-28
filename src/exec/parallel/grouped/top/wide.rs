//! \[DESIGN\] (RULES.md D1300, D1256) A sort's first rows picked of a wide
//! partition where its groups lie (`table/made/wide.rs`). Of a sort whose
//! every key names a set function, the groups of several rows are read
//! of the partition's own table as a table's are, and those of one row
//! where they lie, in the Mades the partition adopted. A Made holds few
//! of a partition's groups, so a batch is read of several Mades one
//! after another, until it holds the rows a scan's batch holds, and the
//! sort keys' results of all of them are made in one column. One top-N
//! takes every batch, so as many rows are kept as of a table, and only
//! the groups picked are made rows. No key is copied.

use std::sync::Arc;

use super::{Top, add, number_of};
use crate::error::Result;
use crate::exec::batch::Kept;
use crate::exec::broken;
use crate::exec::sort::first_of_morsel;
use crate::exec::table::{GroupTable, Lying, Made, Wide, narrow};
use crate::logical::{AggCall, SortKey};
use crate::storage::{Batch, Column, Ints, Row};

/// The Mades a batch is read of, among those a wide partition's groups
/// of one row lie in: from which to which, and the number of the first
/// of their groups among the partition's groups read so.
#[derive(Clone, Copy)]
struct Span {
    from: usize,
    to: usize,
    base: usize,
}

/// The numbers from `from` on, `count` of them, as a column.
fn numbers(from: usize, count: usize) -> Column {
    let wide = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    Column::Ints(Ints::new((from..from.saturating_add(count)).map(wide).collect(), 0, None))
}

/// The number each Made of `lying` begins at among the partition's groups
/// read so, those of the table of its own, `own` of them, numbered
/// first, and the batches the Mades are read in: one after another until
/// a batch holds `filled` groups or more, then what is left.
fn spans(lying: &[Lying], own: usize, filled: usize) -> (Vec<usize>, Vec<Span>) {
    let (mut bases, mut spans) = (Vec::with_capacity(lying.len()), Vec::new());
    let (mut base, mut open) = (own, Span { from: 0, to: 0, base: own });
    for (k, (_, places)) in lying.iter().enumerate() {
        bases.push(base);
        base = base.saturating_add(places.len());
        open.to = k.saturating_add(1);
        if base.saturating_sub(open.base) >= filled {
            spans.push(open);
            open = Span { from: open.to, to: open.to, base };
        }
    }
    if open.to > open.from {
        spans.push(open);
    }
    (bases, spans)
}

/// \[DESIGN\] (RULES.md D1300, D1256) The batch a sort by the set
/// functions at `sorted_by`, of `aggs`, reads of the groups of one row
/// that lie in the Mades of `lying` that `span` names: those functions'
/// results, each group's first row's number, and its number among the
/// partition's groups read so. Every other set function is finished of
/// each group for the error it may raise, no value kept.
fn lying_batch(lying: &[Lying], span: Span, (sorted_by, aggs): (&[usize], &[AggCall])) -> Result<Kept> {
    let runs = lying.get(span.from..span.to).ok_or_else(|| broken("Mades a wide partition does not hold"))?;
    let rows: usize = runs.iter().map(|(_, places)| places.len()).sum();
    let groups: Vec<u32> = (0..rows).map(narrow).collect();
    let mut columns = Vec::with_capacity(sorted_by.len().saturating_add(2));
    for &agg in sorted_by {
        let call = aggs.get(agg).ok_or_else(|| broken("a set function a grouping does not compute"))?;
        columns.push(Made::results_of(runs, (agg, call), &groups)?);
    }
    Made::checked_but_of(runs, (sorted_by, aggs), &groups)?;
    columns.push(Column::Ints(Ints::new(Made::firsts_of(runs), 0, None)));
    columns.push(numbers(span.base, rows));
    Ok((Batch { columns, rows }, None))
}

impl Top {
    /// \[DESIGN\] (RULES.md D1300) [X3.135-1986 §8.3 GR3b] The first rows
    /// of the groups of `wide`, a partition's adopted where they lie, of
    /// the set functions `aggs`, as `of_table` gives them of a table. Of a
    /// sort whose every key names a set function they are picked where
    /// the groups lie. Any other sort reads the partition gathered into a
    /// table. An error is the one the groups raise given in their order,
    /// as a table's.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`, or the sort, fails.
    pub(in crate::exec::parallel::grouped) fn of_wide(&self, wide: Wide, aggs: &[AggCall]) -> Result<Vec<Row>> {
        let Some(sorted_by) = self.sorted_by(wide.width()) else { return self.of_table(wide.into_table()?) };
        match self.picked_of_wide(&wide, (&sorted_by, aggs)) {
            Ok(rows) => {
                add(&self.counts.made, wide.len());
                Ok(rows)
            }
            Err(e) => Err(wide.into_table().and_then(|table| table.finished().map(drop)).err().unwrap_or(e)),
        }
    }

    /// The first rows of `wide`'s groups under the sort's keys, set
    /// functions all, at `sorted_by` among `aggs`: one top-N over the batch
    /// of the partition's own table and the batches of the groups of one
    /// row, so as many rows are kept as of a table.
    fn picked_of_wide(&self, wide: &Wide, (sorted_by, aggs): (&[usize], &[AggCall])) -> Result<Vec<Row>> {
        let (held, lying): (&GroupTable, Arc<Vec<Lying>>) = (wide.own(), Arc::new(wide.lying()));
        let mut columns = sorted_by.iter().map(|&agg| held.results_of(agg)).collect::<Result<Vec<Column>>>()?;
        held.checked_but(sorted_by)?;
        let firsts = held.firsts().iter().map(|&first| i64::try_from(first).unwrap_or(i64::MAX)).collect();
        columns.push(Column::Ints(Ints::new(firsts, 0, None)));
        columns.push(numbers(0, held.len()));
        let (bases, spans) = spans(&lying, held.len(), self.filled);
        let (from, by, calls) = (Arc::clone(&lying), sorted_by.to_vec(), aggs.to_vec());
        let rest = spans.into_iter().map(move |span| lying_batch(&from, span, (&by, &calls)));
        // A batch of no row is not given, as the sort takes one for a morsel
        // it cannot hold and sorts every row.
        let own_batch = (held.len() > 0).then(|| Ok((Batch { columns, rows: held.len() }, None)));
        let batches = own_batch.into_iter().chain(rest);
        let mut keys: Vec<SortKey> = self.own().iter().enumerate().map(|(at, key)| SortKey { column: at, ..*key }).collect();
        // The first rows' numbers, the column after the sort's own.
        keys.push(SortKey::ascending(keys.len()));
        let first = first_of_morsel(&keys, Box::new(batches), (self.most, None), (&self.run.0, self.run.1))?;
        add(&self.counts.given, first.len());
        first.into_iter().map(|picked| self.made_of_wide(wide, (&lying, &bases), aggs, picked)).collect()
    }

    /// The projection's row of the group `picked` names last, a row of
    /// `picked_of_wide`'s batches, made of the partition's own table or
    /// of the Made of `lying` its number falls in, by `bases`, with the
    /// group's first row's number after it.
    fn made_of_wide(&self, wide: &Wide, (lying, bases): (&[Lying], &[usize]), aggs: &[AggCall], mut picked: Row) -> Result<Row> {
        let group = picked.pop().as_ref().and_then(number_of).ok_or_else(|| broken("a picked group has no number"))?;
        let first = picked.pop().ok_or_else(|| broken("a picked group has no first row"))?;
        let whole = if group < wide.own().len() {
            wide.own().row(group)?
        } else {
            let k = bases.partition_point(|&base| base <= group).saturating_sub(1);
            let within = group.saturating_sub(bases.get(k).copied().unwrap_or(0));
            let lies = lying.get(k).and_then(|(made, places)| places.get(within).map(|&place| (made, place)));
            let (made, place) = lies.ok_or_else(|| broken("a picked group lies in no Made"))?;
            made.row_at(place, aggs)?
        };
        let mut row = self.columns.iter().map(|pick| pick.value(&whole)).collect::<Result<Row>>()?;
        row.push(first);
        Ok(row)
    }
}
