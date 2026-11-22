//! \[DESIGN\] (RULES.md D821) The rows of an index scan whose entries come
//! with their keys as the index holds them (`Index::search_encoded`). Each
//! entry's row is read and its key columns encoded and compared with the
//! entry's bytes (`entries::fetch_encoded`), so no key is decoded but of a
//! row whose bytes differ: an entry of a version its reader does not see
//! (`Index`). It reads, records its read and counts as `indexed` does.

use super::{Probed, read_row};
use crate::exec::entries::{Reading, fetch_encoded};
use crate::exec::{Db, RowIter, failed};
use crate::index::{Encoded, EncodedEntry, Using};
use crate::txn::{TxId, checked, counted};

/// The rows of the entries `found` of the index `probed` reads, of the
/// columns `needed` alone, each followed by its id if `located`. Each
/// entry is lent by the search and compared before the next is asked for,
/// and counted as the index's (RULES.md D522) and as a read that a cancel
/// stops (D44).
pub(super) fn rows(probed: Probed, mut found: Encoded, (needed, located): (&[usize], bool), db: &Db, tx: TxId) -> RowIter {
    let Probed { storage, ix, uses, read, .. } = probed;
    if let Err(e) = read.record(&*storage, tx) {
        return failed(e);
    }
    let mut reading = Reading::of(&ix, Some(needed), None);
    let mut using = Using::of(uses);
    using.searched();
    let entries = std::iter::from_fn(move || {
        let Some(entry) = found.next_encoded() else {
            using.flush();
            return None;
        };
        let EncodedEntry { key, row: id, types } = match entry {
            Ok(entry) => entry,
            Err(e) => return Some(Err(e)),
        };
        using.gave(1);
        let row = fetch_encoded(&*storage, &ix, &mut reading, (tx, id), (key, types));
        Some(row.map(|row| row.map(|row| read_row(located, (id, row)))))
    });
    let rows = counted(entries, db.tx.stop_of(tx), |_| 1).filter_map(Result::transpose);
    Box::new(checked(rows, db.tx.stop_of(tx)))
}

// Unit tests: rows of an index's keys as it holds them, of versions
// seen and not (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
