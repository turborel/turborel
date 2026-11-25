//! \[DESIGN\] (RULES.md D1115) The left operand of a semi join or an
//! antijoin that builds its left (`batch/joined/semi/marked.rs`), a
//! pipeline run in parallel, held by the threads that read it. Its
//! batches were each gathered into one of the rows they kept, then
//! joined into one batch, hashed and placed in its table by the
//! statement's thread alone, while the workers that read it waited. Here
//! each morsel's kept rows are gathered into one batch by the thread that
//! read them, the morsels' batches are given in the order of their
//! numbers, so the rows are held in the order the part alone gives them,
//! and they are put in their table by those threads too
//! (`parallel/build.rs`, RULES.md D1080).
//!
//! The rows held are charged as the statement's thread takes each
//! morsel's batch, as the serial reading charges its batches. Past the
//! work area the batches taken so far are given on, then the rest as
//! their morsels give them, so the join that builds its right reads every
//! row of the left in order, once. The first morsel that fails, in the
//! order of their numbers, is the error raised, after the rows before it
//! are taken (RULES.md D15). A pipeline whose morsels are not read by
//! batch, or whose batches hold other columns than the plan's, gives way
//! before any morsel is read.

use std::sync::Arc;

use super::build::{Crew, shared};
use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, Kept, SharedTable, TABLE_ROW_BYTES, Told};
use crate::physical::{JoinKey, Plan};
use crate::storage::Batch;
use crate::txn::TxId;

/// A left operand read by its morsels: held in its table, or, past the
/// work area, its batches, those read first.
pub(in crate::exec) enum Left {
    Held(Box<SharedTable>),
    Rest(Batches),
}

/// The pipeline `plan`, read by its morsels on up to `degree` threads,
/// its scans told the runtime filters `told`, its batches holding the
/// columns and rows' width of `layout`: held in a table on `keys` while
/// its rows hold no more than `limit` bytes, a key's right column the
/// held column it is hashed on. Given way, no morsel read, as the module
/// says.
///
/// # Errors
///
/// As splitting the pipeline, reading a morsel or building the table
/// fails.
pub(in crate::exec) fn held_left(
    plan: &Plan,
    (db, tx): (&Db, TxId),
    (degree, told, layout): (usize, &[Told], &(Vec<usize>, usize)),
    (keys, limit): (&[JoinKey], usize),
) -> Result<Held<Left>> {
    // No probe of the join that told them follows in a morsel, so each
    // morsel judges them whole right after its scans (RULES.md D1063).
    let above: Vec<Told> = told.iter().cloned().map(Told::given_above).collect();
    let Held::Kept(pipeline) = Pipeline::told(plan, (db, tx), &Limit::of(db), &above)? else { return Ok(Held::GaveWay) };
    if pipeline.batched.as_ref().is_none_or(|by| (&by.layout, by.width) != (&layout.0, layout.1)) {
        return Ok(Held::GaveWay);
    }
    let (vectors, count) = (layout.0.len(), pipeline.count());
    let (pipeline, db_for) = (Arc::new(pipeline), db.clone());
    let work = Box::new(move |m: usize| -> Result<Batch> {
        let parts = pipeline.morsel_kept(m, (&db_for, tx))?.map(|kept| {
            kept.map(|(b, rows)| {
                let places = crate::storage::sel::places_of(rows, b.rows);
                (b, places)
            })
        });
        let parts = parts.collect::<Result<Vec<_>>>()?;
        Ok(if parts.is_empty() { Batch::empty(vectors) } else { Batch::joined(parts) })
    });
    let mut morsels = Morsels::start(db, tx, (count, degree), work);
    let (mut parts, mut bytes) = (Vec::new(), 0_usize);
    while let Some(batch) = morsels.next() {
        let batch = batch?;
        bytes = bytes.saturating_add(batch.bytes()).saturating_add(batch.rows.saturating_mul(TABLE_ROW_BYTES));
        parts.push(batch);
        if bytes > limit {
            let read: Vec<Result<Kept>> = parts.into_iter().map(|part| Ok((part, None))).collect();
            let rest = morsels.map(|taken| taken.map(|b| (b, None)));
            return Ok(Held::Kept(Left::Rest(Box::new(read.into_iter().chain(rest)))));
        }
    }
    // A morsel of no rows may hold no vectors; of none at all, one batch
    // of no row holds the layout's.
    parts.retain(|b| b.rows > 0);
    if parts.is_empty() {
        parts.push(Batch::empty(vectors));
    }
    let table = shared(parts, (layout.0.clone(), layout.1), keys, Crew { db, tx, degree })?;
    Ok(Held::Kept(Left::Held(Box::new(table))))
}
