//! \[DESIGN\] An expression compiled into nodes over vectors (RULES.md
//! D362), once for its operator: each operator of a value expression or
//! a search condition its node, a constant LIKE pattern split once
//! (`constant_pattern`). What reads its statement beside its row (a
//! subquery, a routine invoked, NEXT VALUE FOR, a CAST to a domain, a
//! function that reads its statement or gives many rows' values at once)
//! and what raises a warning by its binding (`Expr::Warned`) is evaluated
//! a row at a time, as is a subtree deeper than its bound (RULES.md
//! R-CF-6).

use super::Node;
use crate::logical::{Cond, Expr};
use crate::value::constant_pattern;

/// `e` compiled, no node deeper than `depth`.
pub(super) fn compiled(e: &Expr, depth: usize) -> Node {
    let Some(deeper) = depth.checked_sub(1) else { return Node::Rows(e.clone()) };
    let of = |x: &Expr| Box::new(compiled(x, deeper));
    match e {
        Expr::Column(c) => Node::Input(*c),
        Expr::Const(v) => Node::Const(v.clone()),
        Expr::Neg(x) => Node::Neg(of(x)),
        Expr::Arith(a, op, b) => Node::Arith(*op, of(a), of(b)),
        Expr::Case(whens, otherwise) => {
            Node::Case(whens.iter().map(|(c, r)| (condition(c, deeper), compiled(r, deeper))).collect(), of(otherwise))
        }
        Expr::Cast(x, to) => Node::Cast(of(x), to.clone()),
        Expr::Call(function, args) => {
            let f = &function.0;
            if f.in_statement() || f.uses_context(args.len()) || f.batches() {
                return Node::Rows(e.clone());
            }
            Node::Call(function.clone(), args.iter().map(|a| compiled(a, deeper)).collect())
        }
        Expr::SortKey(x, collation) => Node::SortKey(of(x), collation.clone()),
        Expr::Truth(c) => condition(c, deeper),
        Expr::Domain(..)
        | Expr::Warned(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Subquery(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => Node::Rows(e.clone()),
    }
}

/// `c` compiled into a node giving truth values, none deeper than
/// `depth`.
pub(super) fn condition(c: &Cond, depth: usize) -> Node {
    let Some(deeper) = depth.checked_sub(1) else { return Node::RowsCond(c.clone()) };
    let of = |x: &Cond| Box::new(condition(x, deeper));
    let value = |x: &Expr| Box::new(compiled(x, deeper));
    match c {
        Cond::Const(t) => Node::Const(t.value()),
        Cond::Or(a, b) => Node::Or(of(a), of(b)),
        Cond::And(a, b) => Node::And(of(a), of(b)),
        Cond::Not(a) => Node::Not(of(a)),
        Cond::Is(a, truth, not) => Node::Is(of(a), *truth, *not),
        Cond::Compare(a, op, b) => Node::Compare(*op, value(a), value(b)),
        Cond::IsNull(x, not) => Node::IsNull(value(x), *not),
        Cond::Distinct(a, b) => Node::Distinct(value(a), value(b)),
        Cond::Like(x, pattern, escape) => liked((x, pattern, escape.as_ref()), deeper)
            .unwrap_or_else(|| Node::LikeOf(value(x), value(pattern), escape.as_ref().map(value))),
        Cond::In(x, set) => Node::In(value(x), set.clone()),
        Cond::Value(x) => Node::Truth(value(x)),
        Cond::Exists(_) | Cond::Unique(..) | Cond::Quantified(..) | Cond::Match { .. } => Node::RowsCond(c.clone()),
    }
}

/// The LIKE of `x`, a constant `pattern` and `escape` if any, that
/// cannot raise an error, its pattern split once; none of any other,
/// matched a row's values at a time.
fn liked((x, pattern, escape): (&Expr, &Expr, Option<&Expr>), depth: usize) -> Option<Node> {
    let Expr::Const(pattern) = pattern else { return None };
    let escape = match escape {
        None => None,
        Some(Expr::Const(e)) => Some(e),
        Some(_) => return None,
    };
    let split = constant_pattern(pattern, escape)?;
    Some(Node::Like(Box::new(compiled(x, depth)), split))
}
