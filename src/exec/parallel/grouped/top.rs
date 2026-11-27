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
//!
//! \[DESIGN\] (RULES.md D1256) A sort whose every key names a set
//! function picks its groups before their rows are made. The partition
//! finishes those functions' columns alone, takes its first rows by them
//! and the first rows' numbers, and makes only the picked groups' rows of
//! its table. The other set functions are finished of every group for
//! the error one may raise, which the plan run alone would raise, and no
//! value of them is kept. A key that names a grouping column sorts every
//! column of the groups, as before.
//!
//! \[EXTENSION\] (RULES.md D378, D1254) An analyzed plan counts the
//! projection and the grouping, and the first rows are taken under those
//! counts as under none, so the statement analyzed runs as the statement
//! does. The grouping is counted the groups its tables made, and the
//! projection the rows the partitions kept of them for the sort.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::runs::{Ordered, Run};
use super::{Given, Grouping};
use crate::error::Result;
use crate::exec::batch::Kept;
use crate::exec::counted::Tally;
use crate::exec::sort::first_of_morsel;
use crate::exec::table::{GroupTable, Made, Wide};
use crate::exec::{Db, RowIter, broken};
use crate::logical::{AggCall, Expr, Most, SortKey};
use crate::physical::{NodeCount, Plan};
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

/// \[EXTENSION\] (RULES.md D378, D1254) What an analyzed plan counts of
/// the first rows taken from a grouping. It holds the count of the
/// operator over the part, a projection's, and the grouping's, if the
/// plan has them, and what the threads counted for each, the rows given
/// for the sort and the groups made.
#[derive(Default)]
struct Counts {
    projected: Option<Arc<NodeCount>>,
    grouped: Option<Arc<NodeCount>>,
    given: Arc<AtomicU64>,
    made: Arc<AtomicU64>,
}

/// `plan` under an analyzed plan's count of it, and that count.
fn uncounted(plan: &Plan) -> (&Plan, Option<Arc<NodeCount>>) {
    if let Plan::Counted(c) = plan { (&c.input, Some(Arc::clone(&c.count))) } else { (plan, None) }
}

/// The whole number `value` is, if it is one that counts groups.
fn number_of(value: &Value) -> Option<usize> {
    if let Value::Exact(d) = value { usize::try_from(d.units).ok() } else { None }
}

/// Adds `more` to `counted`.
fn add(counted: &AtomicU64, more: usize) {
    counted.fetch_add(u64::try_from(more).unwrap_or(u64::MAX), Ordering::Relaxed);
}

/// The first rows asked of a grouping's groups: what the projection over
/// it gives of a group's row, in its order; the sort's keys, of those,
/// then the first row's number after them; and how many rows.
pub(in crate::exec::parallel) struct Top {
    columns: Vec<Pick>,
    keys: Vec<SortKey>,
    most: usize,
    run: (Db, TxId),
    counts: Counts,
}

