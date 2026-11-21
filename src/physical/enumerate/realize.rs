//! \[DESIGN\] The chosen plan of a chain made a physical plan (RULES.md
//! D303): each relation built by the planner, in the order its plan asks
//! of it, and each join by its method, the conjuncts it judges over the
//! columns of its pair. A hash join builds the operand of fewer estimated
//! rows unless the other's order was chosen, a LEFT join so becoming the
//! RIGHT join of its operands swapped; an index nested-loop join whose
//! index cannot be probed by the left's values (`physical/indexed.rs`) is
//! a hash join instead; a merge join's operand not in its key's order is
//! sorted; a semi join or an antijoin is hashed, the left built if it has
//! fewer rows, or probes an index. The columns of a join are its left
//! operand's then its right's, a semi join's its left's alone, and a
//! projection over the whole puts them back in the FROM clause's order.

use super::bits::Set;
use super::conflicts::Kind;
use super::extract::Query;
use super::model::{Model, Order};
use super::space::{Method, Space, Tree};
use crate::logical::{Cond, Expr, OuterJoin, SortKey, and_all, judged_by_sets, map_cond_columns, render_cond, render_expr, split_and};
use crate::physical::indexed::{EquiKind, Equijoin, alike, index_join};
use crate::physical::join::{JoinKey, Parted, hashed, hashed_outer};
use crate::physical::semi::{Semi, semi_plan};
use crate::physical::shown::Shown;
use crate::physical::{MergeJoin, PhysicalPlanner, Plan};
use crate::value::{CompOp, DataType};

/// A realized plan: the plan, the relations whose columns it gives, in
/// order, and the order of its rows.
pub(super) struct Realized {
    pub(super) plan: Plan,
    pub(super) layout: Vec<usize>,
    pub(super) order: Order,
    /// The order of its joins, as EXPLAIN shows it, if the plan is shown.
    pub(super) shape: String,
}

/// The shape of a join of `l` and `r`.
fn joined_shape(l: &Realized, r: &Realized) -> String {
    if l.shape.is_empty() { String::new() } else { format!("({} {})", l.shape, r.shape) }
}

/// What realizing reads: the chain, the model, the table and the order
/// asked.
pub(super) struct Context<'c> {
    pub(super) query: &'c Query,
    pub(super) model: &'c Model,
    pub(super) space: &'c Space<'c>,
    pub(super) want: &'c [SortKey],
}

impl Context<'_> {
    /// The place in the columns of `layout` of the chain's column `col`.
    pub(super) fn place_of(&self, layout: &[usize], col: usize) -> Option<usize> {
        let rel = self.query.rel_of(col)?;
        let leaf = self.query.leaves.get(rel)?;
        let before: usize = layout.iter().take_while(|&&r| r != rel).filter_map(|&r| self.query.leaves.get(r)).map(|l| l.width).sum();
        layout.contains(&rel).then(|| before.saturating_add(col.saturating_sub(leaf.start)))
    }

    /// The class of orders of the column at `place` of the columns of
    /// `layout`.
    fn class_at(&self, layout: &[usize], place: usize) -> Option<usize> {
        let col = (0..self.query.columns()).find(|&c| self.place_of(layout, c) == Some(place))?;
        self.model.order_of(col).map(|(class, _)| class)
    }

    /// The width of the columns of `layout`.
    pub(super) fn width(&self, layout: &[usize]) -> usize {
        layout.iter().filter_map(|&r| self.query.leaves.get(r)).map(|l| l.width).sum()
    }

    /// `c`, over the chain's columns, over those of `layout`.
    pub(super) fn mapped(&self, c: &Cond, layout: &[usize]) -> Cond {
        map_cond_columns(c.clone(), &|col| self.place_of(layout, col).unwrap_or(col))
    }

    /// The names of the columns of `layout`, if the plan is shown.
    pub(super) fn names(&self, layout: &[usize], shown: bool) -> Option<Vec<String>> {
        shown.then(|| layout.iter().filter_map(|&r| self.query.leaves.get(r)).flat_map(|l| l.plan.names()).collect())
    }

    /// The rows of set `s`.
    fn rows(&self, s: Set) -> f64 {
        self.space.table.get(&s).map_or(0.0_f64, |e| e.rows)
    }

    /// The equalities of `conds` of a column of `left` and one of `right`,
    /// as places in the columns of the pair, the right's among its own;
    /// and the rest.
    fn split_keys(&self, (left, right): (&[usize], &[usize]), conds: &[Cond]) -> (Vec<(usize, usize)>, Vec<Cond>) {
        let (mut keys, mut rest) = (Vec::new(), Vec::new());
        for c in conds {
            let pair = if let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c {
                match (self.place_of(left, *a), self.place_of(right, *b), self.place_of(left, *b), self.place_of(right, *a)) {
                    (Some(x), Some(y), _, _) | (_, _, Some(x), Some(y)) => Some((x, y)),
                    (None | Some(_), None | Some(_), None | Some(_), None | Some(_)) => None,
                }
            } else {
                None
            };
            match pair {
                Some(p) => keys.push(p),
                None => rest.push(c.clone()),
            }
        }
        (keys, rest)
    }

    /// The conjuncts a join of `sets` judges: its operator's condition's,
    /// or the inner joins' conjuncts that read both, or, if none does, the
    /// equalities their keys imply of a column of each
    /// (`Model::implied_keys`, RULES.md D387).
    fn conds(&self, op: Option<usize>, (l, r): (Set, Set)) -> Vec<Cond> {
        if let Some(cond) = op.and_then(|o| self.query.preds.get(o)).and_then(Option::as_ref) {
            let mut out = Vec::new();
            split_and(cond.clone(), &mut out);
            return out;
        }
        let s = l | r;
        let straddles = |reads: Set| reads & !s == 0 && reads & !l != 0 && reads & !r != 0;
        let stated: Vec<Cond> =
            self.query.conjuncts.iter().filter(|(_, c)| straddles(self.query.reads(c))).map(|(_, c)| c.clone()).collect();
        if !stated.is_empty() {
            return stated;
        }
        self.model.implied_keys((l, r)).into_iter().map(|(a, b)| Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b))).collect()
    }
}

