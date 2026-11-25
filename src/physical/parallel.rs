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

use super::{Plan, SemiMethod};
use crate::db::State;
use crate::index::Probe;
use crate::logical::{AggCall, Cond, Expr, OuterJoin, SetOp};
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
        rows >= self.least && exchange::outweighs(self.db, exchanged, rows, (self.morsel, degree))
    }
}

/// \[DESIGN\] Whether `plan`, a grouping or DISTINCT, would be a part
/// run in parallel as it is: so a streamed one is not chosen in its place
/// (RULES.md D550).
pub(super) fn runs_parallel(db: &State, plan: &Plan) -> bool {
    let asked = Asked::of(db);
    let Some(rows) = rooted(plan).filter(|_| asked.most > 1) else { return false };
    !asked.kept_whole(plan, rows) && asked.degree(plan, rows) > 1
}

/// `plan` with each part worth running in parallel marked so.
pub(crate) fn parallel(db: &State, mut plan: Plan) -> Plan {
    let asked = Asked::of(db);
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
/// read that a run stopping at its first rows would not read.
pub(crate) fn parallel_run(db: &State, plan: Plan, all: bool) -> Plan {
    if all || whole_first(&plan, 0) { super::transferred(db, parallel(db, plan)) } else { plan }
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
    let rooted = rooted(plan);
    if rooted.is_some_and(|rows| asked.kept_whole(plan, rows)) {
        return;
    }
    let degree = rooted.map_or(1, |rows| asked.degree(plan, rows));
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
        if let Plan::SemiJoin(s) = plan
            && s.method == SemiMethod::BuildLeft
        {
            walk(asked, &mut s.left, deeper);
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
    let degree = stream(plan).filter(|_| depth < MAX_DEPTH).map_or(1, |rows| asked.by_rows(rows));
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

/// \[DESIGN\] (RULES.md D1113) The first grouping of `plan`, if `plan`
/// is a grouping over it, an analyzed plan's count between them or none,
/// that a part runs whole: the second by some of the first's grouping
/// columns and no others, neither by a collation nor with groups of empty
/// grouping sets, the second's set functions none DISTINCT nor an inverse
/// distribution function and calm, the first's states merging. Its
/// partitions are then of those columns alone, so each holds every group
/// of the first that one of the second takes, and its thread makes the
/// second's groups too, as DISTINCT_AGGREGATES makes such pairs (RULES.md
/// D340).
pub(crate) fn first_level(plan: &Plan) -> Option<&Plan> {
    let Plan::Aggregate { groups, collations, aggs, seeds, input, streamed: false, .. } = plan else { return None };
    let first = if let Plan::Counted(c) = &**input { &c.input } else { input };
    let Plan::Aggregate { groups: keys, collations: by, aggs: parts, seeds: seeded, streamed: false, .. } = first else { return None };
    let within = !groups.is_empty() && groups.iter().all(|&g| g < keys.len());
    let uncollated = collations.is_empty() && by.is_empty() && seeds.is_empty() && seeded.is_empty();
    let calm_parts = |a: &AggCall| a.arg.as_ref().is_none_or(calm) && a.filter.as_ref().is_none_or(calm_cond);
    let whole = aggs.iter().all(|a| !a.distinct && a.distribution.is_none() && calm_parts(a));
    (within && uncollated && whole && mergeable(parts)).then_some(first)
}

/// The rows a part rooted at `plan` reads, if it is one.
fn rooted(plan: &Plan) -> Option<usize> {
    if let Some(first) = first_level(plan) {
        return rooted(first);
    }
    // A streamed grouping or DISTINCT reads its input in order, a group
    // at a time, which no morsel's table of groups does (RULES.md D550).
    if let Plan::Aggregate { aggs, input, streamed: false, .. } = plan {
        return mergeable(aggs).then(|| stream(input)).flatten();
    }
    if let Plan::Distinct { input, streamed: false, .. } = plan {
        return stream(input);
    }
    // A HAVING condition over a grouping that is a part is judged of each
    // group by the part's threads (RULES.md D1073).
    if let Plan::Filter { cond, input, .. } = plan
        && matches!(**input, Plan::Aggregate { streamed: false, .. })
    {
        return (calm_cond(cond) && !cond.can_raise()).then(|| rooted(input)).flatten();
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
    // A LEFT join's probe on its keys alone is a step of its left's
    // pipeline (RULES.md D1076).
    if let Plan::OuterJoin { kind: OuterJoin::Left, left, keys, cond: None, own: None, .. } = plan {
        return (!keys.is_empty()).then(|| stream(left)).flatten();
    }
    // A hashed semi join's or antijoin's probe is a step of its left's
    // pipeline (RULES.md D1075).
    if let Plan::SemiJoin(s) = plan {
        return (s.method == SemiMethod::Hash && s.cond.as_ref().is_none_or(calm_cond)).then(|| stream(&s.left)).flatten();
    }
    let Plan::Append { left, right, .. } = plan else { return None };
    Some(stream(left)?.saturating_add(stream(right)?))
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
fn calm_cond(c: &Cond) -> bool {
    let mut all = c.subqueries().is_empty();
    c.for_each_expr(&mut |e| all = all && calm(e));
    all
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::value::{Decimal, Value};

    /// Verifies: [DESIGN] RULES.md D1128: a plan reads its input whole
    /// before its first row if it is a grouping or DISTINCT not streamed,
    /// or a sort, under any projections and filters; a streamed grouping
    /// or DISTINCT, or a projection of rows as they come, does not, so a
    /// run that stops at its first rows keeps it as it is.
    #[test]
    fn a_plan_reads_whole_first_under_a_grouping_a_distinct_or_a_sort() {
        let values = || Box::new(Plan::Values { rows: Vec::new(), detail: String::new() });
        let distinct = |streamed: bool| Plan::Distinct { input: values(), est: 0.0_f64, streamed };
        let project = |input: Plan| Plan::Project { exprs: Vec::new(), detail: String::new(), input: Box::new(input) };
        let filter = |input: Plan| {
            let cond = Cond::Const(crate::logical::Truth::True);
            Plan::Filter { cond, detail: String::new(), est: 0.0_f64, of_read: false, input: Box::new(input) }
        };
        let aggregate = |streamed: bool| {
            let (groups, collations, aggs, seeds) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            Plan::Aggregate { groups, collations, aggs, seeds, detail: String::new(), input: values(), est: 0.0_f64, streamed }
        };
        assert!(whole_first(&distinct(false), 0));
        assert!(!whole_first(&distinct(true), 0), "streamed, it gives rows as it reads them");
        assert!(whole_first(&aggregate(false), 0));
        assert!(!whole_first(&aggregate(true), 0), "streamed, it gives groups as it reads them");
        assert!(whole_first(&Plan::Sort { keys: Vec::new(), detail: String::new(), input: values(), most: None }, 0));
        assert!(whole_first(&project(filter(distinct(false))), 0), "under a projection and a filter");
        assert!(!whole_first(&project(*values()), 0), "a projection of rows as they come");
        assert!(!whole_first(&project(distinct(false)), MAX_DEPTH), "past the depth walked, none is found");
        // A run that stops at its first rows keeps such a plan as it is.
        let db = crate::db::Database::new();
        let state = db.snapshot().unwrap();
        assert!(matches!(parallel_run(&state, project(*values()), false), Plan::Project { .. }));
    }

    /// Verifies: [DESIGN] RULES.md D1166: an estimate past what a `u32`
    /// holds is its own ceiling as a count, the most a count holds only
    /// of one reaching it, never of any past 2^32.
    #[test]
    fn an_estimate_past_a_u32_is_counted_as_itself() {
        assert_eq!(rows_of(4_294_967_295.5_f64), 4_294_967_296);
        assert_eq!(rows_of(5.0e9_f64), 5_000_000_000);
        assert_eq!(rows_of(1.0e18_f64), 1_000_000_000_000_000_000);
        assert_eq!(rows_of(1.0e19_f64), usize::MAX >> 1_u32);
        assert_eq!(rows_of(f64::INFINITY), usize::MAX >> 1_u32);
        assert_eq!(rows_of(f64::NAN), 1);
    }

    /// An aggregation by `groups` of `input`, computing `aggs`, of
    /// `seeds`, streamed or not.
    fn grouping(groups: Vec<usize>, (aggs, seeds): (Vec<AggCall>, Vec<Vec<Value>>), input: Plan, streamed: bool) -> Plan {
        let (collations, detail, input, est) = (Vec::new(), String::new(), Box::new(input), 0.0_f64);
        Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est, streamed }
    }

    /// COUNT of column 0, DISTINCT or not.
    fn count(distinct: bool) -> AggCall {
        let func = crate::functions::Registry::with_set_functions().aggregate("COUNT").unwrap();
        let result = func.0.result_type(None).unwrap();
        AggCall { func, arg: Some(Expr::Column(0)), distinct, result, filter: None, distribution: None, label: "COUNT".to_owned() }
    }

    /// Verifies: [DESIGN] RULES.md D1113: a grouping over another is a
    /// pair a part holds whole if it groups by some of the first's
    /// grouping columns and no others, an analyzed plan's count between
    /// them or none; not of no grouping column, of a column past the
    /// first's grouping columns, of a collation, of groups of empty
    /// grouping sets of either, of a first streamed, of a DISTINCT set
    /// function in the second, or of the first's states that do not merge.
    #[test]
    fn a_grouping_over_one_holding_its_columns_is_a_pair() {
        let values = || Plan::Values { rows: Vec::new(), detail: String::new() };
        let none = || (Vec::new(), Vec::new());
        let first = |seeds: Vec<Vec<Value>>, streamed: bool| grouping(vec![0, 1], (Vec::new(), seeds), values(), streamed);
        let pair = |groups: Vec<usize>, aggs: Vec<AggCall>, input: Plan| grouping(groups, (aggs, Vec::new()), input, false);
        assert!(first_level(&pair(vec![0], vec![count(false)], first(Vec::new(), false))).is_some());
        assert!(first_level(&pair(vec![1, 0], Vec::new(), first(Vec::new(), false))).is_some(), "of every column, in any order");
        let count_node = Arc::new(crate::physical::NodeCount::default());
        let counted = Plan::Counted(Box::new(crate::physical::Counted { count: count_node, input: first(Vec::new(), false) }));
        assert!(first_level(&pair(vec![0], Vec::new(), counted)).is_some(), "an analyzed plan's count between");
        assert!(first_level(&pair(Vec::new(), Vec::new(), first(Vec::new(), false))).is_none(), "of no grouping column");
        assert!(first_level(&pair(vec![2], Vec::new(), first(Vec::new(), false))).is_none(), "of a set function's column");
        assert!(first_level(&pair(vec![0], Vec::new(), first(vec![vec![Value::Null, Value::Null]], false))).is_none(), "of seeds");
        assert!(first_level(&grouping(vec![0], (Vec::new(), vec![vec![Value::Null]]), first(Vec::new(), false), false)).is_none());
        assert!(first_level(&pair(vec![0], Vec::new(), first(Vec::new(), true))).is_none(), "of a first streamed");
        assert!(first_level(&grouping(vec![0], none(), first(Vec::new(), false), true)).is_none(), "of a second streamed");
        assert!(first_level(&pair(vec![0], vec![count(true)], first(Vec::new(), false))).is_none(), "of DISTINCT in the second");
        let unmerged = grouping(vec![0, 1], (vec![count(true)], Vec::new()), values(), false);
        assert!(first_level(&pair(vec![0], Vec::new(), unmerged)).is_none(), "of the first's states that do not merge");
        let mut collated = pair(vec![0], Vec::new(), first(Vec::new(), false));
        if let Plan::Aggregate { collations, .. } = &mut collated {
            collations.push(None);
        }
        assert!(first_level(&collated).is_none(), "of a collation");
        assert!(first_level(&first(Vec::new(), false)).is_none(), "of a grouping over no grouping");
    }

    /// Verifies: [DESIGN] RULES.md D405, D682: once a database starts, the
    /// system's threads are kept, and EXECUTOR.WORKERS of 0 is planned by
    /// them, however many times and from whichever thread it is asked.
    #[test]
    fn workers_of_none_set_are_the_threads_kept_as_a_database_starts() {
        let db = crate::db::Database::new();
        let kept = SYSTEM_THREADS.get().copied();
        assert!(kept.is_some_and(|n| n >= 1), "kept as it started");
        db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(0, 0))).unwrap();
        let state = db.snapshot().unwrap();
        let asked = std::thread::scope(|s| s.spawn(|| workers_of(&state)).join().unwrap());
        assert_eq!(Some(asked), kept);
        assert_eq!(Some(workers_of(&state)), kept);
        db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(3, 0))).unwrap();
        assert_eq!(workers_of(&db.snapshot().unwrap()), 3, "a setting is what it says");
    }
}
