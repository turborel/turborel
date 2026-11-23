//! \[DESIGN\] The physical plans of a kept program's leaves (RULES.md
//! D408).
//!
//! A leaf of a program, a value, condition or query, is a subquery whose
//! outer references are slots of the frame (D76), run at each evaluation
//! with their values as constants, optimized and planned each time; a
//! value is a row of expressions, its subqueries run the same way. Of a
//! program kept (`routines/kept.rs`), the leaf's query, or each subquery
//! of its row, whose outer references each name a slot of the frame, is
//! planned with each reference a slot (`Expr::Dynamic`) of the slot's
//! type. A physical plan that is one probe fixing a whole unique key,
//! which no value changes, is kept at once, as a prepared statement keeps
//! one (`physical/kept.rs`), and each run fills its slots with the outer
//! values. Any other such subquery learns its plan as a prepared query
//! does (RULES.md D414): each of its first runs plans it with its values,
//! which it runs, and with its slots, and once their plans keep one shape
//! the one with slots is kept, until a table it reads gains or loses rows
//! as its statistics would be gathered again for. One whose plans keep
//! changing is planned at each run. The plans are the program's, made
//! again with it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::super::State;
use super::super::super::compiled::{plan, plan_slotted};
use super::super::super::prepared::{Fit, Learned, Learning, Learnt};
use crate::catalog::{QualifiedName, SequenceDesc};
use crate::error::{Result, Warnings};
use crate::exec::{Db, Slots, Subqueries, SubqueryRows, execute};
use crate::fulltext::rank::Statistics;
use crate::logical::{Expr, Invocation, Leaf, LogicalPlan, Slot, Subquery, filled, map_plan_exprs};
use crate::physical::{Kept, Plan, keepable, prepared_plan};
use crate::storage::Row;
use crate::sync::lock;
use crate::txn::TxId;
use crate::value::{DataType, Value};

/// The plans of a leaf's subqueries, by their identities.
pub(in crate::db) type LeafPlans = HashMap<u64, LeafPlan>;

/// A subquery of a leaf planned with slots, and what it keeps of its plan.
#[derive(Debug)]
pub(in crate::db) struct LeafPlan {
    /// Its logical plan, each outer reference a slot.
    slotted: LogicalPlan,
    kept: Mutex<Physical>,
}

/// What a leaf's subquery keeps of its physical plan.
#[derive(Debug)]
enum Physical {
    /// Its plan with slots, the tables it reads and, of a plan learned,
    /// what it is measured from (`prepared/plans/grown.rs`).
    Fixed(Arc<Kept>, Arc<[QualifiedName]>, Option<Learnt>),
    /// Its plan learning.
    Learning(Learning),
    /// Planned at each run; with what its tables were as it stopped
    /// learning, if it did, so it learns again once one drifts.
    Varies(Option<Learnt>),
}

impl LeafPlan {
    /// How it runs for the outer values `outer`: its plan kept, a point
    /// read run as it is (RULES.md D752) or as `Kept::opened` opens it
    /// (D610), with the values it reads as its slots, if it does; else,
    /// while it learns, planned with them; none once it varies.
    ///
    /// # Errors
    ///
    /// As `logical::filled`; SQLCODE -910 if a lock is poisoned.
    fn physical(&self, st: &State, outer: &[Value]) -> Result<Option<Run>> {
        let held = std::mem::replace(&mut *lock(&self.kept)?, Physical::Varies(None));
        let learning = match held {
            Physical::Varies(stopped) => {
                if !stopped.as_ref().is_some_and(|s| matches!(s.now(st), Fit::Moved)) {
                    *lock(&self.kept)? = Physical::Varies(stopped);
                    return Ok(None);
                }
                Learning::default()
            }
            Physical::Learning(learning) => learning,
            Physical::Fixed(plan, reads, learnt) => {
                let kept = match learnt.as_ref().map(|l| l.now(st)) {
                    None | Some(Fit::Still) => Some(learnt),
                    Some(Fit::Grown(grown)) => Some(Some(grown)),
                    Some(Fit::Moved) => None,
                };
                if let Some(still) = kept {
                    let run = if plan.point(st).is_some() {
                        Ok(Run::Point(Arc::clone(&plan)))
                    } else {
                        plan.opened(Some(outer.into()), crate::optimizer::prepared::budget(st)).map(|(p, s)| Run::Plan(Box::new(p), s))
                    };
                    *lock(&self.kept)? = Physical::Fixed(plan, reads, still);
                    return run.map(Some);
                }
                Learning::default()
            }
        };
        self.learned(st, outer, learning).map(|(p, s)| Some(Run::Plan(Box::new(p), s)))
    }

