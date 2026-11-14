//! Checks on the syntax tree for the rules about set functions [X3.135-1986
//! §5.8, §5.25; ISO/IEC 9075:1992 §6.5, §7.9]: which columns an expression
//! names outside its set functions, and whether it calls one.

use super::Binder;
use crate::ast::{Composite, SetArg, ValueExpression};
use crate::db::State;

/// Adds to `out` the column references of `v`, each a `<column
/// specification>` or an `<identifier chain>` of a column of a row type
/// and its fields [ISO/IEC 9075-2:2023 §6.6]; not those inside a set
/// function, whose argument source is its own, nor a subquery's.
pub(super) fn column_references<'v>(v: &'v ValueExpression, out: &mut Vec<&'v ValueExpression>) {
    let chain = matches!(v, ValueExpression::Composite(c, _) if matches!(**c, Composite::Chain(_)));
    if chain || matches!(v, ValueExpression::Column(_)) {
        out.push(v);
    } else if !matches!(v, ValueExpression::SetFunction(_)) {
        v.for_each_child(&mut |child| column_references(child, out));
    }
}

/// Whether `v` is itself a set function, or a call of an extension
/// aggregate, read while a session took function calls, of no schema, and
/// naming no routine (`binder/routines.rs`).
pub(super) fn is_aggregate_call(v: &ValueExpression, (db, path): (&State, &str)) -> bool {
    match v {
        ValueExpression::SetFunction(_) => true,
        ValueExpression::Call { schema: None, name, extension: true, .. } => {
            db.functions.aggregate(name).is_some() && !super::routines::names_routine(db, path, name)
        }
        ValueExpression::Call { .. }
        | ValueExpression::Dyadic(..)
        | ValueExpression::Monadic(..)
        | ValueExpression::Value(_)
        | ValueExpression::Column(_)
        | ValueExpression::Nested(_)
        | ValueExpression::Collated(..)
        | ValueExpression::CollationFor(_)
        | ValueExpression::Case(_)
        | ValueExpression::Cast { .. }
        | ValueExpression::Subquery(_)
        | ValueExpression::Datetime(_)
        | ValueExpression::Extract { .. }
        | ValueExpression::Difference { .. }
        | ValueExpression::AtZone { .. }
        | ValueExpression::Boolean(_)
        | ValueExpression::Function { .. }
        | ValueExpression::Composite(..)
        | ValueExpression::Window(_)
        | ValueExpression::NextValue { .. } => false,
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
        // [ISO/IEC 9075-2:2023 §6.9 SR6] GROUPING is of this query unless
        // each of its column references is an outer reference.
        let of_this = |e: &ValueExpression| match grouping_arguments(e) {
            Some(columns) => columns.iter().any(own),
            None => aggregate_argument(e).is_none_or(own),
        };
        v.any(&|e| is_aggregate_call(e, (self.db, self.schema)) && of_this(e))
    }
}

/// The column references of `v` if it is a `<grouping operation>`.
fn grouping_arguments(v: &ValueExpression) -> Option<&[ValueExpression]> {
    let ValueExpression::SetFunction(sf) = v else { return None };
    let SetArg::Grouping(columns) = &sf.arg else { return None };
    Some(columns)
}

/// The argument of the set function or extension aggregate `v`, if it
/// takes one.
fn aggregate_argument(v: &ValueExpression) -> Option<&ValueExpression> {
    match v {
        ValueExpression::SetFunction(sf) => match &sf.arg {
            SetArg::CountStar | SetArg::Grouping(_) => None,
            SetArg::Distinct(a) | SetArg::All(a) => Some(a),
        },
        ValueExpression::Call { args, .. } => args.first(),
        ValueExpression::Dyadic(..)
        | ValueExpression::Monadic(..)
        | ValueExpression::Value(_)
        | ValueExpression::Column(_)
        | ValueExpression::Nested(_)
        | ValueExpression::Collated(..)
        | ValueExpression::CollationFor(_)
        | ValueExpression::Case(_)
        | ValueExpression::Cast { .. }
        | ValueExpression::Subquery(_)
        | ValueExpression::Datetime(_)
        | ValueExpression::Extract { .. }
        | ValueExpression::Difference { .. }
        | ValueExpression::AtZone { .. }
        | ValueExpression::Boolean(_)
        | ValueExpression::Function { .. }
        | ValueExpression::Composite(..)
        | ValueExpression::Window(_)
        | ValueExpression::NextValue { .. } => None,
    }
}
