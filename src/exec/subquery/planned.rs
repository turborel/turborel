//! \[DESIGN\] A correlated subquery's plan kept across its runs (RULES.md
//! D675).
//!
//! A run binds the subquery's outer values as constants (`Subquery::bound`),
//! then optimizes and plans it as any query, so an index answers a
//! correlated comparison. The runner plans it, too, with each outer
//! reference a slot (`Expr::Dynamic`) of the type of its first value's kind
//! (`slot_type`), optimized once: each of its first runs plans that with
//! its values, its paths chosen by them, its probes holding the slots
//! (`physical/kept.rs`), and runs the plan made with its values. A plan
//! with slots that is one probe of a whole unique key is kept at once; any
//! other once `LEARNED_RUNS` runs in a row make both plans one shape, as a
//! prepared query learns its plan (RULES.md D45, `db/prepared/plans`). Each
//! later run fills the plan kept with its values, as they are, as binding
//! them would hold them, and runs it: no rule or planner runs again. A
//! subquery whose plans keep changing is planned at each run after
//! `MOST_LEARNING_RUNS`; one whose outer value is of a kind no index
//! places as it is, or whose plan holds slots of its own, at each run from
//! the first; and a run whose value is null, or of another kind than its
//! slot's, is planned alone. Null values say nothing of their kind, so a
//! subquery learns from its first run with none.
//! A subquery decorrelated after fewer runs than `MOST_LEARNING_RUNS`
//! (EXECUTOR.DECORRELATE_AFTER_RUNS) is planned at each run until then, as
//! its learning would cost more runs than it saves.

use std::collections::HashMap;
use std::fmt;
use std::mem::discriminant;
use std::sync::Mutex;

use super::super::{RowIter, execute};
use super::Runner;
use crate::db::prepared::{Learned, Learning, MOST_LEARNING_RUNS};
use crate::logical::{Expr, LogicalPlan, Slot, Subquery, filled, map_plan_exprs};
use crate::optimizer::RuleContext;
use crate::optimizer::prepared::budget;
use crate::physical::{PhysicalPlanner, Plan, filled_plan_as_is, keepable};
use crate::settings::{DECORRELATE_AFTER_RUNS, KEEP_SUBQUERY_PLANS};
use crate::value::datetime::Moment;
use crate::value::{
    CharUnits, DOUBLE_PRECISION, DataType, MAX_BINARY_LENGTH, MAX_CHARACTER_LENGTH, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Spelled,
    Value,
};

/// What the runner keeps of a correlated subquery's plan.
#[derive(Default)]
pub(super) enum Planned {
    /// Not yet planned with slots: no run had outer values of a known kind.
    #[default]
    Untried,
    /// Its plan with slots, learning.
    Learning(Box<Slotted>, Learning),
    /// Its plan with slots, and its physical plan with slots, kept.
    Kept(Box<Slotted>, Box<Plan>),
    /// Planned at each run.
    Varies,
}

impl Planned {
    /// A copy of it, once it is kept or planned at each run.
    fn settled(&self) -> Option<Planned> {
        match self {
            Planned::Kept(slotted, plan) => Some(Planned::Kept(slotted.clone(), plan.clone())),
            Planned::Varies => Some(Planned::Varies),
            Planned::Untried | Planned::Learning(..) => None,
        }
    }
}

/// \[DESIGN\] What a kept plan's correlated subqueries settled, by their
/// identities, kept across its executions (RULES.md D793): each
/// execution's runner is its own, so one would otherwise plan each
/// subquery again at its first run.
#[derive(Default)]
pub struct KeptSubqueries(Mutex<HashMap<u64, Planned>>);

impl fmt::Debug for KeptSubqueries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeptSubqueries")
    }
}

impl KeptSubqueries {
    /// What the subquery `at` settled, if it did.
    fn get(&self, at: u64) -> Option<Planned> {
        self.0.lock().ok()?.get(&at).and_then(Planned::settled)
    }

    /// `planned` kept as the subquery `at`'s, once it is settled.
    fn put(&self, at: u64, planned: &Planned) {
        if let (Some(settled), Ok(mut kept)) = (planned.settled(), self.0.lock()) {
            kept.insert(at, settled);
        }
    }
}

