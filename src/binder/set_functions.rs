//! Checks on the syntax tree for the rules about set functions [X3.135-1986
//! §5.8, §5.25; ISO/IEC 9075:1992 §6.5, §7.9]: which columns an expression
//! names outside its set functions, and whether it calls one. Split from
//! binder.rs (RULES.md R-SIZE-2).

use super::Binder;
use crate::ast::{ColumnSpecification, SetArg, ValueExpression};
use crate::db::State;

/// Adds to `out` the `<column specification>`s of `v`, not those inside a
/// set function, whose argument source is its own, nor a subquery's.
pub(super) fn column_specifications<'v>(v: &'v ValueExpression, out: &mut Vec<&'v ColumnSpecification>) {
    if let ValueExpression::Column(cs) = v {
        out.push(cs);
    } else if !matches!(v, ValueExpression::SetFunction(_)) {
        v.for_each_child(&mut |child| column_specifications(child, out));
    }
}

/// Whether `v` is itself a set function, or a call of an extension aggregate.
pub(super) fn is_aggregate_call(v: &ValueExpression, db: &State) -> bool {
    match v {
        ValueExpression::SetFunction(_) => true,
        ValueExpression::Call { name, .. } => db.functions.aggregate(name).is_some(),
        ValueExpression::Dyadic(..)
        | ValueExpression::Monadic(..)
        | ValueExpression::Value(_)
        | ValueExpression::Column(_)
        | ValueExpression::Nested(_)
        | ValueExpression::Case(_)
        | ValueExpression::Cast { .. }
        | ValueExpression::Subquery(_)
        | ValueExpression::Datetime(_)
        | ValueExpression::Extract { .. }
        | ValueExpression::Difference { .. }
        | ValueExpression::AtZone { .. }
        | ValueExpression::Function { .. } => false,
    }
}

impl Binder<'_> {
    /// Whether `v` has a set function or extension aggregate anywhere in it,
    /// not in a subquery, whose argument source is this query's: not one
    /// over an outer reference [ISO/IEC 9075:1992 §7.9 GR1a i], which
    /// SQL-86 counts too [X3.135-1986 §5.24 GR1, §5.25 SR7, GR1]. It makes
    /// an ungrouped query aggregated. One whose argument cannot be resolved
    /// counts, its error found as it is bound.
    pub(super) fn has_own_aggregate(&self, v: &ValueExpression) -> bool {
        let own = |a: &ValueExpression| self.outer.is_none() || !matches!(self.is_outer_argument(a), Ok(true));
        v.any(&|e| is_aggregate_call(e, self.db) && aggregate_argument(e).is_none_or(own))
    }
}

/// The argument of the set function or extension aggregate `v`, if it
/// takes one.
fn aggregate_argument(v: &ValueExpression) -> Option<&ValueExpression> {
    match v {
        ValueExpression::SetFunction(sf) => match &sf.arg {
            SetArg::CountStar => None,
            SetArg::Distinct(a) | SetArg::All(a) => Some(a),
        },
        ValueExpression::Call { args, .. } => args.first(),
        ValueExpression::Dyadic(..)
        | ValueExpression::Monadic(..)
        | ValueExpression::Value(_)
        | ValueExpression::Column(_)
        | ValueExpression::Nested(_)
        | ValueExpression::Case(_)
        | ValueExpression::Cast { .. }
        | ValueExpression::Subquery(_)
        | ValueExpression::Datetime(_)
        | ValueExpression::Extract { .. }
        | ValueExpression::Difference { .. }
        | ValueExpression::AtZone { .. }
        | ValueExpression::Function { .. } => None,
    }
}
