//! \[DESIGN\] A chain of joins taken apart into the initial operator tree
//! the enumeration starts from (RULES.md D300, D301): its relations, the
//! operands that are no product, filter or outer join, each with the
//! conjuncts of the filters above it that read it alone; its products,
//! inner joins, each with the conjuncts that read both its operands and
//! are below no other join that has all their columns; and its outer
//! joins, each with its `<join condition>`, a RIGHT one as the LEFT one of
//! its operands swapped. Columns are the chain's, in the FROM clause's
//! order.
//!
//! A conjunct goes down into an outer join's preserved operand only
//! (the optimizer's `push_filter_into_outer_join` has moved the rest
//! where they may go). One that would have to stay over an outer join, a
//! condition that can raise an error (RULES.md D15) by the declared types
//! of the columns it reads (`logical/raises.rs`, D373), or an equality of
//! collated sort keys (`physical/collated.rs`) leaves the chain to the
//! plan it was given. The optimizer makes the parts of conditions that
//! can raise or are sort keys, of one operand alone, columns of that
//! operand (`optimizer/hoisting.rs`), so what is left is of two.

use super::bits::{Set, bit, members};
use super::conflicts::{Kind, Shape};
use crate::catalog::Catalog;
use crate::logical::{
    Cond, Expr, LogicalPlan, OuterJoin, Types, and_all, cond_can_raise_typed, cond_columns, map_cond_columns, plan_column_type, split_and,
};
use crate::optimizer::is_guarded;

/// A relation of the chain: its plan, its own filter's conjuncts within,
/// and its first column in the chain's columns, and how many it has.
#[derive(Debug, Clone)]
pub(super) struct Leaf {
    pub(super) plan: LogicalPlan,
    pub(super) start: usize,
    pub(super) width: usize,
}

/// A node of the initial tree: a relation, or an operator.
#[derive(Debug, Clone, Copy)]
pub(super) enum Node {
    Leaf(usize),
    Op(usize),
}

/// The initial operator tree of a chain.
#[derive(Debug, Clone, Default)]
pub(super) struct Query {
    pub(super) leaves: Vec<Leaf>,
    /// Of each operator, its shape and its operands.
    pub(super) shapes: Vec<Shape>,
    pub(super) operands: Vec<(Node, Node)>,
    /// The conjuncts of inner joins, each with its operator.
    pub(super) conjuncts: Vec<(usize, Cond)>,
    /// Of each operator other than an inner join, its condition.
    pub(super) preds: Vec<Option<Cond>>,
    pub(super) width: usize,
}

/// The chain as found, before its conjuncts are placed.
enum Raw {
    Leaf(usize),
    Filtered(Vec<Cond>, Box<Raw>),
    Product(Box<Raw>, Box<Raw>),
    Outer(Kind, Cond, Box<Raw>, Box<Raw>),
}

/// Whether `plan` is a chain: a product, an outer join, or a filter over a
/// chain, not over a guard (`optimizer::is_guarded`).
pub(super) fn is_chain(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Filter { input, .. } => !is_guarded(plan) && is_chain(input),
        LogicalPlan::Product { .. } | LogicalPlan::OuterJoin { .. } => true,
        LogicalPlan::Scan { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..) => false,
    }
}

/// Whether `c`, over the chain's columns of the types `types` gives,
/// cannot be moved: it can raise an error, or compares sort keys.
fn fixed(c: &Cond, types: Types<'_>) -> bool {
    let mut keyed = false;
    c.for_each_expr(&mut |e| keyed = keyed || matches!(e, Expr::SortKey(..)));
    keyed || cond_can_raise_typed(c, types)
}

