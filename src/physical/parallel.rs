//! \[DESIGN\] Where a query runs in parallel (RULES.md D291): the parts of
//! a plan the executor runs by morsels on several threads at once
//! (`exec/parallel.rs`), each marked `Parallel` with its degree, as EXPLAIN
//! shows it. A part is a pipeline, the reads of tables and the filters,
//! projections, hash joins' probes, LEFT joins' probes on their keys
//! alone (RULES.md D1076) and hashed semi joins' and antijoins' probes
//! (RULES.md D1075) over them, read as one stream; or
//! one of the operators that end a pipeline, over one: a grouping whose
//! set functions keep states that merge, DISTINCT, a sort, window
//! functions of a partitioning, and UNION, EXCEPT and INTERSECT; or a
//! filter that cannot raise over such a grouping, its HAVING condition,
//! judged of each group by the part's threads (RULES.md D1073); or a
//! grouping over such a grouping by some of its grouping columns, both
//! made by the part's threads (RULES.md D1113). A
//! grouping or DISTINCT streamed over rows in its keys' order is no such
//! part, as its order would be lost among the threads (RULES.md D550).
//!
//! The degree is of the rows the part's tables held as it was planned: as
//! many threads as morsels of EXECUTOR.MORSEL_ROWS they hold, up to
//! EXECUTOR.DEGREE, or the workers, EXECUTOR.WORKERS. A part reading fewer
//! rows than EXECUTOR.PARALLEL_MIN_ROWS stays on the statement's thread,
//! as a point query does, its plan unchanged, so it costs nothing more
//! than the walk that finds it. So does one a serial scan by batch of a
//! table kept by column serves better, with no hash join's probe nor
//! window functions, while EXECUTOR.BATCHES is on, and a bare COUNT(*) its
//! table's storage counts (RULES.md D351); the operand a part's hash join
//! builds is a part by its rows alone, of either store, as its morsels
//! are gathered by batch (RULES.md D1051). So does a grouping or DISTINCT
//! of so many groups, as the statistics estimate them, that grouping each
//! morsel apart barely shrinks it, its exchange of pieces costing what its
//! threads gain, and with it the stream it reads, whose rows a channel
//! would carry to it one by one (`parallel/exchange.rs`, RULES.md D355).
//! An expression a worker cannot evaluate alone keeps its pipeline whole on
//! the statement's thread: a subquery's, a routine's, NEXT VALUE FOR's, a
//! parameter's.

mod exchange;
mod inputs;

use std::sync::OnceLock;

use super::{Plan, Seen, SemiMethod};
use crate::db::State;
use crate::index::Probe;
use crate::logical::{AggCall, Cond, Expr, OuterJoin, SetOp};
use crate::settings::{EXECUTOR_DEGREE, EXECUTOR_MORSEL_ROWS, EXECUTOR_PARALLEL_MIN_ROWS, EXECUTOR_WORKERS};
use crate::value::Value;

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
    /// The statistics of its tables as the statement sees them (RULES.md
    /// D1089).
    seen: &'a Seen,
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
        0 => system_threads(),
        n => n,
    }
}

/// \[DESIGN\] (RULES.md D405, D682) The threads the system runs at once,
/// asked of it once, as the first database starts (`Database::over`), by
/// the thread starting it, and kept: asked as each statement is planned,
/// it read files of the system's each time, and a thread held to fewer
/// processors than the process has, as a statement's may be, saw only
/// those.
pub(crate) fn system_threads() -> usize {
    *SYSTEM_THREADS.get_or_init(|| std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get))
}

/// The threads the system runs at once, as `system_threads` asked them.
static SYSTEM_THREADS: OnceLock<usize> = OnceLock::new();

