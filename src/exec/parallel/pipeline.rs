//! \[DESIGN\] A pipeline run by morsels (RULES.md D290, D292): each table
//! it reads split into morsels by its engine (`TableStorage::split`), and
//! each morsel's rows taken through the steps over them, a filter's, a
//! projection's and a hash join's probe, one row at a time to the end, as
//! Leis et al., "Morsel-Driven Parallelism" (2014), run a pipeline
//! fragment on a morsel. The steps are those of the plan, applied as its
//! operators apply them, so a morsel gives the rows, in the order, and
//! raises the error, the plan's operators would of those of its table.
//!
//! An index scan's morsels are parts of its range of keys
//! (`parallel/indexed.rs`).
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
use crate::exec::batch::{Told, runtime_taken as taken, wanted};
use crate::exec::{Runner, broken, eval, eval_cond};
use crate::logical::{Cond, Expr, Truth, pushed_conjuncts};
use crate::physical::{NodeCount, Plan};
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
    /// \[DESIGN\] (RULES.md D389) kept if hash joins' runtime filters
    /// stopped at an operator may match it; judged by batch alone.
    Runtime(Vec<Told>),
    /// \[EXTENSION\] counted, as the rows of a counted operator, each
    /// morsel's added once (RULES.md D378).
    Count(Arc<NodeCount>),
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
        Pipeline::told(plan, (db, tx), (limit, &[]))
    }

    /// As `Pipeline::of`, its rows judged by the runtime filters `told`
    /// of their columns, each taken down the stream as far as it goes
    /// (RULES.md D389, D1070).
    ///
    /// # Errors
    ///
    /// As building an operand fails.
    pub(super) fn told(plan: &Plan, (db, tx): (&Db, TxId), (limit, told): (&Limit, &[Told])) -> Result<Held<Pipeline>> {
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

    /// The rows of morsel `n`, in order, its expressions evaluated by
    /// `runner`.
    ///
    /// # Errors
    ///
    /// As reading a row or a step fails, the rows before it given up.
    pub(super) fn morsel(&self, n: usize, runner: &Runner) -> Result<Vec<Row>> {
        let mut at = n;
        for branch in &self.branches {
            let morsels = branch.split.morsels();
            if at < morsels {
                return branch.morsel(at, runner);
            }
            at = at.saturating_sub(morsels);
        }
        Err(broken("a morsel past a pipeline's last"))
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
        return Ok(scan_branch(db, (table, columns), (tx, rows), (None, &below), out) && stepped(out, first, judged(here)));
    }
    if let Plan::IndexScan { .. } = plan {
        return index_branch(plan, (db, tx, rows), told, out);
    }
    if let Plan::Filter { cond, input, .. } = plan
        && let Plan::SeqScan { table, columns, .. } = input.bare()
    {
        let (scanned, unscanned) = taken(input, &below, db);
        after.extend(judged(unscanned.into_iter().chain(here).collect()));
        let split = scan_branch(db, (table, columns), (tx, rows), (Some(cond), &scanned), out);
        return Ok(split && counted_scan(input, out) && stepped(out, first, after));
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
        (Step::Filter(cond.clone()), input)
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
    } else {
        return Err(broken("a part run in parallel is not a pipeline"));
    };
    Ok(Some(made))
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
        Step::Runtime(told) => Step::Runtime(told.clone()),
        Step::Count(c) => Step::Count(Arc::clone(c)),
    }
}

/// Adds to each counted step of `steps` the rows `tally` counted of it.
fn counted(steps: &[Step], tally: &[u64]) {
    for (step, &rows) in steps.iter().zip(tally) {
        if let Step::Count(count) = step {
            count.stepped(rows);
        }
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

impl Branch {
    /// The rows of its morsel `at`, its steps' expressions evaluated by
    /// `runner`. A pipeline of no counted step, as every one is but an
    /// analyzed plan's, tallies nothing a row (RULES.md D378).
    ///
    /// # Errors
    ///
    /// As reading a row or a step fails.
    fn morsel(&self, at: usize, runner: &Runner) -> Result<Vec<Row>> {
        let mut out = Vec::new();
        if !self.steps.iter().any(|s| matches!(s, Step::Count(_))) {
            for read in self.split.morsel(at) {
                feed(&self.steps, read?.1, runner, &mut out)?;
            }
            return Ok(out);
        }
        let mut tally = vec![0_u64; self.steps.len()];
        for read in self.split.morsel(at) {
            tallied(&self.steps, (&mut tally, read?.1), runner, &mut out)?;
        }
        counted(&self.steps, &tally);
        Ok(out)
    }
}

/// Takes `row` through `steps`, the first first, adding each row it gives
/// to `out`; a counted step passes it on, as no step of a plan not
/// analyzed is one.
fn feed(steps: &[Step], row: Row, runner: &Runner, out: &mut Vec<Row>) -> Result<()> {
    let Some((step, rest)) = steps.split_first() else {
        out.push(row);
        return Ok(());
    };
    match step {
        Step::Filter(cond) => {
            if eval_cond(cond, &row, runner)? == Truth::True {
                feed(rest, row, runner, out)?;
            }
            Ok(())
        }
        Step::Project(exprs) => {
            let projected = exprs.iter().map(|e| eval(e, &row, runner)).collect::<Result<Row>>()?;
            feed(rest, projected, runner, out)
        }
        Step::Probe(table) => table.each_match(&row, |r| feed(rest, row.iter().chain(r.iter()).cloned().collect(), runner, out)),
        Step::Runtime(..) | Step::Count(_) => feed(rest, row, runner, out),
    }
}

/// As `feed`, adding to `tally`, a count for each step, the rows that
/// reach a counted one.
fn tallied(steps: &[Step], (tally, row): (&mut [u64], Row), runner: &Runner, out: &mut Vec<Row>) -> Result<()> {
    let Some((step, rest)) = steps.split_first() else {
        out.push(row);
        return Ok(());
    };
    let (seen, later) = match tally.split_first_mut() {
        Some((seen, later)) => (Some(seen), later),
        None => (None, &mut [][..]),
    };
    match step {
        Step::Filter(cond) => {
            if eval_cond(cond, &row, runner)? == Truth::True {
                tallied(rest, (later, row), runner, out)?;
            }
            Ok(())
        }
        Step::Project(exprs) => {
            let projected = exprs.iter().map(|e| eval(e, &row, runner)).collect::<Result<Row>>()?;
            tallied(rest, (later, projected), runner, out)
        }
        Step::Probe(table) => {
            table.each_match(&row, |r| tallied(rest, (&mut *later, row.iter().chain(r.iter()).cloned().collect()), runner, out))
        }
        Step::Count(_) => {
            if let Some(seen) = seen {
                *seen = seen.saturating_add(1);
            }
            tallied(rest, (later, row), runner, out)
        }
        Step::Runtime(..) => tallied(rest, (later, row), runner, out),
    }
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
}
