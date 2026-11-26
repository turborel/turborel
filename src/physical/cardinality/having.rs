//! \[DESIGN\] The groups a comparison of a set function with a constant
//! keeps (RULES.md D1203), as a HAVING condition's is: from the
//! statistics of the column the function is of, where a default of a
//! third of the groups was taken whatever was compared.
//!
//! A grouping of n rows into G groups has m = n / G rows a group. A group
//! has one row at least, and the rows past it are taken to come as a
//! Poisson number of mean m - 1, each row's value of the column drawn
//! alike and apart from the others, as the estimator takes rows
//! everywhere. With F the column's distribution, as its statistics give
//! the share of its values a comparison keeps, and mu and s2 its mean and
//! variance:
//!
//! - MAX is at most c in F(c)^m of the groups, and MIN above c in
//!   (1 - F(c))^m, each of m values.
//! - COUNT(*) has mean m and variance m - 1.
//! - SUM, of a number of values of mean m and variance m - 1, has mean
//!   m mu and variance m s2 + (m - 1) mu^2.
//! - AVG has mean mu and variance s2 / m.
//!
//! The share past c of COUNT, SUM and AVG is the normal tail of those two
//! moments, Q((c - mean) / deviation), which the central limit theorem
//! gives a sum of values drawn alike. Of a column with nulls, m counts
//! the values that are not null, and COUNT of it has variance m.
//!
//! The mean and variance are those of the column's most common values,
//! each at its share, and of its histogram, whose buckets hold equal
//! shares, each spread evenly between its bounds; of no histogram, of its
//! values spread evenly between the least and the greatest.
//!
//! It is given only of a set function of a column alone, neither DISTINCT
//! nor filtered, whose table's statistics were gathered. One group at
//! least is kept. Any other comparison keeps the default.

use super::lineage::{Over, origin};
use super::{Estimates, MAX_ESTIMATE_DEPTH};
use crate::functions::ColumnarKind;
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, compared_with_constant};
use crate::stats::Statistics;
use crate::value::{CompOp, Value};

/// The coefficients of the rational approximation of the complementary
/// error function of Abramowitz and Stegun, "Handbook of Mathematical
/// Functions" (1964), formula 7.1.26, within 1.5e-7 of it: `P` and the
/// five of its polynomial.
const ERFC_P: f64 = 0.327_591_1;
/// See [`ERFC_P`].
const ERFC_A: [f64; 5] = [0.254_829_592, -0.284_496_736, 1.421_413_741, -1.453_152_027, 1.061_405_429];
/// The variance of values spread evenly over a width is the width squared
/// over this.
const EVEN_VARIANCE: f64 = 12.0;

/// The share of a normal distribution more than `z` deviations above its
/// mean: half the complementary error function of z over the root of 2.
pub(super) fn normal_tail(z: f64) -> f64 {
    let x = (z / std::f64::consts::SQRT_2).abs();
    let t = 1.0_f64 / (1.0_f64 + ERFC_P * x);
    let polynomial = ERFC_A.iter().rev().fold(0.0_f64, |sum, a| (sum + a) * t);
    let above = 0.5_f64 * polynomial * (-x * x).exp();
    if z < 0.0_f64 { 1.0_f64 - above } else { above }
}

/// A column's values as a number each: their mean, and the mean of their
/// squares.
#[derive(Debug, Clone, Copy)]
struct Moments {
    mean: f64,
    square: f64,
}

impl Moments {
    /// Their variance.
    fn variance(self) -> f64 {
        (self.square - self.mean * self.mean).max(0.0_f64)
    }
}

/// `v` as a number on a line.
fn number(v: &Value) -> Option<f64> {
    v.kind()?.position(v)
}

/// The moments of the values of `column`, as `stats` tells them, as the
/// module says: none if it tells neither a histogram nor bounds of
/// numbers.
fn moments(stats: &dyn Statistics, column: usize) -> Option<Moments> {
    let (mut share, mut mean, mut square) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (v, of) in stats.common(column).unwrap_or_default() {
        let x = number(v)?;
        (share, mean, square) = (share + of, mean + of * x, square + of * x * x);
    }
    let rest = (1.0_f64 - stats.nulls(column).unwrap_or(0.0_f64) - share).max(0.0_f64);
    let bounds = stats.histogram(column).filter(|b| b.len() > 1).or_else(|| stats.bounds(column).map(|(low, high)| vec![low, high]))?;
    let places = bounds.iter().map(number).collect::<Option<Vec<f64>>>()?;
    let each = rest / crate::storage::cost_of(places.len().saturating_sub(1)).max(1.0_f64);
    for (low, high) in places.iter().zip(places.iter().skip(1)) {
        let (middle, width) = ((low + high) / 2.0_f64, high - low);
        (mean, square) = (mean + each * middle, square + each * (middle * middle + width * width / EVEN_VARIANCE));
    }
    let all = share + rest;
    (all > 0.0_f64).then(|| Moments { mean: mean / all, square: square / all })
}

/// What a comparison is of: the set function, and the grouping it is
/// computed by.
struct Compared<'p> {
    agg: &'p AggCall,
    groups: &'p [usize],
    input: &'p LogicalPlan,
}