    /// A learning run of its plan for `outer`, which `learning` says it
    /// learned before: planned with them, which it runs, and with slots.
    ///
    /// # Errors
    ///
    /// As `physical`.
    fn learned(&self, st: &State, outer: &[Value], learning: Learning) -> Result<(Plan, Option<Slots>)> {
        let (optimized, _, custom) = plan(st, filled(self.slotted.clone(), outer)?);
        let reads: Arc<[QualifiedName]> = optimized.tables().into();
        let slotted_plan = plan_slotted(st, self.slotted.clone(), outer);
        let physical = match learning.after(&custom, slotted_plan).outcome() {
            Learned::Kept(slotted) => {
                let learnt = Learnt::of(st, &reads, &slotted);
                match prepared_plan(*slotted, crate::optimizer::prepared::budget(st)) {
                    Ok(slotted) => Physical::Fixed(Arc::new(slotted), reads, Some(learnt)),
                    Err(_) => Physical::Varies(None),
                }
            }
            Learned::Learning(learning) => Physical::Learning(learning),
            Learned::Stopped => Physical::Varies(Some(Learnt::rows(st, &reads))),
        };
        *lock(&self.kept)? = physical;
        Ok((custom, None))
    }
}

/// How a leaf's subquery runs.
enum Run {
    /// By its physical plan, and the values it reads as its slots.
    Plan(Box<Plan>, Option<Slots>),
    /// As the point read of its plan kept (RULES.md D752).
    Point(Arc<Kept>),
}

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
        let (st, tx) = self.at;
        let planned = self.plans.get(&subquery.id()).map(|kept| kept.physical(st, outer)).transpose()?.flatten();
        let (physical, slots) = match planned {
            None => return self.runner.rows(subquery, outer, limit),
            Some(Run::Point(kept)) => match kept.point(st) {
                Some(point) => {
                    let found = point.read(st, tx, outer)?;
                    return Ok(SubqueryRows::held(found.into_iter().take(limit.unwrap_or(1)).collect(), st));
                }
                None => kept.opened(Some(outer.into()), crate::optimizer::prepared::budget(st))?,
            },
            Some(Run::Plan(physical, slots)) => (*physical, slots),
        };
        let db = slots.map_or_else(|| Db::of(st), |slots| Db::slotted(st, slots));
        let rows = execute(physical, &db, tx);
        let found = match limit {
            Some(n) => rows.take(n).collect::<Result<Vec<Row>>>()?,
            None => rows.collect::<Result<Vec<Row>>>()?,
        };
        Ok(SubqueryRows::held(found, st))
    }

    fn slot(&self, index: usize) -> Option<&Value> {
        self.runner.slot(index)
    }

    fn outer(&self, index: usize) -> Option<&Value> {
        self.runner.outer(index)
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
    let Some(slots) = query.outer.iter().map(frame_slot).collect::<Option<Vec<(usize, DataType)>>>() else { return LeafPlans::new() };
    let LogicalPlan::Values { rows, .. } = &query.plan else {
        return kept(st, query, &slots).map(|p| (query.id(), p)).into_iter().collect();
    };
    let mut found = Vec::new();
    rows.iter().flatten().for_each(|e| scalar_subqueries(e, &mut found));
    found
        .into_iter()
        .filter_map(|s| {
            let named = s.outer.iter().map(|e| outer_slot(e, &slots)).collect::<Option<Vec<(usize, DataType)>>>()?;
            kept(st, s, &named).map(|p| (s.id(), p))
        })
        .collect()
}

