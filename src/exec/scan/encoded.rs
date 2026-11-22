//! \[DESIGN\] (RULES.md D821) The rows of an index scan whose entries come
//! with their keys as the index holds them (`Index::search_encoded`). Each
//! entry's row is read and its key columns encoded and compared with the
//! entry's bytes (`entries::fetch_encoded`), so no key is decoded but of a
//! row whose bytes differ: an entry of a version its reader does not see
//! (`Index`). It reads, records its read and counts as `indexed` does.

use super::{Probed, read_row};
use crate::exec::Db;
use crate::exec::entries::{Reading, fetch_encoded};
use crate::exec::{RowIter, failed};
use crate::index::{Encoded, used};
use crate::storage::RowId;
use crate::txn::{TxId, checked, counted};

/// The rows of the entries `found` of the index `probed` reads, of the
/// columns `needed` alone, each followed by its id if `located`.
pub(super) fn rows(probed: Probed, found: Encoded, (needed, located): (&[usize], bool), db: &Db, tx: TxId) -> RowIter {
    let Probed { storage, ix, uses, read, .. } = probed;
    if let Err(e) = read.record(&*storage, tx) {
        return failed(e);
    }
    let Encoded { entries, types } = found;
    let mut reading = Reading::of(&ix, Some(needed), None);
    let rows = counted(used(entries, uses), db.tx.stop_of(tx), |_| 1).filter_map(move |entry| {
        let row = |(key, id): (Vec<u8>, RowId)| {
            let row = fetch_encoded(&*storage, &ix, &mut reading, (tx, id), (&key, &types))?;
            Ok(row.map(|row| read_row(located, (id, row))))
        };
        entry.and_then(row).transpose()
    });
    Box::new(checked(rows, db.tx.stop_of(tx)))
}

// Unit tests: rows of an index's keys as it holds them, of versions
// seen and not (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
