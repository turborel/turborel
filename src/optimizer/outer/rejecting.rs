//! \[DESIGN\] Whether a condition over both operands of an outer join is
//! never true of a row whose columns of one operand are all null (RULES.md
//! D1061), whatever the other operand's are. The join gives such a row
//! for a preserved row that matches none [ISO/IEC 9075:1992 §7.5 GR5], so
//! a WHERE conjunct that rejects it makes the join the join that gives
//! none.
//!
//! The condition is not evaluated, as the other operand's values are not
//! known. Its truth values are bounded instead, by the rules that make a
//! value null and a condition unknown whatever else it reads: a column
//! of the null operand is null; monadic minus, an arithmetic operator
//! and CAST of a null are null [X3.135-1986 §5.9 GR1; ISO/IEC 9075:1992
//! §6.10 GR2]; a comparison or LIKE of a null is unknown [X3.135-1986
//! §5.11 GR2, §5.14 GR2]; IS NULL of one is true; AND, OR, NOT and IS
//! follow their truth tables [ISO/IEC 9075:1992 §8.12 GR2]; and a CASE
//! is the result of the first WHEN that can only be true, a WHEN that
//! cannot be true passed over, else null if every result left is
//! [§6.9 GR2]. Anything else may be anything.

use crate::logical::{Cond, Expr, MAX_JUDGED_DEPTH, Truth};
use crate::value::Value;

/// The truth values a condition may have.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct May {
    /// True.
    yes: bool,
    /// False.
    no: bool,
    /// Unknown.
    unknown: bool,
}

impl May {
    /// Any of the three.
    const ANY: May = May { yes: true, no: true, unknown: true };

    /// `truth` alone.
    fn only(truth: Truth) -> May {
        May { yes: truth == Truth::True, no: truth == Truth::False, unknown: truth == Truth::Unknown }
    }

    /// Whether it may be `truth`.
    fn may(self, truth: Truth) -> bool {
        match truth {
            Truth::True => self.yes,
            Truth::False => self.no,
            Truth::Unknown => self.unknown,
        }
    }

    /// NOT of it, which leaves unknown as it is.
    fn not(self) -> May {
        May { yes: self.no, no: self.yes, unknown: self.unknown }
    }

    /// It AND `other`: true of two trues, false of any false, unknown of
    /// an unknown with a true or an unknown.
    fn and(self, other: May) -> May {
        let unknown = (self.unknown && (other.yes || other.unknown)) || (other.unknown && self.yes);
        May { yes: self.yes && other.yes, no: self.no || other.no, unknown }
    }

    /// `it IS truth`, or IS NOT if `not`, which is never unknown.
    fn is(self, truth: Truth, not: bool) -> May {
        let others = [Truth::True, Truth::False, Truth::Unknown].into_iter().any(|t| t != truth && self.may(t));
        let held = May { yes: self.may(truth), no: others, unknown: false };
        if not { held.not() } else { held }
    }
}

/// Whether `c` is false or unknown of every row whose columns `nulls` says
/// are null are so, as the module says.
pub(super) fn never_true(c: &Cond, nulls: &dyn Fn(usize) -> bool) -> bool {
    !truths(c, nulls, 0).yes
}

/// The truth values `c` may have of such a row, `depth` conditions deep;
/// past `MAX_JUDGED_DEPTH` any (RULES.md R-CF-6).
fn truths(c: &Cond, nulls: &dyn Fn(usize) -> bool, depth: usize) -> May {
    if depth > MAX_JUDGED_DEPTH {
        return May::ANY;
    }
    let deeper = depth.saturating_add(1);
    let null = |e: &Expr| is_null(e, nulls, deeper);
    let unknown_if = |found: bool| if found { May::only(Truth::Unknown) } else { May::ANY };
    match c {
        Cond::Const(t) => May::only(*t),
        Cond::And(a, b) => truths(a, nulls, deeper).and(truths(b, nulls, deeper)),
        Cond::Or(a, b) => truths(a, nulls, deeper).not().and(truths(b, nulls, deeper).not()).not(),
        Cond::Not(a) => truths(a, nulls, deeper).not(),
        Cond::Is(a, truth, not) => truths(a, nulls, deeper).is(*truth, *not),
        Cond::Compare(a, _, b) | Cond::Like(a, b, _) => unknown_if(null(a) || null(b)),
        Cond::Value(e) => unknown_if(null(e)),
        Cond::IsNull(e, not) => {
            if null(e) {
                May::only(if *not { Truth::False } else { Truth::True })
            } else {
                May { yes: true, no: true, unknown: false }
            }
        }
        Cond::Distinct(..) | Cond::In(..) | Cond::Exists(_) | Cond::Unique(..) | Cond::Quantified(..) | Cond::Match { .. } => May::ANY,
    }
}

/// Whether `e` is null of every such row, `depth` expressions deep; past
/// `MAX_JUDGED_DEPTH` not known to be.
fn is_null(e: &Expr, nulls: &dyn Fn(usize) -> bool, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return false;
    }
    let deeper = depth.saturating_add(1);
    match e {
        Expr::Column(c) => nulls(*c),
        Expr::Const(v) => matches!(v, Value::Null),
        Expr::Neg(x) | Expr::Cast(x, _) => is_null(x, nulls, deeper),
        Expr::Arith(a, _, b) => is_null(a, nulls, deeper) || is_null(b, nulls, deeper),
        Expr::Case(whens, otherwise) => {
            for (at, (when, result)) in whens.iter().enumerate() {
                let may = truths(when, nulls, deeper);
                if may == May::only(Truth::True) {
                    return is_null(result, nulls, deeper);
                }
                if may.yes {
                    // This result or any after it, so each must be null.
                    let rest = whens.iter().skip(at).map(|(_, r)| r).chain(std::iter::once(&**otherwise));
                    return rest.into_iter().all(|r| is_null(r, nulls, deeper));
                }
            }
            is_null(otherwise, nulls, deeper)
        }
        Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => false,
    }
}

// Unit tests: the truth tables, and which conditions reject a null operand
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
