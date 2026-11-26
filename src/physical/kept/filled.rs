//! A kept physical plan filled and prepared for one execution. Access
//! paths stay fixed; scalar preparation sees the new constant operands and
//! preserves lazy runtime evaluation (D15, D38, D45, D85). An inner join,
//! a product, a hash join or an index join by the outer row's columns, is
//! filled too (D418).

use std::cell::{Cell, RefCell};
use std::sync::{Arc, OnceLock};

use super::{ProbeSlot, filled_scan, slot_value};
use crate::db::State;
use crate::error::{Code, Result, SqlError, fail};
use crate::exec::{KeptSubqueries, PointRead, RangeRead};
use crate::logical::{AggCall, Cond, Distribution, Expr, Fetch, Limit, Placing, Truth, Unnest, filled_cond_as, filled_expr_as};
use crate::memory::MemoryBudget;
use crate::optimizer::prepared;
use crate::physical::{JoinProbe, Plan, Preserved};
use crate::value::Value;

/// Fill slots and optionally prepare scalar calls against `budget`.
///
/// # Errors
///
/// As `logical::filled`; SQLCODE -910 for an operator no kept plan holds.
pub(crate) fn filled_plan(plan: Plan, values: &[Value], budget: Option<&Arc<MemoryBudget>>) -> Result<Plan> {
    Filling { values: Some(values), budget, placing: Placing::Cast, fills: Cell::new(false), raising: RefCell::default() }.plan(plan)
}

/// As `filled_plan`, each value placed as it is, not cast: of a
/// correlated subquery's plan kept across its runs, its outer references
/// the slots (RULES.md D675).
///
/// # Errors
///
/// As `filled_plan`.
pub(crate) fn filled_plan_as_is(plan: Plan, values: &[Value], budget: Option<&Arc<MemoryBudget>>) -> Result<Plan> {
    Filling { values: Some(values), budget, placing: Placing::AsIs, fills: Cell::new(false), raising: RefCell::default() }.plan(plan)
}

/// `plan`, a kept plan whose slots stay for each execution to read where
/// they are (RULES.md D610), its scalar calls prepared against `budget`
/// once, as it is kept.
///
/// # Errors
///
/// SQLCODE -910 for an operator no kept plan holds.
pub(crate) fn prepared_plan(plan: Plan, budget: Option<&Arc<MemoryBudget>>) -> Result<Kept> {
    let filling = Filling { values: None, budget, placing: Placing::Cast, fills: Cell::new(false), raising: RefCell::default() };
    let prepared = filling.plan(plan.clone())?;
    // A plan filled at each execution is kept as it was made, to be filled
    // and prepared then, as before it was kept.
    let fills = filling.fills.get();
    let raising = filling.raising.take();
    let subqueries = Arc::default();
    Ok(Kept { plan: if fills { plan } else { prepared }, fills, raising, point: OnceLock::new(), range: OnceLock::new(), subqueries })
}

struct Filling<'a> {
    /// The slots' values; none, for slots left in place.
    values: Option<&'a [Value]>,
    budget: Option<&'a Arc<MemoryBudget>>,
    placing: Placing,
    /// Whether a scalar call it prepares takes a slot (`Kept`).
    fills: Cell<bool>,
    /// The probe slots it leaves in place whose expressions can raise.
    raising: RefCell<Vec<ProbeSlot>>,
}