impl Top {
    /// The first rows a sort by `keys` asks, `most` of them, of `input`,
    /// with the grouping under it and its part's degree, if `input` is a
    /// grouping by hash run in parallel, or a projection of columns and
    /// constants alone over one, and the sort asks no peers. An analyzed
    /// plan's counts of the projection and of the grouping are passed
    /// and kept (RULES.md D1254).
    pub(in crate::exec::parallel) fn of<'p>(
        (keys, most): (&[SortKey], Most),
        input: &'p Plan,
        (db, tx): (&Db, TxId),
    ) -> Option<(Top, &'p Plan, usize)> {
        let (input, projected) = uncounted(input);
        let (asked, part) = if let Plan::Project { exprs, input: part, .. } = input {
            (Some(exprs.iter().map(Pick::of).collect::<Option<Vec<_>>>()?), &**part)
        } else {
            (None, input)
        };
        let Plan::Parallel(p) = part else { return None };
        let (grouping, grouped) = uncounted(&p.input);
        let Plan::Aggregate { groups, aggs, streamed: false, .. } = grouping else { return None };
        // Of no projection, every column of a group's row.
        let columns = asked.unwrap_or_else(|| (0..groups.len().saturating_add(aggs.len())).map(Pick::Column).collect());
        let mut keys = keys.to_vec();
        keys.push(SortKey::ascending(columns.len()));
        let counts = Counts { projected, grouped, ..Counts::default() };
        (!most.ties).then(|| (Top { columns, keys, most: most.rows, run: (db.clone(), tx), counts }, grouping, p.degree))
    }

    /// \[EXTENSION\] (RULES.md D378, D1254) The rows `run` gives, the
    /// first rows asked, counted as an analyzed plan counts them. The
    /// grouping's count takes the groups made and the count over the part
    /// the rows given for the sort, each with the time of `run` and of
    /// every row pulled. Of a plan that counts neither, `run`'s rows.
    pub(in crate::exec::parallel) fn counted(top: &Arc<Top>, run: impl FnOnce() -> RowIter) -> RowIter {
        let (db, counts) = (&top.run.0, &top.counts);
        if counts.projected.is_none() && counts.grouped.is_none() {
            return run();
        }
        let mut grouped = counts.grouped.as_ref().map(|count| Tally::opened(Arc::clone(count), db));
        let mut projected = counts.projected.as_ref().map(|count| Tally::opened(Arc::clone(count), db));
        let rows = timed((&mut grouped, &mut projected), run);
        Box::new(Counting { rows, top: Arc::clone(top), grouped, projected })
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
        add(&self.counts.made, table.len());
        if let Some(sorted_by) = self.sorted_by(table.width()) {
            return self.picked_of(&table, &sorted_by).map_err(|e| table.finished().err().unwrap_or(e));
        }
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
        let first = first_of_morsel(&self.keys, batches, (self.most, None), (&self.run.0, self.run.1))?;
        add(&self.counts.given, first.len());
        Ok(first)
    }

    /// \[DESIGN\] (RULES.md D1256) The set function each of the sort's own
    /// keys names, by its place among the grouping's, of a table whose
    /// keys are `width` columns, if every key names one. A key that names
    /// a grouping column or a constant is sorted by with every column of
    /// the groups made, as `of_table` makes them.
    fn sorted_by(&self, width: usize) -> Option<Vec<usize>> {
        let named = |key: &SortKey| match self.columns.get(key.column) {
            Some(Pick::Column(c)) => c.checked_sub(width),
            Some(Pick::Const(_)) | None => None,
        };
        self.own().iter().map(named).collect::<Option<Vec<usize>>>().filter(|by| !by.is_empty())
    }

    /// \[DESIGN\] (RULES.md D1256) The first rows of the groups of `table`
    /// picked before they are made. A batch holds the sort keys' columns
    /// alone, the set functions' results at `sorted_by`, with each group's
    /// first row's number and its own number after them. The first rows
    /// under the keys are taken of that, and only those groups' rows are
    /// made of the table and projected. Every other set function is
    /// finished of every group for the error it may raise, no value kept.
    fn picked_of(&self, table: &GroupTable, sorted_by: &[usize]) -> Result<Vec<Row>> {
        let rows = table.len();
        let mut columns = sorted_by.iter().map(|&agg| table.results_of(agg)).collect::<Result<Vec<Column>>>()?;
        table.checked_but(sorted_by)?;
        let narrow = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        columns.push(Column::Ints(Ints::new(
            table.firsts().iter().map(|&first| i64::try_from(first).unwrap_or(i64::MAX)).collect(),
            0,
            None,
        )));
        columns.push(Column::Ints(Ints::new((0..rows).map(narrow).collect(), 0, None)));
        let mut keys: Vec<SortKey> = self.own().iter().enumerate().map(|(at, key)| SortKey { column: at, ..*key }).collect();
        keys.push(SortKey::ascending(sorted_by.len()));
        let batches = Box::new(std::iter::once(Ok((Batch { columns, rows }, None))));
        let first = first_of_morsel(&keys, batches, (self.most, None), (&self.run.0, self.run.1))?;
        add(&self.counts.given, first.len());
        first.into_iter().map(|picked| self.made_of(table, picked)).collect()
    }

    /// The projection's row of the group `picked` names last, a row of
    /// `picked_of`'s batch, made of `table`, with the group's first row's
    /// number after it.
    fn made_of(&self, table: &GroupTable, mut picked: Row) -> Result<Row> {
        let group = picked.pop().as_ref().and_then(number_of).ok_or_else(|| broken("a picked group has no number"))?;
        let first = picked.pop().ok_or_else(|| broken("a picked group has no first row"))?;
        let whole = table.row(group)?;
        let mut row = self.columns.iter().map(|pick| pick.value(&whole)).collect::<Result<Row>>()?;
        row.push(first);
        Ok(row)
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
        let (made, given) = (Arc::clone(&self.counts.made), Arc::clone(&self.counts.given));
        let picked = move |row: Row| {
            // Every group is made a row and projected (RULES.md D1254).
            add(&made, 1);
            add(&given, 1);
            asked.iter().map(|pick| pick.value(&row)).collect()
        };
        let (db, tx) = &self.run;
        crate::exec::sort::first(self.own(), Box::new(rows.map(move |row| row.and_then(&picked))), self.most(), db, *tx)
    }
}

