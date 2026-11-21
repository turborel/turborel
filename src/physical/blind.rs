//! \[DESIGN\] Joins made semi joins under MIN and MAX (RULES.md D542; Yan
//! and Larson, VLDB 1995, §2.2).
//!
//! MIN and MAX give one value of a multiset and of its set, so the rows a
//! join repeats need not be made. Of a grouping of no grouping columns
//! whose every set function is MIN or MAX, the plan of its input, its joins
//! ordered as they cost least, is walked from the top with the columns
//! read above each operator. An inner hash join of whose right operand
//! nothing above reads a column is made a hashed semi join, and an inner
//! index join of whose table nothing above reads is made one that stops at
//! each left row's first match: each left row is given once, not once a
//! match, and the joins above see fewer rows. The order stays the one
//! chosen, so no join costs more than it did.
//!
//! A semi join gives its left's columns alone, so the columns of the
//! operators above are renumbered (`Gone`); a projection gives a null for
//! a column nothing above reads that it could no longer give. An index
//! join whose condition can raise is left, as the first match stops it
//! judging the other pairs (D15).

mod width;

use width::Sized;

use super::{JoinKey, JoinProbe, PhysicalPlanner, Plan, Preserved, SemiJoin, SemiMethod};
use crate::functions::{Registry, Standard};
use crate::logical::{AggCall, Cond, Expr, cond_columns, expr_columns, map_cond_columns, map_expr_columns};
use crate::optimizer::SEMI_REDUCTION;
use crate::value::Value;

/// The columns of an operator's rows the reduction took away, in the
/// numbering of its rows before: runs of a first column and a count.
#[derive(Default)]
struct Gone(Vec<(usize, usize)>);

impl Gone {
    /// The place now of column `c`, none if it was taken away.
    fn at(&self, c: usize) -> Option<usize> {
        let mut before = 0_usize;
        for &(start, count) in &self.0 {
            if (start..start.saturating_add(count)).contains(&c) {
                return None;
            }
            if start < c {
                before = before.saturating_add(count);
            }
        }
        Some(c.saturating_sub(before))
    }

    /// Of a join, its left's gone `self`, and its right's `right`, past the
    /// left's `width` columns.
    fn beside(mut self, right: Gone, width: usize) -> Gone {
        self.0.extend(right.0.into_iter().map(|(start, count)| (start.saturating_add(width), count)));
        self
    }

    /// Column `c` renumbered, kept as it is if it was taken away.
    fn moved(&self, c: usize) -> usize {
        self.at(c).unwrap_or(c)
    }

    /// How many columns were taken away.
    fn count(&self) -> usize {
        self.0.iter().fold(0, |n, &(_, count)| n.saturating_add(count))
    }
}

/// Whether `aggs` are MIN and MAX alone, which the reduction takes.
fn blind(aggs: &[AggCall], functions: &Registry) -> bool {
    let extreme = |a: &AggCall| matches!(functions.standard(&a.func), Some(Standard::Max | Standard::Min));
    !aggs.is_empty() && aggs.iter().all(|a| a.distribution.is_none() && extreme(a))
}

/// `a` of the columns of its input renumbered as `gone` says.
fn renumbered(mut a: AggCall, gone: &Gone) -> AggCall {
    a.arg = a.arg.map(|e| map_expr_columns(e, &|c| gone.moved(c)));
    a.filter = a.filter.map(|c| map_cond_columns(c, &|col| gone.moved(col)));
    a
}

/// `needed`, and the columns `c` reads.
fn with_cond(needed: &[usize], c: Option<&Cond>) -> Vec<usize> {
    let mut out = needed.to_vec();
    if let Some(c) = c {
        cond_columns(c, &mut out);
    }
    out
}

