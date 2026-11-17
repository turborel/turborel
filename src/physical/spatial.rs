//! Spatial function comparisons recognized by filter and join planning.
//! Recognition alone permits no pruning: callers must prove reference
//! and error behavior and retain the complete SQL condition (D15, D82).

use crate::functions::ScalarRef;
use crate::geometry::SpatialOperation;
use crate::logical::{Cond, Expr};
use crate::value::{CompOp, Value};

pub(crate) fn intersection(cond: &Cond) -> Option<(&ScalarRef, &[Expr])> {
    let Cond::Compare(left, CompOp::Eq, right) = cond else { return None };
    let expression = if one(right) {
        left
    } else if one(left) {
        right
    } else {
        return None;
    };
    let Expr::Call(function, args) = expression else { return None };
    let spatial = function.0.spatial_operation() == Some(SpatialOperation::Intersects);
    (spatial && function.0.deterministic()).then_some((function, args))
}

fn one(expr: &Expr) -> bool {
    let Expr::Const(Value::Exact(number)) = expr else { return false };
    number.units == 1 && number.scale == 0
}
