//! \[DESIGN\] A pipeline's morsels read by batch (RULES.md D354): each
//! morsel's rows read as the serial scan by batch reads them
//! (`Split::morsel_batches`), taken through the steps of its pipeline by
//! the functions the serial stream applies (`exec/batch/shaped.rs`), a
//! filter judged over the vectors, a projection of their columns, so a
//! worker costs a row what the statement's thread alone costs it, as
//! Leis et al., "Morsel-Driven Parallelism" (2014), run a morsel's
//! pipeline fragment whole on one thread. A pipeline whose every branch
//! is a table's scan whose engine reads by batch, and whose every step is
//! a filter, a projection or a hash join's probe the serial stream reads
//! by batch, is read so; any other, as an index scan's, row by row
//! (`parallel/pipeline.rs`).
//! A morsel's batches give the rows, in the order, and raise the error,
//! that its rows would.

use std::sync::Arc;

use super::pipeline::{Branch, Pipeline, Step};
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, Deferred, Shaped, filter_step, probe_step, project_step, resolved, rows_of, runtime_step, through};
use crate::exec::broken;
use crate::storage::Row;
use crate::txn::TxId;

/// How a pipeline's morsels are read by batch: each branch's steps over
/// its batches, and the columns of the rows the batches of every branch
/// hold at its end, and the width of those rows.
pub(super) struct ByBatch {
    steps: Vec<Vec<Shaped>>,
    /// Of each branch, the columns its morsels give late (RULES.md D366).
    late: Vec<Vec<usize>>,
    pub(super) layout: Vec<usize>,
    pub(super) width: usize,
}

/// How `branches` are read by batch, if each is (as the module's
/// documentation says) and `db` reads by batch (EXECUTOR.BATCHES).
pub(super) fn by_batch(branches: &[Branch], db: &State) -> Option<ByBatch> {
    if !crate::exec::batched(db) {
        return None;
    }
    let (mut steps, mut late) = (Vec::with_capacity(branches.len()), Vec::with_capacity(branches.len()));
    let mut shape: Option<(Vec<usize>, usize)> = None;
    for branch in branches {
        let (shaped, layout, width) = branch_steps(branch)?;
        if shape.as_ref().is_some_and(|(l, w)| (l, *w) != (&layout, width)) {
            return None;
        }
        shape = Some((layout, width));
        steps.push(shaped);
        late.push(late_of(branch, width));
    }
    let (layout, width) = shape?;
    Some(ByBatch { steps, late, layout, width })
}

/// The steps of `branch` over its batches, and the columns its batches
/// hold at its end and their rows' width, if each step is read by batch.
fn branch_steps(branch: &Branch) -> Option<(Vec<Shaped>, Vec<usize>, usize)> {
    let (mut layout, mut width) = branch.scanned.clone()?;
    let mut shaped = Vec::with_capacity(branch.steps.len());
    for step in &branch.steps {
        match step {
            Step::Filter(cond) => shaped.push(filter_step(cond, &layout)),
            Step::Project(exprs) => {
                let (made, after) = project_step(exprs, &layout)?;
                shaped.push(made);
                (layout, width) = (after, exprs.len());
            }
            Step::Probe(table) => {
                let left: Vec<usize> = table.keys().iter().map(|k| k.left).collect();
                let made;
                (made, layout, width) = probe_step(&table.shared, &left, (&layout, width))?;
                shaped.push(made);
            }
            Step::Runtime(told) => shaped.push(runtime_step(told, &layout)),
            Step::Count(count) => shaped.push(Shaped::Count(Arc::clone(count))),
        }
    }
    Some((shaped, layout, width))
}

/// \[DESIGN\] (RULES.md D361, D366, D1081) The columns of `branch`'s scan
/// no step reads before its rows, `width` wide at its end, are made, if a
/// filter or a hash join's probe drops rows on the way: its morsels give
/// them late, decoded of the rows kept at the pipeline's end
/// (`morsel_kept`), as the serial stream does. A probe reads its keys'
/// columns and carries the left's others as their rows' places
/// (`Deferred::left_of`), so a column of the probing table is decoded of
/// the rows every join of the pipeline kept alone.
fn late_of(branch: &Branch, width: usize) -> Vec<usize> {
    let Some((scanned, read)) = &branch.scanned else { return Vec::new() };
    // The width of the rows each step is given.
    let (mut given, mut wide) = (Vec::with_capacity(branch.steps.len()), *read);
    for step in &branch.steps {
        given.push(wide);
        wide = match step {
            Step::Project(exprs) => exprs.len(),
            Step::Probe(table) => table.shared.joined_layout((&[], wide)).1,
            Step::Filter(_) | Step::Runtime(_) | Step::Count(_) => wide,
        };
    }
    let mut deferred = Deferred::all(width);
    for (step, &left) in branch.steps.iter().zip(&given).rev() {
        deferred = match step {
            Step::Filter(cond) => deferred.filtered(cond),
            Step::Project(exprs) => deferred.projected(exprs),
            Step::Probe(table) => deferred.left_of(table.keys(), left),
            Step::Runtime(told) => deferred.judged(&told.iter().map(|t| t.column).collect::<Vec<_>>()),
            Step::Count(_) => deferred,
        };
    }
    deferred.late_of(scanned)
}

