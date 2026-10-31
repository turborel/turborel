//! \[DESIGN\] Equality joins: a filter over a product whose condition
//! equates a column of each operand is planned as a hash join, which reads
//! one operand once into a hash table and pairs each row of the other with
//! only the rows whose keys are equal, rather than with every row. Split
//! from physical.rs (RULES.md R-SIZE-2).
//!
//! The rows are those the filter keeps: each left row's matches in the
//! order of the right operand, the right built. The equalities are
//! evaluated only on the pairs they could hold for, and the rest of the
//! condition only on the pairs they keep, so the condition must not be
//! able to raise an error (RULES.md D15): a conjunct that can, reading one
//! operand's columns alone, is moved onto that operand before, over a guard
//! (`optimizer::PushRaisingFilterIntoProduct`), and the rest is then this
//! filter's. An outer join is planned the same way, on the equalities of
//! its `<join condition>`, a conjunct that can raise and reads one
//! operand's columns alone evaluated on every row of that operand when the
//! other has a row, before it is matched. Of an inner or a LEFT outer
//! join, few left rows each probe an index of the right operand's table
//! instead, and of a RIGHT one few right rows the left's, if that is
//! estimated cheaper than reading it whole (`physical/indexed.rs`).
//!
//! The operand built is the one of fewer estimated rows (`rows_of`): an
//! inner join whose right operand has more is the join of its operands
//! swapped, the left built, unless the left's order is the order asked
//! for; a RIGHT outer join is the LEFT one of its operands swapped, unless
//! its left has more; each under a projection putting the columns back in
//! order (`Flip`). Swapped, the other operand is read first: of operands
//! that can both raise an error, the other's may be raised, as evaluation
//! order is implementation-dependent [ISO/IEC 9075:1992 §3.3.4.4]
//! (D15); whether one is raised is the same, as each operand is read
//! whole either way. A RIGHT join is swapped only if it keeps no
//! conjunct that can raise (no operand's own, `Parted::keyed_alone`), so
//! the LEFT join is probed by batches as a hash join is, its right moved
//! to a table past the work area, not partitioned (D31).

use super::indexed::{Equijoin, index_join, right_index_join};
use super::{Built, PhysicalPlanner, Plan, of_left};
use crate::logical::{
    Cond, Expr, LogicalPlan, OuterJoin, SortKey, and_all, column_name, cond_columns, judged_by_sets, map_cond_columns, render_cond,
    render_expr, split_and,
};
use crate::value::CompOp;

impl PhysicalPlanner<'_> {
    /// `cond` over `input` as a hash join, if `input` is a product and
    /// `cond` equates a column of each of its operands and cannot raise an
    /// error. The conjuncts that are not such equalities filter its rows.
    pub(super) fn hash_join(&self, cond: &Cond, input: &LogicalPlan, want: &[SortKey]) -> Option<Built> {
        let LogicalPlan::Product { left, right } = input else { return None };
        if cond.can_raise() {
            return None;
        }
        let width = left.width();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut keys, mut rest) = (Vec::new(), Vec::new());
        for c in conjuncts {
            match equated(&c, width) {
                Some(key) => keys.push(key),
                None => rest.push(c),
            }
        }
        if keys.is_empty() {
            return None;
        }
        let names = input.names();
        // As in a product, the rows come in the order of the left operand.
        let want_left = of_left(want, width);
        let (l, r) = (self.build_node(left, want_left), self.build_node(right, &[]));
        let ordered = !want_left.is_empty() && l.ordered;
        // A few left rows each probing an index of the right's table.
        if let Some((probing, rows)) = self.probed_instead((left, right), &keys, &r.plan) {
            let join = Equijoin { width, keys: &keys, rest, outer: false };
            return Some(Built { plan: index_join(probing, join, (l.plan, rows), &names), ordered });
        }
        // The left built instead, if it has fewer rows; its order is lost.
        if !ordered && self.larger(right, left) {
            let flip = Flip { width, right_width: right.width() };
            let flipped = flip.names(&names);
            let rest = rest.into_iter().map(|c| flip.cond(c)).collect();
            let join = hashed((r.plan, l.plan), swapped_keys(&keys), rest, (flip.right_width, &flipped));
            return Some(Built::unordered(flip.restored(join, &flipped)));
        }
        Some(Built { plan: hashed((l.plan, r.plan), keys, rest, (width, &names)), ordered })
    }

    /// Whether `a` is estimated to give more rows than `b`; not if either's
    /// rows are not known.
    fn larger(&self, a: &LogicalPlan, b: &LogicalPlan) -> bool {
        self.rows_of(a).zip(self.rows_of(b)).is_some_and(|(of_a, of_b)| of_a > of_b)
    }
}

