//! \[DESIGN\] The model of a chain's initial tree, from the planner's
//! estimates (RULES.md D302): each relation's rows (`physical/cardinality.rs`), the cost of
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

use super::bits::bit;
use super::extract::Query;
use super::model::{Conj, Model, Order, Pred, Rel};
use crate::logical::{Cond, Expr, SortKey, split_and};
use crate::physical::PhysicalPlanner;
use crate::physical::cardinality::{Classes, find};
use crate::physical::estimate::{self, Known};
use crate::physical::indexed::{access_cost, reads_table};
use crate::value::{CompOp, DataType};

/// The columns of an equality of columns of two different relations.
fn equated(c: &Cond, q: &Query) -> Option<(usize, usize)> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    (q.rel_of(*a)? != q.rel_of(*b)?).then_some((*a, *b))
}

/// The equalities of a column of each operand `pred` holds, the left
/// operand's column first, of an operator other than an inner join.
fn pred_keys(pred: Option<&Cond>, q: &Query) -> Vec<(usize, usize)> {
    let mut conjuncts = Vec::new();
    if let Some(c) = pred {
        split_and(c.clone(), &mut conjuncts);
    }
    conjuncts.iter().filter_map(|c| equated(c, q)).collect()
}

/// The class whose ascending order is `want`'s, if it asks for one
/// column's ascending order, nulls as implied, of a column of `orders` a
/// merge join may join by.
fn wanted_class(want: &[SortKey], orders: &[(usize, usize, bool)]) -> Option<usize> {
    let [key] = want else { return None };
    if *key != SortKey::ascending(key.column) {
        return None;
    }
    orders.iter().find(|&&(c, _, merge)| c == key.column && merge).map(|&(_, class, _)| class)
}

/// The classes of a chain's equalities: of each column a key uses, its
/// class and whether a merge join may join by it; of each class, its
/// members and their distinct values; and of each conjunct, its class.
struct Classed {
    orders: Vec<(usize, usize, bool)>,
    classes: Vec<Vec<(usize, f64)>>,
    of_conjunct: Vec<Option<usize>>,
}

/// Whether a merge join may join by a column of type `t`.
fn merges(t: Option<&DataType>) -> bool {
    matches!(t, Some(DataType::Exact { .. } | DataType::Date))
}