impl Pipeline {
    /// The batches of morsel `n`, each with the rows its steps kept, of a
    /// pipeline read by batch, its expressions evaluated for the statement
    /// of `tx`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the pipeline is not read by batch, or `n` is past its
    /// last morsel.
    pub(super) fn morsel_kept(&self, n: usize, (db, tx): (&Db, TxId)) -> Result<Batches> {
        let by = self.batched.as_ref().ok_or_else(|| broken("a pipeline not read by batch"))?;
        let mut at = n;
        for ((branch, steps), late) in self.branches.iter().zip(&by.steps).zip(&by.late) {
            let morsels = branch.split.morsels();
            if at < morsels {
                let read = branch.split.morsel_batches(at, late).ok_or_else(|| broken("a morsel not read by batch"))?;
                let kept: Batches = Box::new(read.map(|b| b.map(|b| (b, None))));
                return Ok(resolved(through(kept, steps, (db, tx)), crate::db::kernels::chosen(db)));
            }
            at = at.saturating_sub(morsels);
        }
        Err(broken("a morsel past a pipeline's last"))
    }

    /// The rows of morsel `n`, read by batch if the pipeline is, else row
    /// by row (`Pipeline::morsel`), in order.
    ///
    /// # Errors
    ///
    /// As reading a row or a step fails, the rows before it given up.
    pub(super) fn morsel_rows(&self, n: usize, (db, tx): (&Db, TxId), runner: &crate::exec::Runner) -> Result<Vec<Row>> {
        let Some(by) = &self.batched else { return self.morsel(n, runner) };
        rows_of(self.morsel_kept(n, (db, tx))?, by.layout.clone(), by.width).collect()
    }
}

// Unit tests: the columns a pipeline's morsels give late (RULES.md
// R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::super::pipeline::Pipeline;
    use super::super::{Held, Limit};
    use crate::db::Database;
    use crate::exec::Db;
    use crate::logical::{Cond, Expr};
    use crate::physical::{JoinKey, Plan};
    use crate::value::{CompOp, Decimal, Value};

    /// A scan of `columns` of table `ident` of schema A.
    fn scan(ident: &str, columns: Vec<usize>) -> Plan {
        Plan::SeqScan {
            table: crate::catalog::QualifiedName { auth: "A".into(), ident: ident.into() },
            columns,
            rows: 0,
            cost: 0.0_f64,
            order: Vec::new(),
            backward: false,
            most: None,
            located: false,
        }
    }

    /// F joined with D on F's column `left` and D's K.
    fn joined(left: usize) -> Plan {
        let keys = vec![JoinKey { left, right: 0, nulls: false }];
        Plan::HashJoin {
            left: Box::new(scan("F", vec![0, 1, 2])),
            right: Box::new(scan("D", vec![0, 1])),
            keys,
            detail: String::new(),
            est: 0.0_f64,
        }
    }

    /// The columns of F the morsels of `plan` give late.
    fn late(db: &Database, plan: &Plan) -> Vec<usize> {
        let state = Db::from(db.snapshot().unwrap());
        let Held::Kept(pipeline) = Pipeline::of(plan, (&state, crate::txn::NO_TRANSACTION), &Limit::of(&state)).unwrap() else {
            panic!("the pipeline gave way")
        };
        pipeline.batched.unwrap().late.remove(0)
    }

    /// Verifies: [DESIGN] RULES.md D1081: a pipeline's morsels give late
    /// the columns of the probing table that no step reads, across a hash
    /// join's probe: all but the probe's key; not one a filter above the
    /// probe reads; and none of a pipeline that drops no row.
    #[test]
    fn a_probe_carries_the_probing_tables_unread_columns_late() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE F (K INTEGER, P INTEGER, Q INTEGER) CREATE TABLE D (K INTEGER, N INTEGER)")
            .unwrap();
        let a = db.session("A");
        a.run("INSERT INTO F VALUES (1, 10, 100), (2, 20, 200)").unwrap();
        a.run("INSERT INTO D VALUES (1, 7)").unwrap();
        a.commit().unwrap();
        assert_eq!(late(&db, &joined(0)), [1, 2], "all but the key");
        assert_eq!(late(&db, &joined(1)), [0, 2], "the key read is the one the join names");
        let cond = Cond::Compare(Expr::Column(2), CompOp::Ge, Expr::Const(Value::Exact(Decimal::new(100, 0))));
        let filtered = Plan::Filter { cond, detail: String::new(), est: 0.0_f64, of_read: false, input: Box::new(joined(0)) };
        assert_eq!(late(&db, &filtered), [1], "a column a filter above reads is read");
        assert_eq!(late(&db, &scan("F", vec![0, 1, 2])), Vec::<usize>::new(), "no step drops a row");
    }
}
