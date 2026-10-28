//! \[DESIGN\] Bound expressions: `<value expression>` (`Expr`) and
//! `<search condition>` (`Cond`), over the columns of a node's input row.
//! Split from logical.rs (RULES.md R-SIZE-2).
//!
//! Every pass over them goes through one traversal: `Expr::map_children`
//! and `Expr::for_each_child` reach each expression an expression is made
//! of, those of the conditions it holds among them, and `Cond::map_exprs`
//! and `Cond::for_each_expr` the expressions a condition compares; the
//! outer expressions of a subquery are over the same rows, its plan's are
//! not. A new kind of expression is added to its definition, the
//! traversal, evaluation (`exec/eval.rs`) and rendering; the passes built on
//! the traversal follow it.

use std::sync::Arc;

use super::Subquery;
use crate::catalog::MatchType;
use crate::functions::ScalarRef;
use crate::value::{ArithOp, CompOp, DataType, Value};

/// [X3.135-1986 §5.18] "true", "false", or "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    /// true
    True,
    /// false
    False,
    /// unknown: a comparison with a null [X3.135-1986 §5.11 GR2].
    Unknown,
}

/// A bound `<value expression>` [X3.135-1986 §5.9, ISO/IEC 9075:1992 §6.11].
#[derive(Debug, Clone)]
pub enum Expr {
    /// The value of the column at this position of the input row.
    Column(usize),
    /// A constant.
    Const(Value),
    /// Monadic minus [X3.135-1986 §5.9 GR4].
    Neg(Box<Expr>),
    /// A dyadic arithmetic operator [X3.135-1986 §5.9 GR5].
    Arith(Box<Expr>, ArithOp, Box<Expr>),
    /// A registered scalar function: one of the standard's, called with
    /// its own syntax [ISO/IEC 9075:1992 §6.6, §6.7, §6.13], or an
    /// \[EXTENSION\] called by name.
    Call(ScalarRef, Vec<Expr>),
    /// An outer reference [X3.135-1986 §5.7 SR4], in a subquery's plan: the
    /// value of its outer expression at this slot (`Subquery::outer`), and
    /// the column's name, for display. Replaced by that value before the
    /// plan runs (`Subquery::bound`).
    Outer(usize, String),
    /// [ISO/IEC 9075:1992 §6.9] A `<case expression>`, NULLIF and COALESCE
    /// among them: the result of the first WHEN whose condition is true,
    /// else of the ELSE \[GR1, GR2\].
    Case(Vec<(Cond, Expr)>, Box<Expr>),
    /// [ISO/IEC 9075:1992 §6.10] CAST to a data type.
    Cast(Box<Expr>, DataType),
    /// [ISO/IEC 9075:1992 §7.11] A `<scalar subquery>`, or a column, at
    /// this position, of a `<row subquery>`: the value there of its one
    /// row, null if it has none \[GR1, GR2\].
    Subquery(Arc<Subquery>, usize),
}

/// A bound `<search condition>` [X3.135-1986 §5.18].
#[derive(Debug, Clone)]
pub enum Cond {
    /// A condition already known; produced by rewrite rules.
    Const(Truth),
    /// OR [X3.135-1986 §5.18 GR2].
    Or(Box<Cond>, Box<Cond>),
    /// AND [X3.135-1986 §5.18 GR2].
    And(Box<Cond>, Box<Cond>),
    /// NOT [X3.135-1986 §5.18 GR2].
    Not(Box<Cond>),
    /// A `<comparison predicate>` [X3.135-1986 §5.11].
    Compare(Expr, CompOp, Expr),
    /// A `<null predicate>` [X3.135-1986 §5.15]: the value, and true for IS
    /// NOT NULL.
    IsNull(Expr, bool),
    /// A `<like predicate>` [X3.135-1986 §5.14]: the value matched, the
    /// `<pattern>`, and the `<escape character>` if one was given.
    Like(Expr, Expr, Option<Expr>),
    /// An `<exists predicate>` [X3.135-1986 §5.17].
    Exists(Arc<Subquery>),
    /// A comparison of a row of values with the rows of a subquery: a
    /// `<quantified predicate>` [X3.135-1986 §5.16], which `<in predicate>` is
    /// [X3.135-1986 §5.13 GR2], or a `<comparison predicate>` with a
    /// `<subquery>` [X3.135-1986 §5.11]; of degree one, or of a row value
    /// constructor's degree [ISO/IEC 9075:1992 §8.7].
    Quantified(Vec<Expr>, CompOp, Quantifier, Arc<Subquery>),
    /// [ISO/IEC 9075:1992 §8.12] `<boolean test>`: the condition IS (or,
    /// if the flag, IS NOT) the truth value.
    Is(Box<Cond>, Truth, bool),
    /// [ISO/IEC 9075:1992 §8.9] `<unique predicate>`: no two rows of the
    /// subquery equal, nulls not counting.
    Unique(Arc<Subquery>),
    /// [ISO/IEC 9075:1992 §8.10] `<match predicate>`: the row of values
    /// matches a row of the subquery, a unique one if UNIQUE.
    Match {
        /// The row of values.
        values: Vec<Expr>,
        /// UNIQUE was written.
        unique: bool,
        /// The `<match type>`.
        match_type: MatchType,
        /// The `<table subquery>`.
        subquery: Arc<Subquery>,
    },
}