impl PhysicalPlanner<'_> {
    /// The input `plan` of a grouping by `groups` computing `aggs`, which
    /// reads its columns `read`, its joins made semi joins where nothing
    /// above reads their right operand, and `aggs` renumbered so.
    pub(super) fn blind_joins(&self, plan: Plan, (groups, aggs, read): (&[usize], Vec<AggCall>, &[usize])) -> (Plan, Vec<AggCall>) {
        if !groups.is_empty() || !blind(&aggs, &self.db.functions) || !self.db.optimizer.enabled(SEMI_REDUCTION, &self.db.settings) {
            return (plan, aggs);
        }
        let sizes = self.sized(&plan);
        let (plan, gone) = self.reduced(plan, &sizes, read);
        (plan, aggs.into_iter().map(|a| renumbered(a, &gone)).collect())
    }

    /// `plan`, of the widths `sizes`, of which the operators above read
    /// the columns `needed`, reduced, and the columns it took away.
    fn reduced(&self, plan: Plan, sizes: &Sized, needed: &[usize]) -> (Plan, Gone) {
        match plan {
            Plan::Filter { cond, detail, est, of_read, input } => {
                let (input, gone) = self.reduced(*input, sizes.kid(0), &with_cond(needed, Some(&cond)));
                let cond = map_cond_columns(cond, &|c| gone.moved(c));
                (Plan::Filter { cond, detail, est, of_read, input: Box::new(input) }, gone)
            }
            Plan::Project { exprs, detail, input } => (self.projected((exprs, detail, *input), sizes.kid(0), needed), Gone::default()),
            Plan::HashJoin { left, right, keys, detail, est } => self.hash_reduced((*left, *right), (keys, detail, est), (sizes, needed)),
            Plan::IndexJoin { .. } => self.index_reduced(plan, (sizes, needed)),
            other @ (Plan::Nearest(_)
            | Plan::SeqScan { .. }
            | Plan::IndexScan { .. }
            | Plan::Product { .. }
            | Plan::OuterJoin { .. }
            | Plan::MergeJoin(_)
            | Plan::SemiJoin(_)
            | Plan::RangeJoin(_)
            | Plan::Aggregate { .. }
            | Plan::Distinct { .. }
            | Plan::SetOp { .. }
            | Plan::System { .. }
            | Plan::Values { .. }
            | Plan::Append { .. }
            | Plan::MergeAppend { .. }
            | Plan::Sort { .. }
            | Plan::Limit { .. }
            | Plan::Recursive { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. }
            | Plan::Unnest(_)
            | Plan::Window { .. }
            | Plan::Recognize { .. }
            | Plan::Polymorphic(_)
            | Plan::Parallel(_)
            | Plan::Counted(_)) => (other, Gone::default()),
        }
    }

    /// A projection of `exprs` over `input`, of whose columns those above
    /// read `needed`: its input reduced for the expressions read, and of
    /// the others, each that reads a column taken away a null.
    fn projected(&self, (exprs, detail, input): (Vec<Expr>, String, Plan), sizes: &Sized, needed: &[usize]) -> Plan {
        let mut reads = Vec::new();
        for e in exprs.iter().enumerate().filter(|(n, _)| needed.contains(n)).map(|(_, e)| e) {
            expr_columns(e, &mut reads);
        }
        let (input, gone) = self.reduced(input, sizes, &reads);
        if gone.0.is_empty() {
            return Plan::Project { exprs, detail, input: Box::new(input) };
        }
        let pieces: Vec<&str> = detail.split(", ").collect();
        let shown = pieces.len() == exprs.len();
        let (mut out, mut text) = (Vec::with_capacity(exprs.len()), Vec::with_capacity(exprs.len()));
        for (e, piece) in exprs.into_iter().zip(pieces.iter().copied().chain(std::iter::repeat(""))) {
            let mut cols = Vec::new();
            expr_columns(&e, &mut cols);
            let kept = cols.iter().all(|&c| gone.at(c).is_some());
            text.push(if kept { piece } else { "NULL" });
            out.push(if kept { map_expr_columns(e, &|c| gone.moved(c)) } else { Expr::Const(Value::Null) });
        }
        let detail = if shown { text.join(", ") } else { detail };
        Plan::Project { exprs: out, detail, input: Box::new(input) }
    }

    /// An inner hash join of `left` and `right` on `keys`: a semi join if
    /// none of `needed` is of the right, else a hash join of its operands
    /// reduced.
    fn hash_reduced(
        &self,
        (left, right): (Plan, Plan),
        (keys, detail, est): (Vec<JoinKey>, String, f64),
        (sizes, needed): (&Sized, &[usize]),
    ) -> (Plan, Gone) {
        let (Some(lw), Some(rw)) = (sizes.of(0), sizes.of(1)) else {
            return (Plan::HashJoin { left: Box::new(left), right: Box::new(right), keys, detail, est }, Gone::default());
        };
        let mut of_left: Vec<usize> = needed.iter().copied().filter(|&c| c < lw).collect();
        of_left.extend(keys.iter().map(|k| k.left));
        let mut of_right: Vec<usize> = needed.iter().filter_map(|&c| c.checked_sub(lw)).collect();
        let semi = of_right.is_empty();
        of_right.extend(keys.iter().map(|k| k.right));
        let (left, gl) = self.reduced(left, sizes.kid(0), &of_left);
        let (right, gr) = self.reduced(right, sizes.kid(1), &of_right);
        let keys: Vec<JoinKey> =
            keys.into_iter().map(|k| JoinKey { left: gl.moved(k.left), right: gr.moved(k.right), nulls: k.nulls }).collect();
        if !semi {
            let gone = gl.beside(gr, lw);
            return (Plan::HashJoin { left: Box::new(left), right: Box::new(right), keys, detail, est }, gone);
        }
        let now = (lw.saturating_sub(gl.count()), rw.saturating_sub(gr.count()));
        let est = left.est().map_or(est, |rows| rows.min(est));
        let method = SemiMethod::Hash;
        let joined = SemiJoin { anti: false, method, left, right, keys, cond: None, widths: now, detail, est };
        (Plan::SemiJoin(Box::new(joined)), gl.beside(Gone(vec![(0, rw)]), lw))
    }

    /// An inner index join, `plan`: one that stops at each left row's first
    /// match if none of `needed` is of its table and its condition cannot
    /// raise, its left reduced either way, if its probe's columns can be
    /// renumbered.
    fn index_reduced(&self, plan: Plan, (sizes, needed): (&Sized, &[usize])) -> (Plan, Gone) {
        let known = if let Plan::IndexJoin { outer: Preserved::Inner, spatial: None, probe, .. } = &plan {
            sizes.of(0).filter(|_| probe_columns_known(probe))
        } else {
            None
        };
        let Some(lw) = known else { return (plan, Gone::default()) };
        let Plan::IndexJoin {
            outer,
            spatial,
            left: outer_rows,
            table,
            index,
            method,
            probe,
            cond,
            width,
            columns,
            detail,
            est,
            cost,
            located,
        } = plan
        else {
            return (plan, Gone::default());
        };
        let semi = !needed.iter().any(|&c| c >= lw) && !cond.as_ref().is_some_and(Cond::can_raise);
        let mut of_left: Vec<usize> = with_cond(needed, cond.as_ref()).into_iter().filter(|&c| c < lw).collect();
        of_left.extend(probe_columns(&probe));
        let (reduced, gone) = self.reduced(*outer_rows, sizes.kid(0), &of_left);
        let now = lw.saturating_sub(gone.count());
        let pair = |c: usize| if c < lw { gone.moved(c) } else { c.saturating_sub(lw).saturating_add(now) };
        let cond = cond.map(|c| map_cond_columns(c, &pair));
        let probe = renumbered_probe(probe, &gone);
        let outer = if semi { Preserved::Semi } else { outer };
        let left = Box::new(reduced);
        let joined =
            Plan::IndexJoin { outer, spatial, left, table, index, method, probe, cond, width, columns, detail, est, cost, located };
        (joined, if semi { gone.beside(Gone(vec![(0, width)]), lw) } else { gone })
    }
}