/// `query` planned with its outer references slots, each of the frame's
/// slot and type of `slots`, if it has no slot of its own: its plan kept
/// if it is one probe fixing a whole unique key, else learning. Outer
/// references of one slot of the frame are one slot of the plan, so a
/// variable named twice is one value, as the optimizer sees it
/// (RULES.md D418).
fn kept(st: &State, query: &Subquery, slots: &[(usize, DataType)]) -> Option<LeafPlan> {
    if filled(query.plan.clone(), &[]).is_err() {
        return None;
    }
    let first = |frame: usize| slots.iter().position(|(f, _)| *f == frame);
    let places: Vec<(usize, DataType)> = slots.iter().map(|(frame, t)| (first(*frame).unwrap_or_default(), t.clone())).collect();
    let slotted = map_plan_exprs(query.plan.clone(), &mut |e| slotted(e, &places));
    let (optimized, _, physical) = plan(st, slotted.clone());
    let kept = if keepable(&physical, st) {
        let Ok(physical) = prepared_plan(physical, crate::optimizer::prepared::budget(st)) else { return None };
        Physical::Fixed(Arc::new(physical), optimized.tables().into(), None)
    } else {
        Physical::Learning(Learning::default())
    };
    Some(LeafPlan { slotted, kept: Mutex::new(kept) })
}

/// \[DESIGN\] The plan `leaf`, a query of the frame's slots, keeps fixed
/// at once, if it is one point read of a whole unique key (RULES.md D752,
/// D839): the plan the leaf's run by its plans kept runs as its point
/// read, so a compiled program reads the same row through it.
pub(in crate::db) fn point_of(st: &State, leaf: &Leaf) -> Option<Arc<Kept>> {
    let query = &leaf.query;
    if let LogicalPlan::Values { .. } = &query.plan {
        return None;
    }
    let slots = query.outer.iter().map(frame_slot).collect::<Option<Vec<(usize, DataType)>>>()?;
    let planned = kept(st, query, &slots)?;
    let Ok(Physical::Fixed(plan, _, None)) = planned.kept.into_inner() else { return None };
    plan.point(st).is_some().then_some(plan)
}

/// The scalar subqueries of `e`, at any depth of its expressions, not
/// those nested in them.
fn scalar_subqueries<'e>(e: &'e Expr, found: &mut Vec<&'e Subquery>) {
    if let Expr::Subquery(s, _) = e {
        found.push(s);
    }
    e.for_each_child(&mut |child| scalar_subqueries(child, found));
}

/// The frame's slot `e` is, and its type, if it is one.
fn frame_slot(e: &Expr) -> Option<(usize, DataType)> {
    if let Expr::Dynamic(slot) = e { Some((slot.index, slot.data_type.clone())) } else { None }
}

/// The frame's slot and type of `e`, an outer reference of a subquery of
/// a leaf's row, if it names an outer reference of the leaf, of `slots`.
fn outer_slot(e: &Expr, slots: &[(usize, DataType)]) -> Option<(usize, DataType)> {
    if let Expr::Outer(at, _) = e { slots.get(*at).cloned() } else { None }
}

/// `e`, each outer reference the slot of `places` of its place, of its
/// type there.
fn slotted(e: Expr, places: &[(usize, DataType)]) -> Expr {
    if let Expr::Outer(index, name) = e {
        return places
            .get(index)
            .map_or(Expr::Outer(index, name), |(at, t)| Expr::Dynamic(Slot { index: *at, data_type: t.clone(), at: 0 }));
    }
    e.map_children(&mut |child| slotted(child, places))
}
