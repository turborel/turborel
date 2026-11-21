//! \[DESIGN\] (RULES.md D665) A chain of predicate transfer: inner hash
//! joins, LEFT outer joins of no condition of an operand's own, and
//! filters and projections over them, its links; below them its leaves,
//! each any other plan, a filter over a scan among them. The planner and
//! the executor walk a chain alike, past the operators EXPLAIN ANALYZE
//! counts, so a leaf has the same number in both: its place in a walk of
//! the chain's left inputs before its right ones.
//!
//! A key of a LEFT join transfers from its left operand to its right
//! alone: a right row no left row matches is in no row of the join, but a
//! left row no right row matches is, with nulls [ISO/IEC 9075:1992 §7.5
//! GR4]. A key of a join above it of a column of its right operand
//! transfers both ways, as a row the LEFT join made with nulls matches
//! none of it; but for a key on which nulls match, which then would.

use super::Step;
use crate::db::State;
use crate::logical::{Expr, OuterJoin};
use crate::physical::{JoinKey, Plan};

/// \[DESIGN\] The deepest a chain is walked, past which it is no chain:
/// more links than a chain the enumeration plans has (RULES.md R-CF-6).
pub(crate) const LINKS: usize = 512;

/// A key of a chain as a step from its join's left leaf to its right's,
/// and whether it transfers back too.
#[derive(Debug, Clone, Copy)]
pub(super) struct Key {
    pub(super) step: Step,
    pub(super) both: bool,
}

/// Of a join link, its operands, its keys, the width of its left
/// operand's rows if it holds it, and whether it is a LEFT join.
type Joined<'p> = (&'p Plan, &'p Plan, &'p [JoinKey], Option<usize>, bool);

/// `plan`'s join, past a counted operator, if it is a link's.
fn joined(plan: &Plan) -> Option<Joined<'_>> {
    if let Plan::HashJoin { left, right, keys, .. } = plan.bare() {
        return Some((left, right, keys, None, false));
    }
    let Plan::OuterJoin { kind: OuterJoin::Left, own: None, left, right, keys, widths, .. } = plan.bare() else { return None };
    Some((left, right, keys, Some(widths.0), true))
}

/// Whether `plan` is a link of a chain: a join `joined` takes, or a
/// filter or a projection over a link.
pub(crate) fn link(plan: &Plan) -> bool {
    let mut at = plan.bare();
    for _ in 0..LINKS {
        if joined(at).is_some() {
            return true;
        }
        let (Plan::Filter { input, .. } | Plan::Project { input, .. }) = at else { return false };
        at = input.bare();
    }
    false
}

/// Calls `each` with the number and the plan of each leaf of the chain at
/// `plan`, in order.
pub(crate) fn leaves_mut(plan: &mut Plan, each: &mut dyn FnMut(usize, &mut Plan)) {
    let mut next = 0_usize;
    walk_mut(plan, each, (&mut next, 0));
}

/// `leaves_mut`'s walk of `plan`, `depth` links down, `next` the number of
/// the next leaf.
fn walk_mut(plan: &mut Plan, each: &mut dyn FnMut(usize, &mut Plan), (next, depth): (&mut usize, usize)) {
    if depth >= LINKS || !link(plan) {
        each(*next, plan);
        *next = next.saturating_add(1);
        return;
    }
    let deeper = depth.saturating_add(1);
    if let Plan::HashJoin { left, right, .. } | Plan::OuterJoin { left, right, .. } = plan {
        walk_mut(left, each, (next, deeper));
        walk_mut(right, each, (next, deeper));
    } else if let Plan::Counted(c) = plan {
        walk_mut(&mut c.input, each, (next, deeper));
    } else if let Plan::Filter { input, .. } | Plan::Project { input, .. } = plan {
        walk_mut(input, each, (next, deeper));
    }
}

/// The leaves of the chain at `plan`, in order.
pub(super) fn leaves(plan: &Plan) -> Vec<&Plan> {
    let mut out = Vec::new();
    walk(plan, &mut out, 0);
    out
}

/// `leaves`' walk of `plan`, `depth` links down.
fn walk<'p>(plan: &'p Plan, out: &mut Vec<&'p Plan>, depth: usize) {
    if depth >= LINKS || !link(plan) {
        return out.push(plan);
    }
    let deeper = depth.saturating_add(1);
    if let Some((left, right, ..)) = joined(plan) {
        walk(left, out, deeper);
        walk(right, out, deeper);
    } else if let Plan::Filter { input, .. } | Plan::Project { input, .. } = plan.bare() {
        walk(input, out, deeper);
    }
}

/// The number of leaves of the chain at `plan`.
fn count(plan: &Plan) -> usize {
    leaves(plan).len()
}

/// The leaf and its column that column `column` of the rows of `plan`, a
/// chain whose first leaf is `first`, is, if it is one moved as it is.
fn origin(plan: &Plan, column: usize, (first, depth): (usize, usize), db: &State) -> Option<(usize, usize)> {
    if depth >= LINKS || !link(plan) {
        return Some((first, column));
    }
    let deeper = depth.saturating_add(1);
    if let Some((left, right, _, held, _)) = joined(plan) {
        let at = held.or_else(|| crate::exec::batch_layout(left, db).map(|(_, width)| width))?;
        return match column.checked_sub(at) {
            None => origin(left, column, (first, deeper), db),
            Some(c) => origin(right, c, (first.saturating_add(count(left)), deeper), db),
        };
    }
    if let Plan::Project { exprs, input, .. } = plan.bare() {
        let Some(Expr::Column(c)) = exprs.get(column) else { return None };
        return origin(input, *c, (first, deeper), db);
    }
    let Plan::Filter { input, .. } = plan.bare() else { return None };
    origin(input, column, (first, deeper), db)
}

/// Each key of each join of the chain at `plan` that is a column of a leaf
/// on each side, as the module says; of a chain with a LEFT join, none on
/// which nulls match.
pub(super) fn keys(plan: &Plan, db: &State) -> Vec<Key> {
    let mut out = Vec::new();
    let outer = keys_of(plan, (0, 0), db, &mut out);
    out.retain(|k| !(outer && k.step.nulls));
    out
}

/// `keys`' walk of `plan`, whose first leaf is `first`, `depth` links
/// down: whether it holds a LEFT join.
fn keys_of(plan: &Plan, (first, depth): (usize, usize), db: &State, out: &mut Vec<Key>) -> bool {
    if depth >= LINKS || !link(plan) {
        return false;
    }
    let deeper = depth.saturating_add(1);
    let Some((left, right, keys, _, outer)) = joined(plan) else {
        let (Plan::Filter { input, .. } | Plan::Project { input, .. }) = plan.bare() else { return false };
        return keys_of(input, (first, deeper), db, out);
    };
    let second = first.saturating_add(count(left));
    for k in keys {
        let from = origin(left, k.left, (first, deeper), db);
        let to = origin(right, k.right, (second, deeper), db);
        if let (Some(from), Some(to)) = (from, to) {
            out.push(Key { step: Step { from, to, nulls: k.nulls }, both: !outer });
        }
    }
    let below = keys_of(left, (first, deeper), db, out);
    keys_of(right, (second, deeper), db, out) || below || outer
}