impl<'a> Asked<'a> {
    /// What `db`'s settings ask, of a statement that sees its tables'
    /// statistics as `seen`.
    fn of(db: &'a State, seen: &'a Seen) -> Asked<'a> {
        let workers = workers_of(db);
        let most = match db.settings.count(EXECUTOR_DEGREE) {
            0 => workers,
            n => n.min(workers.max(1)),
        };
        let (morsel, least) = (db.settings.count(EXECUTOR_MORSEL_ROWS).max(1), db.settings.count(EXECUTOR_PARALLEL_MIN_ROWS));
        Asked { db, seen, most, morsel, least }
    }

    /// The degree of the part `plan` reading `rows` rows: 1 to run it
    /// alone, as one a serial scan by batch serves better.
    fn degree(self, plan: &Plan, rows: usize) -> usize {
        if crate::exec::batched(self.db) && by_batch(self.db, plan) {
            return 1;
        }
        self.by_rows(rows)
    }

    /// The degree of a part reading `rows` rows, by the rule the costs
    /// anticipate (`cost/parallel.rs`, RULES.md D652).
    fn by_rows(self, rows: usize) -> usize {
        super::Degrees { most: self.most, morsel: self.morsel, least: self.least }.degree(rows)
    }

    /// \[DESIGN\] (RULES.md D355) Whether the part `plan` reading `rows`
    /// rows, a grouping or DISTINCT, stays serial whole, its input too, as
    /// its exchange of groups costs what its threads would gain.
    fn kept_whole(self, plan: &Plan, rows: usize) -> bool {
        let degree = self.most.min(rows.div_ceil(self.morsel));
        // Of a grouping over another whose keys hold its own, the pieces
        // exchanged are the first's (RULES.md D1113).
        let grouping = if let Plan::Filter { input, .. } = plan { input } else { plan };
        let exchanged = first_level(grouping).unwrap_or(plan);
        rows >= self.least && exchange::outweighs(self.db, self.seen, exchanged, rows, (self.morsel, degree))
    }
}

/// \[DESIGN\] Whether `plan`, a grouping or DISTINCT, would be a part
/// run in parallel as it is: so a streamed one is not chosen in its place
/// (RULES.md D550).
pub(super) fn runs_parallel(db: &State, seen: &Seen, plan: &Plan) -> bool {
    let asked = Asked::of(db, seen);
    let Some(rows) = rooted(plan, Some(db)).filter(|_| asked.most > 1) else { return false };
    !asked.kept_whole(plan, rows) && asked.degree(plan, rows) > 1
}

/// `plan` with each part worth running in parallel marked so.
pub(crate) fn parallel(db: &State, seen: &Seen, mut plan: Plan) -> Plan {
    let asked = Asked::of(db, seen);
    if asked.most > 1 {
        walk(asked, &mut plan, 0);
    }
    plan
}

/// \[DESIGN\] (RULES.md D1128) `plan`, that of a subquery run once for
/// its statement, with its parts marked as `parallel` marks a statement's
/// and its chains given predicate transfer (`physical/transfer.rs`), if
/// the run reads every row of it: `all` of them are asked for, or it
/// reads its input whole before it gives its first row, so no row is
/// read that a run stopping at its first rows would not read. Its
/// statistics are seen as its planning saw them, `seen` (RULES.md D1089).
pub(crate) fn parallel_run((db, seen): (&State, &Seen), plan: Plan, all: bool) -> Plan {
    if all || whole_first(&plan, 0) { super::transferred(db, seen, parallel(db, seen, plan)) } else { plan }
}

/// Whether `plan` reads its input whole before it gives its first row: a
/// grouping or DISTINCT not streamed, or a sort, under projections and
/// filters.
fn whole_first(plan: &Plan, depth: usize) -> bool {
    if let Plan::Project { input, .. } | Plan::Filter { input, .. } = plan {
        return depth < MAX_DEPTH && whole_first(input, depth.saturating_add(1));
    }
    matches!(plan, Plan::Aggregate { streamed: false, .. } | Plan::Distinct { streamed: false, .. } | Plan::Sort { .. })
}

