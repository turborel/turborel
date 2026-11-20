//! \[DESIGN\] The physical plans of a kept program's leaves (RULES.md
//! D408).
//!
//! A leaf of a program, a value, condition or query, is a subquery whose
//! outer references are slots of the frame (D76), run at each evaluation
//! with their values as constants, optimized and planned each time; a
//! value is a row of expressions, its subqueries run the same way. Of a
//! program kept (`routines/kept.rs`), the leaf's query, or each subquery
//! of its row, whose outer references each name a slot of the frame, is
//! planned once instead, each reference a slot (`Expr::Dynamic`) of the
//! slot's type. A physical plan that is one probe fixing a whole unique
//! key, which no value changes, is kept, as a prepared statement keeps
//! one (`physical/kept.rs`), and each run fills its slots with the outer
//! values. Any other subquery runs as before. The plans are the
//! program's, made again with it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::super::State;
use super::super::super::compiled::plan;
use crate::catalog::SequenceDesc;
use crate::error::{Result, Warnings};
use crate::exec::{Subqueries, SubqueryRows, execute};
use crate::fulltext::rank::Statistics;
use crate::logical::{Expr, Invocation, Leaf, LogicalPlan, Slot, Subquery, filled, map_plan_exprs};
use crate::physical::{Plan, filled_plan, keepable};
use crate::storage::Row;
use crate::sync::lock;
use crate::txn::TxId;
use crate::value::{DataType, Value};

/// The plans kept of a leaf's subqueries, by their identities.
pub(in crate::db) type LeafPlans = HashMap<u64, Arc<Plan>>;

/// The plans of a program's leaves, by their subqueries' identities.
#[derive(Debug, Default)]
pub(in crate::db) struct KeptLeaves(Mutex<HashMap<u64, Arc<LeafPlans>>>);

impl KeptLeaves {
    /// The plans `leaf` keeps, made the first time they are asked for:
    /// none if it keeps none.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn of(&self, st: &State, leaf: &Leaf) -> Result<Option<Arc<LeafPlans>>> {
        let id = leaf.query.id();
        let found = lock(&self.0)?.get(&id).cloned();
        let plans = if let Some(plans) = found {
            plans
        } else {
            let made = Arc::new(made(st, leaf));
            lock(&self.0)?.insert(id, Arc::clone(&made));
            made
        };
        Ok((!plans.is_empty()).then_some(plans))
    }
}

/// What runs a leaf's subqueries: by their plans kept, those it keeps;
/// the others by the statement's runner.
pub(in crate::db) struct KeptRuns<'a> {
    /// The statement's runner.
    pub(in crate::db) runner: &'a dyn Subqueries,
    /// The plans kept.
    pub(in crate::db) plans: &'a LeafPlans,
    /// The database and the transaction the statement runs in.
    pub(in crate::db) at: (&'a Arc<State>, TxId),
}

impl Subqueries for KeptRuns<'_> {
    fn rows(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<SubqueryRows> {
        let Some(kept) = self.plans.get(&subquery.id()) else { return self.runner.rows(subquery, outer, limit) };
        let (st, tx) = self.at;
        let filled = filled_plan((**kept).clone(), outer, crate::optimizer::prepared::budget(st))?;
        let rows = execute(filled, st, tx);
        let found = match limit {
            Some(n) => rows.take(n).collect::<Result<Vec<Row>>>()?,
            None => rows.collect::<Result<Vec<Row>>>()?,
        };
        Ok(SubqueryRows::held(found, st))
    }

    fn warnings(&self) -> Option<&Warnings> {
        self.runner.warnings()
    }

    fn like(&self, value: &str, pattern: &str, escape: Option<char>) -> Result<bool> {
        self.runner.like(value, pattern, escape)
    }

    fn next_value(&self, seq: &SequenceDesc) -> Result<Value> {
        self.runner.next_value(seq)
    }

    fn interrupted(&self) -> Result<()> {
        self.runner.interrupted()
    }

    fn state(&self) -> Option<&Arc<State>> {
        self.runner.state()
    }

    fn ranking_statistics(&self, index: &str) -> Result<Option<Arc<dyn Statistics>>> {
        self.runner.ranking_statistics(index)
    }

    fn temp_lobs(&self) -> Option<&dyn crate::value::lob::TempLobs> {
        self.runner.temp_lobs()
    }

    fn invoke_hosted(&self, i: &Invocation, values: Vec<Value>) -> Option<Result<Option<Value>>> {
        self.runner.invoke_hosted(i, values)
    }
}

/// The plans `leaf` keeps: of its query, or of each subquery of its row
/// of values, each outer reference of which names a slot of the frame.
fn made(st: &State, leaf: &Leaf) -> LeafPlans {
    let query = &leaf.query;
    let Some(types) = query.outer.iter().map(slot_type).collect::<Option<Vec<DataType>>>() else { return LeafPlans::new() };
    let LogicalPlan::Values { rows, .. } = &query.plan else {
        return kept(st, query, &types).map(|p| (query.id(), p)).into_iter().collect();
    };
    let mut found = Vec::new();
    rows.iter().flatten().for_each(|e| scalar_subqueries(e, &mut found));
    found
        .into_iter()
        .filter_map(|s| {
            let named = s.outer.iter().map(|e| outer_type(e, &types)).collect::<Option<Vec<DataType>>>()?;
            kept(st, s, &named).map(|p| (s.id(), p))
        })
        .collect()
}

/// The plan of `query`, its outer references slots of `types`, if it is
/// one probe fixing a whole unique key and the query has no slot of its
/// own.
fn kept(st: &State, query: &Subquery, types: &[DataType]) -> Option<Arc<Plan>> {
    if filled(query.plan.clone(), &[]).is_err() {
        return None;
    }
    let slotted = map_plan_exprs(query.plan.clone(), &mut |e| slotted(e, types));
    let (_, _, physical) = plan(st, slotted);
    keepable(&physical, st).then(|| Arc::new(physical))
}

/// The scalar subqueries of `e`, at any depth of its expressions, not
/// those nested in them.
fn scalar_subqueries<'e>(e: &'e Expr, found: &mut Vec<&'e Subquery>) {
    if let Expr::Subquery(s, _) = e {
        found.push(s);
    }
    e.for_each_child(&mut |child| scalar_subqueries(child, found));
}

/// The type of `e`, if it is a slot of the frame.
fn slot_type(e: &Expr) -> Option<DataType> {
    if let Expr::Dynamic(slot) = e { Some(slot.data_type.clone()) } else { None }
}

/// The type of `e`, an outer reference of a subquery of a leaf's row, if
/// it names an outer reference of the leaf, of `types`.
fn outer_type(e: &Expr, types: &[DataType]) -> Option<DataType> {
    if let Expr::Outer(at, _) = e { types.get(*at).cloned() } else { None }
}

/// `e`, each outer reference a slot of the same place, of its type of
/// `types`.
fn slotted(e: Expr, types: &[DataType]) -> Expr {
    if let Expr::Outer(index, name) = e {
        return types.get(index).map_or(Expr::Outer(index, name), |t| Expr::Dynamic(Slot { index, data_type: t.clone(), at: 0 }));
    }
    e.map_children(&mut |child| slotted(child, types))
}
