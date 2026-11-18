//! Distance ordering through certified index candidates (D15, D85), or
//! an approximate index's when the query asks for one (D94,
//! `nearest/approx.rs`). The final sort still compares exact scalar
//! results and secondary keys.

mod approx;
mod safe;

pub use approx::ApproximateRead;

use super::{PhysicalPlanner, Plan, estimate};
use crate::catalog::{QualifiedName, TableDesc};
use crate::functions::ScalarRef;
use crate::geometry::{SpatialOperation, fixed_srid};
use crate::index::{NearestQuery, NearestSupport};
use crate::logical::{Cond, Expr, LogicalPlan, SortKey, filled_expr, render_expr};
use crate::storage::cost_of;
use crate::value::Value;

/// A table read in increasing distance-bound order, with visible rows
/// refined by `measure`. It emits the table row followed by that result;
/// the final SQL sort decides ties and secondary ordering. An index that
/// cannot certify error-free pruning makes this a sequential read instead.
#[derive(Debug, Clone)]
pub struct NearestRead {
    /// The table read.
    pub table: QualifiedName,
    /// The index supplying candidates.
    pub index: String,
    /// \[EXTENSION\] The position in the index's key of the column it
    /// orders by: of a hybrid index's vector column, 1 (`index/vector/hybrid.rs`).
    pub position: usize,
    /// Its method, for EXPLAIN.
    pub method: String,
    /// The table columns read.
    pub columns: Vec<usize>,
    /// Whether the table row is followed by its row id before its distance.
    pub located: bool,
    /// The original row width, including that id if located.
    pub width: usize,
    /// The full distance call, evaluated once per qualifying row.
    pub measure: Expr,
    /// Whether it is a score, read greatest first (D87).
    pub descending: bool,
    /// The full WHERE condition, evaluated before ranking.
    pub condition: Option<Cond>,
    /// OFFSET plus FETCH count, excluding WITH TIES and PERCENT.
    pub first: usize,
    /// The measure and filter as text for EXPLAIN.
    pub detail: String,
    /// The estimated access cost.
    pub cost: f64,
    /// \[EXTENSION\] Of an approximate index's read, what it asks of it;
    /// none of a certified one (D94).
    pub approximate: Option<ApproximateRead>,
}

/// The column and its argument position in a declared distance call, or a
/// score's (`ScalarFunction::ranks`), whose arguments after its first two
/// are constants, as the index name the binder gives Rank (D87); or a
/// vector distance's (`ScalarFunction::vector_metric`), its other
/// arguments constants (D94).
pub(crate) fn distance_call(expr: &Expr) -> Option<(usize, usize, &ScalarRef, &Expr)> {
    let Expr::Call(function, args) = expr else { return None };
    if let Some((_, at)) = vector_measure(expr) {
        let (column, argument, other) = operands(args, at)?;
        return Some((column, argument, function, other));
    }
    let f = &function.0;
    let ranks = f.ranks();
    let distance = f.deterministic() && f.spatial_operation() == Some(SpatialOperation::Distance);
    let (two, rest) = args.split_at(args.len().min(2));
    let constants = rest.iter().all(|e| matches!(e, Expr::Const(_)));
    if !(distance || ranks) || !constants || (!rest.is_empty() && !ranks) {
        return None;
    }
    operands(two, 0).map(|(column, argument, other)| (column, argument, function, other))
}

/// The column, its position, and the other operand of the two at `at`
/// and after it, one of them a column.
fn operands(args: &[Expr], at: usize) -> Option<(usize, usize, &Expr)> {
    match (args.get(at)?, args.get(at.checked_add(1)?)?) {
        (Expr::Column(column), other) => Some((*column, at, other)),
        (other, Expr::Column(column)) => Some((*column, at.checked_add(1)?, other)),
        (_, _) => None,
    }
}

/// \[EXTENSION\] Of a deterministic vector distance whose arguments but
/// its two vectors are constants, its metric and its first vector's
/// position (`ScalarFunction::vector_metric`).
pub(crate) fn vector_measure(expr: &Expr) -> Option<(String, usize)> {
    let Expr::Call(function, args) = expr else { return None };
    let constants: Vec<Option<&Value>> = args.iter().map(|e| if let Expr::Const(v) = e { Some(v) } else { None }).collect();
    let (metric, at) = function.0.vector_metric(&constants)?;
    let operand = |i: usize| i == at || Some(i) == at.checked_add(1);
    let others = constants.iter().enumerate().all(|(i, c)| operand(i) || c.is_some());
    (function.0.deterministic() && others).then(|| (metric.to_ascii_uppercase(), at))
}

