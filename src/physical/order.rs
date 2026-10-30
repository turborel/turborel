//! \[DESIGN\] Join ordering (RULES.md D35): the operands of a product are
//! joined in the order estimated to be cheapest, not necessarily the order
//! of the FROM clause. Split from physical.rs (RULES.md R-SIZE-2).
//!
//! A chain of products and the filters over them is taken apart into its
//! operands (relations) and the conjuncts of its conditions, then built
//! again left-deep in a new order, each conjunct over the first product
//! that has all its columns, under a projection that puts the columns back
//! in the FROM clause's order. The product's rows are the same; only their
//! order differs, which no ORDER BY asked for. A conjunct is evaluated on
//! rows of the product it belongs to either way, but on fewer of them, so
//! no condition of the chain may raise an error (RULES.md D15). One that
//! can, reading one operand's columns alone, is moved onto that operand,
//! over a guard, before (`optimizer::PushRaisingFilterIntoProduct`): such
//! an operand is one relation, evaluated whole wherever it is placed.
//!
//! The order: from each relation in turn, the relation joined next is the
//! one that costs least and yields fewest rows; the cheapest of these
//! orders is taken if it is estimated cheaper than the FROM clause's by the
//! gain PLANNER.REORDER_GAIN gives (`CostModel::reorder_gain`). The relation of the first key of the order asked for stays first.

use std::sync::Arc;

use super::{PhysicalPlanner, estimate};
use crate::logical::{Cond, Expr, LogicalPlan, SortKey, and_all, cond_columns, map_cond_columns, split_and};
use crate::optimizer::is_guarded;
use crate::stats::Statistics;
use crate::storage::cost_of;
use crate::value::CompOp;

/// One operand of the chain: a plan that is not a product.
struct Relation<'p> {
    plan: &'p LogicalPlan,
    /// Its first column in the chain's columns, and how many it has.
    start: usize,
    width: usize,
    /// Its estimated rows, and its table's statistics, if it reads a table.
    rows: f64,
    stats: Option<Arc<dyn Statistics>>,
}

impl Relation<'_> {
    fn has(&self, column: usize) -> bool {
        (self.start..self.start.saturating_add(self.width)).contains(&column)
    }

    /// The distinct values of the chain's column `column`, if known.
    fn distinct(&self, column: usize) -> Option<f64> {
        self.stats.as_ref()?.distinct(column.checked_sub(self.start)?)
    }
}

/// A conjunct of the chain's conditions, over the chain's columns, the
/// relations whose columns it uses, and the fraction of pairs it is taken
/// to keep if it is no equality of their columns.
struct Conjunct {
    cond: Cond,
    uses: Vec<usize>,
    guess: f64,
}

