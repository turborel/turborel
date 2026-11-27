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

use super::passes::Placed;
use super::pipeline::{Branch, Pipeline, Step};
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{
    Batches, Bound, Deferred, Shaped, filter_step, outer_step, probe_step, project_step, resolved, rows_of, runtime_step, semi_step,
    through,
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
    /// \[DESIGN\] (RULES.md D1241) Whether a morsel's batches leave it
    /// with their late columns not decoded, for a sort of its first rows
    /// to decode of the rows it gives (`Pipeline::topped`).
    unresolved: bool,
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
        late.push(deferred_of(branch, Deferred::all(width)).late_of(branch.scanned.as_ref().map_or(&[], |s| &s.0)));
    }
    let (layout, width) = shape?;
    Some(ByBatch { steps, late, unresolved: false, layout, width })
}

impl ByBatch {
    /// It, its branches' late columns and whether they are left so those
    /// of `template`, the pipeline of a join's first pass, as a later
    /// pass's steps read the columns the first pass's read (RULES.md
    /// D1240).
    pub(super) fn late_of_first(mut self, template: &ByBatch) -> ByBatch {
        self.late.clone_from(&template.late);
        self.unresolved = template.unresolved;
        self
    }

    /// Of a test, its branches' late columns and whether they are left so.
    #[cfg(test)]
    pub(super) fn late_given(&self) -> (&[Vec<usize>], bool) {
        (&self.late, self.unresolved)
    }
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
            Step::Kept(keep) => {
                let places = keep.left.iter().map(|c| layout.iter().position(|l| l == c)).collect::<Option<Vec<usize>>>()?;
                shaped.push(Shaped::Kept(Arc::new(Placed { keep: Arc::clone(keep), places })));
            }
        }
    }
    Some((shaped, layout, width))
}

/// \[DESIGN\] (RULES.md D361, D366, D1081) What `branch`'s steps leave
/// of its scan's columns unread, `above` what whoever reads its rows at
/// its end leaves of them: its morsels give late the columns no step
/// reads before then, if a filter or a hash join's probe drops rows on the
/// way, decoded of the rows kept at the pipeline's end (`morsel_kept`), as
/// the serial stream does. A probe reads its keys' columns and carries the
/// left's others as their rows' places (`Deferred::left_of`), so a column
/// of the probing table is decoded of the rows every join of the pipeline
/// kept alone.
fn deferred_of(branch: &Branch, above: Deferred) -> Deferred {
    let Some((_, read)) = &branch.scanned else { return Deferred::default() };
    // The width of the rows each step is given.
    let (mut given, mut wide) = (Vec::with_capacity(branch.steps.len()), *read);
    for step in &branch.steps {
        given.push(wide);
        wide = match step {
            Step::Project(exprs) => exprs.len(),
            Step::Probe(table) | Step::Outer(table) => table.shared.joined_layout((&[], wide)).1,
            Step::Filter(_) | Step::Semi(..) | Step::Runtime(_) | Step::Count(_) | Step::Kept(_) => wide,
        };
    }
    let mut deferred = above;
    for (step, &left) in branch.steps.iter().zip(&given).rev() {
        deferred = match step {
            Step::Filter(cond) => deferred.filtered(cond),
            Step::Project(exprs) => deferred.projected(exprs),
            Step::Probe(table) => deferred.left_of(table.keys(), left),
            Step::Outer(_) | Step::Semi(..) => Deferred::default(),
            Step::Runtime(told) => deferred.judged(&told.iter().map(|t| t.column).collect::<Vec<_>>()),
            Step::Count(_) => deferred,
            Step::Kept(keep) => deferred.judged(&keep.left),
        };
    }
    deferred
}

impl Pipeline {
    /// \[DESIGN\] (RULES.md D1241) It, read by a sort of which only the
    /// first rows are asked for, whose keys read the columns `keys` of the
    /// rows at its end, `width` wide: each branch's scan told the bound of
    /// the sort's first key, `bound`, if given and its steps let it reach
    /// the scan (`Deferred::topped`, as the serial stream is told, RULES.md
    /// D379), one bound that every morsel's sort raises and every morsel's
    /// scan reads; and of `late`, the columns no step and no key reads
    /// left late past the pipeline's end (`morsel_late`), for each
    /// morsel's sort to decode of the rows it gives alone. A pipeline not
    /// read by batch is left as it is.
    pub(super) fn topped(mut self, (keys, late): (&[usize], bool), bound: &Bound) -> Pipeline {
        let Some(by) = self.batched.as_mut() else { return self };
        let width = by.width;
        for (branch, given) in self.branches.iter_mut().zip(by.late.iter_mut()) {
            let deferred = deferred_of(branch, Deferred::all(width).topped(keys, bound.clone()));
            if let Some(split) = deferred.told().and_then(|conjunct| branch.split.told(conjunct)) {
                branch.split = split;
            }
            if late {
                *given = deferred.late_of(branch.scanned.as_ref().map_or(&[], |s| &s.0));
            }
        }
        by.unresolved = late;
        self
    }