impl Filling<'_> {
    /// `expr`, noted if a scalar call of it takes a slot.
    fn noted(&self, expr: Expr) -> Expr {
        if calls_slot(&expr) {
            self.fills.set(true);
        }
        expr
    }

    fn expr(&self, expr: Expr) -> Result<Expr> {
        let expr = match self.values {
            Some(values) => filled_expr_as(expr, values, self.placing)?,
            None => self.noted(expr),
        };
        Ok(prepared::expression(expr, self.budget))
    }

    fn cond(&self, cond: Cond) -> Result<Cond> {
        let cond = match self.values {
            Some(values) => filled_cond_as(cond, values, self.placing)?,
            None => cond.map_exprs(&mut |e| self.noted(e)),
        };
        Ok(prepared::condition(cond, self.budget))
    }

    /// A scan's probe filled, and the conjunct it decides, if one, filled
    /// and prepared as a filter's would be.
    fn scan(&self, plan: Plan) -> Result<Plan> {
        let mut plan = match self.values {
            Some(values) => filled_scan(plan, values, self.placing)?,
            None => plan,
        };
        if let (None, Plan::IndexScan { slots, .. }) = (self.values, &plan) {
            let raises = slots.iter().filter(|s| s.expr.as_ref().is_some_and(|e| e.can_raise()));
            self.raising.borrow_mut().extend(raises.cloned());
        }
        if let Plan::IndexScan { decided: Some(decided), .. } = &mut plan {
            let cond = std::mem::replace(&mut decided.cond, Cond::Const(Truth::True));
            decided.cond = self.cond(cond)?;
        }
        Ok(plan)
    }

    fn nearest(&self, mut read: Box<crate::physical::NearestRead>) -> Result<Plan> {
        read.measure = self.expr(read.measure)?;
        read.condition = read.condition.map(|c| self.cond(c)).transpose()?;
        Ok(Plan::Nearest(read))
    }

    /// A product or an inner join, its inputs filled (RULES.md D418).
    fn join(&self, plan: Plan) -> Result<Plan> {
        if let Plan::Product { left, right, est } = plan {
            return Ok(Plan::Product { left: self.boxed(*left)?, right: self.boxed(*right)?, est });
        }
        if let Plan::HashJoin { left, right, keys, detail, est } = plan {
            return Ok(Plan::HashJoin { left: self.boxed(*left)?, right: self.boxed(*right)?, keys, detail, est });
        }
        self.index_join(plan)
    }

    /// An inner index join probing by its outer row's columns, its outer
    /// input and condition filled (RULES.md D418); no other is kept.
    fn index_join(&self, plan: Plan) -> Result<Plan> {
        let Plan::IndexJoin {
            outer: Preserved::Inner,
            spatial: None,
            left,
            table,
            index,
            method,
            probe: probe @ (JoinProbe::Prefix(_) | JoinProbe::Range(_)),
            cond,
            width,
            columns,
            detail,
            est,
            cost,
            located,
        } = plan
        else {
            return fail(Code::Api, "[DESIGN] a kept plan holds an index join no kept plan holds");
        };
        let (left, cond) = (self.boxed(*left)?, cond.map(|c| self.cond(c)).transpose()?);
        let outer = Preserved::Inner;
        Ok(Plan::IndexJoin { outer, spatial: None, left, table, index, method, probe, cond, width, columns, detail, est, cost, located })
    }

    /// Rows of values, each expression filled (RULES.md D793).
    fn rows(&self, rows: Vec<Vec<Expr>>) -> Result<Vec<Vec<Expr>>> {
        rows.into_iter().map(|r| r.into_iter().map(|e| self.expr(e)).collect()).collect()
    }

    /// An UNNEST, its collections, its join condition and its input
    /// filled (RULES.md D793).
    fn unnest(&self, u: Unnest<Plan>) -> Result<Plan> {
        let (mut u, input) = u.split();
        u.collections = u.collections.into_iter().map(|e| self.expr(e)).collect::<Result<_>>()?;
        u.outer = u.outer.map(|c| self.cond(c)).transpose()?;
        let input = self.plan(input)?;
        Ok(Plan::Unnest(Box::new(u.with_input(|&()| input))))
    }

    fn plans(&self, plans: Vec<Plan>) -> Result<Vec<Plan>> {
        plans.into_iter().map(|p| self.plan(p)).collect()
    }

    /// `plan` filled, boxed.
    fn boxed(&self, plan: Plan) -> Result<Box<Plan>> {
        Ok(Box::new(self.plan(plan)?))
    }

    /// The part run in parallel `p`, its plan filled.
    fn parallel(&self, mut p: Box<crate::physical::Parallel>) -> Result<Plan> {
        p.input = self.plan(p.input)?;
        Ok(Plan::Parallel(p))
    }

    /// The set function `a`, its argument, filter and fraction filled.
    fn agg(&self, a: AggCall) -> Result<AggCall> {
        let arg = a.arg.map(|e| self.expr(e)).transpose()?;
        let distribution = match a.distribution {
            Some(d) => Some(Box::new(Distribution { fraction: self.expr(d.fraction.clone())?, ..*d })),
            None => None,
        };
        Ok(AggCall { arg, filter: a.filter.map(|c| self.cond(c)).transpose()?, distribution, ..a })
    }

    fn plan(&self, plan: Plan) -> Result<Plan> {
        match plan {
            Plan::Project { exprs, detail, input } => {
                let exprs = exprs.into_iter().map(|e| self.expr(e)).collect::<Result<_>>()?;
                Ok(Plan::Project { exprs, detail, input: Box::new(self.plan(*input)?) })
            }
            Plan::Filter { cond, detail, est, of_read, input } => {
                let cond = self.cond(cond)?;
                Ok(Plan::Filter { cond, detail, est, of_read, input: Box::new(self.plan(*input)?) })
            }
            Plan::IndexScan { .. } => self.scan(plan),
            // A transition table is read as refilled at each run (RULES.md D793).
            Plan::SeqScan { .. } | Plan::Transition { .. } => Ok(plan),
            Plan::Nearest(read) => self.nearest(read),
            Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est, streamed, first } => {
                let aggs = aggs.into_iter().map(|a| self.agg(a)).collect::<Result<_>>()?;
                Ok(Plan::Aggregate { groups, collations, aggs, seeds, detail, input: Box::new(self.plan(*input)?), est, streamed, first })
            }
            Plan::Distinct { input, est, streamed } => Ok(Plan::Distinct { input: Box::new(self.plan(*input)?), est, streamed }),
            Plan::Sort { keys, detail, input, most } => Ok(Plan::Sort { keys, detail, input: Box::new(self.plan(*input)?), most }),
            Plan::Limit { limit, input, est } => {
                let offset = limit.offset.map(|e| self.expr(e)).transpose()?;
                let fetch = limit.fetch.map(|f| Ok::<_, SqlError>(Fetch { count: self.expr(f.count)?, ..f })).transpose()?;
                Ok(Plan::Limit { limit: Limit { offset, fetch }, input: Box::new(self.plan(*input)?), est })
            }
            Plan::Append { left, right, est } => Ok(Plan::Append { left: Box::new(self.plan(*left)?), right: self.boxed(*right)?, est }),
            Plan::Parallel(p) => self.parallel(p),
            Plan::MergeAppend { inputs, keys, detail } => Ok(Plan::MergeAppend { inputs: self.plans(inputs)?, keys, detail }),
            Plan::Product { .. } | Plan::HashJoin { .. } | Plan::IndexJoin { .. } => self.join(plan),
            Plan::MergeJoin(_)
            | Plan::RangeJoin(_)
            | Plan::SemiJoin(_)
            | Plan::MarkJoin(_)
            | Plan::OuterJoin { .. }
            | Plan::SetOp { .. }
            | Plan::System { .. }
            | Plan::Recursive { .. }
            | Plan::Window { .. }
            | Plan::Polymorphic(..)
            | Plan::Recognize { .. }
            | Plan::WorkTable { .. }
            | Plan::Transfer(_)
            | Plan::Counted(_)
            | Plan::Shared(_) => fail(Code::Api, "[DESIGN] a kept plan holds an operator no kept plan holds"),
            Plan::Values { rows, detail } => Ok(Plan::Values { rows: self.rows(rows)?, detail }),
            Plan::Unnest(u) => self.unnest(*u),
        }
    }
}

