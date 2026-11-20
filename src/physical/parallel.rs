//! \[DESIGN\] Where a query runs in parallel (RULES.md D291): the parts of
//! a plan the executor runs by morsels on several threads at once
//! (`exec/parallel.rs`), each marked `Parallel` with its degree, as EXPLAIN
//! shows it. A part is a pipeline, the reads of tables and the filters,
//! projections and hash joins' probes over them, read as one stream; or
//! one of the operators that end a pipeline, over one: a grouping whose
//! set functions keep states that merge, DISTINCT, a sort, window
//! functions of a partitioning, and UNION, EXCEPT and INTERSECT.
//!
//! The degree is of the rows the part's tables held as it was planned: as
//! many threads as morsels of EXECUTOR.MORSEL_ROWS they hold, up to
//! EXECUTOR.DEGREE, or the workers, EXECUTOR.WORKERS. A part reading fewer
//! rows than EXECUTOR.PARALLEL_MIN_ROWS stays on the statement's thread,
//! as a point query does, its plan unchanged, so it costs nothing more
//! than the walk that finds it. So does one a serial scan by batch of a
//! table kept by column serves better, with no hash join's probe nor
//! window functions, while EXECUTOR.BATCHES is on, and a bare COUNT(*) its
//! table's storage counts (RULES.md D351). An expression a worker cannot evaluate
//! alone keeps its pipeline whole on the statement's thread: a subquery's,
//! a routine's, NEXT VALUE FOR's, a parameter's.

mod inputs;

use std::sync::OnceLock;

use super::Plan;
use crate::db::State;
use crate::index::Probe;
use crate::logical::{AggCall, Cond, Expr, SetOp};
use crate::settings::{EXECUTOR_DEGREE, EXECUTOR_MORSEL_ROWS, EXECUTOR_PARALLEL_MIN_ROWS, EXECUTOR_WORKERS};

/// \[DESIGN\] How deep a plan is walked for its parts (RULES.md R-CF-6):
/// deeper than any plan the binder makes nests.
const MAX_DEPTH: usize = 1024;

/// \[DESIGN\] A part run in parallel (RULES.md D290): `input`, as the
/// executor runs it alone if it gives way, on up to `degree` threads.
#[derive(Debug, Clone)]
pub struct Parallel {
    /// The most threads it runs on at once, the statement's own among them.
    pub degree: usize,
    /// The part.
    pub input: Plan,
}

/// What the settings ask of a statement's parallel parts, and the
/// database whose tables they read.
#[derive(Clone, Copy)]
struct Asked<'a> {
    /// The database.
    db: &'a State,
    /// The most threads a part runs on.
    most: usize,
    /// The rows of a morsel.
    morsel: usize,
    /// The least rows a part reads in parallel.
    least: usize,
}

/// The threads the system runs at once, as EXECUTOR.WORKERS of 0 asks.
pub(crate) fn workers_of(db: &State) -> usize {
    match db.settings.count(EXECUTOR_WORKERS) {
        0 => *SYSTEM_THREADS.get_or_init(|| std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)),
        n => n,
    }
}

/// \[DESIGN\] The threads the system runs at once, asked of it once
/// (RULES.md D405): the asking reads files of the system's at each call,
/// which a statement planned would pay each time.
static SYSTEM_THREADS: OnceLock<usize> = OnceLock::new();