impl PhysicalPlanner<'_> {
    /// `plan`, a chain of products and filters over them, rebuilt in a
    /// cheaper order, if there is one; `want` is the order asked of it.
    pub(super) fn join_order(&self, plan: &LogicalPlan, want: &[SortKey]) -> Option<LogicalPlan> {
        if !is_chain(plan) {
            return None;
        }
        let (mut relations, mut conds) = (Vec::new(), Vec::new());
        self.take_apart(plan, 0, &mut relations, &mut conds)?;
        if conds.iter().any(Cond::can_raise) {
            return None;
        }
        let model = self.db.cost();
        let conjuncts: Vec<Conjunct> = conds
            .into_iter()
            .map(|cond| {
                let mut columns = Vec::new();
                cond_columns(&cond, &mut columns);
                let uses = relations.iter().enumerate().filter(|(_, r)| columns.iter().any(|&c| r.has(c))).map(|(n, _)| n).collect();
                Conjunct { cond, uses, guess: model.range_selectivity }
            })
            .collect();
        let first = want.first().and_then(|k| relations.iter().position(|r| r.has(k.column)));
        let from: Vec<usize> = (0..relations.len()).collect();
        let best = best_order(&relations, &conjuncts, first)?;
        if cost(&best, &relations, &conjuncts) < model.reorder_gain * cost(&from, &relations, &conjuncts) {
            rebuild(&best, &relations, conjuncts)
        } else {
            None
        }
    }

    /// Adds the relations of the chain `plan`, whose columns begin at
    /// `start`, to `relations`, and its conditions' conjuncts, over the
    /// chain's columns, to `conds`; none if a relation's rows are not known.
    fn take_apart<'p>(&self, plan: &'p LogicalPlan, start: usize, relations: &mut Vec<Relation<'p>>, conds: &mut Vec<Cond>) -> Option<()> {
        if let LogicalPlan::Product { left, right } = plan {
            self.take_apart(left, start, relations, conds)?;
            return self.take_apart(right, start.saturating_add(left.width()), relations, conds);
        }
        if let LogicalPlan::Filter { cond, input } = plan
            && is_chain(input)
            && !is_guarded(plan)
        {
            split_and(map_cond_columns(cond.clone(), &|c| c.saturating_add(start)), conds);
            return self.take_apart(input, start, relations, conds);
        }
        let (rows, stats) = self.estimate(plan)?;
        relations.push(Relation { plan, start, width: plan.width(), rows, stats });
        Some(())
    }

    /// The rows of a relation that reads a table, filtered or not, its
    /// conjuncts that can raise an error over a guard or not
    /// (`optimizer::is_guarded`), and the table's statistics.
    fn estimate(&self, plan: &LogicalPlan) -> Option<(f64, Option<Arc<dyn Statistics>>)> {
        let (read, cond) = if let LogicalPlan::Filter { cond, input } = plan { (&**input, Some(cond)) } else { (plan, None) };
        let read = if let LogicalPlan::Product { left, .. } = read
            && is_guarded(plan)
        {
            &**left
        } else {
            read
        };
        let LogicalPlan::Scan { table, .. } = read else { return None };
        let storage = self.db.storage_of(table).ok()?;
        let stats = self.db.statistics(table);
        let all = cost_of(storage.row_count());
        let model = self.db.cost();
        let known = estimate::Known { stats: stats.as_deref(), cost: &model };
        Some((cond.map_or(all, |c| all * estimate::selectivity(c, known)), stats))
    }
}

/// Whether `plan` is a product, or a filter over a chain; not a relation
/// whose conjuncts that can raise an error are over a guard
/// (`optimizer::is_guarded`), which is one relation.
fn is_chain(plan: &LogicalPlan) -> bool {
    if let LogicalPlan::Filter { input, .. } = plan {
        return !is_guarded(plan) && is_chain(input);
    }
    matches!(plan, LogicalPlan::Product { .. })
}