impl PhysicalPlanner<'_> {
    /// The plan of tree `at` of the table.
    pub(super) fn realize(&self, cx: &Context<'_>, at: usize, depth: usize) -> Option<Realized> {
        if depth > super::bits::MAX_RELATIONS.saturating_mul(2) {
            return None;
        }
        match *cx.space.trees.get(at)? {
            Tree::Leaf { rel, order } => self.realize_leaf(cx, rel, order),
            Tree::Join { how, op, left, right, sets } => {
                let l = self.realize(cx, left, depth.saturating_add(1))?;
                let r = self.realize(cx, right, depth.saturating_add(1))?;
                let kind = op.map_or(Kind::Inner, |o| cx.model.kind_of(o));
                let conds = cx.conds(op, sets);
                let rows = (cx.rows(sets.0), cx.rows(sets.1));
                let est = cx.rows(sets.0 | sets.1);
                self.realize_join(cx, (how, kind), (l, r), (conds, rows, est))
            }
        }
    }

    /// Relation `rel` built in the order `order` asks of it.
    fn realize_leaf(&self, cx: &Context<'_>, rel: usize, order: Order) -> Option<Realized> {
        let leaf = cx.query.leaves.get(rel)?;
        let local = |c: usize| c.checked_sub(leaf.start).filter(|&l| l < leaf.width);
        let keys: Vec<SortKey> = match order {
            Order::None => Vec::new(),
            Order::Class(k) => {
                let col = cx.model.orders.iter().find(|&&(c, class, _)| class == k && local(c).is_some()).map(|&(c, _, _)| c)?;
                vec![SortKey::ascending(local(col)?)]
            }
            Order::Want => cx.want.iter().map(|k| Some(SortKey { column: local(k.column)?, ..*k })).collect::<Option<_>>()?,
        };
        let shape = if self.shown { label(&leaf.plan) } else { String::new() };
        // The model built the read of each order it costed (`enumerate/modeled.rs`).
        if let Some(plan) = cx.model.plan_of(rel, order) {
            return Some(Realized { plan: plan.clone(), layout: vec![rel], order, shape });
        }
        let built = self.build(&leaf.plan, &keys);
        Some(Realized { plan: built.plan, layout: vec![rel], order: if built.ordered { order } else { Order::None }, shape })
    }

    /// The join of `l` and `r` by `how`, of kind `kind`, judging `conds`,
    /// its operands giving `rows`, it `est`.
    fn realize_join(
        &self,
        cx: &Context<'_>,
        (how, kind): (Method, Kind),
        (l, r): (Realized, Realized),
        (conds, rows, est): (Vec<Cond>, (f64, f64), f64),
    ) -> Option<Realized> {
        match (how, kind) {
            (Method::Index, Kind::Inner | Kind::Left) => {
                let fallback = (Method::Hash, kind);
                match self.index_realized(cx, kind, (&l, &r), (&conds, rows.0)) {
                    Some(done) => Some(done),
                    None => self.realize_join(cx, fallback, (l, r), (conds, rows, est)),
                }
            }
            (Method::Merge(class), Kind::Inner | Kind::Left) => self.merge_realized(cx, (class, kind), (l, r), (&conds, est)),
            (Method::Index, Kind::Semi | Kind::Anti) => match self.index_realized(cx, kind, (&l, &r), (&conds, rows.0)) {
                Some(done) => Some(done),
                None => Some(self.semi_realized(cx, kind, (l, r), (&conds, rows, est))),
            },
            (Method::Hash | Method::Nested | Method::Merge(_) | Method::Range, Kind::Semi | Kind::Anti) => {
                Some(self.semi_realized(cx, kind, (l, r), (&conds, rows, est)))
            }
            // A range join that cannot be made is a nested loop (`enumerate/ranged.rs`).
            (Method::Range, Kind::Inner | Kind::Left) => match self.range_realized(cx, kind, (&l, &r), (&conds, rows, est)) {
                Some(done) => Some(done),
                None => self.realize_join(cx, (Method::Nested, kind), (l, r), (conds, rows, est)),
            },
            (Method::Hash, Kind::Inner) => Some(self.hash_realized(cx, (l, r), &conds, (rows, est))),
            (Method::Hash | Method::Nested | Method::Index | Method::Merge(_), Kind::Left | Kind::Full) | (Method::Range, Kind::Full) => {
                Some(self.outer_realized(cx, kind, (l, r), (&conds, rows, est)))
            }
            (Method::Nested, Kind::Inner) => {
                let layout: Vec<usize> = l.layout.iter().chain(&r.layout).copied().collect();
                let shape = joined_shape(&l, &r);
                let product = Plan::Product { left: Box::new(l.plan), right: Box::new(r.plan), est };
                Some(Realized { plan: self.filter_over(cx, product, &conds, (&layout, est)), layout, order: l.order, shape })
            }
        }
    }

    /// `plan`, of the columns of `layout`, under a filter of `conds`
    /// giving `est` rows.
    fn filter_over(&self, cx: &Context<'_>, plan: Plan, conds: &[Cond], (layout, est): (&[usize], f64)) -> Plan {
        let Some(cond) = and_all(conds.iter().map(|c| cx.mapped(c, layout)).collect()).map(judged_by_sets) else { return plan };
        let names = cx.names(layout, self.shown);
        let detail = Shown::of(names.as_deref()).text(|n| render_cond(&cond, n));
        Plan::Filter { cond, detail, est, of_read: false, input: Box::new(plan) }
    }

    /// An inner hash join, the operand of fewer rows built unless the
    /// left's order was chosen.
    fn hash_realized(&self, cx: &Context<'_>, (l, r): (Realized, Realized), conds: &[Cond], (rows, est): ((f64, f64), f64)) -> Realized {
        let (l, r) = if l.order == Order::None && rows.0 < rows.1 { (r, l) } else { (l, r) };
        let (keys, rest) = cx.split_keys((&l.layout, &r.layout), conds);
        let layout: Vec<usize> = l.layout.iter().chain(&r.layout).copied().collect();
        let shape = joined_shape(&l, &r);
        let keys = keys.into_iter().map(|(left, right)| JoinKey { left, right, nulls: false }).collect();
        let rest = rest.iter().map(|c| cx.mapped(c, &layout)).collect();
        let names = cx.names(&layout, self.shown);
        let plan = hashed((l.plan, r.plan), keys, rest, (cx.width(&l.layout), Shown::of(names.as_deref()), est));
        Realized { plan, layout, order: l.order, shape }
    }

    /// An outer join, hashed on its equalities: the operand of fewer rows
    /// built unless the left's order was chosen, a LEFT join so the RIGHT
    /// join of its operands swapped.
    fn outer_realized(
        &self,
        cx: &Context<'_>,
        kind: Kind,
        (l, r): (Realized, Realized),
        (conds, rows, est): (&[Cond], (f64, f64), f64),
    ) -> Realized {
        let swap = rows.0 < rows.1 && (kind == Kind::Full || l.order == Order::None);
        let of = match (kind, swap) {
            (Kind::Full, true | false) => OuterJoin::Full,
            (Kind::Left | Kind::Inner | Kind::Semi | Kind::Anti, true) => OuterJoin::Right,
            (Kind::Left | Kind::Inner | Kind::Semi | Kind::Anti, false) => OuterJoin::Left,
        };
        let (l, r) = if swap { (r, l) } else { (l, r) };
        let shape = joined_shape(&l, &r);
        let layout: Vec<usize> = l.layout.iter().chain(&r.layout).copied().collect();
        let width = cx.width(&l.layout);
        let cond = and_all(conds.iter().map(|c| cx.mapped(c, &layout)).collect()).unwrap_or(Cond::Const(crate::logical::Truth::True));
        let names = cx.names(&layout, self.shown);
        let shown = Shown::of(names.as_deref());
        let detail = shown.text(|n| render_cond(&cond, n));
        let order = if of == OuterJoin::Left { l.order } else { Order::None };
        let widths = (width, cx.width(&r.layout));
        let plan = hashed_outer(of, (l.plan, r.plan), Parted::of(&cond, width), (widths, detail, shown, est));
        Realized { plan, layout, order, shape }
    }

    /// A semi join or an antijoin, hashed on its equalities, the left
    /// built if it has fewer rows, else nested (`physical/semi.rs`). Its
    /// columns are its left's, in their order.
    fn semi_realized(
        &self,
        cx: &Context<'_>,
        kind: Kind,
        (l, r): (Realized, Realized),
        (conds, rows, est): (&[Cond], (f64, f64), f64),
    ) -> Realized {
        let shape = joined_shape(&l, &r);
        let pair: Vec<usize> = l.layout.iter().chain(&r.layout).copied().collect();
        let widths = (cx.width(&l.layout), cx.width(&r.layout));
        let cond = and_all(conds.iter().map(|c| cx.mapped(c, &pair)).collect()).unwrap_or(Cond::Const(crate::logical::Truth::True));
        let names = cx.names(&pair, self.shown);
        let semi = Semi { anti: kind == Kind::Anti, cond, widths, build_left: rows.0 < rows.1 };
        let plan = semi_plan(semi, (l.plan, r.plan), (Shown::of(names.as_deref()), est));
        Realized { plan, layout: l.layout, order: l.order, shape }
    }

    /// An index nested-loop join of `l` and the relation `r` reads, if an
    /// index of its table answers the equalities for the left's values.
    fn index_realized(
        &self,
        cx: &Context<'_>,
        kind: Kind,
        (l, r): (&Realized, &Realized),
        (conds, rows): (&[Cond], f64),
    ) -> Option<Realized> {
        let &[rel] = r.layout.as_slice() else { return None };
        let leaf = cx.query.leaves.get(rel)?;
        let (pairs, rest) = cx.split_keys((&l.layout, &r.layout), conds);
        if pairs.is_empty() {
            return None;
        }
        let type_at = |place: usize| -> Option<DataType> {
            let col = (0..cx.query.columns()).find(|&c| cx.place_of(&l.layout, c) == Some(place))?;
            let left_leaf = cx.query.leaves.get(cx.query.rel_of(col)?)?;
            self.column_type(&left_leaf.plan, col.checked_sub(left_leaf.start)?)
        };
        let of_kind = |place: usize, t: &DataType| type_at(place).is_some_and(|lt| alike(&lt, t));
        let probing = self.probing(&leaf.plan, &pairs, &of_kind)?;
        let layout: Vec<usize> = l.layout.iter().chain(&r.layout).copied().collect();
        let rest = rest.iter().map(|c| cx.mapped(c, &layout)).collect();
        let names = cx.names(&layout, self.shown);
        let join = Equijoin { width: cx.width(&l.layout), keys: &pairs, rest, outer: equi_kind(kind) };
        let plan = index_join(probing, join, (l.plan.clone(), rows), Shown::of(names.as_deref()));
        // A semi join's columns are its left's alone.
        let layout = if join_alone(kind) { l.layout.clone() } else { layout };
        Some(Realized { plan, layout, order: l.order, shape: joined_shape(l, r) })
    }

    /// A merge join on the equality of `class`, each operand sorted by its
    /// key unless its order is the key's.
    fn merge_realized(
        &self,
        cx: &Context<'_>,
        (class, kind): (usize, Kind),
        (left, right): (Realized, Realized),
        (conds, est): (&[Cond], f64),
    ) -> Option<Realized> {
        let (pairs, _) = cx.split_keys((&left.layout, &right.layout), conds);
        let &(at_left, at_right) = pairs.iter().find(|&&(place, _)| cx.class_at(&left.layout, place) == Some(class))?;
        let right_class = cx.class_at(&right.layout, at_right)?;
        let shape = joined_shape(&left, &right);
        let layout: Vec<usize> = left.layout.iter().chain(&right.layout).copied().collect();
        let widths = (cx.width(&left.layout), cx.width(&right.layout));
        let (left_names, right_names) = (cx.names(&left.layout, self.shown), cx.names(&right.layout, self.shown));
        let left_plan = sorted(left, (at_left, class), left_names.as_deref());
        let right_plan = sorted(right, (at_right, right_class), right_names.as_deref());
        let pair_key = (at_left, at_right.saturating_add(widths.0));
        let rest: Vec<Cond> = conds.iter().map(|c| cx.mapped(c, &layout)).filter(|c| !is_key(c, pair_key)).collect();
        let cond = and_all(rest).map(judged_by_sets);
        let all_names = cx.names(&layout, self.shown);
        let detail = merge_detail(Shown::of(all_names.as_deref()), pair_key, cond.as_ref());
        let key = JoinKey { left: at_left, right: at_right, nulls: false };
        let merged = MergeJoin { outer: kind == Kind::Left, left: left_plan, right: right_plan, key, cond, widths, detail, est };
        let plan = Plan::MergeJoin(Box::new(merged));
        Some(Realized { plan, layout, order: Order::Class(class), shape })
    }
}