/// A subquery's plan, each outer reference a slot, optimized: the slots'
/// types, of its first outer values none of which was null.
#[derive(Clone)]
pub(super) struct Slotted {
    types: Vec<DataType>,
    plan: LogicalPlan,
}

impl Slotted {
    /// Whether `outer` are values of its slots' kinds, none null. A kept
    /// plan holds a value of its slot's kind as binding it would: an index
    /// places every value of a kind its column takes as it is, whatever
    /// its precision, scale or length (`physical/probe.rs`), so the kind
    /// alone is checked, and a value of another kind has its run planned.
    /// A null is planned too: bound, a comparison with it is unknown and
    /// reads nothing, where a probe by it would find the rows whose key
    /// is null.
    fn fits(&self, outer: &[Value]) -> bool {
        outer.len() == self.types.len()
            && outer.iter().zip(&self.types).all(|(v, t)| slot_type(v).is_some_and(|own| discriminant(&own) == discriminant(t)))
    }
}

/// \[DESIGN\] The type of the slot of an outer reference whose value is
/// `v`: of its kind, as wide as it is. Its value is placed as it is
/// (`Placing::AsIs`), so the type tells only whether an index places it
/// so (`placed_as_is`), and stands for it as the planner estimates; none
/// of a null, whose kind is unknown, nor of a kind no index places so.
pub(crate) fn slot_type(v: &Value) -> Option<DataType> {
    Some(match v {
        Value::Char(_) => DataType::Varying { length: MAX_CHARACTER_LENGTH, units: CharUnits::Characters },
        Value::Exact(d) => DataType::Exact { precision: MAX_EXACT_PRECISION, scale: d.scale, spelled: Spelled::Numeric },
        Value::Approx(_) => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision },
        Value::DecFloat(_) => DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION },
        Value::Binary(_) => DataType::VarBinary { length: MAX_BINARY_LENGTH },
        Value::Bool(_) => DataType::Boolean,
        Value::Datetime(d) => {
            let precision = u32::from(d.precision);
            match d.moment {
                Moment::Date => DataType::Date,
                Moment::Time => DataType::Time { precision, zoned: d.zoned },
                Moment::Timestamp => DataType::Timestamp { precision, zoned: d.zoned },
            }
        }
        Value::Interval(i) => DataType::Interval(i.qualifier),
        Value::Null
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => return None,
    })
}

/// `e`, each outer reference the slot of its place, of its type of `types`.
fn slotted(e: Expr, types: &[DataType]) -> Expr {
    if let Expr::Outer(index, name) = e {
        return types.get(index).map_or(Expr::Outer(index, name), |t| Expr::Dynamic(Slot { index, data_type: t.clone(), at: 0 }));
    }
    e.map_children(&mut |child| slotted(child, types))
}

impl Runner {
    /// `plan` optimized.
    fn optimized(&self, plan: LogicalPlan) -> LogicalPlan {
        self.db.optimizer.optimize(plan, &RuleContext { db: &self.db }).0
    }

    /// The rows of a run of `subquery` for `outer`, by the plan it keeps,
    /// filled, or planned with them; with no outer values, its only run,
    /// and with EXECUTOR.KEEP_SUBQUERY_PLANS OFF, planned with them.
    pub(super) fn per_run(&self, subquery: &Subquery, outer: &[Value]) -> RowIter {
        if outer.is_empty() || !self.db.settings.is_on(KEEP_SUBQUERY_PLANS) {
            return self.run(subquery.bound(outer));
        }
        let at = subquery.id();
        let mut was = self.seen.borrow_mut().get_mut(&at).map(|s| std::mem::take(&mut s.planned)).unwrap_or_default();
        // Of a kept plan, as its earlier executions settled it (RULES.md D793).
        let kept = self.db.subqueries();
        if let (Planned::Untried, Some(settled)) = (&was, kept.and_then(|k| k.get(at))) {
            was = settled;
        }
        let fresh = matches!(was, Planned::Untried | Planned::Learning(..));
        let (plan, now) = self.planned(subquery, outer, was);
        if let (true, Some(k)) = (fresh, kept) {
            k.put(at, &now);
        }
        self.seen.borrow_mut().entry(at).or_default().planned = now;
        execute(plan, &self.db, self.tx)
    }

