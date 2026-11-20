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

use std::sync::Arc;

use super::indexed::{self, Scanned};
use super::probe::{Table, built};
use super::{Held, Limit};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::{Runner, broken, eval, eval_cond};
use crate::logical::{Cond, Expr, Truth, pushed_conjuncts};
use crate::physical::Plan;
use crate::settings::EXECUTOR_MORSEL_ROWS;
use crate::storage::{Conjunction, Row, Split};
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
    pub(super) fn of(plan: &Plan, (db, tx): (&Arc<State>, TxId), limit: &Limit) -> Result<Held<Pipeline>> {
        let rows = db.settings.count(EXECUTOR_MORSEL_ROWS).max(1);
        let mut branches = Vec::new();
        if !branched(plan, (db, tx, rows), limit, &mut branches)? {
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
                let mut out = Vec::new();
                for read in branch.split.morsel(at) {
                    feed(&branch.steps, read?.1, runner, &mut out)?;
                }
                return Ok(out);
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
    cond: Option<&Cond>,
    out: &mut Vec<Branch>,
) -> bool {
    let Ok(storage) = db.storage_of(table) else { return false };
    let told = cond.and_then(pushed_conjuncts);
    let Some(split) = split(storage, (tx, columns, told.as_ref(), rows)) else { return false };
    let scanned = db.catalog.table(table).map(|t| (columns.to_vec(), t.columns.len()));
    out.push(Branch { split, steps: cond.map(|c| Step::Filter(c.clone())).into_iter().collect(), scanned });
    true
}

/// Adds the branches of `plan` to `out`, each split into morsels of about
/// `rows` rows: whether each could be, and each built operand held.
fn branched(plan: &Plan, (db, tx, rows): (&Arc<State>, TxId, usize), limit: &Limit, out: &mut Vec<Branch>) -> Result<bool> {
    if let Plan::Append { left, right, .. } = plan {
        return Ok(branched(left, (db, tx, rows), limit, out)? && branched(right, (db, tx, rows), limit, out)?);
    }
    if let Plan::SeqScan { table, columns, .. } = plan {
        return Ok(scan_branch(db, (table, columns), (tx, rows), None, out));
    }
    if let Plan::IndexScan { table, index, probe, columns, covering, est, .. } = plan {
        let scanned = Scanned { table, index, probe, columns, covering: *covering };
        let Some(split) = indexed::split(db, tx, &scanned, (crate::physical::parallel_rows(*est), rows))? else { return Ok(false) };
        out.push(Branch { split, steps: Vec::new(), scanned: None });
        return Ok(true);
    }
    if let Plan::Filter { cond, input, .. } = plan
        && let Plan::SeqScan { table, columns, .. } = &**input
    {
        return Ok(scan_branch(db, (table, columns), (tx, rows), Some(cond), out));
    }
    let (step, input) = if let Plan::Filter { cond, input, .. } = plan {
        (Step::Filter(cond.clone()), input)
    } else if let Plan::Project { exprs, input, .. } = plan {
        (Step::Project(exprs.clone()), input)
    } else if let Plan::HashJoin { left, right, keys, .. } = plan {
        match built(right, keys, (db, tx), limit)? {
            Held::Kept(table) => (Step::Probe(table), left),
            Held::GaveWay => return Ok(false),
        }
    } else {
        return Err(broken("a part run in parallel is not a pipeline"));
    };
    let first = out.len();
    if !branched(input, (db, tx, rows), limit, out)? {
        return Ok(false);
    }
    // Of an append below, each operand's rows take the step.
    let (last, mut step) = (out.len().saturating_sub(1), Some(step));
    for (at, branch) in out.iter_mut().enumerate().skip(first) {
        let taken = if at == last { step.take() } else { step.as_ref().map(copied) };
        branch.steps.extend(taken);
        if branch.steps.len() > MAX_STEPS {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A copy of `step`, for another operand of an append.
fn copied(step: &Step) -> Step {
    match step {
        Step::Filter(c) => Step::Filter(c.clone()),
        Step::Project(e) => Step::Project(e.clone()),
        Step::Probe(t) => Step::Probe(Arc::clone(t)),
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

/// Takes `row` through `steps`, the first first, adding each row it gives
/// to `out`.
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
    }
}
