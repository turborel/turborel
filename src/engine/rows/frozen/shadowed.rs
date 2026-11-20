//! Unit tests of a scan by batch of frozen groups some of whose rows the
//! tree shadows (`batches.rs`, `columns/scan.rs`): the frozen rows left
//! are read at their positions, of every kind of column.

use std::ops::Bound;
use std::sync::Arc;

use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::engine::rows::options::{Columns, Places};
use crate::engine::rows::{RowTable, TableFiles, TableOptions};
use crate::engine::tree::Direction;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::index::Probe;
use crate::log::MemoryLog;
use crate::storage::{Condition, Conjunct, Conjunction, Row, RowId, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

/// An integer, a Boolean, a decimal of a scale, a double and a text.
fn types() -> [DataType; 5] {
    [
        DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer },
        DataType::Boolean,
        DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Decimal },
        DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision },
        DataType::varying(20),
    ]
}

/// Row `n`, its Boolean null of every seventh.
fn row(n: i128) -> Row {
    let flag = if n % 7 == 0 { Value::Null } else { Value::Bool(n % 2 == 0) };
    let float = f64::from(i32::try_from(n).unwrap_or(0_i32));
    vec![Value::Exact(Decimal::new(n, 0)), flag, Value::Exact(Decimal::new(n, 2)), Value::Approx(float), Value::Char(format!("t{n}"))]
}