/// How a comparison with a subquery's values is quantified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantifier {
    /// ALL [X3.135-1986 §5.16 GR2a, b].
    All,
    /// SOME or ANY [X3.135-1986 §5.16 GR2c, d].
    Some,
    /// No quantifier: the subquery gives at most one value [X3.135-1986 §5.11
    /// GR1], none being unknown [X3.135-1986 §5.11 GR2].
    One,
}

impl Expr {
    /// `self` with each expression it is made of replaced by `f` of it:
    /// its operands and arguments, the expressions of its conditions, and
    /// a subquery's outer expressions.
    #[must_use]
    pub fn map_children(self, f: &mut dyn FnMut(Expr) -> Expr) -> Expr {
        match self {
            Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) => self,
            Expr::Neg(x) => Expr::Neg(Box::new(f(*x))),
            Expr::Arith(a, op, b) => {
                let a = f(*a);
                Expr::Arith(Box::new(a), op, Box::new(f(*b)))
            }
            Expr::Call(func, args) => Expr::Call(func, args.into_iter().map(&mut *f).collect()),
            Expr::Case(whens, otherwise) => {
                let whens = whens.into_iter().map(|(c, e)| (c.map_exprs(f), f(e))).collect();
                Expr::Case(whens, Box::new(f(*otherwise)))
            }
            Expr::Cast(x, t) => Expr::Cast(Box::new(f(*x)), t),
            Expr::Subquery(s, column) => Expr::Subquery(s.map_outer(f), column),
        }
    }

    /// Calls `f` with each expression `self` is made of, as `map_children`
    /// reaches them.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        match self {
            Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) => {}
            Expr::Neg(x) | Expr::Cast(x, _) => f(x),
            Expr::Arith(a, _, b) => {
                f(a);
                f(b);
            }
            Expr::Call(_, args) => args.iter().for_each(f),
            Expr::Case(whens, otherwise) => {
                for (c, e) in whens {
                    c.for_each_expr(f);
                    f(e);
                }
                f(otherwise);
            }
            Expr::Subquery(s, _) => s.outer.iter().for_each(f),
        }
    }

    /// Whether evaluating this expression can raise a run-time error.
    /// Arithmetic can ([X3.135-1986 §5.9 GR5, GR6]), and so can a function,
    /// a CAST [ISO/IEC 9075:1992 §6.10 GR], and a scalar subquery with more
    /// than one row [§7.11 GR1]; a column, a constant and monadic minus
    /// cannot (exact values never exceed `MAX_EXACT_PRECISION` digits, so
    /// negation cannot overflow), and CASE only through its parts.
    pub fn can_raise(&self) -> bool {
        match self {
            Expr::Arith(..) | Expr::Call(..) | Expr::Cast(..) | Expr::Subquery(..) => true,
            Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) | Expr::Neg(_) | Expr::Case(..) => {
                let mut raises = false;
                self.for_each_child(&mut |e| raises = raises || e.can_raise());
                if let Expr::Case(whens, _) = self {
                    raises = raises || whens.iter().any(|(c, _)| c.can_raise());
                }
                raises
            }
        }
    }
}