/// The hash join of `left` and `right` on `keys`, the left `width` columns
/// wide, its rows filtered by the rest of the condition, `rest`, over its
/// pairs' columns, named `names`.
fn hashed((left, right): (Plan, Plan), keys: Vec<(usize, usize)>, rest: Vec<Cond>, (width, names): (usize, &[String])) -> Plan {
    let detail = keys_detail(&keys, names, width);
    let join = Plan::HashJoin { left: Box::new(left), right: Box::new(right), keys, detail };
    match and_all(rest).map(judged_by_sets) {
        Some(c) => Plan::Filter { detail: render_cond(&c, names), cond: c, est: None, input: Box::new(join) },
        None => join,
    }
}

/// A join of operands `width` and `right_width` columns wide, planned as
/// the join of its operands swapped: the columns of its pairs, its keys and
/// its conditions, of the swapped join's, which a projection puts back in
/// order.
#[derive(Clone, Copy)]
pub(super) struct Flip {
    pub(super) width: usize,
    pub(super) right_width: usize,
}

impl Flip {
    /// The place of the join's column `i` in the swapped join's pairs.
    fn column(self, i: usize) -> usize {
        i.checked_sub(self.width).unwrap_or_else(|| i.saturating_add(self.right_width))
    }

    /// `c`, over the join's columns, over the swapped join's.
    pub(super) fn cond(self, c: Cond) -> Cond {
        map_cond_columns(c, &|i| self.column(i))
    }

    /// The names of the join's columns, `names`, in the swapped join's order.
    pub(super) fn names(self, names: &[String]) -> Vec<String> {
        names.iter().skip(self.width).chain(names.iter().take(self.width)).cloned().collect()
    }

    /// The swapped join `joined`, of columns named `flipped`, under a
    /// projection putting its columns in the join's order.
    pub(super) fn restored(self, joined: Plan, flipped: &[String]) -> Plan {
        let exprs: Vec<Expr> = (0..self.width.saturating_add(self.right_width)).map(|i| Expr::Column(self.column(i))).collect();
        let detail = exprs.iter().map(|e| render_expr(e, flipped)).collect::<Vec<_>>().join(", ");
        Plan::Project { exprs, detail, input: Box::new(joined) }
    }
}

/// A join's keys, pairs of a left column and a right column, as the keys
/// of the join of its operands swapped.
fn swapped_keys(keys: &[(usize, usize)]) -> Vec<(usize, usize)> {
    keys.iter().map(|&(l, r)| (r, l)).collect()
}

/// The equalities `keys` of a join whose left operand is `width` columns
/// wide, as text, of its pairs' column `names`.
fn keys_detail(keys: &[(usize, usize)], names: &[String], width: usize) -> String {
    keys.iter()
        .map(|&(l, r)| format!("{} = {}", column_name(names, l), column_name(names, r.saturating_add(width))))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The columns `c` equates, one of the left operand of a product whose
/// left operand is `width` columns wide and one of the right, numbered in
/// it: "left = right" or "right = left".
fn equated(c: &Cond, width: usize) -> Option<(usize, usize)> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    let (a, b) = (*a, *b);
    if a < width && b >= width {
        Some((a, b.saturating_sub(width)))
    } else if b < width && a >= width {
        Some((b, a.saturating_sub(width)))
    } else {
        None
    }
}

/// The conjuncts of an outer join's `<join condition>`, of operands the
/// first `width` columns of whose pairs are the left's, parted: the
/// equalities of a column of each, its keys; the rest that cannot raise an
/// error; and those that can, of the left's columns alone, or of none, and
/// of the right's alone, renumbered as its own. If one that can raise
/// reads both operands' columns, every conjunct is the rest's: every pair
/// is then judged (RULES.md D15).
struct Parted {
    keys: Vec<(usize, usize)>,
    rest: Vec<Cond>,
    own: (Vec<Cond>, Vec<Cond>),
}

impl Parted {
    fn of(cond: &Cond, width: usize) -> Parted {
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        // Whether a conjunct reads the left's columns alone, or none.
        let lefts = |c: &Cond| {
            let mut columns = Vec::new();
            cond_columns(c, &mut columns);
            columns.iter().all(|&i| i < width)
        };
        let rights = |c: &Cond| {
            let mut columns = Vec::new();
            cond_columns(c, &mut columns);
            columns.iter().all(|&i| i >= width)
        };
        let apart = conjuncts.iter().filter(|c| c.can_raise()).all(|c| lefts(c) || rights(c));
        let mut parted = Parted { keys: Vec::new(), rest: Vec::new(), own: (Vec::new(), Vec::new()) };
        for c in conjuncts {
            if !apart {
                parted.rest.push(c);
            } else if c.can_raise() && lefts(&c) {
                parted.own.0.push(c);
            } else if c.can_raise() {
                parted.own.1.push(map_cond_columns(c, &|i| i.saturating_sub(width)));
            } else if let Some(key) = equated(&c, width) {
                parted.keys.push(key);
            } else {
                parted.rest.push(c);
            }
        }
        parted
    }

