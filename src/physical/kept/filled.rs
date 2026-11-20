//! A kept physical plan filled and prepared for one execution. Access
//! paths stay fixed; scalar preparation sees the new constant operands and
//! preserves lazy runtime evaluation (D15, D38, D45, D85).

use std::sync::Arc;

use super::filled_scan;
use crate::error::{Code, Result, SqlError, fail};
use crate::logical::{AggCall, Cond, Distribution, Expr, Fetch, Limit, Truth, filled_cond, filled_expr};
use crate::memory::MemoryBudget;
use crate::optimizer::prepared;
use crate::physical::Plan;
use crate::value::Value;

/// Fill slots and optionally prepare scalar calls against `budget`.
///
/// # Errors
///
/// As `logical::filled`; SQLCODE -910 for an operator no kept plan holds.
pub(crate) fn filled_plan(plan: Plan, values: &[Value], budget: Option<&Arc<MemoryBudget>>) -> Result<Plan> {
    Filling { values, budget }.plan(plan)
}

struct Filling<'a> {
    values: &'a [Value],
    budget: Option<&'a Arc<MemoryBudget>>,
}

impl Filling<'_> {
    fn expr(&self, expr: Expr) -> Result<Expr> {
        Ok(prepared::expression(filled_expr(expr, self.values)?, self.budget))
    }

    fn cond(&self, cond: Cond) -> Result<Cond> {
        Ok(prepared::condition(filled_cond(cond, self.values)?, self.budget))
    }

    /// A scan's probe filled, and the conjunct it decides, if one, filled
    /// and prepared as a filter's would be.
    fn scan(&self, plan: Plan) -> Result<Plan> {
        let mut plan = filled_scan(plan, self.values)?;
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

    fn plans(&self, plans: Vec<Plan>) -> Result<Vec<Plan>> {
        plans.into_iter().map(|p| self.plan(p)).collect()
    }

    /// The part run in parallel `p`, its plan filled.
    /// `plan` filled, boxed.
    fn boxed(&self, plan: Plan) -> Result<Box<Plan>> {
        Ok(Box::new(self.plan(plan)?))
    }

    fn parallel(&self, mut p: Box<crate::physical::Parallel>) -> Result<Plan> {
        p.input = self.plan(p.input)?;
        Ok(Plan::Parallel(p))
    }

    /// The set function `a`, its expressions filled.
    fn agg(&self, a: AggCall) -> Result<AggCall> {
        let arg = a.arg.map(|e| self.expr(e)).transpose()?;
        let distribution =
            a.distribution.map(|d| Ok::<_, SqlError>(Box::new(Distribution { fraction: self.expr(d.fraction.clone())?, ..*d })));
        Ok(AggCall { arg, filter: a.filter.map(|c| self.cond(c)).transpose()?, distribution: distribution.transpose()?, ..a })
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
            Plan::SeqScan { .. } => Ok(plan),
            Plan::Nearest(read) => self.nearest(read),
            Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est } => {
                let aggs = aggs.into_iter().map(|a| self.agg(a)).collect::<Result<_>>()?;
                Ok(Plan::Aggregate { groups, collations, aggs, seeds, detail, input: Box::new(self.plan(*input)?), est })
            }
            Plan::Distinct { input, est } => Ok(Plan::Distinct { input: Box::new(self.plan(*input)?), est }),
            Plan::Sort { keys, detail, input, most } => Ok(Plan::Sort { keys, detail, input: Box::new(self.plan(*input)?), most }),
            Plan::Limit { limit, input, est } => {
                let offset = limit.offset.map(|e| self.expr(e)).transpose()?;
                let fetch = limit.fetch.map(|f| Ok::<_, SqlError>(Fetch { count: self.expr(f.count)?, ..f })).transpose()?;
                Ok(Plan::Limit { limit: Limit { offset, fetch }, input: Box::new(self.plan(*input)?), est })
            }
            Plan::Append { left, right, est } => Ok(Plan::Append { left: Box::new(self.plan(*left)?), right: self.boxed(*right)?, est }),
            Plan::Parallel(p) => self.parallel(p),
            Plan::MergeAppend { inputs, keys, detail } => Ok(Plan::MergeAppend { inputs: self.plans(inputs)?, keys, detail }),
            Plan::Product { .. }
            | Plan::HashJoin { .. }
            | Plan::GroupJoin(..)
            | Plan::IndexJoin { .. }
            | Plan::MergeJoin(_)
            | Plan::OuterJoin { .. }
            | Plan::SetOp { .. }
            | Plan::System { .. }
            | Plan::Values { .. }
            | Plan::Recursive { .. }
            | Plan::Unnest(..)
            | Plan::Window { .. }
            | Plan::Polymorphic(..)
            | Plan::Recognize { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. } => fail(Code::Api, "[DESIGN] a kept plan holds an operator no kept plan holds"),
        }
    }
}
