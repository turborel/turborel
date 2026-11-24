//! \[DESIGN\] (RULES.md D1070) A pipeline run in parallel read by an
//! operator that needs no order of its rows, as a semi join that builds
//! its left operand reads its right (`batch/joined/semi/marked.rs`): each
//! morsel's batches, taken through the pipeline's steps on up to the
//! degree's threads (`parallel/batched.rs`), are handed to the operator's
//! work on the thread that read them, so nothing of a morsel crosses to
//! the statement's thread but that it ended. The first morsel that fails,
//! in the order of their numbers, is the error raised, as the plan alone
//! raises the first of its rows' (RULES.md D15). A pipeline whose morsels
//! are not read by batch, or whose batches hold other columns than the
//! plan's alone, gives way before any morsel is read.

use std::sync::Arc;

use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, Told};
use crate::physical::Plan;
use crate::txn::TxId;

/// Hands `work` the batches of each morsel of the pipeline `plan`, each
/// with the rows its steps kept, on up to `degree` threads, its scans
/// told the runtime filters `told`; given way, no morsel read, if its
/// morsels are not read by batch in `layout`, the columns its plan's
/// batches hold and its rows' width.
///
/// # Errors
///
/// As splitting the pipeline, building a hash join's operand, reading a
/// morsel or `work` fails.
pub(in crate::exec) fn each_morsel<F>(
    plan: &Plan,
    (db, tx): (&Db, TxId),
    (degree, told, layout): (usize, &[Told], &(Vec<usize>, usize)),
    work: F,
) -> Result<Held<()>>
where
    F: Fn(Batches) -> Result<()> + Send + Sync + 'static,
{
    let limit = Limit::of(db);
    let Held::Kept(pipeline) = Pipeline::told(plan, (db, tx), (&limit, told))? else { return Ok(Held::GaveWay) };
    if pipeline.batched.as_ref().is_none_or(|by| (&by.layout, by.width) != (&layout.0, layout.1)) {
        return Ok(Held::GaveWay);
    }
    let (pipeline, db_for) = (Arc::new(pipeline), db.clone());
    let count = pipeline.count();
    let each = Box::new(move |m: usize| work(pipeline.morsel_kept(m, (&db_for, tx))?));
    Morsels::start(db, tx, (count, degree), each).collect::<Result<()>>()?;
    Ok(Held::Kept(()))
}
