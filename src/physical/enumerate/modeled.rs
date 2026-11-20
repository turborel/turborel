//! \[DESIGN\] The model of a chain's initial tree, from the planner's
//! estimates (RULES.md D302): each relation's rows (`rows_of`), the cost of
//! its best read in no order and in each interesting order it gives, from
//! the access paths the planner would choose (`physical/access.rs`), the
//! probes its table's indexes answer (`physical/indexed.rs`), the classes
//! of the equalities of the inner joins with their columns' distinct
//! values (`distinct_of`, from the statistics, a plug-in engine's too), and
//! each other operator's predicate's fraction.
//!
//! An order is interesting if it is the order asked of the chain, or that
//! of a column a merge join may join by: an exact number or a date, whose
//! order is the order its equality compares by, so equal keys are
//! adjacent in it (`exec/join/merge.rs`).

use std::collections::HashMap;

use super::extract::Query;
use super::model::{Conj, Model, Order, Pred, Rel};
use crate::logical::{Cond, Expr, SortKey, split_and};
use crate::physical::PhysicalPlanner;
use crate::physical::estimate::{self, Known};
use crate::physical::indexed::access_cost;
use crate::value::{CompOp, DataType};

/// The columns of an equality of columns of two different relations.
fn equated(c: &Cond, q: &Query) -> Option<(usize, usize)> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    (q.rel_of(*a)? != q.rel_of(*b)?).then_some((*a, *b))
}

/// The root of `c` among `parents`.
fn find(parents: &[usize], mut c: usize) -> usize {
    for _ in 0..parents.len() {
        let Some(&p) = parents.get(c) else { break };
        if p == c {
            break;
        }
        c = p;
    }
    c
}

/// Whether a merge join may join by a column of type `t`.
fn merges(t: Option<&DataType>) -> bool {
    matches!(t, Some(DataType::Exact { .. } | DataType::Date))
}

