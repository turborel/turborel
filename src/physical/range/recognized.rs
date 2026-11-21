//! \[DESIGN\] What of a join's condition a range join's search answers
//! (RULES.md D580): its conjuncts parted into equalities of a column of
//! each operand, comparisons of a value of each by `<`, `<=`, `>` or `>=`,
//! and the rest. Of the comparisons, those of one value of the right
//! operand give the bounds it is searched between, a column bounded on
//! both sides chosen first, as a band is; one comparison of another value
//! is answered by pruning; every other is the rest's.
//!
//! A value compared is an expression of one operand's columns alone, at
//! least one, that gives the same value of the same row each time it is
//! evaluated: of columns, constants, arithmetic, CAST, sort keys and
//! deterministic calls; no subquery, CASE, outer reference, NEXT VALUE FOR,
//! routine or warning, whose evaluation once for each row, rather than for
//! each pair, could differ.

use super::{Bound, Pruned};
use crate::logical::{Cond, Expr, map_expr_columns};
use crate::physical::JoinKey;
use crate::value::CompOp;

/// \[DESIGN\] RULES.md R-CF-6: the deepest expression judged; a deeper one
/// is no value compared.
const MAX_DEPTH: usize = 64;

/// What a range join's search answers of a condition over pairs whose
/// first `width` columns are the left operand's, and the rest of it.
pub(in crate::physical) struct Ranged {
    /// The equalities of a column of each operand.
    pub(in crate::physical) keys: Vec<JoinKey>,
    /// The value of the right's columns, as its own, searched by bounds.
    pub(in crate::physical) sorted: Expr,
    pub(in crate::physical) lower: Option<Bound>,
    pub(in crate::physical) upper: Option<Bound>,
    pub(in crate::physical) pruned: Option<Pruned>,
    /// The conjuncts the bounds and the keys answer, over the pairs.
    pub(in crate::physical) searched: Vec<Cond>,
    /// The rest, over the pairs.
    pub(in crate::physical) rest: Vec<Cond>,
}

/// A comparison `build op probe` of a value of the right's columns, as its
/// own, and one of the left's, and the conjunct it is.
struct Cross {
    build: Expr,
    op: CompOp,
    probe: Expr,
    conjunct: Cond,
}

/// Of which operand an expression reads columns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// `conjuncts`, over pairs whose first `width` columns are the left
/// operand's, as a range join's search and its rest, if one compares a
/// value of each operand by `<`, `<=`, `>` or `>=`.
pub(in crate::physical) fn ranged(conjuncts: Vec<Cond>, width: usize) -> Option<Ranged> {
    let (mut keys, mut crosses, mut rest, mut searched) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for c in conjuncts {
        if let Some(key) = equality(&c, width) {
            keys.push(key);
            searched.push(c);
        } else if let Some(cross) = crossing(&c, width) {
            crosses.push(cross);
        } else {
            rest.push(c);
        }
    }
    let at = primary(&crosses)?;
    let sorted = crosses.get(at)?.build.clone();
    let mut found = Found { lower: None, upper: None, pruned: None };
    for (i, cross) in crosses.into_iter().enumerate() {
        let of_sorted = i == at || same_column(&cross.build, &sorted);
        let (taken, conjunct) = found.take(cross, of_sorted);
        if taken { searched.push(conjunct) } else { rest.push(conjunct) }
    }
    let Found { lower, upper, pruned } = found;
    Some(Ranged { keys, sorted, lower, upper, pruned, searched, rest })
}

/// The bounds and the pruning comparison found so far.
struct Found {
    lower: Option<Bound>,
    upper: Option<Bound>,
    pruned: Option<Pruned>,
}

