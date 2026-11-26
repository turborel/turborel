//! \[DESIGN\] (RULES.md D1115) The left operand of a semi join or an
//! antijoin that builds its left (`batch/joined/semi/marked.rs`), a
//! pipeline run in parallel, held by the threads that read it. Its
//! batches were each gathered into one of the rows they kept, then
//! joined into one batch, hashed and placed in its table by the
//! statement's thread alone, while the workers that read it waited. Here
//! each morsel's kept rows are gathered into one batch by the thread that
//! read them, the morsels' batches are taken in the order of their
//! numbers, so the rows are held in the order the part alone gives them,
//! and they are put in their table by those threads too
//! (`parallel/build.rs`, RULES.md D1080).
//!
//! A morsel's rows are charged to the join's work grant as they are
//! gathered (`parallel/charged.rs`, RULES.md D1150). Once a morsel
//! outgrows the grant, or the rows taken are on course to, the batches
//! taken are given on, then the rest in order, a morsel past the grant
//! read again by the statement's thread a batch at a time, so the join
//! that builds its right reads every row of the left once. The first
//! morsel that fails, in the order of their numbers, is the error raised,
//! after the rows before it are taken (RULES.md D15). A pipeline whose
//! morsels are not read by batch, or whose batches hold other columns
//! than the plan's, gives way before any morsel is read.

use std::sync::Arc;

use super::build::{Crew, shared};
use super::charged::{Taken, Tally, charged_batches};
use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, Kept, SharedTable, TABLE_ROW_BYTES, Told};
use crate::physical::{JoinKey, Plan};
use crate::spill::Grant;
use crate::storage::Batch;
use crate::txn::TxId;

/// A left operand read by its morsels: held in its table, or, past the
/// work area, its batches, those read first.
pub(in crate::exec) enum Left {
    Held(Box<SharedTable>),
    Rest(Batches),
}

/// The morsels of a left operand not yet taken, each by its number.
type Taking = Box<dyn Iterator<Item = (usize, Result<Taken<Batch>>)> + Send>;

/// The pipeline `plan`, read by its morsels on up to `degree` threads,
/// its scans told the runtime filters `told`, its batches holding the
/// columns and rows' width of `layout`: held in a table on `keys` while
/// its rows fit `grant`, a key's right column the held column it is
/// hashed on. Given way, no morsel read, as the module says.
///
/// # Errors
///
/// As splitting the pipeline, reading a morsel or building the table
/// fails.
pub(in crate::exec) fn held_left(
    plan: &Plan,
    (db, tx): (&Db, TxId),
    (degree, told, layout): (usize, &[Told], &(Vec<usize>, usize)),
    (keys, grant): (&[JoinKey], Arc<Grant>),
) -> Result<Held<Left>> {
    let limit = Limit::of_grant(grant);
    // No probe of the join that told them follows in a morsel, so each
    // morsel judges them whole right after its scans (RULES.md D1063).
    let above: Vec<Told> = told.iter().cloned().map(Told::given_above).collect();
    let Held::Kept(pipeline) = Pipeline::told(plan, (db, tx), &limit, &above)? else { return Ok(Held::GaveWay) };
    if pipeline.batched.as_ref().is_none_or(|by| (&by.layout, by.width) != (&layout.0, layout.1)) {
        return Ok(Held::GaveWay);
    }
    let (vectors, count) = (layout.0.len(), pipeline.count());
    let (pipeline, db_for, within) = (Arc::new(pipeline), db.clone(), limit.clone());
    let read = Arc::clone(&pipeline);
    let work = Box::new(move |m: usize| gathered(&read, m, (&db_for, tx), (vectors, &within)));
    let mut morsels: Taking = Box::new((0_usize..).zip(Morsels::start(db, tx, (count, degree), work)));
    let (mut parts, mut bytes) = (Vec::new(), 0_usize);
    while let Some((m, taken)) = morsels.next() {
        let Taken::Whole(batch, held) = taken? else {
            limit.give(bytes);
            return Ok(Held::Kept(Left::Rest(rest(parts, (Some(m), morsels), (&pipeline, &limit), (db, tx)))));
        };
        parts.push(batch);
        bytes = bytes.saturating_add(held);
        if !limit.on_course((0, bytes), (m.saturating_add(1), count)) {
            limit.give(bytes);
            return Ok(Held::Kept(Left::Rest(rest(parts, (None, morsels), (&pipeline, &limit), (db, tx)))));
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

/// Morsel `m` of `pipeline` gathered into a batch of `vectors` vectors,
/// charged to `limit` as it grows, then what it holds with its table's
/// share; nothing if it outgrows the grant.
///
/// # Errors
///
/// As reading the morsel fails.
fn gathered(pipeline: &Pipeline, m: usize, (db, tx): (&Db, TxId), (vectors, limit): (usize, &Limit)) -> Result<Taken<Batch>> {
    let tally = Tally::of(limit);
    let parts = charged_batches(pipeline.morsel_kept(m, (db, tx))?, &tally).map(|kept| {
        kept.map(|(b, rows)| {
            let places = crate::storage::sel::places_of(rows, b.rows);
            (b, places)
        })
    });
    let parts = parts.collect::<Result<Vec<_>>>()?;
    let batch = if parts.is_empty() || tally.past() { Batch::empty(vectors) } else { Batch::joined(parts) };
    tally.settle(batch.bytes().saturating_add(batch.rows.saturating_mul(TABLE_ROW_BYTES)));
    Ok(tally.taken(batch))
}

/// The batches `parts`, taken, then of the morsel `past` past the grant,
/// if any, then of those left in `morsels`, in order: a morsel past the
/// grant read again of `pipeline` by the statement's thread, a batch at a
/// time, and each other's bytes given back to `limit` as it is given on,
/// as what reads them holds them.
fn rest(
    parts: Vec<Batch>,
    (past, morsels): (Option<usize>, Taking),
    (pipeline, limit): (&Arc<Pipeline>, &Limit),
    (db, tx): (&Db, TxId),
) -> Batches {
    let (pipeline, db, limit) = (Arc::clone(pipeline), db.clone(), limit.clone());
    let again = move |m: usize| -> Batches {
        match pipeline.morsel_kept(m, (&db, tx)) {
            Ok(batches) => batches,
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    };
    let given: Vec<Result<Kept>> = parts.into_iter().map(|part| Ok((part, None))).collect();
    let past = past.map(|m| (m, Ok(Taken::Past)));
    let left = past.into_iter().chain(morsels).flat_map(move |(m, taken)| -> Batches {
        match taken {
            Ok(Taken::Whole(batch, held)) => {
                limit.give(held);
                Box::new(std::iter::once(Ok((batch, None))))
            }
            Ok(Taken::Past) => again(m),
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    });
    Box::new(given.into_iter().chain(left))
}
