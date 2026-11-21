//! \[DESIGN\] (RULES.md D490) Groupjoins: a grouping on a join's key
//! computed in the join's table, as Moerkotte and Neumann, "Accelerating
//! Queries with Group-By and Join by Groupjoin" (PVLDB 2011), and Fent and
//! Neumann, "A Practical Approach to Groupjoin and Nested Aggregates"
//! (PVLDB 2021), compute it. Once the plan is made, each grouping of a
//! join recognized as one (`groupjoin/recognized.rs`) is planned again in
//! one of their two forms, or left as it is, as their costs say:
//!
//! * memoizing, `Plan::GroupJoin`: the group operand read into a table by
//!   its keys, each row a group, and the other operand's rows each finding
//!   its group there and feeding its set functions, no joined row made
//!   (`exec/groupjoin.rs`);
//! * eager, of an inner join alone: the other operand grouped by its keys
//!   first, then joined with the group operand (`groupjoin/eager.rs`).
//!
//! Both give the rows of the grouping, in its order: groups come in the
//! order of their first rows, which is the group operand's of a join it
//! was the left operand of, else the order of their first rows joined of
//! the other's; and each set function takes its values in the order the
//! join gave them, so a sum of approximate numbers is the same sum. The
//! setting OPTIMIZER.GROUPJOIN turns it off (RULES.md D36).

mod columns;
mod eager;
mod recognized;

use crate::db::State;
use crate::logical::{AggCall, Cond, Expr};
use crate::physical::{JoinKey, Plan};
use columns::rows;
use recognized::{Found, recognized};

/// The name of the rule whose setting turns groupjoins on or off,
/// OPTIMIZER.GROUPJOIN (`optimizer/groupjoin.rs`).
pub const GROUPJOIN: &str = "groupjoin";

/// \[DESIGN\] How deep a plan is walked for its groupings (RULES.md
/// R-CF-6): deeper than any plan the binder makes nests.
const MAX_DEPTH: usize = 1024;

/// \[DESIGN\] How many times the rows of the group operand may outnumber
/// the other's, of an inner join the group operand probed, for a
/// memoizing groupjoin to be made: past it, building the group operand
/// costs more than the join's build of the other and the grouping of the
/// few rows it gives.
const MEMO_RATIO: f64 = 4.0;

/// \[DESIGN\] How many times the rows of the group operand must outnumber
/// the groups the other's keys make for the eager form to be made: then
/// grouping the other first leaves a join of few rows, where the
/// memoizing form would build every group operand row.
const EAGER_RATIO: f64 = 8.0;

/// \[DESIGN\] (RULES.md D490) A memoizing groupjoin: the rows of `build`
/// read into a table by their keys' columns, each a group, then each row
/// of `probe` whose keys equal a group's fed to its set functions; each
/// group given as `columns` of its build row then its set functions'
/// results, in the order of the build's rows, or of each group's first
/// probe row; of a LEFT join, each group no row joined fed one row of
/// nulls, as the join gives it.
#[derive(Debug, Clone)]
pub struct GroupJoin {
    /// The group operand, read whole first.
    pub build: Plan,
    /// The operand whose rows feed the groups.
    pub probe: Plan,
    /// Each pair of keys: a column of the build, `left`, and of the probe,
    /// `right`, matched as the join matched them.
    pub keys: Vec<JoinKey>,
    /// The set functions each group computes, of the probe's columns.
    pub aggs: Vec<AggCall>,
    /// Of each set function, whether a LEFT join's row of nulls feeds it
    /// nothing, as its filter by a marker, dropped, would not
    /// (`groupjoin/recognized.rs`).
    pub marked: Vec<bool>,
    /// The build's column each grouping column is.
    pub columns: Vec<usize>,
    /// Of a LEFT join: a group no probe row joined is given, fed one row of
    /// nulls; else it is none.
    pub outer: bool,
    /// Whether groups come in the order of their first probe rows, the
    /// build having been the join's right operand; else of the build's.
    pub by_first: bool,
    /// The widths of the build's rows and of the probe's.
    pub widths: (usize, usize),
    /// The grouping it stands for, run instead if it gives way (RULES.md
    /// D38).
    pub instead: Plan,
    /// The grouping and the join, as text.
    pub detail: String,
    /// Its estimated groups.
    pub est: f64,
}

/// `plan` with each grouping of a join that a groupjoin computes better
/// planned so, unless OPTIMIZER.GROUPJOIN is off.
pub(crate) fn groupjoined(db: &State, mut plan: Plan) -> Plan {
    if db.optimizer.enabled(GROUPJOIN, &db.settings) {
        walk(db, &mut plan, 0);
    }
    plan
}

