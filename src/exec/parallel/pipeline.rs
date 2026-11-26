//! \[DESIGN\] A pipeline run by morsels (RULES.md D290, D292): each table
//! it reads split into morsels by its engine (`TableStorage::split`), and
//! each morsel's rows taken through the steps over them, a filter's, a
//! projection's, a hash join's probe, a LEFT join's (RULES.md D1076) and
//! a hashed semi join's or antijoin's (RULES.md D1075), one row at a time
//! through them all, each as it is asked for (`parallel/rows.rs`, RULES.md
//! D1150), as Leis et al., "Morsel-Driven Parallelism" (2014), run a
//! pipeline fragment on a morsel. The steps are those of the plan,
//! applied as its operators apply them, so a morsel gives the rows, in the
//! order, and raises the error, the plan's operators would of those of its
//! table.
//!
//! An index scan's morsels are parts of its range of keys
//! (`parallel/indexed.rs`).
//!
//! A semi join or an antijoin that builds its left starts a pipeline, its
//! morsels ranges of its held rows, marked as the pipeline is made
//! (`batch/joined/semi/split.rs`, RULES.md D1156).
//!
//! The morsels of the operands of a UNION ALL are numbered on, the left's
//! first, so their rows come in the order the append gives them. A filter
//! over a table tells its scan its simple conditions, as `exec/scan.rs`
//! does, and judges every row itself still.
//!
//! A hash join's operand is built before its probing side is split, so
//! its runtime filters (RULES.md D369, D389) go down the probing side as
//! the serial stream takes them (`batch/runtime.rs`): a scan is told a
//! filter's range among the conjuncts its split judges, so a morsel skips
//! what lies outside it, and a step right after the operator a filter
//! stops at judges the filters of a morsel's batches. A morsel's scan
//! judges no Bloom filter, which costs a morsel more than the probe it
//! spares. A morsel read a row at a time passes the step by, as a filter
//! keeps only rows the join would drop.

use std::sync::Arc;

use super::indexed::{self, Scanned};
use super::probe::{Table, built};
use super::{Held, Limit};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Told, held_split, runtime_taken as taken, wanted};
use crate::exec::{Runner, broken, eval};
use crate::logical::{Cond, Expr, pushed_conjuncts};
use crate::physical::{NodeCount, Plan, SemiMethod};
use crate::settings::EXECUTOR_MORSEL_ROWS;
use crate::storage::{Conjunct, Conjunction, Row, Split};
use crate::txn::TxId;

/// \[DESIGN\] The most steps a pipeline takes a row through (RULES.md
/// R-CF-6), each a level of a plan the planner walked no deeper than this.
const MAX_STEPS: usize = 1024;

/// A step a row is taken through.
pub(super) enum Step {
    /// [X3.135-1986 §5.21 GR2] kept only if the condition is true of it.
    Filter(Cond),
    /// [X3.135-1986 §5.25 GR2] each expression of it.
    Project(Vec<Expr>),
    /// [X3.135-1986 §5.20 GR2b] joined with each row of the built operand
    /// whose keys equal its own.
    Probe(Arc<Table>),
    /// [ISO/IEC 9075:1992 §7.5 GR4, GR5b] joined with each row of the
    /// built operand whose keys equal its own, or with nulls if none does
    /// (RULES.md D1076).
    Outer(Arc<Table>),
    /// [X3.135-1986 §5.17 GR2, GR3] kept if a row of the built operand
    /// matches it, or of an antijoin if none does (RULES.md D1075).
    Semi(Arc<Table>, Arc<SemiOf>),
    /// \[DESIGN\] (RULES.md D389) kept if hash joins' runtime filters
    /// stopped at an operator may match it; judged by batch alone.
    Runtime(Vec<Told>),
    /// \[EXTENSION\] counted, as the rows of a counted operator, each
    /// morsel's added once (RULES.md D378).
    Count(Arc<NodeCount>),
}

/// What a semi join that is a step asks beyond its keys: whether it is an
/// antijoin, the rest of its condition, and its operands' widths.
pub(super) struct SemiOf {
    pub(super) anti: bool,
    pub(super) cond: Option<Cond>,
    pub(super) widths: (usize, usize),
}

