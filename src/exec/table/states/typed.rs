//! \[DESIGN\] (RULES.md D356) Columns of states of the built-in set
//! functions: COUNT's counts; SUM's, AVG's, MAX's and MIN's of integers of
//! one scale, a 128-bit total or best value and a count a group; SUM's and
//! AVG's of approximate numbers, a `Wide` sum and a count a group. Each is
//! fed only the vectors it holds; any other turns it into accumulators
//! (`states.rs`). A sum past 128 bits raises as SUM's accumulator does.

use crate::bytes::Reader;
use crate::error::{Code, Result, fail};
use crate::exec::Runner;
use crate::exec::batch::{Fed, Taken, eliminated};
use crate::functions::{
    Accumulator, ColumnarKind, NO_FLOATS, Summed, Wide, avg_acc, avg_finished, count_acc, count_finished, extreme_acc, plus_float, sum_acc,
    sum_finished,
};
use crate::logical::AggCall;
use crate::storage::{Batch, Column, Ints, Mask};
use crate::value::codec::value;
use crate::value::{DataType, Decimal, Value};

mod few;
mod merged;
mod written;

use super::super::Pending;
use super::{Boxed, columns_of};

/// Exact states: of which function, each group's total (of SUM and AVG)
/// or best value (of MAX and MIN), how many values it took, the scale of
/// its integers once one came, and the result's type.
pub(in crate::exec) struct Exact {
    kind: ColumnarKind,
    units: Vec<i128>,
    counts: Vec<i64>,
    scale: Option<u32>,
    result: DataType,
}

/// Approximate states: of a mean or a sum, each group's sum and how many
/// values it took, and the result's type.
pub(in crate::exec) struct Approx {
    mean: bool,
    sums: Vec<Wide>,
    counts: Vec<i64>,
    result: DataType,
}

/// A column of states of a built-in set function.
pub(in crate::exec) enum Typed {
    Count(Vec<i64>),
    Exact(Exact),
    Approx(Approx),
}

/// SQLCODE -900, as SUM raises of a sum on the way past 128 bits.
fn beyond<T>() -> Result<T> {
    fail(Code::Unsupported, "[DESIGN] a sum on the way needs more than the 128 bits this implementation sums in")
}

