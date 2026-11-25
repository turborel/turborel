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
//! read on by the statement's thread from where its own thread left it,
//! as an index's part is read once, so the join that builds its right
//! reads every row of the left once. The first
//! morsel that fails, in the order of their numbers, is the error raised,
//! after the rows before it are taken (RULES.md D15). A pipeline whose
//! morsels are not read by batch, or whose batches hold other columns
//! than the plan's, gives way before any morsel is read.

use std::sync::Arc;

use super::build::{Crew, shared};
use super::charged::{Gathered, held_batches};
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

/// A morsel of a left operand, as its thread gathered it: its kept rows
/// joined into one batch, with the bytes it holds of the grant; or, past
/// the grant, the batches it gathered and the rest of its read.
enum Morsel {
    Whole(Batch, usize),
    Past(Vec<Kept>, Batches),
}

/// The morsels of a left operand not yet taken, in order.
type Taking = Morsels<Morsel>;

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
    let (db_for, within) = (db.clone(), limit.clone());
    let work = Box::new(move |m: usize| gathered(pipeline.morsel_kept(m, (&db_for, tx))?, (vectors, &within)));
    let mut morsels: Taking = Morsels::start(db, tx, (count, degree), work);
    let (mut parts, mut bytes, mut done) = (Vec::new(), 0_usize, 0_usize);
    while let Some(taken) = morsels.next() {
        let (batch, held) = match taken? {
            Morsel::Whole(batch, held) => (batch, held),
            Morsel::Past(items, read) => {
                limit.give(bytes);
                return Ok(Held::Kept(Left::Rest(rest(parts, (items.into_iter().map(Ok).chain(read), morsels), &limit))));
            }
        };
        parts.push(batch);
        (bytes, done) = (bytes.saturating_add(held), done.saturating_add(1));
        if !limit.on_course(0, (done, count)) {
            limit.give(bytes);
            return Ok(Held::Kept(Left::Rest(rest(parts, (std::iter::empty(), morsels), &limit))));
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

/// A morsel's batches `batches` gathered within `limit` into one batch of
/// `vectors` vectors, charged then what it holds with its table's share;
/// or, past the grant, as they were gathered, with the rest of the read.
///
/// # Errors
///
/// As reading the morsel fails.
fn gathered(batches: Batches, (vectors, limit): (usize, &Limit)) -> Result<Morsel> {
    let (items, charged) = match held_batches(batches, limit)? {
        Gathered::Whole(items, charged) => (items, charged),
        Gathered::Past(items, read) => return Ok(Morsel::Past(items, read)),
    };
    let parts: Vec<_> = items.into_iter().map(|(b, rows)| (crate::storage::sel::places_of(rows, b.rows), b)).map(|(p, b)| (b, p)).collect();
    let batch = if parts.is_empty() { Batch::empty(vectors) } else { Batch::joined(parts) };
    // What the batch holds, its table's share with it, in place of what
    // its batches held as they were gathered.
    let held = batch.bytes().saturating_add(batch.rows.saturating_mul(TABLE_ROW_BYTES));
    limit.give(charged);
    let _ = limit.take(held);
    Ok(Morsel::Whole(batch, held))
}

/// The batches `parts`, taken, then those of the morsel `past` the grant
/// left read, then of those left in `morsels`, in order, a morsel past
/// the grant read on from where its thread left it, so none is read
/// twice, as an index's part is read once; each other's bytes given back
/// to `limit` as it is given on, as what reads them holds them.
fn rest<P>(parts: Vec<Batch>, (past, morsels): (P, Taking), limit: &Limit) -> Batches
where
    P: Iterator<Item = Result<Kept>> + Send + 'static,
{
    let limit = limit.clone();
    let given: Vec<Result<Kept>> = parts.into_iter().map(|part| Ok((part, None))).collect();
    let left = morsels.flat_map(move |taken| -> Batches {
        match taken {
            Ok(Morsel::Whole(batch, held)) => {
                limit.give(held);
                Box::new(std::iter::once(Ok((batch, None))))
            }
            Ok(Morsel::Past(items, read)) => Box::new(items.into_iter().map(Ok).chain(read)),
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    });
    Box::new(given.into_iter().chain(past).chain(left))
}
