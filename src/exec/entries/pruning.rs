//! \[DESIGN\] Index entries of rows gone for every reader, taken back by
//! the scan that meets them (RULES.md D837). A row whose newest version is
//! a deletion committed at or before the horizon is seen by no snapshot
//! open or to come, so each of its index entries is garbage, which its
//! collection would take back by the same call (`Index::remove`,
//! `txn/end.rs`). A queue's deletions pile up at the low end of its key's
//! range, where every search for its first entry begins, so a scan that
//! takes back what it passes leaves the next a shorter way, however far
//! collection is behind. Collection, finding an entry gone, has nothing
//! to take back, and recovery and undo are as they were: no change of an
//! open transaction is taken back, and nothing a crash could lose is
//! logged, as collection logs nothing.

use std::sync::Arc;

use crate::engine::key::decode_key;
use crate::index::{Index, Key};
use crate::storage::RowId;
use crate::txn::{Timestamp, Transactions};
use crate::value::DataType;

/// What a scan of an index needs to take back the entries of rows gone
/// for every reader: the index, and the horizon, read again only when a
/// deletion is later than the one last read.
pub(in crate::exec) struct Pruning {
    index: Arc<dyn Index>,
    tx: Arc<Transactions>,
    horizon: Timestamp,
}

impl Pruning {
    /// Of a scan of `index`, under the transactions `tx`.
    pub(in crate::exec) fn of(index: &Arc<dyn Index>, tx: &Arc<Transactions>) -> Pruning {
        Pruning { index: Arc::clone(index), tx: Arc::clone(tx), horizon: 0 }
    }

    /// Takes back the entry of row `id` under `key` if the read that found
    /// no row found it `gone`, a deletion committed at or before the
    /// horizon: whether it did. A snapshot taken later is never before the
    /// horizon, so one read before stays true, and a horizon read lower,
    /// as one is while a transaction begins (`txn/clock.rs`), is not kept.
    pub(in crate::exec) fn prune(&mut self, gone: Option<Timestamp>, key: &Key, id: RowId) -> bool {
        let Some(at) = gone else { return false };
        if at > self.horizon {
            self.horizon = self.horizon.max(self.tx.horizon());
        }
        if at > self.horizon {
            return false;
        }
        // An entry that cannot be taken back now stays collection's, as
        // every entry no scan meets is.
        self.index.remove(key, id).is_ok()
    }

    /// As `prune`, of an entry whose key is the bytes `key` of columns of
    /// `types` (`entries::fetch_encoded`), decoded only if the row is gone.
    /// A key that cannot be decoded is left, the entry collection's.
    pub(in crate::exec) fn prune_encoded(&mut self, gone: Option<Timestamp>, (key, types): (&[u8], &[DataType]), id: RowId) -> bool {
        if gone.is_none() {
            return false;
        }
        let Ok(values) = decode_key(key, types) else { return false };
        self.prune(gone, &Key(values), id)
    }
}

#[cfg(test)]
mod tests;
