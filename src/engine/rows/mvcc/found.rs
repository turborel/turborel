//! \[DESIGN\] A transaction's bookkeeping in a table as its own thread
//! keeps it (RULES.md D871). A transaction runs on its session's thread,
//! and its reads and writes of a table each need its bookkeeping there,
//! which the table keeps in a map every transaction of the table shares
//! (`opens.rs`). So the thread keeps the bookkeeping it last found, of a
//! few tables, and finds it there again with no lock and no hash. A
//! transaction that moves to another thread finds it in the map, as
//! before, and that thread keeps it from then on.
//!
//! What is kept names its table by the table's token, an allocation of
//! its own that the thread holds too, so no other table is ever taken for
//! it, though one be made where a table dropped was. Bookkeeping whose
//! transaction has ended there is never given, whichever thread ended it
//! (`Opened::ended`), and goes as the next is kept.

use std::cell::RefCell;
use std::sync::Arc;

use super::opens::Opened;
use crate::txn::TxId;

/// \[DESIGN\] The tables a thread keeps bookkeeping of. A transaction of
/// order entry touches nine, and one more than is kept is found in the
/// map, as all were.
const KEPT: usize = 16;

/// The bookkeeping of `tx` in the table of `table`.
struct Found {
    tx: TxId,
    table: Arc<()>,
    opened: Arc<Opened>,
}

thread_local! {
    /// What this thread last found, the oldest first.
    static FOUND: RefCell<Vec<Found>> = const { RefCell::new(Vec::new()) };
}

/// The bookkeeping of `tx` in the table whose token is `table`, if this
/// thread keeps it and the transaction has not ended there.
pub(super) fn kept(tx: TxId, table: &Arc<()>) -> Option<Arc<Opened>> {
    let kept = FOUND.try_with(|found| {
        let found = found.borrow();
        let mine = found.iter().find(|f| f.tx == tx && Arc::ptr_eq(&f.table, table))?;
        (!mine.opened.ended()).then(|| Arc::clone(&mine.opened))
    });
    // A thread ending keeps nothing more, and finds it in the map.
    kept.ok().flatten()
}

/// Keeps `opened`, the bookkeeping of `tx` in the table whose token is
/// `table`, for this thread's next reads and writes. What it kept of
/// transactions ended goes, and the oldest if it keeps `KEPT`.
pub(super) fn keep(tx: TxId, table: &Arc<()>, opened: &Arc<Opened>) {
    let gone = FOUND.try_with(|found| {
        let mut found = found.borrow_mut();
        let mut gone: Vec<Found> = found.extract_if(.., |f| f.opened.ended()).collect();
        if found.len() >= KEPT {
            gone.push(found.remove(0));
        }
        found.push(Found { tx, table: Arc::clone(table), opened: Arc::clone(opened) });
        gone
    });
    // Dropped once the thread's list is let go (RULES.md R-ERR-4).
    drop(gone);
}

// Unit tests: bookkeeping found again with the table's map held, and never
// another table's nor an ended transaction's (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