/// The set function `plan`'s column `column` is, through filters and
/// select lists of columns, if it is one of a grouping of no group made
/// before any row.
fn compared(plan: &LogicalPlan, column: usize) -> Option<Compared<'_>> {
    let (mut at, mut column) = (plan, column);
    for _ in 0..MAX_ESTIMATE_DEPTH {
        match at {
            LogicalPlan::Filter { input, .. } => at = input,
            LogicalPlan::Project { exprs, input } => {
                let Some(Expr::Column(c)) = exprs.get(column) else { return None };
                (at, column) = (input, *c);
            }
            LogicalPlan::Aggregate { groups, aggs, seeds, input, .. } => {
                let agg = aggs.get(column.checked_sub(groups.len())?)?;
                return seeds.is_empty().then_some(Compared { agg, groups, input });
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SemiJoin { .. }
            | LogicalPlan::MarkJoin(_)
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::Recognize(..)
            | LogicalPlan::Polymorphic(..) => return None,
        }
    }
    None
}

/// The share of a quantity of mean `mean` and variance `variance` that
/// "`op` `c`" keeps, by its normal tail; none of an equality.
fn tail((mean, variance): (f64, f64), op: CompOp, c: f64) -> Option<f64> {
    let deviation = variance.sqrt();
    let above = if deviation > 0.0_f64 { normal_tail((c - mean) / deviation) } else { f64::from(u8::from(mean > c)) };
    match op {
        CompOp::Gt | CompOp::Ge => Some(above),
        CompOp::Lt | CompOp::Le => Some(1.0_f64 - above),
        CompOp::Eq | CompOp::Ne => None,
    }
}

/// The share of groups of `m` values of a column an extreme of which,
/// the greatest if `greatest`, "`op` `value`" keeps, `kept` giving the
/// share of the column's values a comparison with `value` keeps.
fn extreme(greatest: bool, op: CompOp, m: f64, kept: &dyn Fn(CompOp) -> Option<f64>) -> Option<f64> {
    let all = |each: CompOp| kept(each).map(|share| share.clamp(0.0_f64, 1.0_f64).powf(m));
    match (greatest, op) {
        // Every value is within the bound.
        (true, CompOp::Le | CompOp::Lt) | (false, CompOp::Ge | CompOp::Gt) => all(op),
        // Not every value is on the other side of it.
        (true, CompOp::Gt) => all(CompOp::Le).map(|within| 1.0_f64 - within),
        (true, CompOp::Ge) => all(CompOp::Lt).map(|within| 1.0_f64 - within),
        (false, CompOp::Lt) => all(CompOp::Ge).map(|within| 1.0_f64 - within),
        (false, CompOp::Le) => all(CompOp::Gt).map(|within| 1.0_f64 - within),
        (_, CompOp::Eq | CompOp::Ne) => None,
    }
}

impl Estimates<'_> {
    /// The share of the groups the conjunct `c`, over `over`'s columns,
    /// keeps, if it compares a set function with a constant and the
    /// statistics tell, as the module says.
    pub(super) fn set_function_share(&self, c: &Cond, over: Over<'_>) -> Option<f64> {
        let Over::One(plan) = over else { return None };
        let (column, op, value) = compared_with_constant(c)?;
        let Compared { agg, groups, input } = compared(plan, column)?;
        if agg.distinct || agg.filter.is_some() || agg.distribution.is_some() || groups.is_empty() {
            return None;
        }
        let rows = self.card_at(input, 1).rows;
        let made = self.groups(input, groups, rows).max(1.0_f64);
        let m = (rows / made).max(1.0_f64);
        let kind = agg.func.0.columnar()?.0;
        let share = match &agg.arg {
            None => tail((m, m - 1.0_f64), op, number(value)?).filter(|_| kind == ColumnarKind::Count),
            Some(Expr::Column(of)) => self.column_share((kind, m), (input, *of), (op, value)),
            Some(_) => None,
        }?;
        Some(share.clamp(1.0_f64 / made, 1.0_f64))
    }

    /// The share of groups of `m` rows that "`kind` of column `of` of
    /// `input` `op` `value`" keeps, by the gathered statistics of the
    /// column's table.
    fn column_share(
        &self,
        (kind, m): (ColumnarKind, f64),
        (input, of): (&LogicalPlan, usize),
        (op, value): (CompOp, &Value),
    ) -> Option<f64> {
        let found = origin(input, of)?;
        let stats = self.statistics(found.table)?;
        stats.bounds(found.column)?;
        let held = 1.0_f64 - stats.nulls(found.column).unwrap_or(0.0_f64);
        let m = m * held;
        let kept = |each: CompOp| stats.selectivity(found.column, each, value).map(|share| share / held.max(f64::MIN_POSITIVE));
        let sized = |per: &dyn Fn(Moments) -> (f64, f64)| tail(per(moments(&*stats, found.column)?), op, number(value)?);
        match kind {
            ColumnarKind::Max => extreme(true, op, m, &kept),
            ColumnarKind::Min => extreme(false, op, m, &kept),
            ColumnarKind::Count => tail((m, m), op, number(value)?),
            ColumnarKind::Sum => sized(&|x| (m * x.mean, m * x.variance() + (m - 1.0_f64).max(0.0_f64) * x.mean * x.mean)),
            ColumnarKind::Avg => sized(&|x| (x.mean, x.variance() / m.max(1.0_f64))),
        }
    }
}

// Unit tests: each set function's share, and the comparisons left the
// default (RULES.md R-TEST-1, R-TEST-7).
#[cfg(test)]
mod tests;
