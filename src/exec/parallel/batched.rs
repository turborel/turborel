//! \[DESIGN\] A pipeline's morsels read by batch (RULES.md D354): each
//! morsel's rows read as the serial scan by batch reads them
//! (`Split::morsel_batches`), taken through the steps of its pipeline by
//! the functions the serial stream applies (`exec/batch/shaped.rs`), a
//! filter judged over the vectors, a projection of their columns, so a
//! worker costs a row what the statement's thread alone costs it, as
//! Leis et al., "Morsel-Driven Parallelism" (2014), run a morsel's
//! pipeline fragment whole on one thread. A pipeline whose every branch
//! is a table's scan whose engine reads by batch, and whose every step is
//! a filter, a projection or a probe the serial stream reads by batch, is
//! read so; any other, as an index scan's, row by row
//! (`parallel/pipeline.rs`).
//! A morsel's batches give the rows, in the order, and raise the error,
//! that its rows would.

use std::sync::Arc;

use super::pipeline::{Branch, Pipeline, Step};
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{
    Batches, Deferred, Shaped, filter_step, outer_step, probe_step, project_step, resolved, rows_of, runtime_step, semi_step, through,
};
use crate::exec::{RowIter, broken};
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
        let (scanned, built) = late_of(branch);
        let (shaped, layout, width) = branch_steps(branch, &built)?;
        if shape.as_ref().is_some_and(|(l, w)| (l, *w) != (&layout, width)) {
            return None;
        }
        shape = Some((layout, width));
        steps.push(shaped);
        late.push(scanned);
    }
    let (layout, width) = shape?;
    Some(ByBatch { steps, late, layout, width })
}

/// The steps of `branch` over its batches, and the columns its batches
/// hold at its end and their rows' width, if each step is read by batch;
/// of each step at `n`, the columns `built[n]` of a probe's built rows given
/// late (RULES.md D1096).
fn branch_steps(branch: &Branch, built: &[Vec<usize>]) -> Option<(Vec<Shaped>, Vec<usize>, usize)> {
    let (mut layout, mut width) = branch.scanned.clone()?;
    let mut shaped = Vec::with_capacity(branch.steps.len());
    for (n, step) in branch.steps.iter().enumerate() {
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
                let late = built.get(n).map_or(&[][..], Vec::as_slice);
                (made, layout, width) = probe_step(&table.shared, (&left, late), (&layout, width))?;
                shaped.push(made);
            }
            Step::Outer(table) => {
                let left: Vec<usize> = table.keys().iter().map(|k| k.left).collect();
                let made;
                (made, layout, width) = outer_step(&table.shared, &left, (&layout, width))?;
                shaped.push(made);
            }
            Step::Semi(table, semi) => {
                let join = (table.keys(), semi.cond.as_ref(), semi.anti);
                shaped.push(semi_step(&table.shared, join, (layout.as_slice(), semi.widths))?);
            }
            Step::Runtime(told) => shaped.push(runtime_step(told, &layout)),
            Step::Count(count) => shaped.push(Shaped::Count(Arc::clone(count))),
        }
    }
    Some((shaped, layout, width))
}

