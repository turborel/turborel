//! \[DESIGN\] A bare COUNT(*) of a table answered by the table's storage
//! (RULES.md D351, `TableStorage::count`). An aggregation of one group
//! whose set functions each count rows [X3.135-1986 §5.8 GR4b], of a scan
//! of one table in no order, with no filter and no row id, is fed the
//! counts the storage gives in place of the scan's batches, each read
//! only while the statement is not cancelled (RULES.md D44). The counts
//! are those of the rows the scan would give, so the result is the same;
//! a storage that cannot count leaves the aggregation to the scan.

use std::sync::Arc;

use super::{Read, Taken};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::physical::Plan;
use crate::storage::Row;
use crate::txn::{NO_TRANSACTION, TxId, checked};

/// The table whose rows the aggregation `read` reads of `input` would
/// count alone, of one group, each of its set functions counting rows, of
/// a bare scan of the table.
pub(super) fn counted_table<'p>(read: &Read, input: &'p Plan) -> Option<&'p QualifiedName> {
    if !read.groups.is_empty() || !read.taken.iter().all(|t| matches!(t, Taken::Count)) {
        return None;
    }
    // [EXTENSION] of a counted scan too (RULES.md D378).
    let Plan::SeqScan { table, order, located: false, .. } = input.bare() else { return None };
    order.is_empty().then_some(table)
}

/// The finished set functions `aggs` of the rows of `table` that `tx`
/// sees, fed the counts of its storage; none if it gives none.
pub(super) fn counted(table: &QualifiedName, aggs: &[AggCall], db: &Db, tx: TxId) -> Option<Result<Row>> {
    let counts = Arc::clone(db.storage_of(table).ok()?).count(tx)?;
    let mut states: Vec<Box<dyn Accumulator>> = aggs.iter().map(|a| a.func.0.accumulator(a.result.clone())).collect();
    let fed = checked(counts, db.tx.stop_of(tx)).try_for_each(|count| {
        let rows = usize::try_from(count?).unwrap_or(usize::MAX);
        states.iter_mut().try_for_each(|state| state.update_rows(rows))
    });
    Some(fed.and_then(|()| states.iter().map(|s| s.finish()).collect()))
}

/// \[DESIGN\] The table whose rows the bare COUNT(*) at the top of `plan`
/// counts by its storage, under the projections, sorts, limits and
/// filters of its one row, if it is read by batches and the storage
/// counts, as EXPLAIN notes it (`db/explain.rs`). Asking the storage reads
/// nothing, its counts being read once asked for.
pub(crate) fn storage_counted<'p>(plan: &'p Plan, db: &State) -> Option<&'p QualifiedName> {
    let mut top = plan;
    while let Plan::Project { input, .. } | Plan::Sort { input, .. } | Plan::Limit { input, .. } | Plan::Filter { input, .. } = top {
        top = input;
    }
    let Plan::Aggregate { input, .. } = top else { return None };
    let table = counted_table(&super::read_of(top, db)?, input)?;
    let counts = super::super::batched(db) && Arc::clone(db.storage_of(table).ok()?).count(NO_TRANSACTION).is_some();
    counts.then_some(table)
}

// Unit tests: which aggregations are counted by the storage (RULES.md
// R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;

    /// A scan of T, located or not, in the order of `order`.
    fn scan(located: bool, order: Vec<usize>) -> Plan {
        let table = QualifiedName { auth: "A".into(), ident: "T".into() };
        Plan::SeqScan { table, columns: vec![], rows: 0, cost: 0.0_f64, order, backward: false, most: None, located }
    }

    /// Verifies: [DESIGN] RULES.md D351: an aggregation is counted by the
    /// storage only of one group, each set function counting rows, of a
    /// bare scan.
    ///
    /// Decision `!groups.is_empty() || !all counting`: neither (counted)
    /// against groups alone (scanned) shows the groups' condition; a set
    /// function of a column alone (scanned) shows the counting's. The
    /// scan's, a located scan or one in an order is not bare.
    #[test]
    fn only_a_bare_count_is_counted_by_the_storage() {
        let read = |groups: Vec<usize>, taken: Vec<Taken>| Read { taken, groups };
        let bare = scan(false, Vec::new());
        assert!(counted_table(&read(Vec::new(), vec![Taken::Count, Taken::Count]), &bare).is_some());
        assert!(counted_table(&read(vec![0], vec![Taken::Count]), &bare).is_none());
        assert!(counted_table(&read(Vec::new(), vec![Taken::Count, Taken::Column(0)]), &bare).is_none());
        assert!(counted_table(&read(Vec::new(), vec![Taken::Count]), &scan(true, Vec::new())).is_none());
        assert!(counted_table(&read(Vec::new(), vec![Taken::Count]), &scan(false, vec![0])).is_none());
    }
}