/// The morsels of one table's split and the steps over its rows, the
/// first applied first; of a table's scan, the columns it reads and its
/// width, which its batches hold (`parallel/batched.rs`).
pub(super) struct Branch {
    pub(super) split: Arc<dyn Split>,
    pub(super) steps: Vec<Step>,
    pub(super) scanned: Option<(Vec<usize>, usize)>,
}

/// A pipeline, its tables split; and, if each of its morsels is read by
/// batch, how (`parallel/batched.rs`).
pub(super) struct Pipeline {
    pub(super) branches: Vec<Branch>,
    pub(super) batched: Option<super::batched::ByBatch>,
}

impl Pipeline {
    /// The stream `plan` split for `tx`'s statement, its hash joins' built
    /// operands built within `limit`; given way if a table cannot be split
    /// or an operand outgrows it.
    ///
    /// # Errors
    ///
    /// As building an operand fails.
    pub(super) fn of(plan: &Plan, (db, tx): (&Db, TxId), limit: &Limit) -> Result<Held<Pipeline>> {
        Pipeline::told(plan, (db, tx), limit, &[])
    }

    /// \[DESIGN\] (RULES.md D1063) As `of`, `told` the runtime filters of
    /// the stream's columns a join above the part tells it
    /// (`Told::given_above`), as a grouping run in parallel is told those
    /// of its grouping columns. Their ranges go down to the scans as the
    /// pipeline's own joins' do, and a step right after each scan they
    /// reach judges them whole of each morsel's batches (`above`): no
    /// probe of their join follows in the morsel to drop the rows, so
    /// judging them there spares what reads the pipeline every row they
    /// drop.
    ///
    /// # Errors
    ///
    /// As building an operand fails.
    pub(super) fn told(plan: &Plan, (db, tx): (&Db, TxId), limit: &Limit, told: &[Told]) -> Result<Held<Pipeline>> {
        let rows = db.settings.count(EXECUTOR_MORSEL_ROWS).max(1);
        let mut branches = Vec::new();
        if !branched(plan, (db, tx, rows), (limit, told), &mut branches)? {
            return Ok(Held::GaveWay);
        }
        let batched = super::batched::by_batch(&branches, db);
        Ok(Held::Kept(Pipeline { branches, batched }))
    }

    /// How many morsels it has.
    pub(super) fn count(&self) -> usize {
        self.branches.iter().map(|b| b.split.morsels()).sum()
    }
}

/// Adds to `out` the branch of a scan of `columns` of `table`, filtered by
/// `cond` if given, its conjuncts the engine can judge told it, in morsels
/// of about `rows` rows: whether it could be split. A table of no storage
/// of its own, as a view of the catalog's, is read alone.
fn scan_branch(
    db: &State,
    (table, columns): (&QualifiedName, &[usize]),
    (tx, rows): (TxId, usize),
    (cond, filters): (Option<&Cond>, &[Told]),
    out: &mut Vec<Branch>,
) -> bool {
    let Ok(storage) = db.storage_of(table) else { return false };
    let mut told = cond.and_then(pushed_conjuncts);
    let ranges: Vec<Conjunct> = filters.iter().flat_map(|t| t.filter.range_conjuncts(t.column)).collect();
    if !ranges.is_empty() {
        told.get_or_insert_with(|| Conjunction(Vec::new())).0.extend(ranges);
    }
    let Some(split) = split(storage, (tx, columns, told.as_ref(), rows)) else { return false };
    let scanned = db.catalog.table(table).map(|t| (columns.to_vec(), t.columns.len()));
    out.push(Branch { split, steps: cond.map(|c| Step::Filter(c.clone())).into_iter().collect(), scanned });
    true
}

/// Adds the branch of `plan`, an index scan, to `out`, split into morsels
/// of about `rows` rows, its probe placed with its statement's slots
/// (RULES.md D610), judged by the runtime filters `told`: whether it
/// could be.
fn index_branch(plan: &Plan, (db, tx, rows): (&Db, TxId, usize), told: &[Told], out: &mut Vec<Branch>) -> Result<bool> {
    let Plan::IndexScan { table, index, probe, slots, columns, covering, est, .. } = plan else { return Ok(false) };
    let placed = db.probe(probe, slots)?;
    let scanned = Scanned { table, index, probe: &placed, columns, covering: *covering };
    let Some(split) = indexed::split(db, tx, &scanned, (crate::physical::parallel_rows(*est), rows))? else { return Ok(false) };
    out.push(Branch { split, steps: judged(told.to_vec()), scanned: None });
    Ok(true)
}