/// A kept plan, its scalar calls prepared once (RULES.md D610): each
/// execution reads its slots where they are; or, if a scalar call takes a
/// slot, which a value would prepare, it is filled and prepared again at
/// each execution, as the values come.
#[derive(Debug, Clone)]
pub(crate) struct Kept {
    plan: Plan,
    fills: bool,
    /// Its probes' slots whose expressions can raise (RULES.md D793).
    raising: Vec<ProbeSlot>,
    /// The point read it is, if one, found as first asked (RULES.md D752).
    point: OnceLock<Option<PointRead>>,
    /// The direct range read it is, if one, found as first asked (RULES.md D930).
    range: OnceLock<Option<RangeRead>>,
    /// What its correlated subqueries settled (RULES.md D793).
    subqueries: Arc<KeptSubqueries>,
}

impl Kept {
    /// Whether an execution with the values `slots` places each of its
    /// probes' values: none of them raises, as a plan made with the values
    /// folds each that raises nothing into its probe (RULES.md D793). An
    /// execution of one that raises is planned with its values, so the
    /// error is raised where that plan raises it, if anywhere.
    pub(crate) fn places(&self, slots: &[Value]) -> bool {
        self.raising.iter().all(|s| slot_value(s, slots, Placing::Cast).is_ok())
    }

