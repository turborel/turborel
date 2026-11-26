//! \[DESIGN\] Of the zones (see the module's documentation), a table's
//! zones kept cold until something asks them (RULES.md D868). A table
//! written and read by key alone, as an order entry's are, has no use for
//! zones: no scan told a condition reads it, and no index-only scan asks
//! whether its rows are visible. Its writes then touch no zone at all:
//! none is made, locked, compared with or widened.
//!
//! A cold table has no zone, so every block of its row ids is read and
//! none of its rows counts as visible to every reader: a zone is never
//! narrower than its rows, as one not known is not. The first scan whose
//! conditions read a column kept, the first plan that weighs an index-only
//! read of the table or read that asks whether a row is visible to all,
//! and the first count warm the table. Warming makes a
//! zone not known for every block of row ids given so far, to be summed
//! up by the maintenance thread as a table opened has its zones summed
//! up, and from then on every write keeps its zone as before. A table
//! stays warm while it is open.
//!
//! A write that found the table cold has its row in the tree, and its id
//! given, before it looked. Warming sets the table warm and only then
//! reads the ids given, under the lock zones are added under, so that
//! row's zone is one not known, summed up from the tree.
//!
//! The versions commits supersede are counted for the table while it is
//! cold (`Zones::owed`), by stripe, as no zone is there to count them.
//! Their index entries are still to be taken back, so once the table is
//! warm no zone is settled visible until that count is back to none. A
//! version collected is taken from its zone's count if that has one, else
//! from the table's, so the two together are the versions left.
//!
//! A table opened is warm if its last checkpoint saved zones, which only
//! a warm table has: they are read, and recovery widens them. A table of
//! the column store is warm from the first, its scans reading by zone.
//!
//! A table opened has its zones due summed up and settled only once one
//! is asked for after it opens, by what warms a cold table (RULES.md
//! D1212): the table notes the first ask, the transaction that asked
//! notes it for maintenance as it ends, and until then the maintenance
//! thread is not asked for, so a start does no pass over a table nothing
//! reads. An ask after the first reads the note and writes nothing.

use std::sync::atomic::Ordering;

use super::{ZONE_ROWS, Zone, Zones};
use crate::sync::lock_builtin;

impl Zones {
    /// Whether no zone is kept yet.
    pub(in crate::engine::rows) fn cold(&self) -> bool {
        !self.warm.load(Ordering::SeqCst)
    }

    /// Makes the table warm from its first row, of one that holds no row
    /// yet: every zone made is known, and its zones are asked for from
    /// the first, so the maintenance thread keeps them as it is written.
    pub(in crate::engine::rows) fn begin_warm(&self) {
        self.warm.store(true, Ordering::SeqCst);
        self.asked.store(true, Ordering::Release);
    }

    /// Whether a zone was asked for since the table was made or opened
    /// (RULES.md D1212): only then is the maintenance thread asked to sum
    /// up or settle its zones.
    pub(in crate::engine::rows) fn asked(&self) -> bool {
        self.asked.load(Ordering::Acquire)
    }

    /// Notes the table opened, its zones read as saved: none asked for
    /// yet, whatever made it warm (RULES.md D1212).
    pub(super) fn opened(&self) {
        self.asked.store(false, Ordering::Release);
    }

    /// Notes a zone asked for, warming the table if it is cold: a zone not
    /// known for each block of the row ids given so far (see the module's
    /// documentation). A table asked is warm, so every ask after the
    /// first reads one flag and writes nothing, as a read of a row by an
    /// index asks of each row (RULES.md D1212).
    pub(in crate::engine::rows) fn warm(&self) {
        if self.asked() {
            return;
        }
        self.warm_cold();
        // After it is warm: a table asked is never cold.
        self.asked.store(true, Ordering::Release);
    }

