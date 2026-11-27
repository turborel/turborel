//! \[DESIGN\] (RULES.md D1124) The first rows of a sort over a grouping
//! run in parallel, taken from the partitions' tables. A sort of which
//! only the first `n` rows are asked for, over a projection of a
//! grouping's columns, read every group back as a row, made by the
//! partitions' threads and merged in order by the statement's, to keep
//! `n` of them. Each partition's thread now gives its table's groups as a
//! batch (`table/given.rs`), the columns the projection names in its
//! order, with each group's first row's number a column after them, and
//! keeps that batch's first `n` rows under the sort's keys and then that
//! number (`sort::first_of_morsel`). No row is made of any other group.
//! The statement's thread sorts the partitions' rows the same way and
//! gives the first `n`, the number left out.
//!
//! The plan run alone sorts the groups stably in the order of their first
//! rows, so of two groups of equal keys it gives the one whose first row
//! came earlier. A group's first row's number as the last key says the
//! same of any two groups, in whatever partition they are, so the rows
//! given and their order are the plan's. WITH TIES asks for the peers of
//! the last row under the sort's own keys, which the number would part,
//! so such a sort reads the grouping's rows as before.

use super::runs::{Ordered, Run};
use super::{Given, Grouping};
use crate::error::Result;
use crate::exec::sort::first_of_morsel;
use crate::exec::table::GroupTable;
use crate::exec::{Db, RowIter, broken};
use crate::logical::{Expr, Most, SortKey};
use crate::physical::Plan;
use crate::storage::{Batch, Column, Ints, Row};
use crate::txn::TxId;
use crate::value::Value;

/// What a projection over a grouping gives of a group's row at one
/// place: the row's column at this place, or a constant.
#[derive(Clone)]
enum Pick {
    Column(usize),
    Const(Value),
}

impl Pick {
    /// What `e`, an expression of a projection, picks, if it is a column
    /// or a constant.
    fn of(e: &Expr) -> Option<Pick> {
        if let Expr::Column(c) = e {
            return Some(Pick::Column(*c));
        }
        if let Expr::Const(v) = e { Some(Pick::Const(v.clone())) } else { None }
    }

    /// Whether it is the row's column at `c`.
    fn is_column(&self, c: usize) -> bool {
        if let Pick::Column(own) = self { *own == c } else { false }
    }

    /// Its value of `row`, a group's.
    fn value(&self, row: &Row) -> Result<Value> {
        match self {
            Pick::Column(c) => row.get(*c).cloned().ok_or_else(|| broken("a projected column past its group's row")),
            Pick::Const(v) => Ok(v.clone()),
        }
    }
}

/// The first rows asked of a grouping's groups: what the projection over
/// it gives of a group's row, in its order; the sort's keys, of those,
/// then the first row's number after them; and how many rows.
pub(in crate::exec::parallel) struct Top {
    columns: Vec<Pick>,
    keys: Vec<SortKey>,
    most: usize,
    run: (Db, TxId),
}

