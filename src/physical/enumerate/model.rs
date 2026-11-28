//! \[DESIGN\] What the enumeration estimates by (RULES.md D302): each
//! relation's rows and the costs of reading it, in no order and in each
//! interesting order, the probes its table's indexes answer, the
//! conjuncts of the inner joins, and the predicates of the other
//! operators, all from the statistics as the planner reads them.
//!
//! The rows of a join are those of its operands times the fraction its
//! predicates keep, its equalities by classes (`physical/cardinality/joins.rs`,
//! the formula every estimate of a join uses, RULES.md D370). Other
//! conjuncts are taken as independent, each by its own estimate
//! (`physical/estimate.rs`). A join is estimated to give at least one row,
//! if its operands' product has one, so no estimate of zero makes every
//! join over it free (Leis et al., "How Good Are Query Optimizers,
//! Really?", 2015, §3.4). Each set also has an upper bound, which its rows
//! never pass (`physical/cardinality/bounds.rs`, RULES.md D385): a join to
//! one relation gives each row of the other operand at most the most rows
//! of one value of that relation's column it equates.

use super::bits::{Set, bit, members};
use super::conflicts::Kind;
use super::space::Applied;
use crate::physical::CostModel;
use crate::physical::Plan;
use crate::physical::cardinality::Classes;
use crate::physical::indexed::KeyProbe;

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
    /// At most this many.
    pub(super) upper: f64,
    /// Of its columns a key of the chain uses, by the chain's places, the
    /// most rows of one value.
    pub(super) most: Vec<(usize, f64)>,
    /// The costs of reading it: in no order, then in each order it gives.
    pub(super) reads: Vec<(Order, f64)>,
    /// The plan of each of those reads, kept for the realized plan, which
    /// takes the one its order asks rather than building it again.
    pub(super) plans: Vec<(Order, Plan)>,
    /// The probes an index of its table answers, each by its columns, by
    /// the chain's places, and its cost (`indexed/keyed.rs`, RULES.md D1297).
    pub(super) probes: Vec<KeyProbe>,
    /// \[DESIGN\] (RULES.md D652) The rows its read gives a parallel part,
    /// its stream's, 0 if it is none (`enumerate/weighed.rs`).
    pub(super) held: f64,
    /// \[DESIGN\] (RULES.md D651) The bytes a row of it holds.
    pub(super) bytes: f64,
    /// \[DESIGN\] (RULES.md D652) The share of each thread past the first
    /// a part reading it gains (`cost/parallel.rs`).
    pub(super) gain: f64,
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
    /// The classes of columns the inner joins' keys make equal, each with
    /// the relations its columns are of (`implied_classes`, RULES.md D387).
    pub(super) implied: Vec<(Vec<usize>, Set)>,
    /// Of each relation whose column of such a class is a key of it, the
    /// class's other relations, the relation of fewest rows first: the
    /// edges the graph may take of the class (`graph/implied.rs`,
    /// RULES.md D1204).
    pub(super) stars: Vec<(usize, Set)>,
    /// Which conjuncts and predicates a range join searches by
    /// (`enumerate/ranged.rs`, RULES.md D580).
    pub(super) ranged: super::ranged::Ranges,
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
    /// keys make equal; none of a chain with an outer join, whose keys hold
    /// only above it. Made once, as the model is.
    pub(super) fn key_classes(&self) -> Vec<Vec<usize>> {
        if self.kinds.iter().any(|&k| k != Kind::Inner) {
            return Vec::new();
        }
        let mut classes: Vec<Vec<usize>> = Vec::new();
        for (a, b) in self.conjuncts.iter().filter_map(|c| c.key) {
            let mut merged = vec![a, b];
            classes.retain(|class| {
                let joins = class.contains(&a) || class.contains(&b);
                if joins {
                    merged.extend(class);
                }
                !joins
            });
            merged.sort_unstable();
            merged.dedup();
            classes.push(merged);
        }
        classes
    }

    /// The equalities of a column of `l` and one of `r` the inner joins'
    /// keys imply through others, of a join no conjunct of which reads
    /// both: one of each class with a column on each side, the left's
    /// first. `joined_rows` counts a class's fraction of any join that
    /// holds two of its columns, so such a join judges these, and is never
    /// a product (RULES.md D387).
    pub(super) fn implied_keys(&self, (l, r): (Set, Set)) -> Vec<(usize, usize)> {
        let side = |class: &[usize], s: Set| class.iter().copied().find(|&c| self.holds(s, c));
        self.implied
            .iter()
            .filter(|&&(_, rels)| rels & l != 0 && rels & r != 0)
            .filter_map(|(class, _)| Some((side(class, l)?, side(class, r)?)))
            .collect()
    }

    /// `key_classes`, each with the relations its columns are of, so a
    /// join whose sets hold none of a class's on one side passes it by a
    /// test of bits.
    pub(super) fn implied_classes(&self) -> Vec<(Vec<usize>, Set)> {
        let rels = |class: &[usize]| class.iter().filter_map(|&c| self.rel_of(c)).fold(0, |s, r| s | bit(r));
        self.key_classes()
            .into_iter()
            .map(|class| {
                let of = rels(&class);
                (class, of)
            })
            .collect()
    }

    /// \[DESIGN\] (RULES.md D1204) Of each column of an implied class that
    /// `keyed` says is a key of its relation, the relation and the others
    /// of the class, in ascending order of the relations' rows, so the
    /// star whose join reduces most is offered the graph first.
    pub(super) fn stars_of(&self, keyed: &dyn Fn(usize) -> bool) -> Vec<(usize, Set)> {
        let mut stars: Vec<(usize, Set)> = Vec::new();
        for (class, rels) in &self.implied {
            for hub in class.iter().filter(|&&c| keyed(c)).filter_map(|&c| self.rel_of(c)) {
                let others = rels & !bit(hub);
                if others != 0 && !stars.contains(&(hub, others)) {
                    stars.push((hub, others));
                }
            }
        }
        stars.sort_by(|a, b| self.rows(a.0).total_cmp(&self.rows(b.0)));
        stars
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
        // A semi join keeps the share of the left's rows the pairs reach,
        // at most each, an antijoin the rest: shares of the left, so every
        // order of the set's joins gives it the same rows (RULES.md D540).
        let reached = r1 * (r2 * fraction).min(1.0_f64);
        let rows = match kind {
            Kind::Inner => joined,
            Kind::Left => joined.max(r1),
            Kind::Full => joined.max(r1).max(r2),
            Kind::Semi => reached,
            Kind::Anti => (r1 - reached).max(r1.min(1.0_f64)),
        };
        rows.max(product.min(1.0_f64))
    }

    /// The bound of the rows of the join of `left` and `right`, each a set
    /// and its bound, of kind `kind` on the equalities `keys`, each a
    /// column of the left and one of the right: their bounds' product, or
    /// where one side is one relation, the other's bound times the most
    /// rows of one value of its column; a LEFT join's at most adds the
    /// left's rows, a FULL join's both operands', and a semi join or
    /// antijoin keeps at most its left's. A set holding a relation
    /// of rows given in place of the estimates has none, as those rows
    /// overrule the statistics the bound is drawn from (RULES.md D377).
    pub(super) fn joined_upper(&self, (s1, u1): (Set, f64), (s2, u2): (Set, f64), (kind, keys): (Kind, &[(usize, usize)])) -> f64 {
        if self.injected.iter().any(|&(given, _)| given & (s1 | s2) != 0) {
            return f64::INFINITY;
        }
        let most = |s: Set, column: usize| {
            let rel = super::space::Space::single(s)?;
            self.rels.get(rel)?.most.iter().find(|(c, _)| *c == column).map(|(_, m)| *m)
        };
        let by_right = keys.iter().filter_map(|&(_, b)| most(s2, b)).map(|m| u1 * m);
        let by_left = keys.iter().filter_map(|&(a, _)| most(s1, a)).map(|m| u2 * m);
        let inner = by_right.chain(by_left).fold(u1 * u2, f64::min);
        match kind {
            Kind::Inner => inner,
            Kind::Left => inner + u1,
            Kind::Full => inner + u1 + u2,
            // Each left row at most once.
            Kind::Semi | Kind::Anti => u1,
        }
    }

    /// The kind of operator `op`: inner unless it has a predicate's kind.
    pub(super) fn kind_of(&self, op: usize) -> Kind {
        self.kinds.get(op).copied().unwrap_or(Kind::Inner)
    }

    /// \[DESIGN\] (RULES.md D1297) The probe cost of an index of relation
    /// `rel`'s table by the columns `by` a join equates: the cheapest of its
    /// probes whose columns are all among them, if one is.
    pub(super) fn probe(&self, rel: usize, by: &[usize]) -> Option<f64> {
        let fits = |(columns, _): &&KeyProbe| columns.iter().all(|c| by.contains(c));
        self.rels.get(rel)?.probes.iter().filter(fits).map(|(_, cost)| *cost).reduce(f64::min)
    }

    /// \[DESIGN\] (RULES.md D1232) The distinct values of `column`, a
    /// key of an inner join's equality of a class, as the class's member of
    /// its relation counts them.
    pub(super) fn key_distinct(&self, column: usize) -> Option<f64> {
        let rel = self.rel_of(column)?;
        let class = self.conjuncts.iter().find_map(|c| c.key.filter(|&(a, b)| a == column || b == column).and(c.class))?;
        self.classes.members().get(class)?.iter().find(|(r, _)| *r == rel).map(|&(_, distinct)| distinct)
    }

    /// The relations of `s`'s columns hold `column`.
    pub(super) fn holds(&self, s: Set, column: usize) -> bool {
        self.rel_of(column).is_some_and(|r| members(s).any(|m| m == r))
    }
}