impl PhysicalPlanner<'_> {
    /// The model of `q`, the chain asked for the order `want`; none if a
    /// relation's rows are not known.
    pub(super) fn model(&self, q: &Query, want: &[SortKey]) -> Option<Model> {
        let cost = self.db.cost();
        let inner: Vec<Option<(usize, usize)>> = q.conjuncts.iter().map(|(_, c)| equated(c, q)).collect();
        let pred_keys: Vec<Vec<(usize, usize)>> = q.preds.iter().map(|p| self.pred_keys(p.as_ref(), q)).collect();
        let mut parents: Vec<usize> = (0..q.width).collect();
        for &(a, b) in inner.iter().flatten() {
            let (ra, rb) = (find(&parents, a), find(&parents, b));
            if let Some(p) = parents.get_mut(ra) {
                *p = rb;
            }
        }
        let mut dense: HashMap<usize, usize> = HashMap::new();
        let mut orders = Vec::new();
        for &col in inner.iter().flatten().chain(pred_keys.iter().flatten()).flat_map(|(a, b)| [a, b]) {
            let next = dense.len();
            let class = *dense.entry(find(&parents, col)).or_insert(next);
            if !orders.iter().any(|&(c, _, _)| c == col) {
                orders.push((col, class, merges(self.chain_type(q, col).as_ref())));
            }
        }
        let class_of = |col: usize| dense.get(&find(&parents, col)).copied();
        let mut classes: Vec<Vec<(usize, f64)>> = vec![Vec::new(); dense.len()];
        for &col in inner.iter().flatten().flat_map(|(a, b)| [a, b]) {
            let (Some(class), Some(rel)) = (class_of(col), q.rel_of(col)) else { continue };
            let members = classes.get_mut(class)?;
            if !members.iter().any(|(r, _)| *r == rel) {
                members.push((rel, self.chain_distinct(q, col)?));
            }
        }
        let known = Known { stats: None, cost: &cost };
        let conjuncts = q
            .conjuncts
            .iter()
            .zip(&inner)
            .map(|((op, c), key)| Conj {
                reads: q.reads(c),
                op: *op,
                fraction: estimate::selectivity(c, known),
                key: *key,
                class: key.and_then(|(a, _)| class_of(a)),
            })
            .collect();
        let preds =
            q.preds.iter().zip(pred_keys).map(|(p, keys)| p.as_ref().map(|c| Pred { fraction: self.pred_fraction(c, q), keys })).collect();
        let want_class = match want {
            [key] if *key == SortKey::ascending(key.column) => {
                orders.iter().find(|&&(c, _, merge)| c == key.column && merge).map(|&(_, class, _)| class)
            }
            _ => None,
        };
        let spans = q.leaves.iter().map(|l| (l.start, l.width)).collect();
        let mut model = Model {
            rels: Vec::new(),
            conjuncts,
            preds,
            kinds: q.shapes.iter().map(|s| s.kind).collect(),
            classes,
            orders,
            spans,
            cost,
            want_class,
        };
        model.rels = (0..q.leaves.len()).map(|r| self.rel(q, r, (&model, want))).collect::<Option<_>>()?;
        Some(model)
    }

    /// The type of the chain's column `col`, if it is a table's.
    fn chain_type(&self, q: &Query, col: usize) -> Option<DataType> {
        let leaf = q.leaves.get(q.rel_of(col)?)?;
        self.column_type(&leaf.plan, col.checked_sub(leaf.start)?)
    }

    /// The distinct values of the chain's column `col`, else its
    /// relation's rows, as if a key.
    fn chain_distinct(&self, q: &Query, col: usize) -> Option<f64> {
        let leaf = q.leaves.get(q.rel_of(col)?)?;
        self.distinct_of(&leaf.plan, col.checked_sub(leaf.start)?).or_else(|| self.rows_of(&leaf.plan))
    }

    /// The equalities of a column of each operand `pred` holds, the left
    /// operand's column first, of an operator other than an inner join.
    fn pred_keys(&self, pred: Option<&Cond>, q: &Query) -> Vec<(usize, usize)> {
        let mut conjuncts = Vec::new();
        if let Some(c) = pred {
            split_and(c.clone(), &mut conjuncts);
        }
        conjuncts.iter().filter_map(|c| equated(c, q)).collect()
    }

    /// The fraction of pairs `pred` keeps: of each equality of columns of
    /// two relations, one in as many as the more distinct has values; of
    /// each other conjunct, its default.
    fn pred_fraction(&self, pred: &Cond, q: &Query) -> f64 {
        let cost = self.db.cost();
        let known = Known { stats: None, cost: &cost };
        let mut conjuncts = Vec::new();
        split_and(pred.clone(), &mut conjuncts);
        conjuncts
            .iter()
            .map(|c| match equated(c, q) {
                Some((a, b)) => {
                    let d = self.chain_distinct(q, a).unwrap_or(1.0_f64).max(self.chain_distinct(q, b).unwrap_or(1.0_f64));
                    1.0_f64 / d.max(1.0_f64)
                }
                None => estimate::selectivity(c, known),
            })
            .product()
    }

    /// Relation `r` of `q`: its rows, its reads in no order and in each
    /// interesting order of `model` or `want` it gives, and its probes.
    fn rel(&self, q: &Query, r: usize, (model, want): (&Model, &[SortKey])) -> Option<Rel> {
        let leaf = q.leaves.get(r)?;
        let rows = self.rows_of(&leaf.plan)?;
        let read = |keys: &[SortKey]| {
            let built = self.build(&leaf.plan, keys);
            (built.ordered, access_cost(&built.plan).unwrap_or(rows))
        };
        let mut reads = vec![(Order::None, read(&[]).1)];
        let local = |c: usize| c.checked_sub(leaf.start).filter(|&l| l < leaf.width);
        for &(col, class, merge) in &model.orders {
            let Some(at) = local(col).filter(|_| merge) else { continue };
            if reads.iter().any(|(o, _)| *o == Order::Class(class)) {
                continue;
            }
            if let (true, cost) = read(&[SortKey::ascending(at)]) {
                reads.push((Order::Class(class), cost));
            }
        }
        let wanted: Option<Vec<SortKey>> = want.iter().map(|k| Some(SortKey { column: local(k.column)?, ..*k })).collect();
        if let Some(keys) = wanted.filter(|k| !k.is_empty())
            && let (true, cost) = read(&keys)
        {
            reads.push((Order::Want, cost));
        }
        let any = |_: usize, _: &DataType| true;
        let probes = (0..leaf.width)
            .filter_map(|c| self.probing(&leaf.plan, &[(0, c)], &any).map(|p| (c.saturating_add(leaf.start), p.cost)))
            .collect();
        Some(Rel { rows, reads, probes })
    }
}