/// A wide partition's groups of one row where they lie: each Made that
/// holds any, with their places in it (`table/made/wide.rs`).
type Lying = Arc<Vec<(Arc<Made>, Vec<u32>)>>;

/// The numbers from `from` on, `count` of them, as a column.
fn numbers(from: usize, count: usize) -> Column {
    let narrow = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    Column::Ints(Ints::new((from..from.saturating_add(count)).map(narrow).collect(), 0, None))
}

/// \[DESIGN\] (RULES.md D1300, D1256) The batch a sort by the set
/// functions at `sorted_by`, of `aggs`, reads of the groups of one row
/// that lie in the `k`th Made of `lying`: those functions' results, each
/// group's first row's number, and its number among the partition's
/// groups read so, from `base` on. Every other set function is finished
/// of each group for the error it may raise, no value kept.
fn lying_batch(lying: &Lying, k: usize, (sorted_by, aggs): (&[usize], &[AggCall]), base: usize) -> Result<Kept> {
    let (made, places) = lying.get(k).ok_or_else(|| broken("a Made a wide partition does not hold"))?;
    let result = |agg: usize| {
        let call = aggs.get(agg).ok_or_else(|| broken("a set function a grouping does not compute"))?;
        made.results_at((agg, call), places)
    };
    let mut columns = sorted_by.iter().map(|&agg| result(agg)).collect::<Result<Vec<Column>>>()?;
    made.checked_but_at(sorted_by, aggs, places)?;
    columns.push(Column::Ints(Ints::new(made.firsts_at(places), 0, None)));
    columns.push(numbers(base, places.len()));
    Ok((Batch { columns, rows: places.len() }, None))
}