/// The numbers a state is fed of `take`: a column's integers or values,
/// of rows read by their place in the batch; an expression's evaluated by
/// vectors, by their place among those given; none of a column of nulls
/// alone.
#[derive(Clone, Copy)]
enum Source<'a> {
    Read(&'a Ints),
    Given(&'a Ints),
    Values(&'a [Value]),
    GivenValues(&'a [Value]),
    Const(&'a Value),
    Nulls,
}

/// Whether each value at `places` of `value` is a null or an exact number
/// of one scale, `scale` if one came before, which is set to it.
fn exact_scale<'v>(places: &[u32], value: impl Fn(usize) -> &'v Value, scale: &mut Option<u32>) -> bool {
    for &at in places {
        let v = value(crate::storage::mask::index(at));
        if let Value::Exact(d) = v {
            if *scale.get_or_insert(d.scale) != d.scale {
                return false;
            }
        } else if !v.is_null() {
            return false;
        }
    }
    true
}

impl Typed {
    /// A column of states of the built-in `kind` of result `result`, if
    /// one holds it.
    pub(super) fn of(kind: Option<ColumnarKind>, result: &DataType) -> Option<Typed> {
        let kind = kind?;
        if kind == ColumnarKind::Count {
            return Some(Typed::Count(Vec::new()));
        }
        let (exact, approx) = columns_of(kind, result);
        let (units, counts, sums) = (Vec::new(), Vec::new(), Vec::new());
        if exact {
            return Some(Typed::Exact(Exact { kind, units, counts, scale: None, result: result.clone() }));
        }
        approx.then(|| Typed::Approx(Approx { mean: kind == ColumnarKind::Avg, sums, counts, result: result.clone() }))
    }

    /// Adds a group's, fed nothing.
    pub(super) fn grow(&mut self) {
        match self {
            Typed::Count(counts) => counts.push(0),
            Typed::Exact(exact) => {
                exact.units.push(0);
                exact.counts.push(0);
            }
            Typed::Approx(approx) => {
                approx.sums.push(NO_FLOATS);
                approx.counts.push(0);
            }
        }
    }

    /// Feeds each row of `pending` to its group's state, of `batch`, as
    /// `take` says, of `lane` if evaluated by vectors, warnings heard by
    /// `runner`: whether it could, false of values it does not hold.
    pub(super) fn feed(
        &mut self,
        pending: &Pending,
        (batch, take, lane): (&Batch, &Taken, Option<&Fed<'_>>),
        runner: &Runner,
    ) -> Result<bool> {
        match self {
            Typed::Count(counts) => Ok(counted(counts, pending, (batch, take, lane), runner)),
            Typed::Exact(exact) => {
                let Some(source) = source_of(batch, take, lane) else { return Ok(false) };
                exact.feed(pending, source, runner)
            }
            Typed::Approx(approx) => Ok(approx.feed(pending, (batch, take), runner)),
        }
    }

    /// Merges group `i`'s of `other` into group `g`'s, if both are of one
    /// kind and scale: whether they were.
    pub(super) fn merge_from(&mut self, g: usize, other: &Typed, i: usize) -> Result<bool> {
        match (self, other) {
            (Typed::Count(counts), Typed::Count(theirs)) => {
                add_count(counts, g, theirs.get(i).copied().unwrap_or(0));
                Ok(true)
            }
            (Typed::Exact(exact), Typed::Exact(theirs)) => exact.merge_from(g, theirs, i),
            (Typed::Count(_) | Typed::Exact(_) | Typed::Approx(_), _) => Ok(false),
        }
    }

    /// Merges `state`, as the accumulator writes it, into group `g`'s, if
    /// it holds it: whether it did.
    pub(super) fn merge_written(&mut self, g: usize, state: &[u8]) -> Result<bool> {
        match self {
            Typed::Count(counts) => {
                let n = <[u8; 16]>::try_from(state).map(i128::from_le_bytes);
                let n = n.or_else(|_| fail(Code::Corrupt, "[DESIGN] a state of COUNT is not 16 bytes"))?;
                add_count(counts, g, i64::try_from(n).unwrap_or(i64::MAX));
                Ok(true)
            }
            Typed::Exact(exact) => exact.merge_written(g, state),
            Typed::Approx(_) => Ok(false),
        }
    }

    /// Group `g`'s accumulator, as it would be had it been given the same
    /// values.
    fn boxed_of(&self, g: usize) -> Box<dyn Accumulator> {
        match self {
            Typed::Count(counts) => count_acc(i128::from(counts.get(g).copied().unwrap_or(0))),
            Typed::Exact(exact) => exact.boxed_of(g),
            Typed::Approx(approx) => approx.boxed_of(g),
        }
    }

    /// Each group's accumulator, as it would be, of the function and
    /// result of `agg`.
    pub(super) fn to_boxed(&self, agg: &AggCall) -> Boxed {
        self.to_boxed_by(agg, |acc| acc)
    }

    /// As `to_boxed`, each accumulator given to `wrap` first: a part's,
    /// which holds its function's (RULES.md D1186).
    pub(super) fn to_boxed_by(&self, agg: &AggCall, wrap: impl Fn(Box<dyn Accumulator>) -> Box<dyn Accumulator>) -> Boxed {
        Boxed::made((0..self.len()).map(|g| wrap(self.boxed_of(g))).collect(), agg)
    }

    /// How many groups.
    fn len(&self) -> usize {
        match self {
            Typed::Count(counts) => counts.len(),
            Typed::Exact(exact) => exact.counts.len(),
            Typed::Approx(approx) => approx.counts.len(),
        }
    }

    /// Group `g`'s result, as its accumulator finishes it.
    ///
    /// # Errors
    ///
    /// As the accumulator's `finish`.
    pub(super) fn finish(&self, g: usize) -> Result<Value> {
        match self {
            Typed::Count(counts) => count_finished(i128::from(counts.get(g).copied().unwrap_or(0))),
            Typed::Exact(exact) => exact.finish(g),
            Typed::Approx(approx) => approx.finish(g),
        }
    }

    /// Keeps the first `len` groups'.
    pub(super) fn truncate(&mut self, len: usize) {
        match self {
            Typed::Count(counts) => {
                counts.truncate(len);
                counts.shrink_to_fit();
            }
            Typed::Exact(exact) => {
                exact.units.truncate(len);
                exact.units.shrink_to_fit();
                exact.counts.truncate(len);
                exact.counts.shrink_to_fit();
            }
            Typed::Approx(approx) => {
                approx.sums.truncate(len);
                approx.sums.shrink_to_fit();
                approx.counts.truncate(len);
                approx.counts.shrink_to_fit();
            }
        }
    }

    /// The bytes it holds.
    pub(in crate::exec) fn bytes(&self) -> usize {
        let count = size_of::<i64>();
        match self {
            Typed::Count(counts) => counts.capacity().saturating_mul(count),
            Typed::Exact(exact) => exact.units.capacity().saturating_mul(size_of::<i128>().saturating_add(count)),
            Typed::Approx(approx) => approx.sums.capacity().saturating_mul(size_of::<Wide>().saturating_add(count)),
        }
    }
}

/// Adds `n` to group `g`'s count of `counts`.
fn add_count(counts: &mut [i64], g: usize, n: i64) {
    if let Some(count) = counts.get_mut(g) {
        *count = count.saturating_add(n);
    }
}

/// The integers an exact state takes of `take` of `batch`, of `lane` if
/// evaluated by vectors, if they are integers or nulls alone.
fn source_of<'a>(batch: &'a Batch, take: &Taken, lane: Option<&'a Fed<'_>>) -> Option<Source<'a>> {
    match (take, lane) {
        (Taken::Column(c), _) => match batch.columns.get(*c) {
            Some(Column::Ints(ints)) => Some(Source::Read(ints)),
            Some(Column::Values(values)) => Some(Source::Values(values)),
            Some(Column::Nulls(_)) => Some(Source::Nulls),
            Some(Column::Floats(_) | Column::Texts(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Late(_)) | None => None,
        },
        (Taken::Expr(_), Some(Fed::Made(column))) => match column {
            Column::Ints(ints) => Some(Source::Given(ints)),
            Column::Values(values) => Some(Source::GivenValues(values)),
            Column::Nulls(_) => Some(Source::Nulls),
            Column::Floats(_) | Column::Texts(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Late(_) => None,
        },
        (Taken::Expr(_), Some(Fed::Const(v))) => Some(Source::Const(v)),
        (Taken::Expr(_), None) | (Taken::Count | Taken::Kept(_), _) => None,
    }
}

/// Whether row `n` of `column` is null, or past its end.
fn null_in(column: &Column, n: usize) -> bool {
    match column {
        Column::Values(values) => values.get(n).is_none_or(Value::is_null),
        Column::Ints(ints) => n >= ints.units.len() || ints.is_null(n),
        Column::Floats(floats) => floats.get(n).is_none(),
        Column::Texts(texts) => texts.get(n).is_none(),
        Column::Datetimes(datetimes) => datetimes.get(n).is_none(),
        Column::Bools(bools) => bools.get(n).is_none(),
        Column::Nulls(_) => true,
        Column::Late(late) => late.made().is_none_or(|made| null_in(made, n)),
    }
}

/// Whether row `n` of a vector of these `nulls` is null.
fn null_at(nulls: Option<&Mask>, n: usize) -> bool {
    nulls.is_some_and(|marks| marks.get(n))
}

/// Counts each row of `pending` of `batch` its group's state takes, as
/// `take` says, of `lane` if evaluated by vectors, a null eliminated heard
/// by `runner`: whether it could, false of an expression evaluated a row at
/// a time.
fn counted(counts: &mut [i64], pending: &Pending, (batch, take, lane): (&Batch, &Taken, Option<&Fed<'_>>), runner: &Runner) -> bool {
    // [X3.135-1986 §5.8 GR4b] COUNT(*) of few groups, by lanes (`few.rs`).
    if let Taken::Count = take
        && few::counted(counts, &pending.groups)
    {
        return true;
    }
    let rows = pending.read.iter().zip(&pending.given).map(|(&r, &n)| (crate::storage::mask::index(r), crate::storage::mask::index(n)));
    let null: Box<dyn Fn(usize, usize) -> bool + '_> = match (take, lane) {
        // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
        (Taken::Count, _) => Box::new(|_, _| false),
        // [X3.135-1986 §5.8 GR1] nulls taken out.
        (Taken::Column(c), _) => Box::new(move |r, _| batch.cell(*c, r).is_null()),
        (Taken::Expr(_), Some(Fed::Made(column))) => Box::new(move |_, n| null_in(column, n)),
        (Taken::Expr(_), Some(Fed::Const(v))) => Box::new(move |_, _| v.is_null()),
        (Taken::Expr(_), None) | (Taken::Kept(_), _) => return false,
    };
    let mut any = false;
    for ((r, n), &g) in rows.zip(&pending.groups) {
        if null(r, n) {
            any = true;
        } else {
            add_count(counts, crate::storage::mask::index(g), 1);
        }
    }
    eliminated(any, runner);
    true
}

impl Exact {
    /// Feeds each row of `pending` its integer of `source`, warnings heard
    /// by `runner`: whether it could, false of integers of another scale.
    fn feed(&mut self, pending: &Pending, source: Source<'_>, runner: &Runner) -> Result<bool> {
        let (ints, places) = match source {
            Source::Read(ints) => (ints, &pending.read),
            Source::Given(ints) => (ints, &pending.given),
            Source::Values(values) => {
                return self.feed_values(|at| values.get(at).unwrap_or(&Value::Null), (&pending.read, pending), runner);
            }
            Source::GivenValues(values) => {
                return self.feed_values(|at| values.get(at).unwrap_or(&Value::Null), (&pending.given, pending), runner);
            }
            Source::Const(v) => return self.feed_values(|_| v, (&pending.given, pending), runner),
            Source::Nulls => {
                eliminated(!pending.groups.is_empty(), runner);
                return Ok(true);
            }
        };
        if *self.scale.get_or_insert(ints.scale) != ints.scale {
            return Ok(false);
        }
        // Of sums of few groups and no null, by lanes (`few.rs`).
        let sums = self.kind == ColumnarKind::Sum || self.kind == ColumnarKind::Avg;
        if sums
            && ints.nulls.is_none()
            && let Some(over) = few::summed(&mut self.units, &mut self.counts, (&ints.units, places, &pending.groups))
        {
            return if over { beyond() } else { Ok(true) };
        }
        let mut any = false;
        for (&at, &g) in places.iter().zip(&pending.groups) {
            let at = crate::storage::mask::index(at);
            if null_at(ints.nulls.as_ref(), at) {
                any = true;
                continue;
            }
            self.take(crate::storage::mask::index(g), i128::from(ints.units.get(at).copied().unwrap_or(0)), 1)?;
        }
        eliminated(any, runner);
        Ok(true)
    }

    /// Feeds each row of `pending` its value of `value` at its place of
    /// `places`, warnings heard by `runner`: whether it could, false, fed
    /// none, if any is not a null or an exact number of the scale.
    fn feed_values<'v>(
        &mut self,
        value: impl Fn(usize) -> &'v Value,
        (places, pending): (&[u32], &Pending),
        runner: &Runner,
    ) -> Result<bool> {
        let mut scale = self.scale;
        if !exact_scale(places, &value, &mut scale) {
            return Ok(false);
        }
        self.scale = scale;
        let mut any = false;
        for (&at, &g) in places.iter().zip(&pending.groups) {
            if let Value::Exact(d) = value(crate::storage::mask::index(at)) {
                self.take(crate::storage::mask::index(g), d.units, 1)?;
            } else {
                any = true;
            }
        }
        eliminated(any, runner);
        Ok(true)
    }

    /// Takes into group `g`'s `units` of `count` values: added, of a sum or
    /// mean; kept if better, of an extreme, a tie keeping the one first.
    fn take(&mut self, g: usize, units: i128, count: i64) -> Result<()> {
        let (Some(total), Some(held)) = (self.units.get_mut(g), self.counts.get_mut(g)) else { return Ok(()) };
        let better = match self.kind {
            ColumnarKind::Sum | ColumnarKind::Avg | ColumnarKind::Count => {
                *total = total.checked_add(units).map_or_else(beyond, Ok)?;
                false
            }
            ColumnarKind::Min => *held == 0 || units < *total,
            ColumnarKind::Max => *held == 0 || units > *total,
        };
        if better {
            *total = units;
        }
        *held = held.saturating_add(count);
        Ok(())
    }

    /// Merges group `i`'s of `other` into group `g`'s, if both are of one
    /// function and scale: whether they were.
    fn merge_from(&mut self, g: usize, other: &Exact, i: usize) -> Result<bool> {
        let held = other.counts.get(i).copied().unwrap_or(0);
        if held == 0 {
            return Ok(other.kind == self.kind);
        }
        let scale = *self.scale.get_or_insert(other.scale.unwrap_or(0));
        if other.kind != self.kind || other.scale != Some(scale) {
            return Ok(false);
        }
        self.take(g, other.units.get(i).copied().unwrap_or(0), held)?;
        Ok(true)
    }

    /// Merges `state`, as the accumulator writes it, into group `g`'s, if
    /// its value is an exact number of the scale: whether it did.
    fn merge_written(&mut self, g: usize, state: &[u8]) -> Result<bool> {
        let (count, rest) = match self.kind {
            ColumnarKind::Avg => match state.split_first_chunk::<16>() {
                Some((count, rest)) => (i64::try_from(i128::from_le_bytes(*count)).unwrap_or(i64::MAX), rest),
                None => return fail(Code::Corrupt, "[DESIGN] a state of AVG is short"),
            },
            ColumnarKind::Sum | ColumnarKind::Min | ColumnarKind::Max | ColumnarKind::Count => (1, state),
        };
        let mut reader = Reader::new(rest, "a state of a set function");
        let written = value(&mut reader)?;
        reader.finish()?;
        let d = match written {
            Value::Null => return Ok(true),
            Value::Exact(d) => d,
            Value::Char(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Lob(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Json(_)
            | Value::Xml(_)
            | Value::Extension(_) => return Ok(false),
        };
        if *self.scale.get_or_insert(d.scale) != d.scale {
            return Ok(false);
        }
        self.take(g, d.units, count)?;
        Ok(true)
    }

    /// Group `g`'s total or best value, if it took one.
    fn summed(&self, g: usize) -> Summed {
        let held = self.counts.get(g).is_some_and(|&n| n > 0);
        match (held, self.units.get(g)) {
            (true, Some(&units)) => Summed::Exact(Decimal::new(units, self.scale.unwrap_or(0))),
            (true, None) | (false, _) => Summed::Empty,
        }
    }

    /// Group `g`'s accumulator, as it would be.
    fn boxed_of(&self, g: usize) -> Box<dyn Accumulator> {
        let count = i128::from(self.counts.get(g).copied().unwrap_or(0));
        let best = || if let Summed::Exact(d) = self.summed(g) { Value::Exact(d) } else { Value::Null };
        match self.kind {
            ColumnarKind::Sum | ColumnarKind::Count => sum_acc(self.summed(g), self.result.clone()),
            ColumnarKind::Avg => avg_acc(self.summed(g), count, self.result.clone()),
            ColumnarKind::Min => extreme_acc(false, best(), &self.result),
            ColumnarKind::Max => extreme_acc(true, best(), &self.result),
        }
    }

    /// Group `g`'s result, as its accumulator finishes it.
    fn finish(&self, g: usize) -> Result<Value> {
        let count = i128::from(self.counts.get(g).copied().unwrap_or(0));
        match (self.kind, self.summed(g)) {
            (ColumnarKind::Sum | ColumnarKind::Count, summed) => sum_finished(summed, &self.result),
            (ColumnarKind::Avg, summed) => avg_finished(summed, count, &self.result),
            (ColumnarKind::Min | ColumnarKind::Max, Summed::Exact(d)) => Ok(Value::Exact(d)),
            (ColumnarKind::Min | ColumnarKind::Max, Summed::Empty | Summed::Approx(_)) => Ok(Value::Null),
        }
    }
}

impl Approx {
    /// Feeds each row of `pending` its number of the column `take` names of
    /// `batch`, in order, warnings heard by `runner`: whether it could,
    /// false of any vector but numbers or nulls.
    fn feed(&mut self, pending: &Pending, (batch, take): (&Batch, &Taken), runner: &Runner) -> bool {
        let Taken::Column(c) = take else { return false };
        match batch.columns.get(*c) {
            Some(Column::Floats(floats)) => self.feed_numbers(pending, |r| floats.get(r), runner),
            Some(Column::Values(values)) => {
                let number = |r: usize| if let Some(Value::Approx(f)) = values.get(r) { Some(*f) } else { None };
                let fits = pending.read.iter().all(|&r| {
                    values
                        .get(crate::storage::mask::index(r))
                        .is_none_or(|v| v.is_null() || number(crate::storage::mask::index(r)).is_some())
                });
                fits && self.feed_numbers(pending, number, runner)
            }
            Some(Column::Nulls(_)) => {
                eliminated(!pending.groups.is_empty(), runner);
                true
            }
            Some(Column::Ints(_) | Column::Texts(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Late(_)) | None => false,
        }
    }

    /// Feeds each row of `pending` its number of `number` at the row's
    /// place, none of a null, in order, warnings heard by `runner`.
    fn feed_numbers(&mut self, pending: &Pending, number: impl Fn(usize) -> Option<f64>, runner: &Runner) -> bool {
        let mut any = false;
        for (&r, &g) in pending.read.iter().zip(&pending.groups) {
            let Some(f) = number(crate::storage::mask::index(r)) else {
                any = true;
                continue;
            };
            let g = crate::storage::mask::index(g);
            if let (Some(sum), Some(count)) = (self.sums.get_mut(g), self.counts.get_mut(g)) {
                *sum = plus_float(*sum, f);
                *count = count.saturating_add(1);
            }
        }
        eliminated(any, runner);
        true
    }

    /// Group `g`'s sum, if it took a number.
    fn summed(&self, g: usize) -> Summed {
        let held = self.counts.get(g).is_some_and(|&count| count > 0);
        match (held, self.sums.get(g)) {
            (true, Some(&sum)) => Summed::Approx(sum),
            (true, None) | (false, _) => Summed::Empty,
        }
    }

    /// Group `g`'s accumulator, as it would be.
    fn boxed_of(&self, g: usize) -> Box<dyn Accumulator> {
        let count = i128::from(self.counts.get(g).copied().unwrap_or(0));
        if self.mean { avg_acc(self.summed(g), count, self.result.clone()) } else { sum_acc(self.summed(g), self.result.clone()) }
    }

    /// Group `g`'s result, as its accumulator finishes it.
    fn finish(&self, g: usize) -> Result<Value> {
        let count = i128::from(self.counts.get(g).copied().unwrap_or(0));
        if self.mean { avg_finished(self.summed(g), count, &self.result) } else { sum_finished(self.summed(g), &self.result) }
    }
}
