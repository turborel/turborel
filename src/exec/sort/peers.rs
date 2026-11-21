//! \[DESIGN\] RULES.md D560: a sort's batches given with the normal keys of
//! their rows (`normal.rs`), so a window over the sort finds where its
//! partitions and peer groups begin by comparing each row's key bytes with
//! the one's before it (`Kernels::boundaries`), no value compared. Every
//! batch of a sort held in memory shares one layout, so the last row of a
//! batch is compared with the first of the next by bytes too; of a sort
//! past its work area, or of rows not sorted here, each batch's keys are
//! written of it alone, in a layout of its own.

use std::cmp::Ordering;
use std::sync::Arc;

use super::keyed::{made_of, sorted_places};
use super::out::{Dictionaries, Placed, picked};
use super::vectors::{Held, compare_cells};
use super::{Work, looked_every};
use crate::db::State;
use crate::error::Result;
use crate::exec::batch::Batches;
use crate::kernels::Kernels;
use crate::logical::SortKey;
use crate::storage::{BATCH_ROWS, Batch};
use crate::txn::{Stop, TxId};

/// The normal keys of a batch's rows, one after another.
pub(in crate::exec) struct RunKeys {
    /// The keys, `width` bytes each.
    pub(in crate::exec) bytes: Vec<u8>,
    /// The bytes of a key.
    pub(in crate::exec) width: usize,
    /// Where each key's field ends in a row's key, in order.
    pub(in crate::exec) ends: Vec<usize>,
    /// Whether rows of equal bytes may differ, a string longer than its
    /// field holds.
    pub(in crate::exec) truncates: bool,
    /// Whether every batch of its stream is of one layout, so keys of two
    /// batches compare by bytes.
    pub(in crate::exec) shared: bool,
}

/// A sort's batches, each with its rows' keys if they have a normal form.
pub(in crate::exec) type KeyedBatches = Box<dyn Iterator<Item = Result<(Batch, Option<RunKeys>)>> + Send>;

/// `batch`, given back, and the keys of its rows under `keys`, in a layout
/// of its own, written on `kernels`; none if they have no normal form.
pub(in crate::exec) fn keys_of(keys: &[SortKey], batch: Batch, kernels: &dyn Kernels) -> (Batch, Option<RunKeys>) {
    let rows: Vec<usize> = (0..batch.rows).collect();
    let held = [(batch, rows)];
    let made = made_of(keys, &held, None, kernels);
    let [(given, _)] = held;
    let run = made.map(|m| {
        let ends = m.layout.fields.iter().map(|f| f.at.saturating_add(f.width())).collect();
        RunKeys { bytes: m.bytes, width: m.layout.width, ends, truncates: m.layout.truncates, shared: false }
    });
    (given, run)
}

/// [ISO/IEC 9075-2:2023 §10.10 GR1] Whether row `ra` of `a` and row `rb`
/// of `b` are alike under `keys`: of one partition, or peers.
pub(in crate::exec) fn alike_cells(keys: &[SortKey], (a, ra): (&Batch, usize), (b, rb): (&Batch, usize)) -> bool {
    compare_cells(keys, (a, ra), (b, rb)) == Ordering::Equal
}

/// \[DESIGN\] (RULES.md D560) The rows of `input`, a stream's batches each
/// holding every column of its rows, `width` wide, sorted by `keys` as
/// `sorted_out` sorts them, each batch given with its rows' keys: those the
/// sort made, of a sort held in memory; else none, its rows merged from
/// runs.
///
/// # Errors
///
/// As `sorted_out`.
pub(in crate::exec) fn sorted_keyed(keys: &[SortKey], input: Batches, width: usize, db: &State, tx: TxId) -> Result<KeyedBatches> {
    let mut work = Work::of(db, tx);
    match super::vectors::held_or_runs(keys, input, Held::on(db.functions.kernels_shared()), &mut work)? {
        super::vectors::Sorted::Held(held) => {
            work.look()?;
            Ok(keyed_batches(held, keys, width, work.stop()))
        }
        super::vectors::Sorted::Runs(runs) => {
            let rows = super::merged(keys, runs, &mut work)?;
            let batches = crate::storage::batches_of(Box::new(rows.map(|r| r.map(|row| (0, row)))), &(0..width).collect::<Vec<_>>());
            Ok(Box::new(batches.map(|b| b.map(|b| (b, None)))))
        }
    }
}

/// The rows of `held`, sorted by `keys`, as `Held::into_batches` gives
/// them, each batch beside the keys the sort made of its rows.
fn keyed_batches(held: Held, keys: &[SortKey], width: usize, stop: Option<Arc<Stop>>) -> KeyedBatches {
    let (batches, places, kernels) = held.parts();
    let (order, made) = sorted_places(keys, &batches, &places, None, kernels.as_ref());
    let dictionaries = Dictionaries::of(&batches, width);
    let chunks: Vec<Vec<u32>> = order.chunks(BATCH_ROWS).map(<[u32]>::to_vec).collect();
    Box::new(looked_every(
        chunks.into_iter().map(move |chunk| {
            let picks: Vec<Placed> = chunk.iter().filter_map(|&at| places.get(usize::try_from(at).ok()?).copied()).collect();
            let batch = picked(&batches, &picks, width, &dictionaries);
            let run = made.as_ref().map(|m| {
                let bytes = chunk.iter().flat_map(|&at| m.key(at).iter().copied()).collect();
                let ends = m.layout.fields.iter().map(|f| f.at.saturating_add(f.width())).collect();
                RunKeys { bytes, width: m.layout.width, ends, truncates: m.layout.truncates, shared: true }
            });
            Ok((batch, run))
        }),
        (stop, 1),
    ))
}