/// Whether the columns of the outer row `probe` reads are all known by
/// `probe_columns`.
fn probe_columns_known(probe: &JoinProbe) -> bool {
    match probe {
        JoinProbe::Prefix(_) | JoinProbe::Columns(_) => true,
        JoinProbe::Predicate { beside, .. } => beside.is_none(),
    }
}

/// The columns of the outer row `probe` reads.
fn probe_columns(probe: &JoinProbe) -> Vec<usize> {
    match probe {
        JoinProbe::Prefix(cols) => cols.to_vec(),
        JoinProbe::Columns(compared) => compared.iter().map(|c| c.outer).collect(),
        JoinProbe::Predicate { outer, .. } => vec![*outer],
    }
}

/// `probe` reading the outer row's columns renumbered as `gone` says.
fn renumbered_probe(probe: JoinProbe, gone: &Gone) -> JoinProbe {
    match probe {
        JoinProbe::Prefix(cols) => JoinProbe::Prefix(cols.iter().map(|&c| gone.moved(c)).collect()),
        JoinProbe::Columns(mut compared) => {
            for c in &mut compared {
                c.outer = gone.moved(c.outer);
            }
            JoinProbe::Columns(compared)
        }
        JoinProbe::Predicate { predicate, outer, beside } => JoinProbe::Predicate { predicate, outer: gone.moved(outer), beside },
    }
}