/// The chain `plan`, its columns beginning at `start`, its relations
/// added to `leaves`, the chain's columns of the types `types` gives.
fn raw(plan: &LogicalPlan, (start, types): (usize, Types<'_>), leaves: &mut Vec<Leaf>, depth: usize) -> Option<Raw> {
    if depth > super::bits::MAX_RELATIONS.saturating_mul(2) {
        return None;
    }
    let shifted = |c: &Cond| map_cond_columns(c.clone(), &|i| i.saturating_add(start));
    let deeper = depth.saturating_add(1);
    match plan {
        LogicalPlan::Product { left, right } => {
            let l = raw(left, (start, types), leaves, deeper)?;
            Some(Raw::Product(Box::new(l), Box::new(raw(right, (start.saturating_add(left.width()), types), leaves, deeper)?)))
        }
        LogicalPlan::Filter { cond, input } => {
            if !is_chain(plan) {
                return Some(leaf(plan, start, leaves));
            }
            let mut conds = Vec::new();
            split_and(shifted(cond), &mut conds);
            if conds.iter().any(|c| fixed(c, types)) {
                return None;
            }
            Some(Raw::Filtered(conds, Box::new(raw(input, (start, types), leaves, deeper)?)))
        }
        LogicalPlan::OuterJoin { kind, cond, left, right } => {
            if fixed(&shifted(cond), types) {
                return None;
            }
            let l = raw(left, (start, types), leaves, deeper)?;
            let r = raw(right, (start.saturating_add(left.width()), types), leaves, deeper)?;
            Some(match kind {
                OuterJoin::Left => Raw::Outer(Kind::Left, shifted(cond), Box::new(l), Box::new(r)),
                OuterJoin::Right => Raw::Outer(Kind::Left, shifted(cond), Box::new(r), Box::new(l)),
                OuterJoin::Full => Raw::Outer(Kind::Full, shifted(cond), Box::new(l), Box::new(r)),
            })
        }
        LogicalPlan::Scan { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..) => Some(leaf(plan, start, leaves)),
    }
}

/// `plan` as a relation of the chain, its columns beginning at `start`.
fn leaf(plan: &LogicalPlan, start: usize, leaves: &mut Vec<Leaf>) -> Raw {
    leaves.push(Leaf { plan: plan.clone(), start, width: plan.width() });
    Raw::Leaf(leaves.len().saturating_sub(1))
}

/// The initial operator tree of the chain `plan`, if its conjuncts may all
/// be placed, its columns' types those `catalog` declares.
pub(super) fn extract(plan: &LogicalPlan, catalog: &dyn Catalog) -> Option<Query> {
    let mut query = Query { width: plan.width(), ..Query::default() };
    let types = |c: usize| plan_column_type(plan, c, catalog);
    let tree = raw(plan, (0, &types), &mut query.leaves, 0)?;
    let mut filters: Vec<Vec<Cond>> = vec![Vec::new(); query.leaves.len()];
    query.place(tree, Vec::new(), &mut filters)?;
    for (leaf, conds) in query.leaves.iter_mut().zip(filters) {
        let start = leaf.start;
        let own = conds.into_iter().map(|c| map_cond_columns(c, &|i| i.saturating_sub(start))).collect();
        leaf.plan = filtered(std::mem::replace(&mut leaf.plan, LogicalPlan::Values { rows: Vec::new(), types: Vec::new() }), own);
    }
    Some(query)
}

/// `plan` under the conjuncts `conds` too: in its own filter, unless that
/// is over a guard.
fn filtered(plan: LogicalPlan, mut conds: Vec<Cond>) -> LogicalPlan {
    if conds.is_empty() {
        return plan;
    }
    if let LogicalPlan::Filter { cond, input } = &plan
        && !is_guarded(&plan)
    {
        conds.insert(0, cond.clone());
        let Some(joined) = and_all(conds) else { return plan };
        return LogicalPlan::Filter { cond: joined, input: input.clone() };
    }
    match and_all(conds) {
        Some(cond) => LogicalPlan::Filter { cond, input: Box::new(plan) },
        None => plan,
    }
}

impl Query {
    /// The relations whose columns `c` reads.
    pub(super) fn reads(&self, c: &Cond) -> Set {
        let mut columns = Vec::new();
        cond_columns(c, &mut columns);
        columns.iter().filter_map(|&col| self.rel_of(col)).fold(0, |s, r| s | bit(r))
    }

    /// The relation whose column the chain's column `column` is.
    pub(super) fn rel_of(&self, column: usize) -> Option<usize> {
        self.leaves.iter().position(|l| (l.start..l.start.saturating_add(l.width)).contains(&column))
    }

    /// Places `pending`, the conjuncts of the filters over `raw`, and
    /// those of `raw`'s own, as the module says, and adds its operators.
    fn place(&mut self, raw: Raw, mut pending: Vec<Cond>, filters: &mut [Vec<Cond>]) -> Option<Node> {
        match raw {
            Raw::Leaf(r) => {
                filters.get_mut(r)?.extend(pending);
                Some(Node::Leaf(r))
            }
            Raw::Filtered(conds, inner) => {
                pending.extend(conds);
                self.place(*inner, pending, filters)
            }
            Raw::Product(l, r) => self.product((*l, *r), pending, filters),
            Raw::Outer(kind, cond, l, r) => self.outer((kind, cond), (*l, *r), pending, filters),
        }
    }

    /// A product of `l` and `r`: each pending conjunct down to the operand
    /// holding its columns, or this inner join's; one of no column down
    /// the left.
    fn product(&mut self, (l, r): (Raw, Raw), pending: Vec<Cond>, filters: &mut [Vec<Cond>]) -> Option<Node> {
        let (tl, tr) = (raw_tables(&l), raw_tables(&r));
        let (mut lefts, mut rights, mut own) = (Vec::new(), Vec::new(), Vec::new());
        for c in pending {
            let reads = self.reads(&c);
            if reads & !tl == 0 {
                lefts.push(c);
            } else if reads & !tr == 0 {
                rights.push(c);
            } else {
                own.push(c);
            }
        }
        let (left, right) = (self.place(l, lefts, filters)?, self.place(r, rights, filters)?);
        let op = self.shapes.len();
        let reads = own.iter().fold(0, |s, c| s | self.reads(c));
        self.conjuncts.extend(own.into_iter().map(|c| (op, c)));
        self.shapes.push(Shape { kind: Kind::Inner, left: tl, right: tr, reads, strong: true });
        self.operands.push((left, right));
        self.preds.push(None);
        Some(Node::Op(op))
    }

    /// An outer join of `l` and `r` on `cond`: a pending conjunct down to
    /// a LEFT join's preserved operand, or none placed.
    fn outer(&mut self, (kind, cond): (Kind, Cond), (l, r): (Raw, Raw), pending: Vec<Cond>, filters: &mut [Vec<Cond>]) -> Option<Node> {
        let (tl, tr) = (raw_tables(&l), raw_tables(&r));
        let preserved = |c: &Cond| kind == Kind::Left && self.reads(c) != 0 && self.reads(c) & !tl == 0;
        if !pending.iter().all(preserved) {
            return None;
        }
        let (left, right) = (self.place(l, pending, filters)?, self.place(r, Vec::new(), filters)?);
        let reads = self.reads(&cond);
        let strong = members(reads).all(|t| self.rejects(&cond, t));
        let op = self.shapes.len();
        self.shapes.push(Shape { kind, left: tl, right: tr, reads, strong });
        self.operands.push((left, right));
        self.preds.push(Some(cond));
        Some(Node::Op(op))
    }

    /// Whether `c` is not true of a row whose columns of relation `t` are
    /// all null: a comparison, IN or IS NOT NULL of one of them, alone or
    /// under AND, or under OR on both sides.
    fn rejects(&self, c: &Cond, t: usize) -> bool {
        let of = |e: &Expr| if let Expr::Column(col) = e { self.rel_of(*col) == Some(t) } else { false };
        match c {
            Cond::Compare(a, _, b) => of(a) || of(b),
            Cond::In(e, _) | Cond::IsNull(e, true) => of(e),
            Cond::And(a, b) => self.rejects(a, t) || self.rejects(b, t),
            Cond::Or(a, b) => self.rejects(a, t) && self.rejects(b, t),
            Cond::IsNull(_, false)
            | Cond::Const(_)
            | Cond::Not(_)
            | Cond::Distinct(..)
            | Cond::Like(..)
            | Cond::Exists(_)
            | Cond::Quantified(..)
            | Cond::Is(..)
            | Cond::Unique(..)
            | Cond::Match { .. }
            | Cond::Value(_) => false,
        }
    }
}

/// The relations of `raw`.
fn raw_tables(raw: &Raw) -> Set {
    match raw {
        Raw::Leaf(r) => bit(*r),
        Raw::Filtered(_, inner) => raw_tables(inner),
        Raw::Product(l, r) | Raw::Outer(_, _, l, r) => raw_tables(l) | raw_tables(r),
    }
}