    /// What its correlated subqueries settled, kept across its executions
    /// (RULES.md D793).
    pub(crate) fn subqueries(&self) -> &Arc<KeptSubqueries> {
        &self.subqueries
    }

    /// The plan one execution runs with its slots' values `slots`, and the
    /// values it reads where its slots are, if it does.
    ///
    /// # Errors
    ///
    /// As `filled_plan`.
    pub(crate) fn opened(&self, slots: Option<Arc<[Value]>>, budget: Option<&Arc<MemoryBudget>>) -> Result<(Plan, Option<Arc<[Value]>>)> {
        if !self.fills {
            return Ok((self.plan.clone(), slots));
        }
        Ok((filled_plan(self.plan.clone(), slots.as_deref().unwrap_or_default(), budget)?, None))
    }

    /// Its plan, lent to an execution that reads it where it is kept
    /// (RULES.md D903): none of a plan filled and prepared again at each
    /// execution (`fills`), which is a copy of its own.
    pub(crate) fn lent(&self) -> Option<&Plan> {
        (!self.fills).then_some(&self.plan)
    }

    /// The point read its plan is, run without the executor's operators,
    /// if it is one and `st` runs point operations (RULES.md D752). A
    /// point read gives its columns as they are and places its probe's
    /// values with no call, so its plan is never one filled at each
    /// execution (`fills`), and is kept as made.
    pub(crate) fn point(&self, st: &State) -> Option<&PointRead> {
        if !crate::exec::point_operations(st) {
            return None;
        }
        self.point.get_or_init(|| PointRead::of(&self.plan, st)).as_ref()
    }

    /// The direct range read its plan is, run where it is kept with no
    /// operator built, if it is one and `st` runs point operations
    /// (RULES.md D930). Its expressions are read as the plan keeps them,
    /// so a plan filled at each execution (`fills`) is none.
    pub(crate) fn range(&self, st: &State) -> Option<&RangeRead> {
        if self.fills || !crate::exec::point_operations(st) {
            return None;
        }
        self.range.get_or_init(|| RangeRead::of(&self.plan, st)).as_ref()
    }
}

/// Whether a scalar call of `e`, at any depth, takes an argument that reads
/// a slot.
fn calls_slot(e: &Expr) -> bool {
    let mut work = vec![e];
    while let Some(next) = work.pop() {
        if let Expr::Call(_, args) = next
            && args.iter().any(reads_slot)
        {
            return true;
        }
        next.for_each_child(&mut |child| work.push(child));
    }
    false
}

/// Whether `e` reads a slot, at any depth.
fn reads_slot(e: &Expr) -> bool {
    let mut work = vec![e];
    while let Some(next) = work.pop() {
        if let Expr::Dynamic(_) = next {
            return true;
        }
        next.for_each_child(&mut |child| work.push(child));
    }
    false
}