    /// The batches of morsel `n`, each with the rows its steps kept, of a
    /// pipeline read by batch, its expressions evaluated for the statement
    /// of `tx`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the pipeline is not read by batch, or `n` is past its
    /// last morsel.
    pub(super) fn morsel_kept(&self, n: usize, (db, tx): (&Db, TxId)) -> Result<Batches> {
        let unresolved = self.batched.as_ref().is_some_and(|by| by.unresolved);
        let kept = self.morsel_late(n, (db, tx))?;
        Ok(if unresolved { kept } else { resolved(kept, crate::db::kernels::chosen(db)) })
    }

    /// As `morsel_kept`, the late columns of each batch as the steps left
    /// them, not decoded.
    fn morsel_late(&self, n: usize, (db, tx): (&Db, TxId)) -> Result<Batches> {
        // A morsel of a pass past the first is of that pass's pipeline,
        // whose late columns are this one's, so they are decoded here once
        // or left to the sort (RULES.md D1240, D1241).
        if let Some((pass, m)) = super::passes::passed(self, n)? {
            return pass.morsel_late(m, (db, tx));
        }
        let by = self.batched.as_ref().ok_or_else(|| broken("a pipeline not read by batch"))?;
        let mut at = n;
        for ((branch, steps), late) in self.branches.iter().zip(&by.steps).zip(&by.late) {
            let morsels = branch.split.morsels();
            if at < morsels {
                let read = branch.split.morsel_batches(at, late).ok_or_else(|| broken("a morsel not read by batch"))?;
                let kept: Batches = Box::new(read.map(|b| b.map(|b| (b, None))));
                return Ok(through(kept, steps, (db, tx)));
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

    /// Verifies: [DESIGN] RULES.md D1241: a pipeline read by a sort of its
    /// first rows leaves late past its end the columns neither its filter
    /// nor the sort's keys read, and its scan is told the bound of the
    /// first key; asked for no late columns, it gives late what any reader
    /// of it is given, decoded at its end, the bound told still; and a
    /// hash join's probing side is told no bound, as the join may drop or
    /// repeat its rows.
    #[test]
    fn a_sort_of_first_rows_leaves_its_other_columns_late_and_tells_its_bound() {
        use super::super::batched::deferred_of;
        use crate::exec::batch::Deferred;
        use crate::storage::{Conjunct, Threshold};
        use std::sync::Arc;
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE F (K INTEGER, P INTEGER, Q INTEGER) CREATE TABLE D (K INTEGER, N INTEGER)")
            .unwrap();
        let a = db.session("A");
        a.run("INSERT INTO F VALUES (1, 10, 100), (2, 20, 200)").unwrap();
        a.run("INSERT INTO D VALUES (1, 7)").unwrap();
        a.commit().unwrap();
        let state = Db::from(db.snapshot().unwrap());
        let pipeline_of = |plan: &Plan| {
            let Held::Kept(pipeline) = Pipeline::of(plan, (&state, crate::txn::NO_TRANSACTION), &Limit::of(&state)).unwrap() else {
                panic!("the pipeline gave way")
            };
            pipeline
        };
        let cond = Cond::Compare(Expr::Column(2), CompOp::Ge, Expr::Const(Value::Exact(Decimal::new(100, 0))));
        let filtered =
            Plan::Filter { cond, detail: String::new(), est: 0.0_f64, of_read: false, input: Box::new(scan("F", vec![0, 1, 2])) };
        let bound = || Some((1, Arc::new(Threshold::new(false))));
        let told = |pipeline: &Pipeline| {
            let topped = Deferred::all(3).topped(&[1], bound());
            matches!(deferred_of(&pipeline.branches[0], topped).told(), Some(Conjunct::Dynamic { column: 1, .. }))
        };
        let topped = pipeline_of(&filtered).topped((&[1], true), &bound());
        let by = topped.batched.as_ref().unwrap();
        assert_eq!((by.late.clone(), by.unresolved), (vec![vec![0]], true), "all but the filter's column and the key");
        assert!(told(&topped), "the bound reaches the scan through the filter");
        let early = pipeline_of(&filtered).topped((&[1], false), &bound());
        let read = early.batched.as_ref().unwrap();
        assert_eq!((read.late.clone(), read.unresolved), (vec![vec![0, 1]], false), "as any reader of the pipeline is given");
        let bare = pipeline_of(&scan("F", vec![0, 1, 2])).topped((&[1], true), &bound());
        assert_eq!(bare.batched.as_ref().unwrap().late, vec![vec![0, 2]], "the sort drops rows, with no filter too");
        assert!(!told(&pipeline_of(&joined(0))), "a join's probing side is told none");
    }
}