/// `exprs` over `source`, through a projection of columns alone below
/// it, as a select list naming every column it needs makes: each column
/// the one it projects.
fn flattened<'p>(exprs: &[Expr], source: &'p LogicalPlan) -> (Vec<Expr>, &'p LogicalPlan) {
    let LogicalPlan::Project { exprs: inner, input } = source else { return (exprs.to_vec(), source) };
    if !inner.iter().all(|e| matches!(e, Expr::Column(_))) {
        return (exprs.to_vec(), source);
    }
    (exprs.iter().map(|e| through(e.clone(), inner)).collect(), input)
}

/// `e`, each column the expression `inner` projects for it.
fn through(e: Expr, inner: &[Expr]) -> Expr {
    if let Expr::Column(i) = e {
        return inner.get(i).cloned().unwrap_or(Expr::Column(i));
    }
    e.map_children(&mut |child| through(child, inner))
}

/// Whether a call `distance_call` finds is a score, greatest first.
fn descends(measure: &Expr) -> bool {
    distance_call(measure).is_some_and(|(_, _, f, _)| f.0.ranks())
}

/// Whether `condition` is never true of a row whose `column` is null: one
/// of its conjuncts is a lookup of a call of it (`predicate_call`), which is
/// null of a null column (D34, D87).
fn excludes_nulls(condition: Option<&Cond>, column: usize) -> bool {
    let Some(condition) = condition else { return false };
    if let Cond::And(a, b) = condition {
        return excludes_nulls(Some(a), column) || excludes_nulls(Some(b), column);
    }
    let Some((function, args)) = crate::physical::access::predicate_call(condition) else { return false };
    let Some(Expr::Column(first)) = args.first() else { return false };
    let promised = function.0.index_predicate() || function.0.rechecked_errors();
    promised && *first == column
}

impl NearestRead {
    pub(crate) fn query(&self) -> Option<NearestQuery> {
        let (_, argument, function, Expr::Const(value)) = distance_call(&self.measure)? else { return None };
        Some(NearestQuery {
            function: function.clone(),
            column: self.position,
            argument,
            value: value.clone(),
            descending: self.descending,
            statistics: None,
            metric: vector_measure(&self.measure).map(|(metric, _)| metric),
            approximate: None,
        })
    }
}

/// What `nearest` reads: the table scanned, its columns needed, and its
/// filter.
struct Scanned<'p> {
    table: &'p QualifiedName,
    needed: &'p [usize],
    condition: Option<&'p Cond>,
}