impl Cond {
    /// `self` with each expression it compares replaced by `f` of it, the
    /// outer expressions of its subqueries among them; not those of a
    /// subquery's plan, which are over the subquery's rows.
    #[must_use]
    pub fn map_exprs(self, f: &mut dyn FnMut(Expr) -> Expr) -> Cond {
        match self {
            Cond::Const(_) => self,
            Cond::Or(a, b) => {
                let a = a.map_exprs(f);
                Cond::Or(Box::new(a), Box::new(b.map_exprs(f)))
            }
            Cond::And(a, b) => {
                let a = a.map_exprs(f);
                Cond::And(Box::new(a), Box::new(b.map_exprs(f)))
            }
            Cond::Not(a) => Cond::Not(Box::new(a.map_exprs(f))),
            Cond::Is(a, truth, not) => Cond::Is(Box::new(a.map_exprs(f)), truth, not),
            Cond::Compare(a, op, b) => {
                let a = f(a);
                Cond::Compare(a, op, f(b))
            }
            Cond::IsNull(e, not) => Cond::IsNull(f(e), not),
            Cond::Like(e, pattern, escape) => {
                let (e, pattern) = (f(e), f(pattern));
                Cond::Like(e, pattern, escape.map(&mut *f))
            }
            Cond::Exists(s) => Cond::Exists(s.map_outer(f)),
            Cond::Unique(s) => Cond::Unique(s.map_outer(f)),
            Cond::Quantified(values, op, quantifier, s) => {
                let values = values.into_iter().map(&mut *f).collect();
                Cond::Quantified(values, op, quantifier, s.map_outer(f))
            }
            Cond::Match { values, unique, match_type, subquery } => {
                let values = values.into_iter().map(&mut *f).collect();
                Cond::Match { values, unique, match_type, subquery: subquery.map_outer(f) }
            }
        }
    }

    /// Calls `f` with each expression `self` compares, as `map_exprs`
    /// reaches them.
    pub fn for_each_expr<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        match self {
            Cond::Const(_) => {}
            Cond::Or(a, b) | Cond::And(a, b) => {
                a.for_each_expr(f);
                b.for_each_expr(f);
            }
            Cond::Not(a) | Cond::Is(a, ..) => a.for_each_expr(f),
            Cond::Compare(a, _, b) => {
                f(a);
                f(b);
            }
            Cond::IsNull(e, _) => f(e),
            Cond::Like(e, pattern, escape) => {
                f(e);
                f(pattern);
                escape.iter().for_each(f);
            }
            Cond::Exists(s) | Cond::Unique(s) => s.outer.iter().for_each(f),
            Cond::Quantified(values, .., s) | Cond::Match { values, subquery: s, .. } => {
                values.iter().for_each(&mut *f);
                s.outer.iter().for_each(f);
            }
        }
    }

    /// The subqueries of this condition, not those nested in them nor in
    /// its expressions.
    pub fn subqueries(&self) -> Vec<&Subquery> {
        match self {
            Cond::Or(a, b) | Cond::And(a, b) => [a.subqueries(), b.subqueries()].concat(),
            Cond::Not(a) | Cond::Is(a, ..) => a.subqueries(),
            Cond::Exists(s) | Cond::Unique(s) | Cond::Quantified(.., s) | Cond::Match { subquery: s, .. } => vec![&**s],
            Cond::Const(_) | Cond::Compare(..) | Cond::IsNull(..) | Cond::Like(..) => Vec::new(),
        }
    }

    /// Whether evaluating this condition can raise a run-time error: only
    /// through the expressions it compares. Comparison itself cannot fail: it is
    /// unknown if either value is null and otherwise true or false ([X3.135-1986 §5.11 GR2, GR3]).
    /// LIKE can: a pattern with an escape character may have no partitioning
    /// [X3.135-1986 §5.14 GR2a ii]. A subquery can if its plan can, and a
    /// comparison with one value can find more [X3.135-1986 §5.11 GR1].
    pub fn can_raise(&self) -> bool {
        let own = match self {
            Cond::Like(..) | Cond::Quantified(_, _, Quantifier::One, _) => true,
            Cond::Or(a, b) | Cond::And(a, b) => a.can_raise() || b.can_raise(),
            Cond::Not(a) | Cond::Is(a, ..) => a.can_raise(),
            Cond::Const(_)
            | Cond::Compare(..)
            | Cond::IsNull(..)
            | Cond::Exists(_)
            | Cond::Unique(_)
            | Cond::Quantified(..)
            | Cond::Match { .. } => false,
        };
        let mut raises = own || self.subqueries().iter().any(|s| s.can_raise());
        self.for_each_expr(&mut |e| raises = raises || e.can_raise());
        raises
    }
}

/// The top-level conjuncts of a condition. A row satisfies "a AND b" exactly
/// when it satisfies both ([X3.135-1986 §5.18] truth table), so they can be applied separately.
pub fn split_and(c: Cond, out: &mut Vec<Cond>) {
    if let Cond::And(a, b) = c {
        split_and(*a, out);
        split_and(*b, out);
    } else {
        out.push(c);
    }
}

/// The conjunction of `conds`, or `None` if there are none. It is built
/// balanced, about log2(n) deep, so that rules which gather conjuncts from
/// several clauses never make a condition much deeper than the parser allowed
/// (RULES.md R-CF-6). AND is associative under its truth table
/// [X3.135-1986 §5.18 GR2], so the shape does not change the result.
pub fn and_all(conds: Vec<Cond>) -> Option<Cond> {
    let mut level = conds;
    // Join neighbours pairwise, in order, until one condition is left.
    while level.len() > 1 {
        let mut joined = Vec::new();
        let mut pending: Option<Cond> = None;
        for c in level {
            pending = match pending.take() {
                Some(left) => {
                    joined.push(Cond::And(Box::new(left), Box::new(c)));
                    None
                }
                None => Some(c),
            };
        }
        joined.extend(pending);
        level = joined;
    }
    level.pop()
}

