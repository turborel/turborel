//! \[DESIGN\] Throwaway experiment, branch `storagebound`, never merged: an
//! index probe read row by row as a scan of the executor reads it, for
//! the bench's procedures written against the storage API (`db/native.rs`).

#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction, missing_docs)]

use super::entries::{Reading, fetch};
use super::scan::{Found, probe_index};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::index::Probe;
use crate::storage::{Row, RowId};
use crate::txn::{TxId, counted};

/// Each row `tx` sees under `probe` of `index` of `table`, in key order or
/// reversed, given to `each` until it says to stop. The probe's read is
/// recorded and each entry's row is read and checked against its key as
/// the index scan does.
pub(crate) fn probe_rows(
    st: &State,
    tx: TxId,
    (table, index): (&QualifiedName, &str),
    (probe, reverse): (&Probe, bool),
    each: &mut dyn FnMut(RowId, Row) -> Result<bool>,
) -> Result<()> {
    let Found { storage, ix, entries, projection, read, pruning } = probe_index(st, table, index, probe, (reverse, true))?;
    read.record(&*storage, tx)?;
    let mut reading = Reading::of(&ix, None, projection).pruned(pruning);
    for entry in counted(entries, st.tx.stop_of(tx), |_| 1) {
        let (key, id) = entry?;
        if let Some((id, row)) = fetch(&*storage, &ix, &mut reading, (tx, id), &key)? {
            if !each(id, row)? {
                break;
            }
        }
    }
    Ok(())
}