impl PhysicalPlanner<'_> {
    pub(super) fn nearest(&self, keys: &[SortKey], input: &LogicalPlan) -> Option<Plan> {
        let first = self.first.filter(|&n| n > 0)?;
        let key = keys.first()?;
        let LogicalPlan::Project { exprs, input: source } = input else { return None };
        let (exprs, source) = &flattened(exprs, source);
        let measure = exprs.get(key.column)?;
        let descending = descends(measure);
        if key.descending != descending {
            return None;
        }
        let (scan, condition) =
            if let LogicalPlan::Filter { cond, input: filtered } = *source { (&**filtered, Some(cond)) } else { (*source, None) };
        let LogicalPlan::Scan { table, needed, located, .. } = scan else { return None };
        let desc = self.db.catalog.table(table)?;
        if !safe::projection(exprs, key.column, desc) || condition.is_some_and(|c| !safe::condition(c, desc)) {
            return None;
        }
        let (column, query) = self.nearest_query(measure, desc)?;
        // A null measure comes last, or the rows of none come at all.
        if key.nulls_first && !(descending && excludes_nulls(condition, column)) {
            return None;
        }
        let scanned = Scanned { table, needed, condition };
        let ((index, position), approximate, cost) = self.nearest_index((&scanned, desc), column, &query, first)?;
        let detail = self.described(scan, |names| {
            let measure = render_expr(measure, names);
            condition.map_or_else(|| measure.clone(), |c| format!("{measure} where {}", crate::logical::render_cond(c, names)))
        });
        let read = NearestRead {
            table: table.clone(),
            index: index.name.clone(),
            position,
            method: index.method.clone(),
            columns: needed.clone(),
            located: *located,
            width: scan.width(),
            measure: measure.clone(),
            descending,
            condition: condition.cloned(),
            first,
            detail,
            cost,
            approximate,
        };
        let projection = self.described(source, |names| exprs.iter().map(|e| render_expr(e, names)).collect::<Vec<_>>().join(", "));
        let mut exprs = exprs.clone();
        *exprs.get_mut(key.column)? = Expr::Column(read.width);
        Some(Plan::Project { exprs, detail: projection, input: Box::new(Plan::Nearest(Box::new(read))) })
    }

    /// The index a nearest read takes, of those on `column`, with the
    /// column's position in its key, what it asks of it if it is
    /// approximate, and the read's cost: a certified index if cheaper than
    /// a scan and a sort, else an approximate one if the query asks for an
    /// approximate search and its metric is the call's (R13.1,
    /// `nearest/approx.rs`). An index of several columns is asked of its
    /// column by its position, which no built-in method but the hybrid one
    /// serves.
    fn nearest_index(
        &self,
        (scanned, desc): (&Scanned<'_>, &TableDesc),
        column: usize,
        query: &NearestQuery,
        first: usize,
    ) -> Option<((crate::catalog::IndexDesc, usize), Option<ApproximateRead>, f64)> {
        let asked = self.approximation(scanned.table);
        let mut certified = None;
        let mut approximate = None;
        for ix in self.db.catalog.indexes(scanned.table) {
            let Some(position) = ix.columns.iter().position(|c| *c == column) else { continue };
            if ix.is_collated() || self.db.building.contains(&ix.name) {
                continue;
            }
            let (Some(method), Some(types)) = (self.db.methods.get(&ix.method), ix.key_types(desc)) else { continue };
            if self.db.index_of(&ix.name).is_err() {
                continue;
            }
            let probe = NearestQuery { approximate: asked.map(|a| approx::probe(a, first)), column: position, ..query.clone() };
            match method.supports_nearest(&probe, &types) {
                NearestSupport::No => {}
                NearestSupport::Exact => {
                    certified.get_or_insert((ix.clone(), position));
                }
                NearestSupport::Approximate => {
                    let served = query.metric.as_deref().is_some_and(|call| method.serves_metric(ix, call));
                    approximate = approx::preferred(approximate, ((ix.clone(), position), served), asked, self.hints());
                }
            }
        }
        if let Some(ix) = certified {
            let cost = self.nearest_cost((scanned.table, scanned.needed), scanned.condition, first);
            if let Some(cost) = cost {
                return Some((ix, None, cost));
            }
        }
        let ix = approximate?;
        let read = self.approximate_read(asked?, (scanned.table, scanned.condition), query)?;
        let cost = self.approximate_cost(scanned.table, &read, first)?;
        Some((ix, Some(read), cost))
    }

    fn nearest_query(&self, measure: &Expr, desc: &TableDesc) -> Option<(usize, NearestQuery)> {
        let (column, argument, function, other) = distance_call(measure)?;
        let descending = function.0.ranks();
        let metric = vector_measure(measure).map(|(metric, _)| metric);
        if !descending && metric.is_none() {
            fixed_srid(&desc.columns.get(column)?.data_type)?;
        }
        let value = if let Expr::Const(value) = other {
            value.clone()
        } else if let Expr::Dynamic(_) = other {
            let Expr::Const(value) = filled_expr(other.clone(), self.values?).ok()? else { return None };
            value
        } else {
            return None;
        };
        let query = NearestQuery {
            function: function.clone(),
            column: 0,
            argument,
            value,
            descending,
            statistics: None,
            metric,
            approximate: None,
        };
        Some((column, query))
    }

    fn nearest_cost(&self, (table, columns): (&QualifiedName, &[usize]), condition: Option<&Cond>, first: usize) -> Option<f64> {
        let storage = self.db.storage_of(table).ok()?;
        let model = self.db.cost();
        let all = cost_of(storage.row_count());
        let fraction = self.selectivity(table, condition);
        if fraction <= 0.0_f64 {
            return None;
        }
        // Overlap can require more refinement than this estimate. That
        // affects cost alone: only runtime bounds decide when to stop.
        let read = (cost_of(first) / fraction).min(all);
        let cost = model.index_descent * (all + 1.0_f64).log2() + read * storage.fetch_cost();
        let sorted = model.sort(all * fraction);
        (cost + model.sort(cost_of(first)) < storage.scan_cost(columns) + sorted).then_some(cost)
    }

    /// The share of the rows of `table` `condition` is estimated to keep.
    fn selectivity(&self, table: &QualifiedName, condition: Option<&Cond>) -> f64 {
        let model = self.db.cost();
        let stats = self.db.statistics(table);
        let known = estimate::Known { stats: stats.as_deref(), cost: &model };
        condition.map_or(1.0_f64, |c| safe::selectivity(c, known))
    }
}
