//! \[DESIGN\] (RULES.md D1127) A filter's scalar subqueries that read
//! nothing of its rows, made constants as its first row is judged. Such a
//! subquery has one value for the statement [ISO/IEC 9075:1992 §7.11
//! GR1], which its runner keeps, yet the condition holding it was judged
//! a row at a time, as no vector holds a subquery. A filter evaluates
//! both operands of every AND and OR of each row it judges (RULES.md
//! D15), so its first row runs each subquery that is an operand of a
//! comparison, of IS NULL or of IS DISTINCT FROM, or of arithmetic, a
//! monadic minus or a cast under one. Running them as that row is judged,
//! and judging every row with their values in their places, gives each
//! row the truth it had and the statement the errors it had: a filter
//! that judges no row runs none, and one whose subquery raises an error
//! is judged a row at a time as before, so its first row raises what it
//! raised. A subquery under CASE, a function or another predicate is
//! left, as a row may not reach it.

use crate::error::Result;
use crate::exec::{Runner, eval};
use crate::logical::{Cond, Expr, MAX_JUDGED_DEPTH};
use crate::storage::Row;

/// What makes a constant of a subquery found.
type Bind<'a> = &'a mut dyn FnMut(Expr) -> Result<Expr>;

/// Whether `cond` holds a subquery `bound` makes a constant.
pub(super) fn binds(cond: &Cond) -> bool {
    let mut found = false;
    let mut note = |e: Expr| {
        found = true;
        Ok(e)
    };
    walked(cond.clone(), &mut note, 0).is_ok() && found
}

/// `cond` with each scalar subquery of no outer reference that every row
/// judged runs made the constant its row gives, run by `runner`.
///
/// # Errors
///
/// A subquery's own, and that of one giving two rows.
pub(super) fn bound(cond: &Cond, runner: &Runner) -> Result<Cond> {
    let none = Row::new();
    walked(cond.clone(), &mut |e| eval(&e, &none, runner).map(Expr::Const), 0)
}

/// `cond`, `depth` below its filter's, with `bind` of each subquery every
/// row judged runs: none past `MAX_JUDGED_DEPTH` (RULES.md R-CF-6).
fn walked(cond: Cond, bind: Bind<'_>, depth: usize) -> Result<Cond> {
    if depth > MAX_JUDGED_DEPTH {
        return Ok(cond);
    }
    let deeper = depth.saturating_add(1);
    Ok(match cond {
        Cond::And(a, b) => Cond::And(Box::new(walked(*a, bind, deeper)?), Box::new(walked(*b, bind, deeper)?)),
        Cond::Or(a, b) => Cond::Or(Box::new(walked(*a, bind, deeper)?), Box::new(walked(*b, bind, deeper)?)),
        Cond::Not(a) => Cond::Not(Box::new(walked(*a, bind, deeper)?)),
        Cond::Is(a, truth, not) => Cond::Is(Box::new(walked(*a, bind, deeper)?), truth, not),
        Cond::Compare(a, op, b) => Cond::Compare(operand(a, bind, deeper)?, op, operand(b, bind, deeper)?),
        Cond::Distinct(a, b) => Cond::Distinct(operand(a, bind, deeper)?, operand(b, bind, deeper)?),
        Cond::IsNull(e, not) => Cond::IsNull(operand(e, bind, deeper)?, not),
        other @ (Cond::Const(_)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Unique(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_)) => other,
    })
}

/// `e`, an operand every row judged evaluates, as `walked` gives a
/// condition.
fn operand(e: Expr, bind: Bind<'_>, depth: usize) -> Result<Expr> {
    if depth > MAX_JUDGED_DEPTH {
        return Ok(e);
    }
    let deeper = depth.saturating_add(1);
    Ok(match e {
        Expr::Subquery(s, at) => {
            if s.outer.is_empty() {
                bind(Expr::Subquery(s, at))?
            } else {
                Expr::Subquery(s, at)
            }
        }
        Expr::Arith(a, op, b) => Expr::Arith(Box::new(operand(*a, bind, deeper)?), op, Box::new(operand(*b, bind, deeper)?)),
        Expr::Neg(x) => Expr::Neg(Box::new(operand(*x, bind, deeper)?)),
        Expr::Cast(x, to) => Expr::Cast(Box::new(operand(*x, bind, deeper)?), to),
        other @ (Expr::Column(_)
        | Expr::Const(_)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Truth(_)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_)) => other,
    })
}
