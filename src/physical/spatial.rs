//! Spatial function comparisons recognized by filter and join planning.
//! Recognition alone permits no pruning: callers must prove reference
//! and error behavior and retain the complete SQL condition (D15, D85).
//! Beside the relationships true only of meeting operands, ST_Relate of a
//! constant pattern that implies meeting and ST_Distance compared `<` or
//! `<=` with a limit find their rows through windows too (D135).

use crate::functions::ScalarRef;
use crate::geometry::SpatialOperation;
use crate::logical::{Cond, Expr};
use crate::spatial::window;
use crate::value::{CompOp, Value};

/// A spatial conjunct an R-tree can find candidates for: its call, the
/// call's two geometry operands, and what the index reads beside them.
pub(crate) struct Recognized<'a> {
    pub(crate) function: &'a ScalarRef,
    pub(crate) operands: [&'a Expr; 2],
    pub(crate) beside: Beside<'a>,
}

/// What a window reads beside the geometry (`spatial::window`).
pub(crate) enum Beside<'a> {
    /// Nothing: a relationship true only of meeting operands, or equality.
    Nothing,
    /// ST_Relate's constant pattern, one that implies meeting.
    Pattern(&'a Value),
    /// ST_Distance's limit, and its unit if the call names one.
    Within(&'a Expr, Option<&'a str>),
}

impl Recognized<'_> {
    /// The column operand, its argument position, and the other operand:
    /// `f(column, value)` need not equal `f(value, column)`.
    pub(crate) fn split(&self) -> Option<(usize, usize, &Expr)> {
        match self.operands {
            [Expr::Column(column), other] => Some((*column, 0, other)),
            [other, Expr::Column(column)] => Some((*column, 1, other)),
            [_, _] => None,
        }
    }
}

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
    let spatial = function.0.spatial_operation().is_some_and(SpatialOperation::windowed);
    (spatial && function.0.deterministic()).then_some((function, args))
}

/// The windowed spatial conjunct `cond` is, if it is one: as
/// `intersection`, a relate of a meeting pattern compared equal to 1, or a
/// distance compared with a limit either way round.
pub(crate) fn recognized(cond: &Cond) -> Option<Recognized<'_>> {
    if let Some((function, [first, second])) = intersection(cond) {
        return Some(Recognized { function, operands: [first, second], beside: Beside::Nothing });
    }
    related(cond).or_else(|| distance(cond))
}

fn related(cond: &Cond) -> Option<Recognized<'_>> {
    let Cond::Compare(left, CompOp::Eq, right) = cond else { return None };
    let expression = if one(right) { left } else { right };
    let Expr::Call(function, args) = expression else { return None };
    let [first, second, Expr::Const(pattern)] = args.as_slice() else { return None };
    let relate = function.0.spatial_operation() == Some(SpatialOperation::Relate) && function.0.deterministic();
    let compared = one(left) || one(right);
    (relate && compared && window::pattern_meets(pattern)).then_some(Recognized {
        function,
        operands: [first, second],
        beside: Beside::Pattern(pattern),
    })
}

fn distance(cond: &Cond) -> Option<Recognized<'_>> {
    let Cond::Compare(left, op, right) = cond else { return None };
    let (call, limit) = match op {
        CompOp::Lt | CompOp::Le => (left, right),
        CompOp::Gt | CompOp::Ge => (right, left),
        CompOp::Eq | CompOp::Ne => return None,
    };
    let Expr::Call(function, args) = call else { return None };
    if function.0.spatial_operation() != Some(SpatialOperation::Distance) || !function.0.deterministic() {
        return None;
    }
    let (operands, unit) = match args.as_slice() {
        [first, second] => ([first, second], None),
        [first, second, Expr::Const(Value::Char(unit))] => ([first, second], Some(unit.as_str())),
        _ => return None,
    };
    Some(Recognized { function, operands, beside: Beside::Within(limit, unit) })
}

fn one(expr: &Expr) -> bool {
    let Expr::Const(Value::Exact(number)) = expr else { return false };
    number.units == 1 && number.scale == 0
}