/// Adds to `out` every column position `e` refers to: not those of a
/// subquery's own rows, but those of this row in its outer expressions.
pub fn expr_columns(e: &Expr, out: &mut Vec<usize>) {
    if let Expr::Column(i) = e {
        out.push(*i);
    }
    e.for_each_child(&mut |child| expr_columns(child, out));
}

/// Adds to `out` every column position `c` refers to.
pub fn cond_columns(c: &Cond, out: &mut Vec<usize>) {
    c.for_each_expr(&mut |e| expr_columns(e, out));
}

/// Renumber every column reference with `f`.
pub fn map_expr_columns(e: Expr, f: &impl Fn(usize) -> usize) -> Expr {
    if let Expr::Column(i) = e {
        return Expr::Column(f(i));
    }
    e.map_children(&mut |child| map_expr_columns(child, f))
}

/// Renumber every column reference in `c` with `f`.
pub fn map_cond_columns(c: Cond, f: &impl Fn(usize) -> usize) -> Cond {
    c.map_exprs(&mut |e| map_expr_columns(e, f))
}

/// `c` with every expression over its rows replaced by `f(expression)`:
/// those it compares, and the outer expressions of its subqueries, which
/// are over the same rows; not those of a subquery's plan, which are over
/// the subquery's rows.
pub fn map_cond_exprs(c: Cond, f: &impl Fn(Expr) -> Expr) -> Cond {
    c.map_exprs(&mut |e| f(e))
}

// Unit tests: `can_raise` decides whether a rewrite may evaluate a condition
// on more rows (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn column() -> Expr {
        Expr::Column(0)
    }

    fn quotient() -> Expr {
        Expr::Arith(Box::new(Expr::Const(Value::Exact(Decimal::new(1, 0)))), ArithOp::Div, Box::new(column()))
    }

    fn compare(left: Expr, right: Expr) -> Cond {
        Cond::Compare(left, CompOp::Gt, right)
    }

    /// Verifies: [DESIGN] arithmetic can raise ([X3.135-1986 §5.9 GR5]); columns,
    /// constants and the negation of either cannot; CASE through its parts.
    #[test]
    fn expressions_that_can_raise() {
        assert!(!column().can_raise());
        assert!(!Expr::Neg(Box::new(column())).can_raise());
        assert!(quotient().can_raise());
        assert!(Expr::Neg(Box::new(quotient())).can_raise());
        let case = |c: Cond, e: Expr| Expr::Case(vec![(c, e)], Box::new(column()));
        assert!(!case(compare(column(), column()), column()).can_raise());
        assert!(case(compare(quotient(), column()), column()).can_raise());
        assert!(case(compare(column(), column()), quotient()).can_raise());
        assert!(Expr::Cast(Box::new(column()), DataType::Character { length: 1 }).can_raise());
    }

    /// Verifies: [DESIGN] a condition can raise exactly when one of its operands can.
    ///
    /// Decision `left.can_raise() || right.can_raise()` in `Compare`, `And` and `Or`.
    /// MC/DC: (F F) false; (T F) true shows `left`; (F T) true shows `right`.
    #[test]
    fn conditions_raise_through_their_operands() {
        assert!(!compare(column(), column()).can_raise()); // F F
        assert!(compare(quotient(), column()).can_raise()); // T F
        assert!(compare(column(), quotient()).can_raise()); // F T
        let safe = || compare(column(), column());
        let risky = || compare(quotient(), column());
        assert!(!Cond::And(Box::new(safe()), Box::new(safe())).can_raise()); // F F
        assert!(Cond::And(Box::new(risky()), Box::new(safe())).can_raise()); // T F
        assert!(Cond::And(Box::new(safe()), Box::new(risky())).can_raise()); // F T
        assert!(!Cond::Or(Box::new(safe()), Box::new(safe())).can_raise()); // F F
        assert!(Cond::Or(Box::new(risky()), Box::new(safe())).can_raise()); // T F
        assert!(Cond::Or(Box::new(safe()), Box::new(risky())).can_raise()); // F T
        assert!(Cond::Not(Box::new(risky())).can_raise());
        assert!(!Cond::IsNull(Expr::Column(0), false).can_raise());
        assert!(Cond::Is(Box::new(risky()), Truth::True, false).can_raise());
    }
}
