//! \[EXTENSION\] The rows of several queries, UNION ALL, aggregated by
//! one set function: a subquery of one row and one column, as XMLMAP
//! makes its data of the tables of a schema or a catalog (RULES.md D106).

use std::cell::RefCell;

use super::super::query::Nesting;
use super::super::set_operations::bind_query_expression;
use super::super::{Binder, Context, OutColumn, Purpose, Scope};
use super::{Outer, aggregated};
use crate::ast;
use crate::error::{At, Result};
use crate::functions::AggregateRef;
use crate::logical::{Expr, LogicalPlan, SetOp};
use crate::value::DataType;

/// What a set function of the rows of a query is, made of the query's
/// columns: the function, its argument, its label, and its result's type.
pub(in crate::binder) type RowsAggregate = (AggregateRef, Expr, String, DataType);

/// Of the `n`th of a union's queries, of its columns, the expressions it
/// is projected by, if it is kept.
pub(in crate::binder) type MakeBranch<'m> = dyn FnMut(usize, &[OutColumn]) -> Result<Option<Vec<Expr>>> + 'm;

impl Binder<'_> {
    /// \[EXTENSION\] The rows of `queries`, each bound for `purpose` as a
    /// subquery of this query in `scope`, and kept, projected by the expressions `make`
    /// gives of its columns, if it gives any; then the rows of all those
    /// kept, one query's after another's, UNION ALL, aggregated by the set
    /// function `aggregate` makes: a subquery of one row and one column,
    /// its outer references made as they are bound (XMLMAP, RULES.md D106);
    /// none if no query is kept.
    pub(in crate::binder) fn aggregated_union(
        &self,
        (queries, purpose): (&[ast::Subquery], Purpose),
        scope: Scope<'_>,
        make: &mut MakeBranch<'_>,
        aggregate: &mut dyn FnMut() -> Result<RowsAggregate>,
    ) -> Result<Option<Expr>> {
        let outer = Outer { binder: self, scope, captured: RefCell::new(Vec::new()) };
        let nesting = Nesting::Subquery { outer: &outer, result: None };
        let mut kept = Vec::new();
        for (i, query) in queries.iter().enumerate() {
            let bound = bind_query_expression(Context { purpose, ..self.context() }, &query.query, &nesting).or_at(query.at)?;
            if let Some(exprs) = make(i, &bound.columns)? {
                kept.push(LogicalPlan::Project { exprs, input: Box::new(bound.plan) });
            }
        }
        let Some(input) = appended(kept) else { return Ok(None) };
        let (func, arg, label, result) = aggregate()?;
        Ok(Some(aggregated((func, arg, label), result, input, outer).0))
    }
}

/// `plans`, in order, UNION ALL, of their rows one plan's after
/// another's: paired as a balanced tree, so the plan of many is not deep.
fn appended(mut plans: Vec<LogicalPlan>) -> Option<LogicalPlan> {
    while plans.len() > 1 {
        let mut pairs = Vec::with_capacity(plans.len().div_ceil(2));
        let mut each = plans.into_iter();
        while let Some(left) = each.next() {
            pairs.push(match each.next() {
                Some(right) => {
                    LogicalPlan::SetOp { op: SetOp::Union, all: true, compared: None, left: Box::new(left), right: Box::new(right) }
                }
                None => left,
            });
        }
        plans = pairs;
    }
    plans.pop()
}
