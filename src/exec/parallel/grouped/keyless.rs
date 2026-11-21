//! \[DESIGN\] (RULES.md D293, D680) A grouping of no grouping columns run
//! in parallel: one group, of every row, so a morsel's rows are fed
//! straight to set functions of its own, as the serial plain aggregation
//! feeds them (`exec/batch.rs`), a vector at a time if its pipeline is
//! read by batch, with no table of groups, no key and no hash. Each morsel
//! writes its states (`Accumulator::write_state`), and the statement's
//! thread merges them as they come, in morsel order, so each set function
//! takes its values in the order it would alone.

use std::sync::Arc;

use super::super::pipeline::Pipeline;
use super::super::run::Morsels;
use super::{Grouping, feed, finished, fresh, written};
use crate::error::Result;
use crate::exec::batch::fed;
use crate::exec::{Db, Runner};
use crate::functions::Accumulator;
use crate::storage::{Row, Sel};
use crate::txn::TxId;

/// [X3.135-1986 §5.23 GR1] The one group of `grouping`'s pipeline, of no
/// grouping columns, its set functions fed by morsels on up to `degree`
/// threads and merged in morsel order: its row of their results, even of
/// no rows.
///
/// # Errors
///
/// As reading the pipeline, or a set function, fails.
pub(super) fn keyless(pipeline: Pipeline, grouping: &Arc<Grouping>, (db, tx, degree): (&Db, TxId, usize)) -> Result<Row> {
    let (pipeline, db_for, making) = (Arc::new(pipeline), db.clone(), Arc::clone(grouping));
    let count = pipeline.count();
    let work = Box::new(move |m: usize| {
        let runner = Runner::new(&db_for, tx);
        let mut states = fresh(&making.spec.aggs);
        fed_morsel((&pipeline, &making), (&db_for, tx, m), &runner, &mut states)?;
        Ok(states.iter().map(|s| written(s.as_ref())).collect::<Vec<_>>())
    });
    let mut states = fresh(&grouping.spec.aggs);
    for morsel in Morsels::start(db, tx, (count, degree), work) {
        for (acc, state) in states.iter_mut().zip(&morsel?) {
            acc.merge(state)?;
        }
    }
    finished(states, Vec::new())
}

/// Feeds `states` the rows of morsel `m` of `pipeline`, read for the
/// statement of `tx`, by batch if `grouping` reads it so, else a row at a
/// time, their warnings heard by `runner`.
fn fed_morsel(
    (pipeline, grouping): (&Pipeline, &Grouping),
    (db, tx, m): (&Db, TxId, usize),
    runner: &Runner,
    states: &mut [Box<dyn Accumulator>],
) -> Result<()> {
    let Some(read) = &grouping.read else {
        for row in pipeline.morsel(m, runner)? {
            feed(&grouping.spec.aggs, states, &row, runner)?;
        }
        return Ok(());
    };
    for kept in pipeline.morsel_kept(m, (db, tx))? {
        let (batch, rows) = kept?;
        fed(&batch, (&read.taken, runner), rows.as_ref().map(Sel::as_slice), states)?;
    }
    Ok(())
}