    /// `warm`'s work of a table cold.
    fn warm_cold(&self) {
        if !self.cold() {
            return;
        }
        let _growing = lock_builtin(&self.growing);
        if !self.cold() {
            return;
        }
        // Warm before the ids given are read: a write that found it cold
        // was given its id before this.
        self.warm.store(true, Ordering::SeqCst);
        let below = self.given.load(Ordering::SeqCst);
        self.unknown_below.fetch_max(below, Ordering::SeqCst);
        let zones = usize::try_from(below.div_ceil(ZONE_ROWS)).unwrap_or(usize::MAX);
        let mut added = true;
        while added && self.slots.len() < zones {
            added = self.push(Zone::empty(false));
            if added {
                self.stale.fetch_add(1, Ordering::AcqRel);
                self.unsettled.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    /// Counts a version superseded while the table is cold: whether it
    /// did. One counted as the table warms is taken back, for its zone to
    /// count, so after warming the table's count only falls.
    pub(super) fn owe(&self) -> bool {
        if !self.cold() {
            return false;
        }
        self.owed.fetch_add(1, Ordering::SeqCst);
        if self.cold() {
            return true;
        }
        self.owed.fetch_sub(1, Ordering::SeqCst);
        false
    }

    /// Whether a version superseded while the table was cold is still to
    /// be collected.
    pub(super) fn owing(&self) -> bool {
        self.owed.load(Ordering::SeqCst) > 0
    }
}

// Unit tests: a cold table's writes touch no zone, a table warmed prunes
// once summed up, and versions superseded while it was cold keep its zones
// from being settled visible (RULES.md R-TEST-1, R-TEST-7, R-TEST-8).
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::super::tests::{TYPES, files, row};
    use crate::engine::rows::{RowTable, TableOptions};
    use crate::index::Probe;
    use crate::storage::{Condition, Conjunction, RowId, TableStorage};
    use crate::txn::NO_TRANSACTION;
    use crate::value::{CompOp, Decimal, Value};

    fn equal(column: usize, n: i128) -> Conjunction {
        let value = Value::Exact(Decimal::new(n, 0));
        Conjunction::of(Condition { columns: vec![column], probe: Probe::Compare { op: CompOp::Eq, value } })
    }

    /// Whether a scan told `conditions` reads the zone of row `id`.
    fn reads(table: &RowTable, conditions: &Conjunction, id: RowId) -> bool {
        table.zones.ranges(Some(conditions)).iter().any(|&(first, end)| first <= id && end.is_none_or(|e| id < e))
    }

    /// A table of the row store of `rows` rows committed before every
    /// snapshot, the first updated and the second deleted by a
    /// transaction committed at 2; and their ids.
    fn written(rows: i128) -> (Arc<RowTable>, Vec<RowId>) {
        let table = Arc::new(RowTable::create(&files(), &TYPES, TableOptions::default()).unwrap());
        let ids: Vec<RowId> = (0..rows).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
        table.begin(10, 1).unwrap();
        table.update(10, ids[0], row(-1)).unwrap();
        table.delete(10, ids[1]).unwrap();
        table.validate(10, 2).unwrap();
        table.commit(10, 2);
        (table, ids)
    }

    /// Verifies: [DESIGN] RULES.md D868: a table no scan told a condition
    /// has read, and no read asked whether a row is visible, keeps no
    /// zone: its inserts, an update, a deletion and their commit make
    /// none, so none is locked, and nothing is due; a scan told a
    /// condition of a column not kept leaves it cold, and reads every row.
    #[test]
    fn a_cold_tables_writes_touch_no_zone() {
        let (table, ids) = written(3000);
        assert!(table.zones.cold());
        assert_eq!(table.zones.slots.len(), 0, "no zone made, so none locked");
        assert!(!table.zones.due() && !table.maintenance_due());
        assert_eq!(table.zones.ranges(Some(&equal(1, 5))), [(0, None)], "a column not kept");
        assert_eq!(table.zones.ranges(None), [(0, None)]);
        assert!(table.zones.cold() && table.zones.slots.len() == 0);
        assert_eq!(table.get(NO_TRANSACTION, ids[0]).unwrap().map(|r| r[0].to_string()), Some("-1".to_owned()));
    }

    /// Verifies: [DESIGN] RULES.md D868, D31: the first scan told a
    /// condition of a column kept warms the table: it reads every zone,
    /// each not known, and once they are summed up a scan reads only the
    /// zones whose rows may match; a row written after the warming is held
    /// by its zone at once. A regression test (R-TEST-7): it fails if a
    /// table warmed takes the zones of rows written while it was cold as
    /// known.
    #[test]
    fn a_table_warmed_prunes_once_summed_up() {
        let (table, ids) = written(3000);
        let (near, far) = (ids[5], ids[2999]);
        assert!(reads(&table, &equal(0, 5), far), "warmed, its zones not known");
        assert!(!table.zones.cold() && table.zones.stale.load(Ordering::Acquire) > 0);
        table.gc(3, &mut |_, _, _| true).unwrap();
        while table.maintain(3, true).unwrap() {}
        assert!(reads(&table, &equal(0, 5), near) && !reads(&table, &equal(0, 5), far), "summed up");
        assert!(reads(&table, &equal(0, 2999), far) && !reads(&table, &equal(0, 2999), near));
        let late = table.insert(NO_TRANSACTION, row(777_777)).unwrap();
        assert!(reads(&table, &equal(0, 777_777), late), "written warm, held at once");
        assert!(!reads(&table, &equal(0, 777_778), late));
    }

    /// Verifies: [DESIGN] RULES.md D868, D34: a read that asks whether a
    /// row of a cold table is visible to every reader is told it is not,
    /// and warms the table; a version superseded while the table was cold
    /// keeps every zone from being settled visible until it is collected,
    /// and then the zones settle visible. MC/DC of the settling (R-TEST-8):
    /// rows visible with such a version left, and with none.
    #[test]
    fn versions_superseded_cold_keep_zones_unsettled_until_collected() {
        let table = Arc::new(RowTable::create(&files(), &TYPES, TableOptions::default()).unwrap());
        let ids: Vec<RowId> = (0..10).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
        table.begin(10, 1).unwrap();
        table.update(10, ids[0], row(-1)).unwrap();
        table.validate(10, 2).unwrap();
        table.commit(10, 2);
        assert!(table.zones.cold() && table.zones.owing(), "counted for the table");
        assert!(!table.visible_to_all(ids[3]), "cold: not visible");
        assert!(!table.zones.cold());
        while table.maintain(3, true).unwrap() {}
        assert!(!table.visible_to_all(ids[3]), "the version superseded is not collected");
        table.gc(3, &mut |_, _, _| true).unwrap();
        assert!(!table.zones.owing());
        while table.maintain(3, true).unwrap() {}
        assert!(table.visible_to_all(ids[3]), "collected, and settled");
    }
}