/// The plan of `side`, sorted by its column `key` unless it is in the
/// order of `class` already.
fn sorted(side: Realized, (key, class): (usize, usize), names: Option<&[String]>) -> Plan {
    if side.order == Order::Class(class) {
        return side.plan;
    }
    let keys = vec![SortKey::ascending(key)];
    let detail = Shown::of(names).text(|shown| crate::logical::render_keys(&keys, shown));
    Plan::Sort { keys, detail, input: Box::new(side.plan), most: None }
}

/// How EXPLAIN shows a merge join: its key's equality, then the rest of
/// its condition.
fn merge_detail(shown: Shown<'_>, (left, right): (usize, usize), rest: Option<&Cond>) -> String {
    let key = shown.text(|names| format!("{} = {}", render_expr(&Expr::Column(left), names), render_expr(&Expr::Column(right), names)));
    let more = rest.map(|c| shown.text(|names| format!(" AND {}", render_cond(c, names)))).unwrap_or_default();
    format!("{key}{more}")
}

/// The name a relation is shown by: the qualifier of its first column's
/// name, or its first column's name.
fn label(plan: &crate::logical::LogicalPlan) -> String {
    let first = plan.names().into_iter().next().unwrap_or_default();
    first.rsplit_once('.').map_or(first.clone(), |(q, _)| q.to_owned())
}

/// Whether `c` is the equality of the pair's columns `left` and `right`.
fn is_key(c: &Cond, (left, right): (usize, usize)) -> bool {
    let Cond::Compare(Expr::Column(first), CompOp::Eq, Expr::Column(second)) = c else { return false };
    (*first, *second) == (left, right) || (*first, *second) == (right, left)
}

/// Which rows of its left an index join of kind `kind` gives.
fn equi_kind(kind: Kind) -> EquiKind {
    match kind {
        Kind::Inner | Kind::Full => EquiKind::Inner,
        Kind::Left => EquiKind::Left,
        Kind::Semi => EquiKind::Semi,
        Kind::Anti => EquiKind::Anti,
    }
}

/// Whether a join of kind `kind` gives its left's columns alone.
pub(super) fn join_alone(kind: Kind) -> bool {
    matches!(kind, Kind::Semi | Kind::Anti)
}
