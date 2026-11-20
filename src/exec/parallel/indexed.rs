//! \[DESIGN\] An index scan run by morsels (RULES.md D297): the range of
//! keys its probe reads cut at keys of the index's own (`Index::ranges`),
//! a B-link tree's at its inner nodes' keys, each part's entries a morsel,
//! each entry's row read as the scan alone reads it (`exec/scan.rs`). The
//! parts are in key order, and each morsel's rows in its part's, so the
//! morsels in order give the rows in the order the scan gives them. The
//! read is recorded once for the whole scan, as the scan records it.

use std::sync::{Arc, Mutex, PoisonError};

use crate::catalog::{IndexDesc, KeyShape, QualifiedName};
use crate::db::State;
use crate::error::Result;
use crate::exec::broken;
use crate::exec::entries::{Reading, entry_row};
use crate::exec::read::Asked;
use crate::index::{Entries, KeyProjection, Probe};
use crate::storage::{Rows, Split, TableStorage};
use crate::txn::TxId;

/// An index scan's parts, each a morsel.
struct IndexSplit {
    storage: Arc<dyn TableStorage>,
    ix: Arc<KeyShape>,
    /// The table's width, of a scan covering the columns it reads.
    width: Option<usize>,
    columns: Vec<usize>,
    projection: Option<Arc<dyn KeyProjection>>,
    tx: TxId,
    /// Each part's entries, taken by the morsel that reads it.
    parts: Vec<Mutex<Option<Entries>>>,
}

/// What an index scan reads: its table, index, probe, the columns it
/// reads and whether it covers them.
pub(super) struct Scanned<'a> {
    pub(super) table: &'a QualifiedName,
    pub(super) index: &'a str,
    pub(super) probe: &'a Probe,
    pub(super) columns: &'a [usize],
    pub(super) covering: bool,
}

/// The scan `scanned` split for `tx` into parts of about `rows` rows of
/// the `est` it is estimated to read, its read recorded; none if its index
/// cannot cut its range.
///
/// # Errors
///
/// As finding the index, or recording the read, fails.
pub(super) fn split(db: &State, tx: TxId, scanned: &Scanned<'_>, (est, rows): (usize, usize)) -> Result<Option<Arc<dyn Split>>> {
    let Scanned { table, index, probe, columns, covering } = *scanned;
    let storage: Arc<dyn TableStorage> = if crate::catalog::is_global_index(index) {
        Arc::new(crate::db::partitions::dispatch::Dispatched::of(db, table))
    } else {
        Arc::clone(db.storage_of(table)?)
    };
    let ix = db.catalog.index(index).map(IndexDesc::key_shape).ok_or_else(|| broken("an index scan run in parallel has no index"))?;
    let found = db.index_of(index)?;
    let Some(parts) = Arc::clone(found).ranges(probe, est.div_ceil(rows.max(1)).max(1)) else { return Ok(None) };
    Asked::of(&**found, &ix, probe).record(&*storage, tx)?;
    let width = covering.then(|| db.catalog.table(table).map(|t| t.columns.len())).flatten();
    let projection = found.key_projection();
    let parts = parts.into_iter().map(|p| Mutex::new(Some(p))).collect();
    Ok(Some(Arc::new(IndexSplit { storage, ix: Arc::new(ix), width, columns: columns.to_vec(), projection, tx, parts })))
}

impl Split for IndexSplit {
    fn morsels(&self) -> usize {
        self.parts.len()
    }

    fn morsel(&self, n: usize) -> Rows {
        let taken = self.parts.get(n).and_then(|p| p.lock().unwrap_or_else(PoisonError::into_inner).take());
        let Some(entries) = taken else { return Box::new(std::iter::once(Err(broken("a part of an index scan read twice")))) };
        let (storage, ix, width, tx) = (Arc::clone(&self.storage), Arc::clone(&self.ix), self.width, self.tx);
        let mut reading = Reading::of(&ix, Some(&self.columns), self.projection.clone());
        Box::new(entries.filter_map(move |entry| {
            let read = |(key, id)| Ok(entry_row(&*storage, (&ix, width), &mut reading, (tx, id), key)?.map(|row| (id, row)));
            entry.and_then(read).transpose()
        }))
    }
}
