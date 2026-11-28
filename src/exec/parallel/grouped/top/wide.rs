//! \[DESIGN\] (RULES.md D1300, D1256) A sort's first rows picked of a wide
//! partition where its groups lie (`table/made/wide.rs`). Of a sort whose
//! every key names a set function, the groups of several rows are read
//! of the tables of its sub-parts' own as a table's are, and those of one
//! row where they lie, in the Mades the partition adopted. A Made holds few
//! of a partition's groups, so a batch is read of several Mades one
//! after another, until it holds the rows a scan's batch holds, and the
//! sort keys' results of all of them are made in one column. One top-N
//! takes every batch, so as many rows are kept as of a table, and only
//! the groups picked are made rows. No key is copied. Each batch is
//! judged against the bound the partitions share before the sort is
//! given it (`bound.rs`, RULES.md D1338).

use std::sync::Arc;

use super::bound::Judge;
use super::{Top, add, number_of};
use crate::error::Result;
use crate::exec::batch::Kept;
use crate::exec::broken;
use crate::exec::sort::first_of_morsel;
use crate::exec::table::{GroupTable, Lying, Made, Wide, narrow};
use crate::logical::{AggCall, SortKey};
use crate::storage::{Batch, Column, Ints, Row, Sel};

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
/// each group for the error it may raise, no value kept. The batch is
/// given with the rows `judge` keeps of it, those at or before the bound
/// the partitions share, and none is given if it keeps none.
fn lying_batch(lying: &[Lying], span: Span, (sorted_by, aggs): (&[usize], &[AggCall]), judge: &Judge) -> Result<Option<Kept>> {
    let runs = lying.get(span.from..span.to).ok_or_else(|| broken("Mades a wide partition does not hold"))?;
    let rows: usize = runs.iter().map(|(_, places)| places.len()).sum();
    let groups: Vec<u32> = (0..rows).map(narrow).collect();
    let mut columns = Vec::with_capacity(sorted_by.len().saturating_add(2));
    for &agg in sorted_by {
        let call = aggs.get(agg).ok_or_else(|| broken("a set function a grouping does not compute"))?;
        columns.push(Made::results_of(runs, (agg, call), &groups)?);
    }
    Made::checked_but_of(runs, (sorted_by, aggs), &groups)?;
    let kept = judge.kept(columns.first());
    if kept.as_ref().is_some_and(Sel::is_empty) {
        return Ok(None);
    }
    columns.push(Column::Ints(Ints::new(Made::firsts_of(runs), 0, None)));
    columns.push(numbers(span.base, rows));
    Ok(Some((Batch { columns, rows }, kept)))
}

/// The batch a sort by the set functions at `sorted_by` reads of `own`,
/// a sub-part's table of its own: those functions' results, each group's
/// first row's number, and its number among the partition's groups read
/// so, from `base` on. Every other set function is finished of each
/// group for the error it may raise, no value kept. The batch is given
/// with the rows `judge` keeps of it, and none is given if it keeps
/// none.
fn own_batch(own: &GroupTable, sorted_by: &[usize], base: usize, judge: &Judge) -> Result<Option<Kept>> {
    let mut columns = sorted_by.iter().map(|&agg| own.results_of(agg)).collect::<Result<Vec<Column>>>()?;
    own.checked_but(sorted_by)?;
    let kept = judge.kept(columns.first());
    if kept.as_ref().is_some_and(Sel::is_empty) {
        return Ok(None);
    }
    let firsts = own.firsts().iter().map(|&first| i64::try_from(first).unwrap_or(i64::MAX)).collect();
    columns.push(Column::Ints(Ints::new(firsts, 0, None)));
    columns.push(numbers(base, own.len()));
    Ok(Some((Batch { columns, rows: own.len() }, kept)))
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
    /// functions all, at `sorted_by` among `aggs`: one top-N over a batch
    /// of each sub-part's own table and the batches of the groups of one
    /// row, so as many rows are kept as of a table. The tables of its own
    /// are read first, as their groups took the most rows, so a sort by
    /// a count raises the bound of them before the groups of one row are
    /// judged (RULES.md D1338).
    fn picked_of_wide(&self, wide: &Wide, (sorted_by, aggs): (&[usize], &[AggCall])) -> Result<Vec<Row>> {
        let (owns, lying): (Vec<&GroupTable>, Arc<Vec<Lying>>) = (wide.owns(), Arc::new(wide.lying()));
        let (mut held, mut bases, mut base) = (Vec::with_capacity(owns.len()), Vec::with_capacity(owns.len()), 0_usize);
        let judge = self.judge();
        for own in &owns {
            held.extend(own_batch(own, sorted_by, base, &judge)?);
            bases.push(base);
            base = base.saturating_add(own.len());
        }
        let (lain, spans) = spans(&lying, base, self.filled);
        bases.extend(lain);
        let (from, by, calls) = (Arc::clone(&lying), sorted_by.to_vec(), aggs.to_vec());
        let rest = spans.into_iter().filter_map(move |span| lying_batch(&from, span, (&by, &calls), &judge).transpose());
        // No batch is of no row, nor of none kept, which the sort takes
        // for a morsel it cannot hold and sorts every row of.
        let batches = held.into_iter().map(Ok).chain(rest);
        let mut keys: Vec<SortKey> = self.own().iter().enumerate().map(|(at, key)| SortKey { column: at, ..*key }).collect();
        // The first rows' numbers, the column after the sort's own.
        keys.push(SortKey::ascending(keys.len()));
        let first = first_of_morsel(&keys, Box::new(batches), (self.most, self.bound.clone()), (&self.run.0, self.run.1))?;
        add(&self.counts.given, first.len());
        first.into_iter().map(|picked| self.made_of_wide((&owns, &lying), &bases, aggs, picked)).collect()
    }

    /// The projection's row of the group `picked` names last, a row of
    /// `picked_of_wide`'s batches, made of the table of `owns` or of the
    /// Made of `lying` its number falls in, by `bases`, with the group's
    /// first row's number after it.
    fn made_of_wide(&self, (owns, lying): (&[&GroupTable], &[Lying]), bases: &[usize], aggs: &[AggCall], mut picked: Row) -> Result<Row> {
        let group = picked.pop().as_ref().and_then(number_of).ok_or_else(|| broken("a picked group has no number"))?;
        let first = picked.pop().ok_or_else(|| broken("a picked group has no first row"))?;
        let k = bases.partition_point(|&base| base <= group).saturating_sub(1);
        let within = group.saturating_sub(bases.get(k).copied().unwrap_or(0));
        let whole = if let Some(own) = owns.get(k) {
            own.row(within)?
        } else {
            let lies = lying.get(k.saturating_sub(owns.len())).and_then(|(made, places)| places.get(within).map(|&place| (made, place)));
            let (made, place) = lies.ok_or_else(|| broken("a picked group lies in no Made"))?;
            made.row_at(place, aggs)?
        };
        let mut row = self.columns.iter().map(|pick| pick.value(&whole)).collect::<Result<Row>>()?;
        row.push(first);
        Ok(row)
    }
}
