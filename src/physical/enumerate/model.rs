//! \[DESIGN\] What the enumeration estimates by (RULES.md D302): each
//! relation's rows and the costs of reading it, in no order and in each
//! interesting order, the probes its table's indexes answer, the
//! conjuncts of the inner joins, and the predicates of the other
//! operators, all from the statistics as the planner reads them.
//!
//! The rows of a join are those of its operands times the fraction its
//! predicates keep, its equalities by classes (`physical/cardinality/joins.rs`,
//! the formula every estimate of a join uses, RULES.md D370). Other
//! conjuncts are taken as independent, as the filters of a table are
//! (`physical/estimate.rs`). A join is estimated to give at least one row,
//! if its operands' product has one, so no estimate of zero makes every
//! join over it free (Leis et al., "How Good Are Query Optimizers,
//! Really?", 2015, §3.4).

use super::bits::{Set, bit, members};
use super::conflicts::Kind;
use super::space::Applied;
use crate::physical::CostModel;
use crate::physical::Plan;
use crate::physical::cardinality::Classes;

/// An order of a plan's rows: none, ascending by the columns of a class,
/// or the order asked of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Order {
    None,
    Class(usize),
    Want,
}

/// A relation, as the enumeration sees it.
#[derive(Debug, Clone, Default)]
pub(super) struct Rel {
    /// Its estimated rows.
    pub(super) rows: f64,
    /// The costs of reading it: in no order, then in each order it gives.
    pub(super) reads: Vec<(Order, f64)>,
    /// The plan of each of those reads, kept for the realized plan, which
    /// takes the one its order asks rather than building it again.
    pub(super) plans: Vec<(Order, Plan)>,
    /// Of its columns, by the chain's places, those an index of its table
    /// answers an equality of, and the cost of each probe.
    pub(super) probes: Vec<(usize, f64)>,
}

/// A conjunct of an inner join: the relations it reads, the operator it is
/// of, the fraction it keeps if it is no equality of a class, its columns
/// if it is an equality of two, and its class if one.
#[derive(Debug, Clone, Copy)]
pub(super) struct Conj {
    pub(super) reads: Set,
    pub(super) op: usize,
    pub(super) fraction: f64,
    pub(super) key: Option<(usize, usize)>,
    pub(super) class: Option<usize>,
}

/// An operator other than an inner join: the fraction of pairs its
/// predicate keeps, and the equalities of a column of each operand it
/// holds, the left operand's first.
#[derive(Debug, Clone, Default)]
pub(super) struct Pred {
    pub(super) fraction: f64,
    pub(super) keys: Vec<(usize, usize)>,
}

/// What the enumeration estimates by.
#[derive(Debug, Clone)]
pub(super) struct Model {
    pub(super) rels: Vec<Rel>,
    pub(super) conjuncts: Vec<Conj>,
    /// Of each operator, its predicate's, if it is no inner join.
    pub(super) preds: Vec<Option<Pred>>,
    /// Of each operator, its kind.
    pub(super) kinds: Vec<Kind>,
    /// The classes of the inner joins' equalities, of relations.
    pub(super) classes: Classes,
    /// Of each column a key of some join uses, by the chain's places, its
    /// class of orders, and whether a merge join may join by it.
    pub(super) orders: Vec<(usize, usize, bool)>,
    /// Of each relation, its first column and how many it has.
    pub(super) spans: Vec<(usize, usize)>,
    /// The class whose ascending order is the order asked of the chain, if
    /// it asks one key's, ascending with nulls as implied, of a column a
    /// merge join may join by: a merge join's rows answer it too.
    pub(super) want_class: Option<usize>,
    /// The weights.
    pub(super) cost: CostModel,
    /// \[EXTENSION\] The rows given of sets of its relations in place of
    /// their estimates (`physical/cardinality/injected.rs`, RULES.md D377).
    pub(super) injected: Vec<(Set, f64)>,
}

impl Model {
    /// The rows of relation `rel`.
    pub(super) fn rows(&self, rel: usize) -> f64 {
        self.rels.get(rel).map_or(0.0_f64, |r| r.rows)
    }

    /// The reads of relation `rel` and their costs.
    pub(super) fn reads(&self, rel: usize) -> Vec<(Order, f64)> {
        self.rels.get(rel).map(|r| r.reads.clone()).unwrap_or_default()
    }

    /// The relations conjunct `c` reads.
    pub(super) fn reads_of(&self, c: usize) -> Set {
        self.conjuncts.get(c).map_or(0, |c| c.reads)
    }

    /// \[DESIGN\] (RULES.md D387) The classes of columns the inner joins'
    /// keys make equal, each with the operator of one of its keys; none of
    /// a chain with an outer join, whose keys hold only above it.
    fn key_classes(&self) -> Vec<(Vec<usize>, usize)> {
        if self.kinds.iter().any(|&k| k != Kind::Inner) {
            return Vec::new();
        }
        let mut classes: Vec<(Vec<usize>, usize)> = Vec::new();
        for c in &self.conjuncts {
            let Some((a, b)) = c.key else { continue };
            let mut merged = vec![a, b];
            classes.retain(|(class, _)| {
                let joins = class.contains(&a) || class.contains(&b);
                if joins {
                    merged.extend(class);
                }
                !joins
            });
            classes.push((merged, c.op));
        }
        classes
    }

