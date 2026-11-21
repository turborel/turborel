//! A kept physical plan filled and prepared for one execution. Access
//! paths stay fixed; scalar preparation sees the new constant operands and
//! preserves lazy runtime evaluation (D15, D38, D45, D85). An inner join,
//! a product, a hash join or an index join by the outer row's columns, is
//! filled too (D418).

use std::sync::Arc;

use super::filled_scan;
use crate::error::{Code, Result, SqlError, fail};
use crate::logical::{AggCall, Cond, Distribution, Expr, Fetch, Limit, Truth, filled_cond, filled_expr};
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
            probe: probe @ JoinProbe::Prefix(_),
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
            Plan::Product { .. } | Plan::HashJoin { .. } | Plan::IndexJoin { .. } => self.join(plan),
            Plan::MergeJoin(_)
            | Plan::SemiJoin(_)
            | Plan::RangeJoin(_)
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
            | Plan::Transition { .. }
            | Plan::Counted(_) => fail(Code::Api, "[DESIGN] a kept plan holds an operator no kept plan holds"),
        }
    }
}