/// Marks the parts of `plan`, `depth` below the root.
fn walk(asked: Asked<'_>, plan: &mut Plan, depth: usize) {
    // A bare COUNT(*) its table's storage counts, faster than any split,
    // stays whole (RULES.md D351).
    if depth >= MAX_DEPTH || crate::exec::storage_counted(plan, asked.db).is_some() {
        return;
    }
    let rooted = rooted(plan, Some(asked.db));
    if rooted.is_some_and(|rows| asked.kept_whole(plan, rows)) {
        return;
    }
    let degree = rooted.map_or(1, |rows| asked.degree(plan, rows));
    let deeper = depth.saturating_add(1);
    if degree < 2 {
        // A limit of few rows over a stream reads too few for workers to
        // share, unless a filter leaves out rows it would read.
        if let Plan::Limit { input, .. } = plan
            && stream_in(input, Some(asked.db)).is_some()
            && !filtered(input)
        {
            return outside(asked, input, deeper);
        }
        if let Plan::SemiJoin(s) = plan
            && s.method == SemiMethod::BuildLeft
        {
            // Its left is held as a hash join's built operand is
            // (`exec/batch/joined/semi/marked.rs`), by batch whatever its
            // store (RULES.md D1132).
            built(asked, &mut s.left, deeper);
            return built(asked, &mut s.right, deeper);
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
        return built(asked, right, deeper);
    }
    if let Plan::SemiJoin(s) = plan {
        outside(asked, &mut s.left, deeper);
        return built(asked, &mut s.right, deeper);
    }
    if let Plan::OuterJoin { left, right, .. } = plan {
        outside(asked, left, deeper);
        return built(asked, right, deeper);
    }
    plan.each_input(&mut |input| outside(asked, input, deeper));
}

/// \[DESIGN\] (RULES.md D1051, D1070, D1075, D1076) Marks the parts of
/// `plan`, the operand a hash join, a semi join or a LEFT join of a part
/// builds, or the right operand of a semi join that builds its left, whose
/// morsels mark the left's held rows (`exec/batch/joined/semi/marked.rs`).
/// A pipeline is a part by its rows alone,
/// of a table kept by column too: its morsels are read by batch and
/// gathered into the join's table as vectors (`exec/parallel/probe.rs`),
/// no row made of them, so the reason a scan by batch of such a table
/// stays serial (RULES.md D291) does not hold of it.
fn built(asked: Asked<'_>, plan: &mut Plan, depth: usize) {
    let degree = stream_in(plan, Some(asked.db)).filter(|_| depth < MAX_DEPTH).map_or(1, |rows| asked.by_rows(rows));
    if degree < 2 {
        return walk(asked, plan, depth);
    }
    outside(asked, plan, depth.saturating_add(1));
    let input = std::mem::replace(plan, Plan::Values { rows: Vec::new(), detail: String::new() });
    *plan = Plan::Parallel(Box::new(Parallel { degree, input }));
}

/// Whether the stream `plan` has a filter.
fn filtered(plan: &Plan) -> bool {
    if let Plan::Append { left, right, .. } = plan {
        return filtered(left) || filtered(right);
    }
    if let Plan::Project { input, .. } | Plan::HashJoin { left: input, .. } = plan {
        return filtered(input);
    }
    matches!(plan, Plan::Filter { .. } | Plan::SemiJoin(_))
}

/// \[DESIGN\] (RULES.md D1113, D1133) The first grouping of `plan`, if
/// `plan` is a grouping over it, an analyzed plan's count between them or
/// none, that a part runs whole: neither by a collation nor with groups
/// of empty grouping sets, the second's set functions none DISTINCT nor
/// an inverse distribution function and calm, the first's states merging.
/// The second groups by some of the first's grouping columns and no
/// others, so the partitions are of those columns alone, each holds every
/// group of the first that one of the second takes, and its thread makes
/// the second's groups too, as DISTINCT_AGGREGATES makes such pairs
/// (RULES.md D340). Or it crosses them (`crosses`).
pub(crate) fn first_level(plan: &Plan) -> Option<&Plan> {
    let Plan::Aggregate { groups, collations, aggs, seeds, input, streamed: false, .. } = plan else { return None };
    let (steps, below) = between(input, 0);
    let first = if let Plan::Counted(c) = below { &c.input } else { below };
    let Plan::Aggregate { groups: keys, collations: by, aggs: parts, seeds: seeded, streamed: false, .. } = first else { return None };
    let within = steps.is_empty() && !groups.is_empty() && groups.iter().all(|&g| g < keys.len());
    let uncollated = collations.is_empty() && by.is_empty() && seeds.is_empty() && seeded.is_empty();
    let calm_parts = |a: &AggCall| a.arg.as_ref().is_none_or(calm) && a.filter.as_ref().is_none_or(calm_cond);
    let whole = aggs.iter().all(|a| !a.distinct && a.distribution.is_none() && calm_parts(a));
    let calm_steps = steps.iter().all(|exprs| exprs.iter().all(calm));
    let joined = within || (crosses(plan) && calm_steps);
    (joined && uncollated && whole && mergeable(parts)).then_some(first)
}

/// \[DESIGN\] (RULES.md D1133) Whether `plan`, a grouping over another,
/// crosses the first's partitions: it groups by some columns, its set
/// functions are COUNT alone, and a projection lies between them or it
/// groups by a column other than the first's grouping columns. Its
/// partitions are then the first's, by its whole key, each partition's
/// thread groups its own groups of the first again, and the statement's
/// thread merges each group's counts of the partitions, of no more groups
/// than the second's of each. A count is the same whatever order its
/// rows are counted in, so the merge gives what the serial grouping gives.
pub(crate) fn crosses(plan: &Plan) -> bool {
    let Plan::Aggregate { groups, aggs, input, .. } = plan else { return false };
    let (steps, below) = between(input, 0);
    let first = if let Plan::Counted(c) = below { &c.input } else { below };
    let Plan::Aggregate { groups: keys, .. } = first else { return false };
    let counts = aggs.iter().all(|a| a.func.0.columnar().is_some_and(|c| c.0 == crate::functions::ColumnarKind::Count));
    let within = steps.is_empty() && groups.iter().all(|g| *g < keys.len());
    !groups.is_empty() && counts && !within
}

/// The projections of `plan`'s rows down from it, the nearest first, and
/// the operator under them, `depth` down; past `MAX_DEPTH` none more.
pub(crate) fn between(plan: &Plan, depth: usize) -> (Vec<&[Expr]>, &Plan) {
    let Plan::Project { exprs, input, .. } = plan else { return (Vec::new(), plan) };
    if depth >= MAX_DEPTH {
        return (Vec::new(), plan);
    }
    let (mut steps, below) = between(input, depth.saturating_add(1));
    steps.insert(0, exprs);
    (steps, below)
}

/// The rows a part rooted at `plan` reads, if it is one, its filters'
/// subqueries judged of `db`'s tables if given (`stream_in`).
fn rooted(plan: &Plan, db: Option<&State>) -> Option<usize> {
    if let Some(first) = first_level(plan) {
        return rooted(first, db);
    }
    // A streamed grouping or DISTINCT reads its input in order, a group
    // at a time, which no morsel's table of groups does (RULES.md D550).
    if let Plan::Aggregate { aggs, input, streamed: false, .. } = plan {
        return mergeable(aggs).then(|| stream_in(input, db)).flatten();
    }
    if let Plan::Distinct { input, streamed: false, .. } = plan {
        return stream_in(input, db);
    }
    // A HAVING condition over a grouping that is a part is judged of each
    // group by the part's threads (RULES.md D1073).
    if let Plan::Filter { cond, input, .. } = plan
        && matches!(**input, Plan::Aggregate { streamed: false, .. })
    {
        return (calm_cond(cond) && !cond.can_raise()).then(|| rooted(input, db)).flatten();
    }
    if let Plan::Sort { input, most, .. } = plan {
        return most.is_none_or(|m| !m.ties).then(|| stream_in(input, db)).flatten();
    }
    if let Plan::Window { window, .. } = plan {
        let Plan::Sort { input, most: None, .. } = &window.input else { return None };
        let calm_calls = window.exprs().all(calm) && window.conds().all(calm_cond);
        return (calm_calls && !window.partition.is_empty()).then(|| stream_in(input, db)).flatten();
    }
    if let Plan::SetOp { op: SetOp::Except | SetOp::Intersect, left, right, .. } = plan {
        return Some(stream_in(left, db)?.saturating_add(stream_in(right, db)?));
    }
    stream_in(plan, db)
}

/// The rows the stream `plan` reads, if it is one: a pipeline, its rows
/// in their order, morsel by morsel.
pub(crate) fn stream(plan: &Plan) -> Option<usize> {
    stream_in(plan, None)
}

/// As `stream`, a filter whose scalar subqueries are bound before the
/// part's morsels start taken as a step of it, if `db` is given to judge
/// them of (`bound_calm`).
fn stream_in(plan: &Plan, db: Option<&State>) -> Option<usize> {
    if let Plan::SeqScan { order, located, rows, .. } = plan {
        return (order.is_empty() && !located).then_some(*rows);
    }
    if let Plan::IndexScan { reverse: false, located: false, decided: None, probe, est, .. } = plan {
        return (!matches!(probe, Probe::Xml(_) | Probe::Predicate { .. })).then(|| rows_of(*est));
    }
    if let Plan::Filter { cond, input, .. } = plan {
        let calm = calm_cond(cond) || db.is_some_and(|db| bound_calm(cond, db));
        return calm.then(|| stream_in(input, db)).flatten();
    }
    if let Plan::Project { exprs, input, .. } = plan {
        return exprs.iter().all(calm).then(|| stream_in(input, db)).flatten();
    }
    if let Plan::HashJoin { left, .. } = plan {
        return stream_in(left, db);
    }
    // A LEFT join's probe on its keys alone is a step of its left's
    // pipeline (RULES.md D1076).
    if let Plan::OuterJoin { kind: OuterJoin::Left, left, keys, cond: None, own: None, .. } = plan {
        return (!keys.is_empty()).then(|| stream_in(left, db)).flatten();
    }
    // A hashed semi join's or antijoin's probe is a step of its left's
    // pipeline (RULES.md D1075).
    if let Plan::SemiJoin(s) = plan {
        return (s.method == SemiMethod::Hash && s.cond.as_ref().is_none_or(calm_cond)).then(|| stream_in(&s.left, db)).flatten();
    }
    let Plan::Append { left, right, .. } = plan else { return None };
    Some(stream_in(left, db)?.saturating_add(stream_in(right, db)?))
}

/// \[DESIGN\] Whether the part `plan` reads a table kept by column with
/// nothing a serial scan by batch leaves to rows, a hash join's probe or
/// window functions: that scan, making no row, outruns workers that make
/// each (RULES.md D291). A grouping or DISTINCT is not one: each morsel of
/// frozen groups is read by batch and grouped in a table of its own as the
/// serial grouping reads and groups them (RULES.md D358).
fn by_batch(db: &State, plan: &Plan) -> bool {
    if let Plan::Aggregate { .. } | Plan::Distinct { .. } = plan {
        return false;
    }
    if let Plan::Sort { input, .. } = plan {
        return by_batch(db, input);
    }
    if let Plan::Filter { input, .. } | Plan::Project { input, .. } = plan {
        return by_batch(db, input);
    }
    if let Plan::SetOp { left, right, .. } | Plan::Append { left, right, .. } = plan {
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
pub(crate) fn calm_cond(c: &Cond) -> bool {
    let mut all = c.subqueries().is_empty();
    c.for_each_expr(&mut |e| all = all && calm(e));
    all
}

/// \[DESIGN\] (RULES.md D1132) Whether a worker evaluates `c` alone once
/// its scalar subqueries that read nothing of its rows, and that every
/// row it judges runs, are made constants on the statement's thread
/// before the part's morsels start (`exec/parallel/pipeline.rs`): each
/// such subquery raises no error and no warning, as `quiet` judges it of
/// `db`'s tables, so running it there is seen by nothing but its value,
/// and with them made constants `c` is calm.
fn bound_calm(c: &Cond, db: &State) -> bool {
    let mut quiet = true;
    let mut placed = |e: Expr| {
        let silent = if let Expr::Subquery(s, _) = &e { crate::optimizer::unnesting::quiet(s, db) } else { false };
        quiet = quiet && silent;
        Ok(if silent { Expr::Const(Value::Null) } else { e })
    };
    crate::exec::bound_by(c, &mut placed).is_ok_and(|bound| quiet && calm_cond(&bound))
}

// Unit tests of the parts a plan runs in parallel (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