impl<'a> Asked<'a> {
    /// What `db`'s settings ask.
    fn of(db: &'a State) -> Asked<'a> {
        let workers = workers_of(db);
        let most = match db.settings.count(EXECUTOR_DEGREE) {
            0 => workers,
            n => n.min(workers.max(1)),
        };
        Asked { db, most, morsel: db.settings.count(EXECUTOR_MORSEL_ROWS).max(1), least: db.settings.count(EXECUTOR_PARALLEL_MIN_ROWS) }
    }

    /// The degree of the part `plan` reading `rows` rows: 1 to run it
    /// alone, as one a serial scan by batch serves better.
    fn degree(self, plan: &Plan, rows: usize) -> usize {
        if rows < self.least || (crate::exec::batched(self.db) && by_batch(self.db, plan)) {
            return 1;
        }
        self.most.min(rows.div_ceil(self.morsel))
    }
}

/// `plan` with each part worth running in parallel marked so.
pub(crate) fn parallel(db: &State, mut plan: Plan) -> Plan {
    let asked = Asked::of(db);
    if asked.most > 1 {
        walk(asked, &mut plan, 0);
    }
    plan
}

/// Marks the parts of `plan`, `depth` below the root.
fn walk(asked: Asked<'_>, plan: &mut Plan, depth: usize) {
    // A bare COUNT(*) its table's storage counts, faster than any split,
    // stays whole (RULES.md D351).
    if depth >= MAX_DEPTH || crate::exec::storage_counted(plan, asked.db).is_some() {
        return;
    }
    let degree = rooted(plan).map_or(1, |rows| asked.degree(plan, rows));
    let deeper = depth.saturating_add(1);
    if degree < 2 {
        // A limit of few rows over a stream reads too few for workers to
        // share, unless a filter leaves out rows it would read.
        if let Plan::Limit { input, .. } = plan
            && stream(input).is_some()
            && !filtered(input)
        {
            return outside(asked, input, deeper);
        }
        return plan.each_input(&mut |input| walk(asked, input, deeper));
    }
    outside(asked, plan, deeper);
    let input = std::mem::replace(plan, Plan::Values { rows: Vec::new(), detail: String::new() });
    *plan = Plan::Parallel(Box::new(Parallel { degree, input }));
}

/// Marks the parts of the inputs of `plan`, a part, but its pipelines:
/// those of a hash join's operand built, of a pipeline's.
fn outside(asked: Asked<'_>, plan: &mut Plan, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    let deeper = depth.saturating_add(1);
    if let Plan::HashJoin { left, right, .. } = plan {
        outside(asked, left, deeper);
        return walk(asked, right, deeper);
    }
    plan.each_input(&mut |input| outside(asked, input, deeper));
}

/// Whether the stream `plan` has a filter.
fn filtered(plan: &Plan) -> bool {
    if let Plan::Append { left, right } = plan {
        return filtered(left) || filtered(right);
    }
    if let Plan::Project { input, .. } | Plan::HashJoin { left: input, .. } = plan {
        return filtered(input);
    }
    matches!(plan, Plan::Filter { .. })
}

/// The rows a part rooted at `plan` reads, if it is one.
fn rooted(plan: &Plan) -> Option<usize> {
    if let Plan::Aggregate { aggs, input, .. } = plan {
        return mergeable(aggs).then(|| stream(input)).flatten();
    }
    if let Plan::Distinct { input } = plan {
        return stream(input);
    }
    if let Plan::Sort { input, most, .. } = plan {
        return most.is_none_or(|m| !m.ties).then(|| stream(input)).flatten();
    }
    if let Plan::Window { window, .. } = plan {
        let Plan::Sort { input, most: None, .. } = &window.input else { return None };
        let calm_calls = window.exprs().all(calm) && window.conds().all(calm_cond);
        return (calm_calls && !window.partition.is_empty()).then(|| stream(input)).flatten();
    }
    if let Plan::SetOp { op: SetOp::Except | SetOp::Intersect, left, right, .. } = plan {
        return Some(stream(left)?.saturating_add(stream(right)?));
    }
    stream(plan)
}

/// The rows the stream `plan` reads, if it is one: a pipeline, its rows
/// in their order, morsel by morsel.
pub(crate) fn stream(plan: &Plan) -> Option<usize> {
    if let Plan::SeqScan { order, located, rows, .. } = plan {
        return (order.is_empty() && !located).then_some(*rows);
    }
    if let Plan::IndexScan { reverse: false, located: false, decided: None, probe, est, .. } = plan {
        return (!matches!(probe, Probe::Xml(_) | Probe::Predicate { .. })).then(|| rows_of(*est));
    }
    if let Plan::Filter { cond, input, .. } = plan {
        return calm_cond(cond).then(|| stream(input)).flatten();
    }
    if let Plan::Project { exprs, input, .. } = plan {
        return exprs.iter().all(calm).then(|| stream(input)).flatten();
    }
    if let Plan::HashJoin { left, .. } = plan {
        return stream(left);
    }
    let Plan::Append { left, right } = plan else { return None };
    Some(stream(left)?.saturating_add(stream(right)?))
}

/// \[DESIGN\] Whether the part `plan` reads a table kept by column with
/// nothing a serial scan by batch leaves to rows, a hash join's probe or
/// window functions: that scan, making no row, outruns workers that make
/// each (RULES.md D291).
fn by_batch(db: &State, plan: &Plan) -> bool {
    if let Plan::Aggregate { input, .. } | Plan::Distinct { input } | Plan::Sort { input, .. } = plan {
        return by_batch(db, input);
    }
    if let Plan::Filter { input, .. } | Plan::Project { input, .. } = plan {
        return by_batch(db, input);
    }
    if let Plan::SetOp { left, right, .. } | Plan::Append { left, right } = plan {
        return by_batch(db, left) || by_batch(db, right);
    }
    let Plan::SeqScan { table, .. } = plan else { return false };
    db.storage_of(table).is_ok_and(|s| s.keeps_columns())
}

/// The rows of an estimate `est`, as a count.
pub(crate) fn rows_of(est: f64) -> usize {
    // No float converts to an integer without a cast (R-NUM-2).
    crate::spatial::curves::linearize::count(est, usize::MAX >> 1)
}

/// Whether a grouping of `aggs` merges states made apart: each
/// set function's argument and filter calm, none DISTINCT nor an inverse
/// distribution function, and each keeping a state it writes.
fn mergeable(aggs: &[AggCall]) -> bool {
    aggs.iter().all(|a| {
        let calm_parts = a.arg.as_ref().is_none_or(calm) && a.filter.as_ref().is_none_or(calm_cond);
        let writes = a.func.0.accumulator(a.result.clone()).write_state(&mut Vec::new());
        calm_parts && !a.distinct && a.distribution.is_none() && writes
    })
}

/// Whether a worker evaluates `e` alone: it reads no subquery, invokes no
/// routine, takes no sequence's value and holds no parameter's.
fn calm(e: &Expr) -> bool {
    let own = match e {
        Expr::Subquery(..) | Expr::Invoked(_) | Expr::NextValue(_) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Domain(..) => false,
        Expr::Case(whens, _) => whens.iter().all(|(c, _)| calm_cond(c)),
        Expr::Truth(c) => calm_cond(c),
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Cast(..)
        | Expr::SortKey(..)
        | Expr::Warned(..) => true,
    };
    let mut children = true;
    e.for_each_child(&mut |child| children = children && calm(child));
    own && children
}

/// Whether a worker evaluates `c` alone, as `calm` says of expressions.
fn calm_cond(c: &Cond) -> bool {
    let mut all = c.subqueries().is_empty();
    c.for_each_expr(&mut |e| all = all && calm(e));
    all
}