    /// The equalities of a column of `l` and one of `r` the inner joins'
    /// keys imply through others: one of each class with a column on each
    /// side and no key stated of the two, the left's first. `joined_rows`
    /// counts a class's fraction of any join that holds two of its
    /// columns, so a join of the two judges these though no conjunct
    /// reads both, and is never a product (RULES.md D387).
    pub(super) fn implied_keys(&self, (l, r): (Set, Set)) -> Vec<(usize, usize)> {
        let straddles = |(a, b): (usize, usize)| (self.holds(l, a) && self.holds(r, b)) || (self.holds(l, b) && self.holds(r, a));
        let stated = |class: &[usize]| self.conjuncts.iter().filter_map(|c| c.key).any(|(a, b)| class.contains(&a) && straddles((a, b)));
        let side = |class: &[usize], s: Set| class.iter().copied().find(|&c| self.holds(s, c));
        self.key_classes()
            .iter()
            .filter(|(class, _)| !stated(class))
            .filter_map(|(class, _)| Some((side(class, l)?, side(class, r)?)))
            .collect()
    }

    /// The pairs of relations of a class of the inner joins' keys no key
    /// joins, each with the operator of one of its keys: the edges the
    /// enumeration joins them by (`Graph::implied`, RULES.md D387).
    pub(super) fn implied_pairs(&self) -> Vec<(usize, Set)> {
        let mut out: Vec<(usize, Set)> = Vec::new();
        for (class, op) in self.key_classes() {
            let rels: Vec<usize> = class.iter().filter_map(|&c| self.rel_of(c)).collect();
            let pairs = rels.iter().enumerate().flat_map(|(i, &a)| rels.iter().skip(i.saturating_add(1)).map(move |&b| (a, b)));
            for pair in pairs.filter(|&(a, b)| a != b).map(|(a, b)| bit(a) | bit(b)) {
                let keyed = self.conjuncts.iter().any(|c| c.key.is_some() && c.reads == pair);
                if !keyed && !out.iter().any(|&(_, p)| p == pair) {
                    out.push((op, pair));
                }
            }
        }
        out
    }

    /// The relation whose column the chain's column `column` is.
    pub(super) fn rel_of(&self, column: usize) -> Option<usize> {
        self.spans.iter().position(|&(start, width)| (start..start.saturating_add(width)).contains(&column))
    }

    /// The class of orders of column `column`, and whether a merge join
    /// may join by it.
    pub(super) fn order_of(&self, column: usize) -> Option<(usize, bool)> {
        self.orders.iter().find(|(c, _, _)| *c == column).map(|&(_, class, merges)| (class, merges))
    }

    /// Whether a plan in the order `order` is in the order asked.
    pub(super) fn answers(&self, order: Order) -> bool {
        match order {
            Order::Want => true,
            Order::Class(k) => self.want_class == Some(k),
            Order::None => false,
        }
    }

    /// The cost of sorting `rows` rows.
    pub(super) fn sort(&self, rows: f64) -> f64 {
        self.cost.sort(rows)
    }

    /// The fraction of the product of the relations of `s` the
    /// equalities of the classes keep: of each class, one in as many as
    /// each of its members in `s` but the least distinct has values.
    fn class_fraction(&self, s: Set) -> f64 {
        self.classes.fraction(|rel| bit(rel) & s != 0)
    }

    /// The plan read of relation `rel` in the order `order`, if the model
    /// made one.
    pub(super) fn plan_of(&self, rel: usize, order: Order) -> Option<&Plan> {
        self.rels.get(rel)?.plans.iter().find(|(o, _)| *o == order).map(|(_, plan)| plan)
    }

    /// The rows of the join of `left` and `right`, each a set and its
    /// rows, applying `applied`.
    pub(super) fn joined_rows(&self, (s1, r1): (Set, f64), (s2, r2): (Set, f64), applied: &Applied) -> f64 {
        if let Some(&(_, given)) = self.injected.iter().find(|(s, _)| *s == s1 | s2) {
            return given;
        }
        let product = r1 * r2;
        let fraction = if let Some(pred) = applied.op.and_then(|o| self.preds.get(o)).and_then(Option::as_ref) {
            pred.fraction
        } else {
            let classes = self.class_fraction(s1 | s2) / (self.class_fraction(s1) * self.class_fraction(s2)).max(f64::MIN_POSITIVE);
            let rest: f64 =
                applied.conjuncts.iter().filter_map(|&c| self.conjuncts.get(c)).filter(|c| c.class.is_none()).map(|c| c.fraction).product();
            classes * rest
        };
        let joined = product * fraction;
        let kind = applied.op.map_or(Kind::Inner, |o| self.kind_of(o));
        let rows = match kind {
            Kind::Inner => joined,
            Kind::Left => joined.max(r1),
            Kind::Full => joined.max(r1).max(r2),
        };
        rows.max(product.min(1.0_f64))
    }

    /// The kind of operator `op`: inner unless it has a predicate's kind.
    pub(super) fn kind_of(&self, op: usize) -> Kind {
        self.kinds.get(op).copied().unwrap_or(Kind::Inner)
    }

    /// The probe cost of an index of relation `rel`'s table for column
    /// `column`, if one answers an equality of it.
    pub(super) fn probe(&self, rel: usize, column: usize) -> Option<f64> {
        self.rels.get(rel)?.probes.iter().filter(|(c, _)| *c == column).map(|(_, cost)| *cost).reduce(f64::min)
    }

    /// The relations of `s`'s columns hold `column`.
    pub(super) fn holds(&self, s: Set, column: usize) -> bool {
        self.rel_of(column).is_some_and(|r| members(s).any(|m| m == r))
    }
}