impl Top {
    /// The first rows a sort by `keys` asks, `most` of them, of `input`,
    /// and the part under it, if `input` is a grouping by hash run in
    /// parallel, or a projection of columns and constants alone over one,
    /// and the sort asks no peers.
    pub(in crate::exec::parallel) fn of<'p>(
        (keys, most): (&[SortKey], Most),
        input: &'p Plan,
        (db, tx): (&Db, TxId),
    ) -> Option<(Top, &'p Plan)> {
        let (asked, part) = if let Plan::Project { exprs, input: part, .. } = input {
            (Some(exprs.iter().map(Pick::of).collect::<Option<Vec<_>>>()?), &**part)
        } else {
            (None, input)
        };
        let Plan::Parallel(p) = part else { return None };
        let Plan::Aggregate { groups, aggs, streamed: false, .. } = &p.input else { return None };
        // Of no projection, every column of a group's row.
        let columns = asked.unwrap_or_else(|| (0..groups.len().saturating_add(aggs.len())).map(Pick::Column).collect());
        let mut keys = keys.to_vec();
        keys.push(SortKey::ascending(columns.len()));
        (!most.ties).then(|| (Top { columns, keys, most: most.rows, run: (db.clone(), tx) }, part))
    }

    /// The sort's own keys, the first row's number's left out.
    fn own(&self) -> &[SortKey] {
        self.keys.split_last().map_or(&[], |(_, own)| own)
    }

    /// How many rows are asked for.
    fn most(&self) -> Most {
        Most { rows: self.most, ties: false }
    }

    /// [X3.135-1986 §8.3 GR3b] The first rows of the groups of `table`,
    /// a partition's, under the keys and then their first rows' numbers:
    /// each the projection's row with that number after it.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`, or the sort, fails.
    pub(super) fn of_table(&self, table: GroupTable) -> Result<Vec<Row>> {
        let (batch, firsts) = table.into_batch()?;
        let rows = batch.rows;
        let mut held: Vec<Option<Column>> = batch.columns.into_iter().map(Some).collect();
        let mut columns = Vec::with_capacity(self.columns.len().saturating_add(1));
        for (at, pick) in self.columns.iter().enumerate() {
            let c = match pick {
                Pick::Column(c) => *c,
                Pick::Const(v) => {
                    columns.push(Column::Values(vec![v.clone(); rows]));
                    continue;
                }
            };
            // A column the projection gives again later is copied, the
            // last time moved.
            let again = self.columns.iter().skip(at.saturating_add(1)).any(|later| later.is_column(c));
            let slot = held.get_mut(c).ok_or_else(|| broken("a projected column past its group's row"))?;
            let column = if again { slot.clone() } else { slot.take() };
            columns.push(column.ok_or_else(|| broken("a projected column given twice"))?);
        }
        let numbers = firsts.iter().map(|&first| i64::try_from(first).unwrap_or(i64::MAX)).collect();
        columns.push(Column::Ints(Ints::new(numbers, 0, None)));
        let batches = Box::new(std::iter::once(Ok((Batch { columns, rows }, None))));
        first_of_morsel(&self.keys, batches, (self.most, None), (&self.run.0, self.run.1))
    }

    /// [X3.135-1986 §8.3 GR3b] The first rows of `rows`, the partitions'
    /// first rows each with its number last, under the keys and then the
    /// numbers, the number left out of each.
    fn of_all(&self, rows: Vec<Row>) -> RowIter {
        let (db, tx) = &self.run;
        let sorted = crate::exec::sort::first(&self.keys, Box::new(rows.into_iter().map(Ok)), self.most(), db, *tx);
        Box::new(sorted.map(|row| {
            row.map(|mut row| {
                row.pop();
                row
            })
        }))
    }
}

/// Whether `grouping` gives the first rows asked of it from its tables:
/// its groups are made by batch, so every partition is finished through
/// its table, and each group's row is the table's own, with no HAVING
/// condition nor second grouping over it.
pub(super) fn fits(grouping: &Grouping) -> bool {
    let plain = grouping.having.is_none() && grouping.outer.is_none();
    grouping.read.is_some() && plain && !grouping.distinct && !grouping.spec.groups.is_empty()
}

impl Top {
    /// [X3.135-1986 §8.3 GR3b] The first rows asked, of `runs` and the
    /// runs `given` back from the spill: of a grouping that took them from its tables (`fits`), the partitions'
    /// first rows; of any other, its groups in the order of their first
    /// rows, projected and sorted as the plan run alone sorts them.
    ///
    /// # Errors
    ///
    /// As reading back the runs `given` from the spill fails.
    pub(super) fn given(&self, from_tables: bool, (runs, given): (Vec<Run>, Vec<Given>)) -> Result<RowIter> {
        if from_tables {
            let mut rows: Vec<Row> = runs.into_iter().flat_map(Run::into_rows).map(|group| group.1).collect();
            for group in given.into_iter().flatten() {
                rows.push(group?.1);
            }
            return Ok(self.of_all(rows));
        }
        Ok(self.of_rows(Box::new(Ordered::of(runs, given)?)))
    }

    /// [X3.135-1986 §8.3 GR3b] The first rows asked, of `rows`, the
    /// grouping's in the order of their first rows: projected, then
    /// sorted by the sort's keys, stably.
    pub(in crate::exec::parallel) fn of_rows(&self, rows: RowIter) -> RowIter {
        let asked = self.columns.clone();
        let picked = move |row: Row| asked.iter().map(|pick| pick.value(&row)).collect();
        let (db, tx) = &self.run;
        crate::exec::sort::first(self.own(), Box::new(rows.map(move |row| row.and_then(&picked))), self.most(), db, *tx)
    }
}