    /// It, of the join planned as `flip` says: each operand's own
    /// conditions, over its own columns, the other's.
    fn flipped(self, flip: Flip) -> Parted {
        let Parted { keys, rest, own: (of_left, of_right) } = self;
        Parted { keys: swapped_keys(&keys), rest: rest.into_iter().map(|c| flip.cond(c)).collect(), own: (of_right, of_left) }
    }

    /// Whether the join has keys and no conjunct that can raise an error
    /// (one that can would be an operand's own).
    fn keyed_alone(&self) -> bool {
        !self.keys.is_empty() && self.own.0.is_empty() && self.own.1.is_empty()
    }
}

impl PhysicalPlanner<'_> {
    /// [ISO/IEC 9075:1992 §7.5] The outer join of `left` and `right` on
    /// `cond`: on the equalities of a column of each, through a hash
    /// table, the rest of the condition on the pairs they keep. A conjunct
    /// that can raise an error is evaluated on every row of the operand
    /// whose columns it reads alone when the other has a row, as on every
    /// row of their product [GR1c], before the row is matched; if one
    /// reads both's, or the join has no keys, all of the condition is
    /// evaluated on every pair (RULES.md D15). Its rows come in the order
    /// of `left`, unless the unmatched rows of a preserved `right` follow
    /// them, or it is a RIGHT join planned as the LEFT one of its operands
    /// swapped.
    #[inline(never)]
    pub(super) fn outer_join(&self, (kind, cond): (OuterJoin, &Cond), left: &LogicalPlan, right: &LogicalPlan, want: &[SortKey]) -> Built {
        let width = left.width();
        let parted = Parted::of(cond, width);
        let names: Vec<String> = left.names().into_iter().chain(right.names()).collect();
        let want_left = if kind.keeps_right() { &[] } else { of_left(want, width) };
        let (l, r) = (self.build_node(left, want_left), self.build_node(right, &[]));
        let ordered = !want_left.is_empty() && l.ordered;
        // Of a LEFT join, a few left rows each probing an index of the
        // right's table; of a RIGHT join, a few right rows the left's.
        let swapped = swapped_keys(&parted.keys);
        let probed = if !parted.keyed_alone() {
            None
        } else if kind == OuterJoin::Left {
            self.probed_instead((left, right), &parted.keys, &r.plan).map(|p| (p, false))
        } else if kind == OuterJoin::Right {
            self.probed_instead((right, left), &swapped, &l.plan).map(|p| (p, true))
        } else {
            None
        };
        if let Some(((probing, rows), true)) = probed {
            return Built::unordered(right_index_join(probing, (&swapped, parted.rest), (r.plan, rows), (width, &names)));
        }
        if let Some(((probing, rows), false)) = probed {
            let join = Equijoin { width, keys: &parted.keys, rest: parted.rest, outer: true };
            return Built { plan: index_join(probing, join, (l.plan, rows), &names), ordered };
        }
        // A RIGHT join as the LEFT one of its operands swapped, which
        // builds its left and probes it by batches.
        if kind == OuterJoin::Right && parted.keyed_alone() && !self.larger(left, right) {
            let flip = Flip { width, right_width: right.width() };
            let flipped = flip.names(&names);
            let detail = render_cond(&flip.cond(cond.clone()), &flipped);
            let join = hashed_outer(OuterJoin::Left, (r.plan, l.plan), parted.flipped(flip), ((flip.right_width, width), detail, &flipped));
            return Built::unordered(flip.restored(join, &flipped));
        }
        let plan = hashed_outer(kind, (l.plan, r.plan), parted, ((width, right.width()), render_cond(cond, &names), &names));
        Built { plan, ordered }
    }
}

/// The outer join `kind` of `left` and `right`, of degrees `widths`, on
/// its condition `parted`, as text `detail`, over its pairs' columns,
/// named `names`.
fn hashed_outer(
    kind: OuterJoin,
    (left, right): (Plan, Plan),
    parted: Parted,
    (widths, detail, names): ((usize, usize), String, &[String]),
) -> Plan {
    let Parted { keys, mut rest, own } = parted;
    let width = widths.0;
    let detail = if keys.is_empty() { detail } else { format!("{detail}  (hashed on {})", keys_detail(&keys, names, width)) };
    let own = if keys.is_empty() {
        rest.extend(own.0);
        rest.extend(own.1.into_iter().map(|c| map_cond_columns(c, &|i| i.saturating_add(width))));
        None
    } else {
        let (of_left, of_right) = (and_all(own.0).map(judged_by_sets), and_all(own.1).map(judged_by_sets));
        (of_left.is_some() || of_right.is_some()).then(|| Box::new((of_left, of_right)))
    };
    Plan::OuterJoin {
        kind,
        left: Box::new(left),
        right: Box::new(right),
        keys,
        cond: and_all(rest).map(judged_by_sets),
        own,
        widths,
        detail,
    }
}