/// Plans again the groupings of `plan`, `depth` below the root.
fn walk(db: &State, plan: &mut Plan, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    let deeper = depth.saturating_add(1);
    if let Some(made) = planned(plan, db) {
        *plan = made;
    }
    // A groupjoin's grouping, run if it gives way, is walked below its
    // own node: it is a groupjoin's grouping again.
    if let Plan::GroupJoin(g) = plan {
        walk(db, &mut g.build, deeper);
        walk(db, &mut g.probe, deeper);
        g.instead.each_input(&mut |input| walk(db, input, deeper));
        return;
    }
    plan.each_input(&mut |input| walk(db, input, deeper));
}

/// The plan of the grouping `plan` as a groupjoin, if it is one and one
/// of the two forms costs less than it.
fn planned(plan: &Plan, db: &State) -> Option<Plan> {
    let found = recognized(plan, db)?;
    if !calm_plan(found.build, 0) || !calm_plan(found.probe, 0) {
        return None;
    }
    let (built, probing) = (rows(found.build), rows(found.probe));
    if let Some((eager, groups)) = eager::eager(&found, plan, db)
        && built > EAGER_RATIO * groups
    {
        return Some(eager);
    }
    // Of an inner join that probed with the group operand, building it
    // instead pays only while it is not many times the other.
    if !found.outer && !found.by_first && built > MEMO_RATIO * probing {
        return None;
    }
    Some(memoizing(found, plan))
}

/// The memoizing groupjoin of `found`, standing for the grouping `plan`.
fn memoizing(found: Found<'_>, plan: &Plan) -> Plan {
    let Found { build, probe, keys, aggs, marked, columns, outer, by_first, widths, joined } = found;
    let (detail, est) =
        if let Plan::Aggregate { detail, est, .. } = plan { (described(detail, joined), *est) } else { (String::new(), 1.0_f64) };
    Plan::GroupJoin(Box::new(GroupJoin {
        build: build.clone(),
        probe: probe.clone(),
        keys,
        aggs,
        marked,
        columns,
        outer,
        by_first,
        widths,
        instead: plan.clone(),
        detail,
        est,
    }))
}

/// What EXPLAIN shows of a groupjoin of the grouping `grouped` and the
/// join `joined`: none of a plan not shown.
fn described(grouped: &str, joined: &str) -> String {
    if grouped.is_empty() && joined.is_empty() { String::new() } else { format!("{grouped} on {joined}") }
}

/// Whether every expression of `plan` is evaluated alone, of no subquery,
/// routine, sequence or parameter, as a part run in parallel's are
/// (`physical/parallel.rs`), so the plan read again, as a groupjoin that
/// gives way reads it, gives the same rows: of the operators a groupjoin's
/// operands are made of, each known.
fn calm_plan(plan: &Plan, depth: usize) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    let deeper = depth.saturating_add(1);
    if let Plan::SeqScan { located, .. } | Plan::IndexScan { located, .. } = plan {
        return !located;
    }
    if let Plan::HashJoin { left, right, .. } | Plan::Product { left, right, .. } = plan {
        return calm_plan(left, deeper) && calm_plan(right, deeper);
    }
    if let Plan::OuterJoin { left, right, cond, own, .. } = plan {
        return cond.as_ref().is_none_or(calm_cond) && own.is_none() && calm_plan(left, deeper) && calm_plan(right, deeper);
    }
    if let Plan::Filter { cond, input, .. } = plan {
        return calm_cond(cond) && calm_plan(input, deeper);
    }
    if let Plan::Project { exprs, input, .. } = plan {
        return exprs.iter().all(calm) && calm_plan(input, deeper);
    }
    if let Plan::Distinct { input, .. } = plan {
        return calm_plan(input, deeper);
    }
    let Plan::Aggregate { aggs, input, .. } = plan else { return false };
    aggs.iter().all(|a| a.arg.as_ref().is_none_or(calm) && a.filter.as_ref().is_none_or(calm_cond)) && calm_plan(input, deeper)
}

/// Whether a worker evaluates `e` alone (`physical/parallel.rs`).
fn calm(e: &Expr) -> bool {
    super::parallel::calm(e)
}

/// Whether a worker evaluates `c` alone (`physical/parallel.rs`).
fn calm_cond(c: &Cond) -> bool {
    super::parallel::calm_cond(c)
}