impl Top {
    /// \[DESIGN\] (RULES.md D1300) [X3.135-1986 §8.3 GR3b] The first rows
    /// of the groups of `wide`, a partition's adopted where they lie, of
    /// the set functions `aggs`, as `of_table` gives them of a table. Of a
    /// sort whose every key names a set function they are picked where
    /// the groups lie, the groups of several rows read of the partition's
    /// own table and those of one row a Made at a time, and only the
    /// groups picked are made rows. No key is copied. Any other sort
    /// reads the partition gathered into a table. An error is the one the
    /// groups raise given in their order, as a table's.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`, or the sort, fails.
    pub(super) fn of_wide(&self, wide: Wide, aggs: &[AggCall]) -> Result<Vec<Row>> {
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
    /// of the partition's own table and a batch a Made of the groups of
    /// one row, so as many rows are kept as of a table.
    fn picked_of_wide(&self, wide: &Wide, (sorted_by, aggs): (&[usize], &[AggCall])) -> Result<Vec<Row>> {
        let (table, lying): (&GroupTable, Lying) = (wide.own(), Arc::new(wide.lying()));
        let mut columns = sorted_by.iter().map(|&agg| table.results_of(agg)).collect::<Result<Vec<Column>>>()?;
        table.checked_but(sorted_by)?;
        let firsts = table.firsts().iter().map(|&first| i64::try_from(first).unwrap_or(i64::MAX)).collect();
        columns.push(Column::Ints(Ints::new(firsts, 0, None)));
        columns.push(numbers(0, table.len()));
        let mut bases = Vec::with_capacity(lying.len());
        let mut base = table.len();
        for (_, places) in lying.iter() {
            bases.push(base);
            base = base.saturating_add(places.len());
        }
        let (from, by, calls, starts) = (Arc::clone(&lying), sorted_by.to_vec(), aggs.to_vec(), bases.clone());
        let rest = starts.into_iter().enumerate().map(move |(k, start)| lying_batch(&from, k, (&by, &calls), start));
        // A batch of no row is not given, as the sort takes one for a morsel
        // it cannot hold and sorts every row.
        let held = (table.len() > 0).then(|| Ok((Batch { columns, rows: table.len() }, None)));
        let batches = held.into_iter().chain(rest);
        let mut keys: Vec<SortKey> = self.own().iter().enumerate().map(|(at, key)| SortKey { column: at, ..*key }).collect();
        keys.push(SortKey::ascending(sorted_by.len()));
        let first = first_of_morsel(&keys, Box::new(batches), (self.most, None), (&self.run.0, self.run.1))?;
        add(&self.counts.given, first.len());
        first.into_iter().map(|picked| self.made_of_wide(wide, (&lying, &bases), aggs, picked)).collect()
    }

    /// The projection's row of the group `picked` names last, a row of
    /// `picked_of_wide`'s batches, made of the partition's own table or
    /// of the Made of `lying` its number falls in, by `bases`, with the
    /// group's first row's number after it.
    fn made_of_wide(&self, wide: &Wide, (lying, bases): (&Lying, &[usize]), aggs: &[AggCall], mut picked: Row) -> Result<Row> {
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

    /// A sort, for tests, over a grouping of `width` grouping columns and
    /// `functions` set functions: every column of a group's row, the first
    /// `most` rows by the set function at `by`, the greatest first.
    #[cfg(test)]
    pub(super) fn by_function((width, functions): (usize, usize), (by, most): (usize, usize), (db, tx): (&Db, TxId)) -> Top {
        let columns: Vec<Pick> = (0..width.saturating_add(functions)).map(Pick::Column).collect();
        let key = SortKey { descending: true, ..SortKey::ascending(width.saturating_add(by)) };
        let keys = vec![key, SortKey::ascending(columns.len())];
        Top { columns, keys, most, run: (db.clone(), tx), counts: Counts::default() }
    }
}

/// `f`, timed by each tally there is, the grouping's within the
/// projection's, as an operator's time holds its input's.
fn timed<R>((grouped, projected): (&mut Option<Tally>, &mut Option<Tally>), f: impl FnOnce() -> R) -> R {
    let within = || if let Some(tally) = grouped { tally.timed(f) } else { f() };
    if let Some(tally) = projected { tally.timed(within) } else { within() }
}

/// \[EXTENSION\] (RULES.md D378, D1254) The first rows asked, each pull
/// timed for the counts of an analyzed plan, which take what the threads
/// counted once the rows are read and dropped.
struct Counting {
    rows: RowIter,
    top: Arc<Top>,
    grouped: Option<Tally>,
    projected: Option<Tally>,
}

impl Iterator for Counting {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let rows = &mut self.rows;
        timed((&mut self.grouped, &mut self.projected), || rows.next())
    }
}

impl Drop for Counting {
    fn drop(&mut self) {
        let counts = &self.top.counts;
        if let Some(tally) = &mut self.grouped {
            tally.rows(counts.made.load(Ordering::Relaxed));
        }
        if let Some(tally) = &mut self.projected {
            tally.rows(counts.given.load(Ordering::Relaxed));
        }
    }
}