impl PhysicalPlanner<'_> {
    /// The model of `q`, the chain asked for the order `want`; none if a
    /// relation's rows are not known.
    pub(super) fn model(&self, q: &Query, want: &[SortKey]) -> Option<Model> {
        let cost = self.weights();
        let inner: Vec<Option<(usize, usize)>> = q.conjuncts.iter().map(|(_, c)| equated(c, q)).collect();
        let pred_keys: Vec<Vec<(usize, usize)>> = q.preds.iter().map(|p| pred_keys(p.as_ref(), q)).collect();
        let classed = self.classed(q, &inner, &pred_keys)?;
        let known = Known { stats: None, cost: &cost };
        let conjuncts = q
            .conjuncts
            .iter()
            .zip(inner.iter().zip(&classed.of_conjunct))
            .map(|((op, c), (key, class))| Conj {
                reads: q.reads(c),
                op: *op,
                fraction: estimate::selectivity(c, known),
                key: *key,
                class: *class,
            })
            .collect();
        let preds =
            q.preds.iter().zip(pred_keys).map(|(p, keys)| p.as_ref().map(|c| Pred { fraction: self.pred_fraction(c, q), keys })).collect();
        let want_class = wanted_class(want, &classed.orders);
        let spans = q.leaves.iter().map(|l| (l.start, l.width)).collect();
        let kinds = q.shapes.iter().map(|s| s.kind).collect();
        let Classed { orders, classes, .. } = classed;
        let joins = Classes::new(classes);
        let injected = self
            .injected_sets(q.leaves.iter().map(|l| &l.plan))
            .into_iter()
            .map(|(rels, rows)| (rels.into_iter().fold(0, |s, r| s | bit(r)), rows))
            .collect();
        let mut model = Model {
            rels: Vec::new(),
            conjuncts,
            preds,
            kinds,
            classes: joins,
            orders,
            spans,
            cost,
            want_class,
            injected,
            implied: Vec::new(),
            ranged: self.ranges(q),
        };
        model.rels = (0..q.leaves.len()).map(|r| self.rel(q, r, (&model, want))).collect::<Option<_>>()?;
        model.implied = model.implied_classes();
        Some(model)
    }

    /// The classes of the equalities `inner` of the inner joins, by union of
    /// their columns, with the columns of the equalities `pred_keys` of the
    /// other operators each in its own unless an inner one joins it; none
    /// if a column's distinct values are not known.
    fn classed(&self, q: &Query, inner: &[Option<(usize, usize)>], pred_keys: &[Vec<(usize, usize)>]) -> Option<Classed> {
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
        let of_conjunct = inner.iter().map(|key| key.and_then(|(a, _)| class_of(a))).collect();
        Some(Classed { orders, classes, of_conjunct })
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
        Some(self.distinct_of(&leaf.plan, col.checked_sub(leaf.start)?))
    }

    /// The fraction of pairs `pred` keeps: of each equality of columns of
    /// two relations, one in as many as the more distinct has values; of
    /// each other conjunct, its default.
    fn pred_fraction(&self, pred: &Cond, q: &Query) -> f64 {
        let cost = self.weights();
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

    /// Relation `r` of `q`: its rows (`physical/cardinality.rs`), its
    /// reads in no order and in each interesting order of `model` or
    /// `want` it gives, each read's plan kept, and the probes of its
    /// columns a key of the chain uses, the only ones a join can probe by.
    fn rel(&self, q: &Query, r: usize, (model, want): (&Model, &[SortKey])) -> Option<Rel> {
        let leaf = q.leaves.get(r)?;
        let rows = self.card(&leaf.plan).rows;
        let mut rel = Rel { rows, reads: Vec::new(), plans: Vec::new(), probes: Vec::new() };
        // A relation that is no table's read, a grouping or a join, costs
        // the same in every order, so it is costed as its rows and planned
        // once, as it is realized. Planning it here too, its own relations
        // so at each level below it, took time exponential in the nesting.
        if !reads_table(&leaf.plan) {
            rel.reads.push((Order::None, rows));
            return Some(rel);
        }
        let mut read = |order: Order, keys: &[SortKey]| {
            let built = self.build(&leaf.plan, keys);
            let taken = built.ordered || order == Order::None;
            if taken {
                rel.reads.push((order, access_cost(&built.plan).unwrap_or(rows)));
                rel.plans.push((order, built.plan));
            }
            taken
        };
        let _ = read(Order::None, &[]);
        let local = |c: usize| c.checked_sub(leaf.start).filter(|&l| l < leaf.width);
        let mut classes: Vec<usize> = Vec::new();
        for &(col, class, merge) in &model.orders {
            let Some(at) = local(col).filter(|_| merge && !classes.contains(&class)) else { continue };
            if read(Order::Class(class), &[SortKey::ascending(at)]) {
                classes.push(class);
            }
        }
        let wanted: Option<Vec<SortKey>> = want.iter().map(|k| Some(SortKey { column: local(k.column)?, ..*k })).collect();
        if let Some(keys) = wanted.filter(|k| !k.is_empty()) {
            let _ = read(Order::Want, &keys);
        }
        let any = |_: usize, _: &DataType| true;
        let keyed = model.orders.iter().filter_map(|&(col, _, _)| local(col).map(|at| (col, at)));
        rel.probes = keyed.filter_map(|(col, at)| self.probing(&leaf.plan, &[(0, at)], &any).map(|p| (col, p.cost))).collect();
        Some(rel)
    }
}
