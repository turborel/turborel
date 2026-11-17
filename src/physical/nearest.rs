//! Distance ordering through certified index candidates (D15, D85).
//! The final sort still compares exact scalar results and secondary keys.

mod safe;

use super::{PhysicalPlanner, Plan, estimate};
use crate::catalog::{QualifiedName, TableDesc};
use crate::functions::ScalarRef;
use crate::geometry::{SpatialOperation, fixed_srid};
use crate::index::NearestQuery;
use crate::logical::{Cond, Expr, LogicalPlan, SortKey, filled_expr, render_expr};
use crate::storage::cost_of;

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
    /// The full WHERE condition, evaluated before ranking.
    pub condition: Option<Cond>,
    /// OFFSET plus FETCH count, excluding WITH TIES and PERCENT.
    pub first: usize,
    /// The measure and filter as text for EXPLAIN.
    pub detail: String,
    /// The estimated access cost.
    pub cost: f64,
}

/// The column and its argument position in a declared distance call.
pub(crate) fn distance_call(expr: &Expr) -> Option<(usize, usize, &ScalarRef, &Expr)> {
    let Expr::Call(function, args) = expr else { return None };
    if !function.0.deterministic() || function.0.spatial_operation() != Some(SpatialOperation::Distance) {
        return None;
    }
    match args.as_slice() {
        [Expr::Column(column), other] => Some((*column, 0, function, other)),
        [other, Expr::Column(column)] => Some((*column, 1, function, other)),
        _ => None,
    }
}

impl NearestRead {
    pub(crate) fn query(&self) -> Option<NearestQuery> {
        let (_, argument, function, Expr::Const(value)) = distance_call(&self.measure)? else { return None };
        Some(NearestQuery { function: function.clone(), column: 0, argument, value: value.clone() })
    }
}

impl PhysicalPlanner<'_> {
    pub(super) fn nearest(&self, keys: &[SortKey], input: &LogicalPlan) -> Option<Plan> {
        let first = self.first.filter(|&n| n > 0)?;
        let key = keys.first()?;
        if key.descending || key.nulls_first {
            return None;
        }
        let LogicalPlan::Project { exprs, input: source } = input else { return None };
        let measure = exprs.get(key.column)?;
        let (scan, condition) =
            if let LogicalPlan::Filter { cond, input: filtered } = &**source { (&**filtered, Some(cond)) } else { (&**source, None) };
        let LogicalPlan::Scan { table, needed, located, .. } = scan else { return None };
        let desc = self.db.catalog.table(table)?;
        if !safe::projection(exprs, key.column, desc) || condition.is_some_and(|c| !safe::condition(c, desc)) {
            return None;
        }
        let (column, query) = self.nearest_query(measure, desc)?;
        let cost = self.nearest_cost((table, needed), condition, first)?;
        let index = self.db.catalog.indexes(table).into_iter().find(|ix| {
            if ix.columns.as_slice() != [column] || ix.is_collated() || self.db.building.contains(&ix.name) {
                return false;
            }
            let (Some(method), Some(types)) = (self.db.methods.get(&ix.method), ix.key_types(desc)) else { return false };
            method.supports_nearest(&query, &types) && self.db.index_of(&ix.name).is_ok()
        })?;
        let detail = self.described(scan, |names| {
            let measure = render_expr(measure, names);
            condition.map_or_else(|| measure.clone(), |c| format!("{measure} where {}", crate::logical::render_cond(c, names)))
        });
        let read = NearestRead {
            table: table.clone(),
            index: index.name.clone(),
            method: index.method.clone(),
            columns: needed.clone(),
            located: *located,
            width: scan.width(),
            measure: measure.clone(),
            condition: condition.cloned(),
            first,
            detail,
            cost,
        };
        let projection = self.described(source, |names| exprs.iter().map(|e| render_expr(e, names)).collect::<Vec<_>>().join(", "));
        let mut exprs = exprs.clone();
        *exprs.get_mut(key.column)? = Expr::Column(read.width);
        Some(Plan::Project { exprs, detail: projection, input: Box::new(Plan::Nearest(Box::new(read))) })
    }

    fn nearest_query(&self, measure: &Expr, desc: &TableDesc) -> Option<(usize, NearestQuery)> {
        let (column, argument, function, other) = distance_call(measure)?;
        fixed_srid(&desc.columns.get(column)?.data_type)?;
        let value = if let Expr::Const(value) = other {
            value.clone()
        } else if let Expr::Dynamic(_) = other {
            let Expr::Const(value) = filled_expr(other.clone(), self.values?).ok()? else { return None };
            value
        } else {
            return None;
        };
        Some((column, NearestQuery { function: function.clone(), column: 0, argument, value }))
    }

    fn nearest_cost(&self, (table, columns): (&QualifiedName, &[usize]), condition: Option<&Cond>, first: usize) -> Option<f64> {
        let storage = self.db.storage_of(table).ok()?;
        let model = self.db.cost();
        let stats = self.db.statistics(table);
        let known = estimate::Known { stats: stats.as_deref(), cost: &model };
        let all = cost_of(storage.row_count());
        let fraction = condition.map_or(1.0_f64, |c| safe::selectivity(c, known));
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
}
