//! \[DESIGN\] (RULES.md D1279) The distinct values of an expression, as
//! the sample of the one table it reads shows them. A grouping by a
//! deterministic expression has no more groups than the columns it reads
//! take values together (D1082), which says nothing of what the
//! expression does to them. A time less its seconds, a number divided,
//! the first characters of a string and a field of a date each take far
//! fewer. No shape of expression is looked for. The expression is
//! evaluated on the rows of the sample its table's statistics keep
//! (D382), and two things are read of the values.
//!
//! The first is how many differ, and how many of those one row alone
//! has, counted up to the table by the estimator GEE (Charikar,
//! Chaudhuri, Motwani and Narasayya, "Towards estimation error
//! guarantees for distinct values", PODS 2000). It counts the values
//! seen more than once as they are, and each value seen once as the
//! square root of the table's rows over the sample's. A value seen once
//! stands for the values the sample missed, of which there may be as few
//! as itself or as many as the rows it stands for, and the root is the
//! mean of the two in ratio. So the count is expected to err by about
//! that root at most either way, and the paper shows that no count from
//! a sample is sure of less. The estimator the statistics count a column
//! by (`stats.rs`) takes the values as equally common, and of a sample
//! this small it falls short by far more of an expression a few of whose
//! values hold most rows, where a count of groups is relied on to err
//! high (D1278).
//!
//! GEE counts no more than the root of the sample's rows times the
//! table's, which it gives when every value is seen once, and of more
//! values than that it falls short. So its count is taken only where at
//! most half of the sampled rows hold a value seen once. Of values
//! equally common that is where they are no more than the sampled rows
//! over the natural logarithm of 2, about the sample's own size and a
//! half, and there GEE errs high or not at all. Past it the sample holds
//! less than a row a value and could only guess, so it tells no count,
//! and the lattice or the columns' bound stands.
//!
//! The second, of exact numbers, datetimes or intervals, is the lattice
//! they lie on. Its step is the greatest that all of them are whole steps
//! apart by, and it has (greatest - least) / step + 1 places between the
//! least and the greatest of them.
//!
//! The lesser of the two is what the sample shows, a null one value more
//! of the lattice's. It shows nothing, and the columns' bound stands
//! alone, where no statistics or no sample is kept, where the expression
//! reads the columns of more than one table or cannot be evaluated on a
//! row alone (`estimate::evaluable_expr`), where no sampled row gave a
//! value, where a column it reads may have been cut in the sample
//! (`may_be_cut`), where it tells neither a count nor a lattice, and
//! where no two rows that differ in the columns read
//! gave one value. In the last case the sample shows no values merged,
//! and of the columns' own values the statistics know more than its rows
//! do. A row whose evaluation raises is left out and nothing is raised,
//! as planning raises nothing of what the statement evaluates (D15).
//!
//! Nothing is asked of a plan of no more rows than the sample has, which
//! evaluating the sample would cost more than its own rows do, so a
//! statement of few rows plans as before. An answer is kept for the
//! statement (`Seen::shown`), so each expression is evaluated once however
//! many plans are weighed, and it is the same for the same statistics
//! whenever it is asked.

use std::hash::{BuildHasher, Hash, Hasher};

use super::Estimates;
use super::lineage::{Computed, Over, one_table};
use crate::exec::{NoSubqueries, eval};
use crate::hash::xxhash::{Xxh3BuildHasher, Xxh3Map, Xxh3Set};
use crate::logical::{Expr, map_expr_columns};
use crate::physical::estimate::{evaluable_expr, key_of_expr};
use crate::stats::parts::count;
use crate::stats::sample::{ROW_CHARACTERS, ROW_OCTETS};
use crate::storage::{Row, cost_of};
use crate::value::datetime::Moment;
use crate::value::{NormKey, Value};

/// The question the statement remembers an answer by.
const QUESTION: &str = "shown";

impl Estimates<'_> {
    /// The distinct values the sample of the one table `computed` reads
    /// shows of it, if it shows any, `whole` being the rows of its plan.
    pub(super) fn shown_distinct(&self, computed: &Computed<'_>, whole: f64) -> Option<f64> {
        let (table, places) = one_table(Over::One(computed.input), &computed.read)?;
        let stats = self.statistics(table)?;
        let rows = stats.sample()?.rows();
        if whole <= cost_of(rows.len()) {
            return None;
        }
        let key = key_of_expr(QUESTION, computed.expr, &|h| (table, &places).hash(h));
        self.seen().shown(key, computed.expr, || {
            let of_table = |c: usize| places.iter().find(|&&(at, _)| at == c).map_or(c, |&(_, place)| place);
            let local = map_expr_columns(computed.expr.clone(), &of_table);
            let read: Vec<usize> = places.iter().map(|&(_, place)| place).collect();
            let of = self.table_rows(table).or_else(|| stats.rows()).unwrap_or(0.0_f64);
            evaluable_expr(&local, rows.first().map_or(0, Vec::len)).then(|| shown(&local, &read, rows, of)).flatten()
        })
    }
}