impl Found {
    /// Takes `cross`, a comparison of the sorted value if `of_sorted`, as
    /// a bound or the pruning comparison not yet found: whether it did,
    /// and its conjunct.
    fn take(&mut self, cross: Cross, of_sorted: bool) -> (bool, Cond) {
        let Cross { build, op, probe, conjunct } = cross;
        let above = matches!(op, CompOp::Gt | CompOp::Ge);
        let closed = matches!(op, CompOp::Ge | CompOp::Le);
        let slot_free = if !of_sorted {
            self.pruned.is_none()
        } else if above {
            self.lower.is_none()
        } else {
            self.upper.is_none()
        };
        if !slot_free {
            return (false, conjunct);
        }
        if !of_sorted {
            self.pruned = Some(Pruned { build, op, probe });
        } else if above {
            self.lower = Some(Bound { probe, closed });
        } else {
            self.upper = Some(Bound { probe, closed });
        }
        (true, conjunct)
    }
}

/// Whether `a` and `b` are one column.
fn same_column(a: &Expr, b: &Expr) -> bool {
    if let (Expr::Column(x), Expr::Column(y)) = (a, b) { x == y } else { false }
}

/// The comparison whose value the right is sorted by: of a column bounded
/// on both sides, the first; else the first.
fn primary(crosses: &[Cross]) -> Option<usize> {
    if crosses.is_empty() {
        return None;
    }
    let bounded = |column: &Expr, above: bool| {
        crosses.iter().any(|c| same_column(&c.build, column) && matches!(c.op, CompOp::Gt | CompOp::Ge) == above)
    };
    let band = crosses.iter().position(|c| matches!(c.build, Expr::Column(_)) && bounded(&c.build, true) && bounded(&c.build, false));
    Some(band.unwrap_or(0))
}

/// The equality `c` is of a column of each operand, the left's first, if
/// it is "left = right" or "right = left": IS NOT DISTINCT FROM is left to
/// the rest, as a null key matches nothing in the search.
fn equality(c: &Cond, width: usize) -> Option<JoinKey> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    let (left, right) = if *a < width && *b >= width { (*a, *b) } else { (*b, *a) };
    (left < width && right >= width).then(|| JoinKey { left, right: right.saturating_sub(width), nulls: false })
}

/// `c` as a comparison of a value of each operand, the right's first, if
/// it is one by `<`, `<=`, `>` or `>=`.
fn crossing(c: &Cond, width: usize) -> Option<Cross> {
    let Cond::Compare(a, op, b) = c else { return None };
    if matches!(op, CompOp::Eq | CompOp::Ne) {
        return None;
    }
    let (build, op, probe) = match (side(a, width, 0)?, side(b, width, 0)?) {
        (Side::Right, Side::Left) => (a, *op, b),
        (Side::Left, Side::Right) => (b, op.flip(), a),
        (Side::Left, Side::Left) | (Side::Right, Side::Right) => return None,
    };
    let own = map_expr_columns(build.clone(), &|i| i.saturating_sub(width));
    Some(Cross { build: own, op, probe: probe.clone(), conjunct: c.clone() })
}

/// The operand whose columns `e` reads, if it reads at least one and only
/// one operand's, and gives the same value of the same row each time.
fn side(e: &Expr, width: usize, depth: usize) -> Option<Side> {
    if depth > MAX_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    let parts: Vec<&Expr> = match e {
        Expr::Column(i) => return Some(if *i < width { Side::Left } else { Side::Right }),
        Expr::Const(_) => Vec::new(),
        Expr::Neg(x) | Expr::Cast(x, _) | Expr::SortKey(x, _) => vec![&**x],
        Expr::Arith(x, _, y) => vec![&**x, &**y],
        Expr::Call(f, args) => {
            if !f.0.deterministic() {
                return None;
            }
            args.iter().collect()
        }
        Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Domain(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::NextValue(_)
        | Expr::Invoked(_)
        | Expr::Warned(..) => return None,
    };
    let mut found = None;
    for part in parts {
        if matches!(part, Expr::Const(_)) {
            continue;
        }
        let s = side(part, width, deeper)?;
        if found.is_some_and(|f| f != s) {
            return None;
        }
        found = Some(s);
    }
    found
}