/// Adds the branches of `plan` to `out`, each split into morsels of about
/// `rows` rows, `told` the runtime filters of its rows' columns taken
/// down it: whether each could be, and each built operand held.
fn branched(plan: &Plan, (db, tx, rows): (&Db, TxId, usize), (limit, told): (&Limit, &[Told]), out: &mut Vec<Branch>) -> Result<bool> {
    if let Plan::Append { left, right, .. } = plan {
        return Ok(branched(left, (db, tx, rows), (limit, told), out)? && branched(right, (db, tx, rows), (limit, told), out)?);
    }
    let (below, here) = taken(plan, told, db);
    let first = out.len();
    let mut after = Vec::new();
    if let Plan::SeqScan { table, columns, .. } = plan {
        let steps = judged(above(&below).chain(here).collect());
        return Ok(scan_branch(db, (table, columns), (tx, rows), (None, &below), out) && stepped(out, first, steps));
    }
    if let Plan::IndexScan { .. } = plan {
        return index_branch(plan, (db, tx, rows), told, out);
    }
    if let Plan::Filter { cond, input, .. } = plan
        && let Plan::SeqScan { table, columns, .. } = input.bare()
    {
        let (scanned, unscanned) = taken(input, &below, db);
        after.extend(judged(above(&scanned).chain(unscanned).chain(here).collect()));
        let cond = settled(cond, (db, tx))?;
        let split = scan_branch(db, (table, columns), (tx, rows), (Some(&cond), &scanned), out);
        return Ok(split && counted_scan(input, out) && stepped(out, first, after));
    }
    if let Plan::SemiJoin(s) = plan
        && s.method == SemiMethod::BuildLeft
    {
        let Some((split, scanned)) = held_split(s, &below, (db, tx), rows)? else { return Ok(false) };
        out.push(Branch { split, steps: Vec::new(), scanned: Some(scanned) });
        return Ok(stepped(out, first, judged(here)));
    }
    let mut filters = below;
    let Some((step, input)) = step_of(plan, (db, tx), limit, &mut filters)? else { return Ok(false) };
    if !branched(input, (db, tx, rows), (limit, &filters), out)? {
        return Ok(false);
    }
    after.push(step);
    after.extend(judged(here));
    Ok(stepped(out, first, after))
}

/// The step of `plan`, an operator of a pipeline, and its input; a hash
/// join's built operand held, its runtime filters added to `told` if
/// they are wanted; none if the operand gave way.
fn step_of<'a>(plan: &'a Plan, (db, tx): (&Db, TxId), limit: &Limit, told: &mut Vec<Told>) -> Result<Option<(Step, &'a Plan)>> {
    let made: (Step, &Plan) = if let Plan::Filter { cond, input, .. } = plan {
        (Step::Filter(settled(cond, (db, tx))?), input)
    } else if let Plan::Project { exprs, input, .. } = plan {
        (Step::Project(exprs.clone()), input)
    } else if let Plan::Counted(c) = plan {
        c.count.opened();
        (Step::Count(Arc::clone(&c.count)), &c.input)
    } else if let Plan::HashJoin { left, right, keys, .. } = plan {
        let Held::Kept(table) = built(right, keys, (db, tx), limit)? else { return Ok(None) };
        if wanted(db) {
            told.extend(table.shared.filters(keys)?);
        }
        (Step::Probe(table), left)
    } else if let Plan::OuterJoin { left, right, keys, .. } = plan {
        // No filter of its keys goes to its left, every row of which it
        // gives (RULES.md D369).
        let Held::Kept(table) = built(right, keys, (db, tx), limit)? else { return Ok(None) };
        (Step::Outer(table), left)
    } else if let Plan::SemiJoin(s) = plan {
        let Held::Kept(table) = built(&s.right, &s.keys, (db, tx), limit)? else { return Ok(None) };
        // A semi join's left takes its keys' filters, an antijoin's none
        // (RULES.md D369).
        if wanted(db) && !s.anti {
            told.extend(table.shared.filters(&s.keys)?);
        }
        (Step::Semi(table, Arc::new(SemiOf { anti: s.anti, cond: s.cond.clone(), widths: s.widths })), &s.left)
    } else {
        return Err(broken("a part run in parallel is not a pipeline"));
    };
    Ok(Some(made))
}