/// A table in memory of the column store, groups of 2048 frozen at once
/// and never written again, of rows `0..rows`; and their ids.
fn table(rows: i128) -> (Arc<RowTable>, Vec<RowId>) {
    let fs = MemoryFileSystem::new();
    let pager = Arc::new(
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap(),
    );
    let pool = Arc::new(BufferPool::new(256, Box::new(Clock::new(256))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let columns = Columns {
        group_rows: 2048,
        freeze_rows: 0,
        freeze_delay: 0,
        rewrite_fraction: 0.0,
        compress: true,
        order: Places::default(),
        bloom: Places::default(),
        pax: false,
    };
    let table = Arc::new(RowTable::create(&files, &types(), TableOptions { columns: Some(columns), ..TableOptions::default() }).unwrap());
    let ids = (0..rows).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
    (table, ids)
}

/// The rows a scan by batch of every column gives, each shown, sorted.
fn batched(table: &Arc<RowTable>) -> Vec<String> {
    batched_where(table, None)
}

/// As `batched`, the scan told `condition`, if any: every row it gives,
/// the filter not applied.
fn batched_where(table: &Arc<RowTable>, condition: Option<&Conjunction>) -> Vec<String> {
    let mut shown = Vec::new();
    for batch in Arc::clone(table).scan_batches(NO_TRANSACTION, &[0, 1, 2, 3, 4], condition, &[]) {
        let batch = batch.unwrap();
        let columns: Vec<&[Value]> = batch.columns.iter().map(crate::storage::Column::values).collect();
        for r in 0..batch.rows {
            shown.push(columns.iter().map(|c| c[r].to_string()).collect::<Vec<_>>().join(","));
        }
    }
    shown.sort();
    shown
}

/// Verifies: [DESIGN] RULES.md D31, a group read past the tree's
/// entries: once rows of a frozen group are updated, the tree's entries
/// shadowing them and the group not written again, a scan by batch reads
/// each other row of the group at its position, of an integer, a Boolean
/// with nulls, a decimal, a double and a text column, a chunk all of
/// whose rows are shadowed passed over; every row is given once, the
/// updated ones as updated.
#[test]
fn shadowed_rows_are_read_past_at_their_positions() {
    let (table, ids) = table(5000);
    table.gc(1, &mut |_, _| true).unwrap();
    while table.maintain(1).unwrap() {}
    assert_eq!(table.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).count(), 5000 - 4096, "two groups frozen");
    let changed: Vec<RowId> = ids.iter().copied().step_by(3).chain(ids[1000..1800].iter().copied()).collect();
    table.begin(10, 9).unwrap();
    for &id in &changed {
        table.update(10, id, row(-1)).unwrap();
    }
    table.validate(10, 10).unwrap();
    table.commit(10, 10);
    let mut expected: Vec<String> = (0..5000)
        .filter(|&n| !changed.contains(&ids[usize::try_from(n).unwrap()]))
        .map(|n| row(n).iter().map(ToString::to_string).collect::<Vec<_>>().join(","))
        .collect();
    let updated = row(-1).iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
    let mut unique = changed.clone();
    unique.sort_unstable();
    unique.dedup();
    expected.extend(std::iter::repeat_n(updated, unique.len()));
    expected.sort();
    assert_eq!(batched(&table), expected);
}

/// `column 0 < n`.
fn below(n: i128) -> Conjunction {
    let probe = Probe::Compare { op: CompOp::Lt, value: Value::Exact(Decimal::new(n, 0)) };
    Conjunction(vec![Conjunct::Condition(Condition { columns: vec![0], probe })])
}

/// The scan's rows with the shadow bits trusted, not trusted, and all set:
/// the same each way.
fn same_every_way(table: &Arc<RowTable>) -> Vec<String> {
    let trusted = batched(table);
    table.trust_shadow_bits(false);
    assert_eq!(batched(table), trusted, "the tree read over every group");
    table.trust_shadow_bits(true);
    for entry in &table.frozen.directory().unwrap().groups {
        (entry.summary.first..=entry.summary.last).for_each(|id| entry.shadow.mark(id));
    }
    assert_eq!(batched(table), trusted, "every bit set");
    trusted
}

/// Verifies: [DESIGN] RULES.md D510, shadow bits: a group no write
/// touched has none set; an update, a deletion and an insert into a
/// group's range, by a transaction or outside one, set the bits of their
/// rows alone; an insert still open as its range is frozen is marked as
/// the group is published; and a scan gives the same rows whether it
/// trusts the bits, reads the tree over every group, or finds every bit
/// set, as the bits are rebuilt when the table is opened.
#[test]
fn shadow_bits_mark_writes_and_keep_scans_the_same() {
    let (table, ids) = table(1000);
    table.begin(5, 4).unwrap();
    let open = table.insert(5, row(5000)).unwrap();
    for n in 1000..5000 {
        table.insert(NO_TRANSACTION, row(n)).unwrap();
    }
    table.gc(1, &mut |_, _| true).unwrap();
    while table.maintain(1).unwrap() {}
    let directory = table.frozen.directory().unwrap();
    assert_eq!(directory.groups.len(), 2, "two groups frozen");
    let holding = |id: RowId| directory.holding(id).unwrap();
    assert!(holding(open).shadow.may_hold(open), "the open insert, in the first group's range, marked as it was published");
    assert!(directory.groups[1].shadow.none(), "no write touched the second group");
    table.commit(5, 5);
    table.begin(10, 9).unwrap();
    table.update(10, ids[3], row(-1)).unwrap();
    table.delete(10, ids[4]).unwrap();
    table.validate(10, 10).unwrap();
    table.commit(10, 10);
    assert!(holding(ids[3]).shadow.may_hold(ids[3]) && holding(ids[4]).shadow.may_hold(ids[4]));
    assert!(!holding(ids[5]).shadow.may_hold(ids[5]), "a row not written");
    let shown = same_every_way(&table);
    assert_eq!(shown.len(), 5000, "5000 rows and the open insert, less the one deleted");
}

/// Verifies: [DESIGN] RULES.md D510, zones over the tree's rows of a
/// column store table: a scan told a condition reads the tree's rows only
/// of the zones it may match, a frozen row updated to match given through
/// them although its group cannot match, and every row matching given.
#[test]
fn delta_zones_skip_the_tree_rows_no_condition_matches() {
    let (table, ids) = table(5000);
    table.gc(1, &mut |_, _| true).unwrap();
    while table.maintain(1).unwrap() {}
    let condition = below(10);
    let before = batched_where(&table, Some(&condition));
    assert_eq!(before.len(), 10, "the rows the scan judges of the first group, and none of the tree's");
    table.begin(10, 9).unwrap();
    table.update(10, ids[3000], row(-1)).unwrap();
    table.validate(10, 10).unwrap();
    table.commit(10, 10);
    let given = batched_where(&table, Some(&condition));
    let updated = row(-1).iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
    assert!(given.contains(&updated), "the row updated, of a group its condition cannot match");
    let matching = given.iter().filter(|r| r.split(',').next().unwrap().parse::<i64>().unwrap() < 10).count();
    assert_eq!(matching, 11, "rows 0 to 9 and the one updated");
    assert_eq!(given.len(), 11, "the tree's other zones not read");
}