    /// The physical plan of `subquery` for `outer`, by what `was` keeps of
    /// it, and what it keeps after this run.
    fn planned(&self, subquery: &Subquery, outer: &[Value], was: Planned) -> (Plan, Planned) {
        let custom = || PhysicalPlanner::new(&self.db).plan(&self.optimized(subquery.bound(outer)));
        match was {
            Planned::Varies => (custom(), Planned::Varies),
            Planned::Untried => self.untried(subquery, outer).unwrap_or_else(|untried| (custom(), untried)),
            Planned::Learning(slotted, learning) => {
                if !slotted.fits(outer) {
                    return (custom(), Planned::Learning(slotted, learning));
                }
                self.learn(custom(), outer, slotted, learning)
            }
            Planned::Kept(slotted, plan) => {
                if !slotted.fits(outer) {
                    return (custom(), Planned::Kept(slotted, plan));
                }
                // A kept plan that fails to fill holds an operator a kept
                // plan does not; the run is planned instead, as each after.
                match filled_plan_as_is((*plan).clone(), outer, budget(&self.db)) {
                    Ok(filled) => (filled, Planned::Kept(slotted, plan)),
                    Err(_) => (custom(), Planned::Varies),
                }
            }
        }
    }

    /// Of a subquery not yet planned with slots: its first learning run, if
    /// `outer` holds no null and its slots are typed; else what it keeps,
    /// planned alone this run: untried while a value is null, at each run
    /// once a value is of a kind no slot takes or its plan holds slots of
    /// its own, or once it is decorrelated before learning could pay. A
    /// plan with slots that is one probe of a whole unique key is kept even
    /// so, as its probes may cost less than a build (`probed.rs`).
    fn untried(&self, subquery: &Subquery, outer: &[Value]) -> Result<(Plan, Planned), Planned> {
        if outer.iter().any(Value::is_null) {
            return Err(Planned::Untried);
        }
        let types = outer.iter().map(slot_type).collect::<Option<Vec<DataType>>>().ok_or(Planned::Varies)?;
        if filled(subquery.plan.clone(), &[]).is_err() {
            return Err(Planned::Varies);
        }
        let plan = self.optimized(map_plan_exprs(subquery.plan.clone(), &mut |e| slotted(e, &types)));
        let slotted = Box::new(Slotted { types, plan });
        let with_slots = PhysicalPlanner::slotted(&self.db, outer).plan(&slotted.plan);
        let custom = PhysicalPlanner::new(&self.db).plan(&self.optimized(subquery.bound(outer)));
        if keepable(&with_slots, &self.db) {
            return Ok((custom, Planned::Kept(slotted, Box::new(with_slots))));
        }
        if self.decorrelated_soon(subquery) {
            return Ok((custom, Planned::Varies));
        }
        let learning = Learning::default().after(&custom, with_slots);
        Ok((custom, learned(slotted, learning)))
    }

    /// Whether `subquery` is decorrelated after fewer runs than a plan
    /// learns over, so learning its plan would cost more runs than it saves.
    fn decorrelated_soon(&self, subquery: &Subquery) -> bool {
        self.db.settings.count(DECORRELATE_AFTER_RUNS) < usize::from(MOST_LEARNING_RUNS) && subquery.decorrelated().is_some()
    }

    /// A learning run of the plan with slots `slotted`, `custom` the plan
    /// made with its values `outer`, which it runs.
    fn learn(&self, custom: Plan, outer: &[Value], slotted: Box<Slotted>, was: Learning) -> (Plan, Planned) {
        let with_slots = PhysicalPlanner::slotted(&self.db, outer).plan(&slotted.plan);
        let learning = was.after(&custom, with_slots);
        (custom, learned(slotted, learning))
    }
}

/// What a subquery keeps once `learning` says what it learned.
fn learned(slotted: Box<Slotted>, learning: Learning) -> Planned {
    match learning.outcome() {
        Learned::Kept(plan) => Planned::Kept(slotted, plan),
        Learned::Learning(learning) => Planned::Learning(slotted, learning),
        Learned::Stopped => Planned::Varies,
    }
}

#[cfg(test)]
mod tests;