/// \[DESIGN\] (RULES.md D1132) `cond`, a filter of the pipeline, with
/// each scalar subquery that reads nothing of its rows, that every row it
/// judges runs, and that raises no error and no warning
/// (`optimizer::unnesting::quiet`) made the constant it gives, run once
/// here on the statement's thread before any morsel starts, as the
/// planner took the filter for a step of the part on that account
/// (`physical/parallel.rs`). Another subquery is left as it is.
///
/// # Errors
///
/// None a quiet subquery raises; what running one fails of otherwise.
pub(super) fn settled(cond: &Cond, (db, tx): (&Db, TxId)) -> Result<Cond> {
    let (runner, none) = (Runner::new(db, tx), Row::new());
    crate::exec::bound_by(cond, &mut |e| {
        if let Expr::Subquery(s, _) = &e
            && crate::optimizer::unnesting::quiet(s, db)
        {
            return eval(&e, &none, &runner).map(Expr::Const);
        }
        Ok(e)
    })
}

/// Of a branch just added to `out` of the filtered scan `input`, a
/// counted one's rows counted before the filter's step (RULES.md D378):
/// true, as the step is no more than the scan's.
fn counted_scan(input: &Plan, out: &mut [Branch]) -> bool {
    if let (Plan::Counted(c), Some(branch)) = (input, out.last_mut()) {
        c.count.opened();
        branch.steps.insert(0, Step::Count(Arc::clone(&c.count)));
    }
    true
}

/// \[DESIGN\] (RULES.md D1063) Of `told`, filters a scan is told, those
/// a join above the part told it, which a step judges right after the
/// scan.
fn above(told: &[Told]) -> impl Iterator<Item = Told> {
    told.iter().filter(|t| t.above).cloned()
}

/// The step judging `told`, runtime filters stopped at an operator, if
/// any.
fn judged(told: Vec<Told>) -> Vec<Step> {
    if told.is_empty() { Vec::new() } else { vec![Step::Runtime(told)] }
}

/// Adds `steps` to each branch of `out` from `first`, an append's every
/// operand's: whether none passes `MAX_STEPS`.
fn stepped(out: &mut [Branch], first: usize, steps: Vec<Step>) -> bool {
    let last = out.len().saturating_sub(1);
    let mut steps = Some(steps);
    for (at, branch) in out.iter_mut().enumerate().skip(first) {
        let taken = if at == last { steps.take().unwrap_or_default() } else { steps.iter().flatten().map(copied).collect() };
        branch.steps.extend(taken);
        if branch.steps.len() > MAX_STEPS {
            return false;
        }
    }
    true
}

/// A copy of `step`, for another operand of an append.
fn copied(step: &Step) -> Step {
    match step {
        Step::Filter(c) => Step::Filter(c.clone()),
        Step::Project(e) => Step::Project(e.clone()),
        Step::Probe(t) => Step::Probe(Arc::clone(t)),
        Step::Outer(t) => Step::Outer(Arc::clone(t)),
        Step::Semi(t, semi) => Step::Semi(Arc::clone(t), Arc::clone(semi)),
        Step::Runtime(told) => Step::Runtime(told.clone()),
        Step::Count(c) => Step::Count(Arc::clone(c)),
    }
}

/// The split of `storage`'s rows `tx` sees, `columns` read, those
/// `conditions` may match if given, into morsels of about `rows` rows.
fn split(
    storage: &Arc<dyn crate::storage::TableStorage>,
    (tx, columns, conditions, rows): (TxId, &[usize], Option<&Conjunction>, usize),
) -> Option<Arc<dyn Split>> {
    Arc::clone(storage).split(tx, columns, conditions, rows)
}