/// From each relation in turn (only `first`, if given), the order that
/// joins next the relation costing least: the cheapest of them.
fn best_order(relations: &[Relation<'_>], conjuncts: &[Conjunct], first: Option<usize>) -> Option<Vec<usize>> {
    let starts: Vec<usize> = first.map_or_else(|| (0..relations.len()).collect(), |f| vec![f]);
    starts
        .into_iter()
        .map(|start| {
            let mut order = vec![start];
            while order.len() < relations.len() {
                let next = (0..relations.len())
                    .filter(|r| !order.contains(r))
                    .min_by(|&a, &b| step(&order, a, relations, conjuncts).total_cmp(&step(&order, b, relations, conjuncts)));
                order.extend(next);
            }
            order
        })
        .min_by(|a, b| cost(a, relations, conjuncts).total_cmp(&cost(b, relations, conjuncts)))
}

/// The estimated cost of joining in `order`: each join's, and the rows it
/// yields.
fn cost(order: &[usize], relations: &[Relation<'_>], conjuncts: &[Conjunct]) -> f64 {
    (1..order.len())
        .map(|n| step(order.get(..n).unwrap_or_default(), order.get(n).copied().unwrap_or_default(), relations, conjuncts))
        .sum()
}

/// The estimated cost of joining relation `next` to the product of
/// `placed`, and the rows that yields: a hash join reads each side once, if
/// a conjunct equates a column of each; a nested loop pairs every row.
fn step(placed: &[usize], next: usize, relations: &[Relation<'_>], conjuncts: &[Conjunct]) -> f64 {
    let rows = |order: &[usize]| order.iter().filter_map(|&r| relations.get(r)).map(|r| r.rows).product::<f64>();
    let inner = relations.get(next).map_or(0.0_f64, |r| r.rows);
    let outer = rows(placed) * joined_fraction(placed, relations, conjuncts);
    let mut fraction = 1.0_f64;
    let mut hashed = false;
    for c in conjuncts.iter().filter(|c| c.uses.contains(&next) && c.uses.iter().all(|u| *u == next || placed.contains(u))) {
        if c.uses.len() < 2 {
            continue;
        }
        let equality = equality_fraction(&c.cond, relations);
        hashed |= equality.is_some();
        fraction *= equality.unwrap_or(c.guess);
    }
    let yields = outer * inner * fraction;
    let join = if hashed { outer + inner } else { outer * inner };
    join + yields
}

/// The fraction of the product of `placed` that its conjuncts keep.
fn joined_fraction(placed: &[usize], relations: &[Relation<'_>], conjuncts: &[Conjunct]) -> f64 {
    conjuncts
        .iter()
        .filter(|c| c.uses.len() > 1 && c.uses.iter().all(|u| placed.contains(u)))
        .map(|c| equality_fraction(&c.cond, relations).unwrap_or(c.guess))
        .product()
}

/// For an equality of two columns of different relations, the fraction of
/// pairs it keeps: one in as many as the more distinct of its columns has
/// values, each taken to be a key of its relation if not known.
fn equality_fraction(c: &Cond, relations: &[Relation<'_>]) -> Option<f64> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    let of = |column: usize| relations.iter().find(|r| r.has(column));
    let (ra, rb) = (of(*a)?, of(*b)?);
    if ra.start == rb.start {
        return None;
    }
    let distinct = ra.distinct(*a).unwrap_or(ra.rows).max(rb.distinct(*b).unwrap_or(rb.rows)).max(1.0_f64);
    Some(1.0_f64 / distinct)
}

/// The chain's relations joined left-deep in `order`, each conjunct over
/// the first product that has its columns (a relation's own over it), under
/// a projection putting the columns back in their order.
fn rebuild(order: &[usize], relations: &[Relation<'_>], conjuncts: Vec<Conjunct>) -> Option<LogicalPlan> {
    // Where each relation's columns begin in the new order.
    let mut starts = vec![0; relations.len()];
    let mut width = 0_usize;
    for &r in order {
        *starts.get_mut(r)? = width;
        width = width.saturating_add(relations.get(r)?.width);
    }
    let moved =
        |c: usize| relations.iter().zip(&starts).find(|(r, _)| r.has(c)).map_or(c, |(r, s)| c.saturating_sub(r.start).saturating_add(*s));
    // The conjuncts placed at each join, by the position of the last
    // relation they use; those of no relation over the whole.
    let mut placed: Vec<Vec<Cond>> = vec![Vec::new(); order.len().saturating_add(1)];
    for c in conjuncts {
        let last = c.uses.iter().filter_map(|u| order.iter().position(|o| o == u)).max().unwrap_or(order.len());
        placed.get_mut(last)?.push(map_cond_columns(c.cond, &moved));
    }
    let (&first, rest) = order.split_first()?;
    let mut plan = filtered(relations.get(first)?.plan.clone(), placed.first_mut().map(std::mem::take).unwrap_or_default());
    for (n, &r) in rest.iter().enumerate() {
        let product = LogicalPlan::Product { left: Box::new(plan), right: Box::new(relations.get(r)?.plan.clone()) };
        plan = filtered(product, placed.get_mut(n.saturating_add(1)).map(std::mem::take).unwrap_or_default());
    }
    let plan = filtered(plan, placed.pop().unwrap_or_default());
    let exprs = (0..width).map(|c| Expr::Column(moved(c))).collect();
    Some(LogicalPlan::Project { exprs, input: Box::new(plan) })
}

/// `plan` under a filter of `conds`, if any.
fn filtered(plan: LogicalPlan, conds: Vec<Cond>) -> LogicalPlan {
    match and_all(conds) {
        Some(cond) => LogicalPlan::Filter { cond, input: Box::new(plan) },
        None => plan,
    }
}
