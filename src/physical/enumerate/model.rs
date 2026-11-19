//! \[DESIGN\] What the enumeration estimates by (RULES.md D302): each
//! relation's rows and the costs of reading it, in no order and in each
//! interesting order, the probes its table's indexes answer, the
//! conjuncts of the inner joins, and the predicates of the other
//! operators, all from the statistics as the planner reads them.
//!
//! The rows of a join are those of its operands times the fraction its
//! predicates keep. An equality of two columns keeps one pair in as many
//! as the more distinct of them has values (Selinger et al., 1979, §4,
//! F = 1/MAX(ICARD)). Equalities chained through one value, as `a = b`,
//! `b = c` and `a = c` are, form a class, and a set's class keeps one row
//! in as many as each of its columns but the least distinct has values, so
//! an equality the others imply, which predicate transitivity adds, is
//! not counted again. Other conjuncts are taken as independent, as the
//! filters of a table are (`physical/estimate.rs`). A join is estimated to
//! give at least one row, if its operands' product has one, so no estimate
//! of zero makes every join over it free (Leis et al., "How Good Are Query
//! Optimizers, Really?", 2015, §3.4).

use super::bits::{Set, bit, members};
use super::conflicts::Kind;
use super::space::Applied;
use crate::physical::CostModel;

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
    /// Of each class, its members: a relation and its column's distinct
    /// values, one for each relation.
    pub(super) classes: Vec<Vec<(usize, f64)>>,
    /// Of each column a key of some join uses, by the chain's places, its
    /// class of orders, and whether a merge join may join by it.
    pub(super) orders: Vec<(usize, usize, bool)>,
    /// Of each relation, its first column and how many it has.
    pub(super) spans: Vec<(usize, usize)>,
    /// The weights.
    pub(super) cost: CostModel,
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

    /// The relation whose column the chain's column `column` is.
    pub(super) fn rel_of(&self, column: usize) -> Option<usize> {
        self.spans.iter().position(|&(start, width)| (start..start.saturating_add(width)).contains(&column))
    }

    /// The class of orders of column `column`, and whether a merge join
    /// may join by it.
    pub(super) fn order_of(&self, column: usize) -> Option<(usize, bool)> {
        self.orders.iter().find(|(c, _, _)| *c == column).map(|&(_, class, merges)| (class, merges))
    }

    /// The cost of sorting `rows` rows.
    pub(super) fn sort(&self, rows: f64) -> f64 {
        self.cost.sort(rows)
    }

    /// The fraction of the product of the relations of `s` the
    /// equalities of the classes keep: of each class, one in as many as
    /// each of its members in `s` but the least distinct has values.
    fn class_fraction(&self, s: Set) -> f64 {
        self.classes
            .iter()
            .map(|members| {
                let mut distinct: Vec<f64> = members.iter().filter(|(rel, _)| bit(*rel) & s != 0).map(|(_, d)| d.max(1.0_f64)).collect();
                distinct.sort_by(f64::total_cmp);
                distinct.iter().skip(1).map(|d| 1.0_f64 / d).product::<f64>()
            })
            .product()
    }

    /// The rows of the join of `left` and `right`, each a set and its
    /// rows, applying `applied`.
    pub(super) fn joined_rows(&self, (s1, r1): (Set, f64), (s2, r2): (Set, f64), applied: &Applied) -> f64 {
        let product = r1 * r2;
        let fraction = match applied.op.and_then(|o| self.preds.get(o)).and_then(Option::as_ref) {
            Some(pred) => pred.fraction,
            None => {
                let classes = self.class_fraction(s1 | s2) / (self.class_fraction(s1) * self.class_fraction(s2)).max(f64::MIN_POSITIVE);
                let rest: f64 = applied.conjuncts.iter().filter_map(|&c| self.conjuncts.get(c)).filter(|c| c.class.is_none()).map(|c| c.fraction).product();
                classes * rest
            }
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