/// The distinct values `rows`, a sample of a table of `of` rows, show of
/// `e`, which reads their columns `read`, if they show any.
pub(super) fn shown(e: &Expr, read: &[usize], rows: &[Row], of: f64) -> Option<f64> {
    let mut inputs = Xxh3Set::<u64>::with_capacity_and_hasher(rows.len(), Xxh3BuildHasher::default());
    let mut values = Xxh3Map::<NormKey, u32>::with_capacity_and_hasher(rows.len(), Xxh3BuildHasher::default());
    let (mut lattice, mut sampled) = (Lattice::default(), 0_usize);
    for row in rows {
        if read.iter().any(|&c| row.get(c).is_none_or(may_be_cut)) {
            return None;
        }
        // A row that raises is one the statement fails at if it reads it.
        let Ok(value) = eval(e, row, &NoSubqueries) else { continue };
        sampled = sampled.saturating_add(1);
        inputs.insert(read_key(read, row));
        let times = values.entry(value.norm()).or_insert(0);
        *times = times.saturating_add(1);
        lattice.add(&value);
    }
    if values.len() >= inputs.len() {
        return None;
    }
    let once = values.values().filter(|&&times| times == 1).count();
    // Past its own size the sample counts nothing (see the module).
    let counted = (once.saturating_add(once) <= sampled).then(|| guaranteed((cost_of(values.len()), cost_of(once)), cost_of(sampled), of));
    match (counted, lattice.places()) {
        (Some(counted), Some(places)) => Some(counted.min(places)),
        (Some(told), None) | (None, Some(told)) => Some(told),
        (None, None) => None,
    }
}

/// The hash of `row`'s values of the columns `read`, by XXH3, values
/// identical by the standard hashed alike (`Value::norm`). The rows that
/// differ in the columns read are counted by it, with no copy of a row
/// kept. Two rows of one hash are counted as one, which can only hide a
/// merging and leave the bound to the columns.
fn read_key(read: &[usize], row: &Row) -> u64 {
    let mut h = Xxh3BuildHasher::default().build_hasher();
    for &c in read {
        row.get(c).map(Value::norm).hash(&mut h);
    }
    h.finish()
}

/// \[DESIGN\] The distinct values of `of` values by the estimator GEE,
/// which the module describes, from a sample of `sampled` of them
/// holding `distinct` distinct ones, `once` of them seen once. A sample
/// that is the whole holds them all.
pub(super) fn guaranteed((distinct, once): (f64, f64), sampled: f64, of: f64) -> f64 {
    let each = (of / sampled.max(1.0_f64)).max(1.0_f64);
    each.sqrt() * once + (distinct - once)
}

/// Whether a sampled row may hold `value` cut short of what its table
/// holds (`stats/sample.rs`). That is a character string of
/// `ROW_CHARACTERS` octets or more, which holds as many characters at
/// most, a binary string of `ROW_OCTETS`, or a large object, kept empty.
/// An expression of such a column is not that of the column whole.
fn may_be_cut(value: &Value) -> bool {
    match value {
        Value::Char(s) => s.len() >= ROW_CHARACTERS,
        Value::Binary(b) => b.len() >= ROW_OCTETS,
        Value::Lob(_) => true,
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => false,
    }
}

/// The line the values of a type lie on, in whole units of it. Exact
/// numbers of one scale lie on one, datetimes of one kind on one in
/// microseconds, and intervals on one in months or in microseconds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Exact(u32),
    Datetime(Moment),
    Months,
    Micros,
}

/// `value`'s line and its place there, if it lies on one.
fn placed(value: &Value) -> Option<(Line, i128)> {
    match value {
        Value::Exact(d) => Some((Line::Exact(d.scale), d.units)),
        Value::Datetime(d) => Some((Line::Datetime(d.moment), i128::from(d.utc))),
        Value::Interval(i) => Some((if i.qualifier.year_month() { Line::Months } else { Line::Micros }, i.units)),
        Value::Null
        | Value::Char(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => None,
    }
}

/// The lattice the values added lie on. It keeps their line, the first
/// of them, the least and the greatest, and the greatest step every one
/// is whole steps from the first by, nothing if all are one.
#[derive(Default)]
struct Lattice {
    line: Option<Line>,
    first: i128,
    least: i128,
    greatest: i128,
    step: u128,
    nulls: bool,
    /// Whether a value lay on no line, or on another than the first's.
    off: bool,
}

impl Lattice {
    /// Adds `value`.
    fn add(&mut self, value: &Value) {
        if value.is_null() {
            self.nulls = true;
            return;
        }
        let Some((line, at)) = placed(value) else {
            self.off = true;
            return;
        };
        match self.line {
            None => (self.line, self.first, self.least, self.greatest) = (Some(line), at, at, at),
            Some(known) => self.off = self.off || known != line,
        }
        (self.least, self.greatest) = (self.least.min(at), self.greatest.max(at));
        self.step = common_step(self.step, at.abs_diff(self.first));
    }

    /// The places of the lattice between the least and the greatest
    /// value, and one more of a null, if every value lay on one line.
    fn places(&self) -> Option<f64> {
        self.line.filter(|_| !self.off)?;
        let steps = self.greatest.abs_diff(self.least).checked_div(self.step).unwrap_or(0);
        let places = steps.saturating_add(1).saturating_add(u128::from(self.nulls));
        Some(count(u64::try_from(places).unwrap_or(u64::MAX)))
    }
}

/// The greatest step both `a` and `b` are whole steps of, by Euclid's
/// algorithm. Of nothing and a distance it is the distance.
fn common_step(a: u128, b: u128) -> u128 {
    let (mut a, mut b) = (a, b);
    while let Some(rest) = a.checked_rem(b) {
        (a, b) = (b, rest);
    }
    a
}

// Unit tests of what a sample shows of an expression, and of where it
// shows nothing (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
