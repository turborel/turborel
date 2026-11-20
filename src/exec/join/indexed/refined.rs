//! Refine a spatial candidate using borrowed input values. The same
//! binding serves index probes and adaptive scans; every remaining ON
//! conjunct is retained for the ordinary row evaluator (D15, D85).

use std::sync::Arc;

use super::IndexJoin;
use crate::error::Result;
use crate::exec::{broken, eval::compared};
use crate::functions::ScalarRef;
use crate::geometry::SpatialOperation;
use crate::logical::{Cond, Expr, Truth, and_all, split_and};
use crate::memory::MemoryBudget;
use crate::physical::{JoinProbe, spatial::intersection};
use crate::storage::Row;
use crate::value::lob::TempLobs;
use crate::value::{CompOp, Decimal, Value};

/// The two source columns and argument order of the one spatial
/// conjunct. Other conjuncts cannot raise or have side effects (D15).
pub(super) struct Binding {
    pub(super) outer: usize,
    pub(super) inner: usize,
    pub(super) argument: usize,
    pub(super) function: ScalarRef,
    pub(super) rest: Option<Cond>,
}

impl Binding {
    pub(super) fn of(join: &IndexJoin, width: usize) -> Option<Self> {
        let JoinProbe::Predicate { predicate, outer, .. } = &join.probe else { return None };
        if !predicate.function.0.spatial_operation().is_some_and(SpatialOperation::windowed) {
            return None;
        }
        let [inner] = join.table.ix.columns.as_slice() else { return None };
        let argument = predicate.argument?;
        let mut terms = Vec::new();
        split_and(join.cond.clone()?, &mut terms);
        let mut found = false;
        let mut rest = Vec::new();
        for term in terms {
            if let Some((_, args)) = intersection(&term) {
                let [Expr::Column(left), Expr::Column(right)] = args else { return None };
                let actual = if argument == 0 { (*right, *left) } else { (*left, *right) };
                if found || actual != (*outer, width.saturating_add(*inner)) {
                    return None;
                }
                found = true;
            } else if term.can_raise() {
                return None;
            } else {
                rest.push(term);
            }
        }
        found.then(|| Self { outer: *outer, inner: *inner, argument, function: predicate.function.clone(), rest: and_all(rest) })
    }
}

impl Binding {
    pub(super) fn prepared(&self, row: &Row, budget: Option<&Arc<MemoryBudget>>) -> Result<ScalarRef> {
        let fixed = row.get(self.outer).ok_or_else(|| broken("a spatial build column is missing"))?;
        let args = if self.argument == 0 { [None, Some(fixed)] } else { [Some(fixed), None] };
        Ok(budget.and_then(|budget| self.function.0.specialize(&args, budget)).unwrap_or_else(|| self.function.clone()))
    }

    pub(super) fn intersects(&self, outer: &Row, inner: &Row, function: &ScalarRef, temp: Option<&dyn TempLobs>) -> Result<bool> {
        let outer_value = outer.get(self.outer).ok_or_else(|| broken("a spatial build column is missing"))?;
        let inner_value = inner.get(self.inner).ok_or_else(|| broken("a spatial scan column is missing"))?;
        let args = if self.argument == 0 { [inner_value, outer_value] } else { [outer_value, inner_value] };
        let intersects = function.0.call_borrowed(&args, temp)?;
        Ok(compared(&intersects, CompOp::Eq, &Value::Exact(Decimal::new(1, 0))) == Truth::True)
    }

    /// `intersects` of an inner value that is one point, at `point` of
    /// `srid`, decided from the point as an index entry of it is (RULES.md
    /// D85, `ScalarFunction::point_compared`), its value not decoded again
    /// for each outer row; none if the function cannot decide it, and the
    /// call is made.
    pub(super) fn point_intersects(&self, outer: &Row, function: &ScalarRef, point: ([f64; 2], i32)) -> Option<Result<bool>> {
        let outer_value = outer.get(self.outer)?;
        let decided = function.0.point_compared(point, outer_value, CompOp::Eq, &Value::Exact(Decimal::new(1, 0)))?;
        Some(decided.map(|truth| truth == Some(true)))
    }
}