/// \[DESIGN\] (RULES.md D361, D366, D1081) The columns of `branch`'s scan
/// no step reads before its rows are made at its end, if a
/// filter or a hash join's probe drops rows on the way: its morsels give
/// them late, decoded of the rows kept at the pipeline's end
/// (`morsel_kept`), as the serial stream does. A probe reads its keys'
/// columns and carries the left's others as their rows' places
/// (`Deferred::left_of`), so a column of the probing table is decoded of
/// the rows every join of the pipeline kept alone. Of each step, the
/// columns of a probe's built rows given late too (`Deferred::right_of`),
/// resolved at the pipeline's end with the scan's, while its table lives.
fn late_of(branch: &Branch) -> (Vec<usize>, Vec<Vec<usize>>) {
    let Some((scanned, read)) = &branch.scanned else { return (Vec::new(), Vec::new()) };
    // The width of the rows each step is given.
    let (mut given, mut wide) = (Vec::with_capacity(branch.steps.len()), *read);
    for step in &branch.steps {
        given.push(wide);
        wide = match step {
            Step::Project(exprs) => exprs.len(),
            Step::Probe(table) | Step::Outer(table) => table.shared.joined_layout((&[], wide)).1,
            Step::Filter(_) | Step::Semi(..) | Step::Runtime(_) | Step::Count(_) => wide,
        };
    }
    let mut deferred = Deferred::all(wide);
    let mut built = vec![Vec::new(); branch.steps.len()];
    for ((step, &left), late) in branch.steps.iter().zip(&given).zip(&mut built).rev() {
        deferred = match step {
            Step::Filter(cond) => deferred.filtered(cond),
            Step::Project(exprs) => deferred.projected(exprs),
            Step::Probe(table) => {
                *late = deferred.right_of(left);
                deferred.left_of(table.keys(), left)
            }
            Step::Outer(_) | Step::Semi(..) => Deferred::default(),
            Step::Runtime(told) => deferred.judged(&told.iter().map(|t| t.column).collect::<Vec<_>>()),
            Step::Count(_) => deferred,
        };
    }
    (deferred.late_of(scanned), built)
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
    /// by row (`parallel/rows.rs`), in order, each made as it is asked for.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `n` is past its last morsel; a row or a step that
    /// fails is an error item after the rows before it.
    pub(super) fn morsel_rows(self: &Arc<Self>, n: usize, (db, tx): (&Db, TxId), runner: crate::exec::Runner) -> Result<RowIter> {
        let Some(by) = &self.batched else { return super::rows::morsel_rows(self, n, runner) };
        Ok(rows_of(self.morsel_kept(n, (db, tx))?, by.layout.clone(), by.width))
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

    /// Of each built vector of the first probe of `plan`'s pipeline,
    /// whether it is given late.
    fn built_late(db: &Database, plan: &Plan) -> Vec<bool> {
        let state = Db::from(db.snapshot().unwrap());
        let Held::Kept(pipeline) = Pipeline::of(plan, (&state, crate::txn::NO_TRANSACTION), &Limit::of(&state)).unwrap() else {
            panic!("the pipeline gave way")
        };
        let steps = pipeline.batched.unwrap().steps.remove(0);
        steps.into_iter().find_map(|s| if let super::Shaped::Probe(_, _, late) = s { Some(late) } else { None }).unwrap()
    }

    /// Verifies: [DESIGN] RULES.md D1096: a probe gives late the built
    /// operand's columns no step above reads, once a step above drops rows:
    /// none read whole with no row dropped; both of D under a filter of F's
    /// column; D's N read by a filter above, its K alone.
    #[test]
    fn a_probe_gives_its_built_columns_late() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE F (K INTEGER, P INTEGER, Q INTEGER) CREATE TABLE D (K INTEGER, N INTEGER)")
            .unwrap();
        let a = db.session("A");
        a.run("INSERT INTO F VALUES (1, 10, 100), (2, 20, 200)").unwrap();
        a.run("INSERT INTO D VALUES (1, 7)").unwrap();
        a.commit().unwrap();
        assert_eq!(built_late(&db, &joined(0)), [false, false], "read whole, no row dropped");
        let over = |column: usize| {
            let cond = Cond::Compare(Expr::Column(column), CompOp::Ge, Expr::Const(Value::Exact(Decimal::new(5, 0))));
            Plan::Filter { cond, detail: String::new(), est: 0.0_f64, of_read: false, input: Box::new(joined(0)) }
        };
        assert_eq!(built_late(&db, &over(2)), [true, true], "a filter of F's column above");
        assert_eq!(built_late(&db, &over(4)), [true, false], "D's N read by the filter");
    }
}