// Unit tests: a pipeline's runtime filters (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::physical::JoinKey;
    use crate::value::{ArithOp, CompOp};

    /// The scan of every column of table `t` of schema A, of one column.
    fn scan(t: &str) -> Box<Plan> {
        let table = QualifiedName { auth: "A".into(), ident: t.into() };
        Box::new(Plan::SeqScan {
            table,
            columns: vec![0],
            rows: 0,
            cost: 0.0_f64,
            order: Vec::new(),
            backward: false,
            most: None,
            located: false,
        })
    }

    /// Verifies: [DESIGN] RULES.md D389, D15: a pipeline's hash join,
    /// built first, tells its probing side its runtime filter, which stops
    /// at a filter that can raise, `1 / K > 0`, and is judged by a step
    /// right after it; with EXECUTOR.RUNTIME_FILTERS off, none.
    #[test]
    fn a_probing_side_is_told_the_filters() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE TABLE U (K INTEGER)").unwrap();
        let a = db.session("A");
        let keys: Vec<String> = (0_i32..2_000_i32).map(|k| format!("({k})")).collect();
        a.run(&format!("INSERT INTO T VALUES {}", keys.join(", "))).unwrap();
        a.run("INSERT INTO U VALUES (100), (105)").unwrap();
        a.commit().unwrap();
        let one = Expr::Const(crate::value::Value::Exact(crate::value::Decimal::new(1, 0)));
        let raising = Cond::Compare(Expr::Arith(Box::new(one.clone()), ArithOp::Div, Box::new(Expr::Column(0))), CompOp::Gt, one);
        let left = Box::new(Plan::Filter { cond: raising, detail: String::new(), est: 0.0_f64, of_read: false, input: scan("T") });
        let join = Plan::HashJoin {
            left,
            right: scan("U"),
            keys: vec![JoinKey { left: 0, right: 0, nulls: false }],
            detail: String::new(),
            est: 0.0_f64,
        };
        let steps_of = || {
            let state = Db::from(db.snapshot().unwrap());
            let Held::Kept(pipeline) = Pipeline::of(&join, (&state, crate::txn::NO_TRANSACTION), &Limit::of(&state)).unwrap() else {
                panic!("gave way")
            };
            pipeline.branches.into_iter().next().unwrap().steps
        };
        assert!(matches!(steps_of().as_slice(), [Step::Filter(_), Step::Runtime(told), Step::Probe(_)] if told.len() == 1));
        db.set_setting("EXECUTOR.RUNTIME_FILTERS", &crate::value::Value::Char("OFF".into())).unwrap();
        assert!(matches!(steps_of().as_slice(), [Step::Filter(_), Step::Probe(_)]));
    }

    /// Verifies: [DESIGN] RULES.md D1132: a filter's quiet scalar subquery
    /// is made its constant as the pipeline is made, before any morsel,
    /// and one that warns is left for each row to run.
    #[test]
    fn a_quiet_subquery_is_bound_before_the_morsels() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, B DECIMAL(10,2) NOT NULL, N DECIMAL(10,2))").unwrap();
        let a = db.session("A");
        a.run("INSERT INTO T VALUES (1, 1.50, NULL), (2, 2.50, 3.00)").unwrap();
        a.commit().unwrap();
        let settled_of = |sql: &str| {
            let (st, optimized) = a.optimized(sql).unwrap();
            let crate::logical::LogicalPlan::Project { input, .. } = optimized else { panic!("a projection") };
            let crate::logical::LogicalPlan::Filter { cond, .. } = *input else { panic!("a filter") };
            format!("{:?}", settled(&cond, (&Db::of(&st), crate::txn::NO_TRANSACTION)).unwrap())
        };
        let bound = settled_of("SELECT K FROM T WHERE B > (SELECT AVG(B) FROM T) OR K < 0");
        assert!(!bound.contains("Subquery") && bound.contains("Const(Exact"), "AVG(B) made its constant: {bound}");
        let left = settled_of("SELECT K FROM T WHERE B > (SELECT AVG(N) FROM T) OR K < 0");
        assert!(left.contains("Subquery"), "a subquery that warns is left: {left}");
    }
}
