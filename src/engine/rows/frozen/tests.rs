//! Unit tests of a table's frozen row groups (`frozen.rs`, `freeze.rs`,
//! `merge.rs`, `rewrite.rs`, `batches.rs`).

use std::ops::Bound;
use std::sync::Arc;

use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::engine::rows::options::{Columns, Places};
use crate::engine::rows::{RowTable, TableFiles, TableOptions, key};
use crate::engine::tree::Direction;
use crate::error::Code;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::index::Probe;
use crate::log::MemoryLog;
use crate::storage::{Condition, Conjunction, Row, RowId, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

const TYPES: [DataType; 2] = [DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, DataType::Varying { length: 20 }];

fn row(n: i128) -> Row {
    vec![Value::Exact(Decimal::new(n, 0)), Value::Char(format!("v{n}"))]
}

/// A pager file in memory, its pager, and the tables' files in it.
fn files() -> (Arc<Pager>, TableFiles) {
    let fs = MemoryFileSystem::new();
    let pager = Arc::new(
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap(),
    );
    let pool = Arc::new(BufferPool::new(256, Box::new(Clock::new(256))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    (pager, TableFiles::new(pool, file, Arc::new(MemoryLog::new())))
}

/// Of the column store, in groups of 2048, frozen after `delay` seconds.
fn options(delay: u32) -> TableOptions {
    rewriting(delay, crate::engine::rows::options::DEFAULT_REWRITE_FRACTION)
}

/// As `options`, groups written again past `fraction` of their rows.
fn rewriting(delay: u32, fraction: f64) -> TableOptions {
    TableOptions {
        columns: Some(Columns {
            group_rows: 2048,
            freeze_rows: 0,
            freeze_delay: delay,
            rewrite_fraction: fraction,
            compress: true,
            order: Places::default(),
            bloom: Places::default(),
        }),
        ..TableOptions::default()
    }
}

/// A table of `rows` rows, written outside any transaction, so committed
/// before every snapshot; and their ids.
fn table(files: &TableFiles, delay: u32, rows: i128) -> (Arc<RowTable>, Vec<RowId>) {
    let table = Arc::new(RowTable::create(files, &TYPES, options(delay)).unwrap());
    let ids = (0..rows).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
    (table, ids)
}

/// What a transaction's end has the table do: its garbage collected past
/// `horizon`, then its maintenance, as the database's maintenance thread
/// does it (`TableStorage::maintain`).
fn ended(table: &RowTable, horizon: u64) {
    table.gc(horizon, &mut |_, _| {}).unwrap();
    while table.maintain(horizon).unwrap() {}
}

/// The tree's entries.
fn entries(table: &RowTable) -> usize {
    table.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).count()
}

/// The first value of each row `table` gives transaction `tx`.
fn scanned(table: &Arc<RowTable>, tx: u64) -> Vec<String> {
    Arc::clone(table).scan(tx, &[0]).map(|r| r.unwrap().1[0].to_string()).collect()
}

/// Verifies: [DESIGN] RULES.md D31, freezing: once a group's rows are
/// committed past the last group, they are frozen, groups of 2048, their
/// entries taken out of the tree, the rest left; every row is read as
/// before, by id and by a scan, and counted; fewer rows than a group are
/// not frozen.
#[test]
fn rows_are_frozen_when_due() {
    let (_, files) = files();
    let (few, _) = table(&files, 0, 2000);
    ended(&few, 1);
    assert!(few.frozen.directory().unwrap().groups.is_empty(), "too few");
    let (table, ids) = table(&files, 0, 5000);
    ended(&table, 1);
    let directory = table.frozen.directory().unwrap();
    assert_eq!(directory.groups.iter().map(|e| e.summary.rows).collect::<Vec<_>>(), [2048, 2048]);
    assert_eq!(entries(&table), 5000 - 4096, "the frozen rows' entries taken out");
    assert_eq!(table.get(NO_TRANSACTION, ids[7]).unwrap().map(|r| r[1].to_string()), Some("v7".to_owned()));
    assert_eq!(table.get(NO_TRANSACTION, ids[4999]).unwrap().map(|r| r[1].to_string()), Some("v4999".to_owned()));
    let all = scanned(&table, NO_TRANSACTION);
    assert_eq!(all.len(), 5000);
    assert_eq!(all.first().map(String::as_str), Some("0"));
    assert_eq!(table.count_newest().unwrap(), 5000);
}

/// Verifies: [DESIGN] RULES.md D31, freezing under readers: a scan begun
/// before a freeze reads every row, the frozen rows' entries kept until it
/// is done, and taken out at the next freeze tried after.
#[test]
fn a_scan_begun_before_a_freeze_misses_no_row() {
    let (_, files) = files();
    let (table, _) = table(&files, 0, 5000);
    let mut scan = Arc::clone(&table).scan(NO_TRANSACTION, &[0]);
    let first = scan.next().unwrap().unwrap();
    ended(&table, 1);
    assert_eq!(table.frozen.directory().unwrap().groups.len(), 2);
    assert_eq!(entries(&table), 5000, "kept for the scan");
    assert_eq!(scan.count() + 1, 5000);
    assert_eq!(first.1[0].to_string(), "0");
    table.retire_unread().unwrap();
    assert_eq!(entries(&table), 5000 - 4096, "taken out once it is done");
}

/// Verifies: [DESIGN] RULES.md D31, D29, frozen rows written: an update
/// and a deletion, each seen by its transaction and, once committed, by
/// later ones, a reader of an earlier snapshot seeing the frozen values;
/// a deletion of a frozen row is kept after collection; an update rolled
/// back leaves the frozen values; two transactions writing one frozen row
/// conflict; a row neither in the tree nor frozen is no row.
#[test]
fn frozen_rows_are_written() {
    let (_, files) = files();
    let (table, ids) = table(&files, 0, 5000);
    ended(&table, 1);
    let value = |tx, id| table.get(tx, id).unwrap().map(|r| r[1].to_string());
    table.begin(10, 1).unwrap();
    table.begin(11, 1).unwrap();
    table.update(10, ids[3], row(-3)).unwrap();
    table.delete(10, ids[4]).unwrap();
    assert_eq!((value(10, ids[3]), value(10, ids[4])), (Some("v-3".to_owned()), None));
    assert_eq!(value(11, ids[3]), Some("v3".to_owned()), "an earlier snapshot");
    assert_eq!(table.update(11, ids[3], row(0)).err().map(|e| e.code), Some(Code::Conflict));
    table.validate(10, 2).unwrap();
    table.commit(10, 2);
    table.commit(11, 2);
    ended(&table, 3);
    assert_eq!((value(NO_TRANSACTION, ids[3]), value(NO_TRANSACTION, ids[4])), (Some("v-3".to_owned()), None));
    assert!(table.tree.get(&key(ids[4])).unwrap().is_some(), "its deletion kept");
    table.begin(12, 3).unwrap();
    table.update(12, ids[5], row(-5)).unwrap();
    table.rollback(12).unwrap();
    assert_eq!(value(NO_TRANSACTION, ids[5]), Some("v5".to_owned()), "rolled back");
    assert_eq!(table.count_newest().unwrap(), 4999);
    assert_eq!(table.update(NO_TRANSACTION, 1 << 40, row(0)).err().map(|e| e.code), Some(Code::Api), "no such row");
}

/// Verifies: [DESIGN] RULES.md D31, freezing waits: rows committed after
/// the horizon, or within COLUMNSTORE.FREEZE_DELAY of now, are not frozen; a
/// freeze that found too few rows is not tried again at once.
#[test]
fn a_freeze_waits_for_its_horizon_and_its_delay() {
    let (_, files) = files();
    let (late, _) = table(&files, 0, 0);
    late.begin(10, 1).unwrap();
    for n in 0..5000 {
        late.insert(10, row(n)).unwrap();
    }
    late.validate(10, 5).unwrap();
    late.commit(10, 5);
    ended(&late, 4);
    assert!(late.frozen.directory().unwrap().groups.is_empty(), "committed after the horizon");
    ended(&late, 5);
    assert!(late.frozen.directory().unwrap().groups.is_empty(), "not tried again at once");
    let (delayed, _) = table(&files, 3600, 0);
    delayed.begin(20, 1).unwrap();
    for n in 0..5000 {
        delayed.insert(20, row(n)).unwrap();
    }
    delayed.validate(20, 6).unwrap();
    delayed.commit(20, 6);
    ended(&delayed, 7);
    assert!(delayed.frozen.directory().unwrap().groups.is_empty(), "within its delay");
}

/// Verifies: [DESIGN] RULES.md D31, D25: recovery's writes of frozen rows
/// (`TableStorage::apply`): a deletion kept, a row replaced, each counted;
/// a table reopened finds its groups, its rows and its count; and a meta
/// page naming a directory that is not one is refused with SQLCODE -916.
#[test]
fn frozen_rows_are_recovered_and_reopened() {
    let (_, files) = files();
    let (table, ids) = table(&files, 0, 5000);
    ended(&table, 1);
    table.apply(ids[1], None).unwrap();
    table.apply(ids[2], Some(row(-2))).unwrap();
    table.apply(ids[1], None).unwrap();
    table.update(NO_TRANSACTION, ids[6], row(-6)).unwrap();
    table.delete(NO_TRANSACTION, ids[7]).unwrap();
    assert_eq!(table.row_count(), 4998);
    let meta = table.meta();
    let reopened = Arc::new(RowTable::open(&files, meta, &TYPES, options(0)).unwrap());
    assert_eq!(reopened.frozen.directory().unwrap().groups.len(), 2);
    assert_eq!((reopened.row_count(), reopened.count_newest().unwrap()), (4998, 4998));
    let values: Vec<Option<String>> =
        [1, 2, 6, 7, 8].iter().map(|&n| reopened.get(NO_TRANSACTION, ids[n]).unwrap().map(|r| r[1].to_string())).collect();
    assert_eq!(values, [None, Some("v-2".to_owned()), Some("v-6".to_owned()), None, Some("v8".to_owned())]);
    assert_eq!(scanned(&reopened, NO_TRANSACTION).len(), 4998);
    // Two groups named, one listed.
    let bad = crate::engine::columns::group::write_chain(&reopened.group_pages().unwrap(), &[2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(reopened.load_directory(bad[0].0).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [EXTENSION] RULES.md D31, D33: a dropped table of frozen
/// groups frees their pages and their directory's when the last reference
/// goes; an index's versions count each frozen row once.
#[test]
fn a_dropped_table_frees_its_groups() {
    let (pager, files) = files();
    let in_use = || pager.page_count() - pager.free_pages();
    let before = in_use();
    let (table, _) = table(&files, 0, 5000);
    ended(&table, 1);
    assert_eq!(Arc::clone(&table).versions(1, 1, &[0]).count(), 5000);
    assert!(in_use() > before);
    table.discard();
    drop(table);
    assert_eq!(in_use(), before, "every page freed");
}

/// Updates rows `ids` of `table` to their negated keys and deletes rows
/// `deleted`, in a transaction committed at `at`, and collects past it.
fn changed(table: &RowTable, (ids, deleted): (&[RowId], &[RowId]), at: u64) {
    table.begin(at, at.saturating_sub(1)).unwrap();
    for (n, &id) in ids.iter().enumerate() {
        table.update(at, id, row(i128::try_from(n).unwrap().checked_neg().unwrap())).unwrap();
    }
    for &id in deleted {
        table.delete(at, id).unwrap();
    }
    table.validate(at, at).unwrap();
    table.commit(at, at);
    ended(table, at);
}

/// Verifies: [DESIGN] RULES.md D31, groups written again: once a fifth of
/// a group's rows are changed or deleted, the group is written again with
/// them: the deleted rows gone from it, the updates its values, their
/// entries out of the tree, the other group as it was; a group whose rows
/// are all deleted is taken out; reopened, the table reads the same; and
/// with COLUMNSTORE.REWRITE_FRACTION 0, no group is written again.
#[test]
fn groups_are_written_again() {
    let (_, held) = files();
    let (table, ids) = table(&held, 0, 5000);
    ended(&table, 1);
    let before = table.frozen.directory().unwrap();
    changed(&table, (&ids[0..400], &ids[400..500]), 10);
    let after = table.frozen.directory().unwrap();
    assert_eq!(after.groups.iter().map(|e| e.summary.rows).collect::<Vec<_>>(), [2048 - 100, 2048]);
    assert!(Arc::ptr_eq(&before.groups[1], &after.groups[1]), "the other group as it was");
    drop(before);
    table.retire_unread().unwrap();
    assert_eq!(entries(&table), 5000 - 4096, "the entries folded in taken out");
    assert_eq!(table.get(NO_TRANSACTION, ids[3]).unwrap().map(|r| r[0].to_string()), Some("-3".to_owned()));
    assert!(table.get(NO_TRANSACTION, ids[450]).unwrap().is_none());
    assert_eq!(scanned(&table, NO_TRANSACTION).len(), 4900);
    changed(&table, (&[], &ids[2048..4096]), 20);
    assert_eq!(table.frozen.directory().unwrap().groups.len(), 1, "a group of no rows taken out");
    let reopened = Arc::new(RowTable::open(&held, table.meta(), &TYPES, options(0)).unwrap());
    assert_eq!((scanned(&reopened, NO_TRANSACTION).len(), reopened.row_count()), (2852, 2852));
    let (never, more) = {
        let (_, others) = files();
        let never = Arc::new(RowTable::create(&others, &TYPES, rewriting(0, 0.0)).unwrap());
        let more: Vec<RowId> = (0..5000).map(|n| never.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
        ended(&never, 1);
        (never, more)
    };
    changed(&never, (&more[0..1000], &[]), 30);
    assert_eq!(never.frozen.directory().unwrap().groups.iter().map(|e| e.summary.rows).collect::<Vec<_>>(), [2048, 2048], "never");
}

/// The pages of the groups a table of rows `0..4096` of `types`, made by
/// `make`, freezes, compressed or not; and its values read back.
fn group_pages_of(types: &[DataType], make: impl Fn(i128) -> Row, compress: bool) -> (usize, Vec<String>) {
    let (_, files) = files();
    let options = TableOptions {
        columns: Some(Columns {
            group_rows: 2048,
            freeze_rows: 0,
            freeze_delay: 0,
            rewrite_fraction: 0.0,
            compress,
            order: Places::default(),
            bloom: Places::default(),
        }),
        ..TableOptions::default()
    };
    let table = Arc::new(RowTable::create(&files, types, options).unwrap());
    for n in 0..4096 {
        table.insert(NO_TRANSACTION, make(n)).unwrap();
    }
    ended(&table, 1);
    let pages = table.frozen.directory().unwrap().groups.iter().map(|e| table.group_of(e).unwrap().pages().len()).sum();
    let all: Vec<usize> = (0..types.len()).collect();
    let read = Arc::clone(&table)
        .scan(NO_TRANSACTION, &all)
        .map(|r| r.unwrap().1.iter().map(ToString::to_string).collect::<Vec<_>>().join(","))
        .collect();
    (pages, read)
}

/// Verifies: [DESIGN] RULES.md D31, compressed groups: rows of numbers of
/// few values or near one another, and of texts of few values, freeze into
/// a fraction of the pages compressed that they take plain, and read back
/// the same.
#[test]
fn compressed_groups_take_fewer_pages() {
    let types = [
        DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer },
        DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer },
        DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Decimal },
        DataType::Varying { length: 20 },
    ];
    let make = |n: i128| {
        vec![
            Value::Exact(Decimal::new(n, 0)),
            Value::Exact(Decimal::new(n.checked_rem(7).unwrap(), 0)),
            Value::Exact(Decimal::new(n.checked_rem(100).unwrap().checked_mul(100).unwrap().checked_add(50).unwrap(), 2)),
            Value::Char(format!("status {}", n.checked_rem(5).unwrap())),
        ]
    };
    let (compressed, read) = group_pages_of(&types, make, true);
    let (plain, plain_read) = group_pages_of(&types, make, false);
    assert_eq!(read, plain_read, "the same rows");
    eprintln!("group pages: {compressed} compressed, {plain} not");
    assert!(compressed.saturating_mul(3) < plain, "{compressed} against {plain}");
}

/// Verifies: [DESIGN] RULES.md D31, D29, a deletion as rows are frozen: a
/// deletion committed and collected while a freeze that may freeze its row
/// is under way keeps its entry, so the row is not seen again once frozen;
/// one of a row before those it may freeze, or once none is under way, is
/// collected.
#[test]
fn a_deletion_during_a_freeze_is_kept() {
    let (_, files) = files();
    let (table, ids) = table(&files, 0, 100);
    {
        let _freezing = super::freeze::Freezing::from(&table.frozen.freezing_from, ids[50]);
        changed(&table, (&[], &[ids[10], ids[60]]), 10);
    }
    assert!(table.tree.get(&key(ids[60])).unwrap().is_some(), "kept: it may be frozen");
    assert!(table.tree.get(&key(ids[10])).unwrap().is_none(), "collected: it is not");
    changed(&table, (&[], &[ids[70]]), 12);
    assert!(table.tree.get(&key(ids[70])).unwrap().is_none(), "collected: no freeze under way");
    assert!(table.get(NO_TRANSACTION, ids[60]).unwrap().is_none());
    assert_eq!(entries(&table), 98, "the deletion kept among the rows");
}

/// Verifies: [DESIGN] RULES.md D31, a read by batch as entries are taken
/// out: the rows of a group the tree still holds entries for are read
/// from them with the group's vector, so none is missed nor read twice
/// when they are taken out before the scan reaches the tree's rows.
#[test]
fn a_read_by_batch_misses_no_entry_taken_out() {
    let (_, files) = files();
    let (table, _) = table(&files, 0, 5000);
    let held = table.frozen.directory().unwrap();
    ended(&table, 1);
    assert_eq!(entries(&table), 5000, "kept for the directory held");
    let mut batches = Arc::clone(&table).scan_batches(NO_TRANSACTION, &[0], None);
    let first = batches.next().unwrap().unwrap().rows;
    drop(held);
    table.retire_unread().unwrap();
    assert_eq!(entries(&table), 5000 - 4096, "taken out as it reads");
    let rest: usize = batches.map(|b| b.unwrap().rows).sum();
    assert_eq!(first.checked_add(rest), Some(5000));
}

/// Whether `table`'s first group's rows are in order of their texts.
fn sorted_by_text(table: &RowTable) -> bool {
    let directory = table.frozen.directory().unwrap();
    let group = &*table.group_of(&directory.groups[0]).unwrap();
    let texts = crate::engine::columns::scan::values(group, &table.group_pages().unwrap(), (1, &TYPES[1]), (0, group.rows)).unwrap();
    group.sorted() && texts.windows(2).all(|w| crate::value::sort_cmp(&w[0], &w[1]).is_le())
}

/// Verifies: [DESIGN] RULES.md D31, COLUMNSTORE.ORDER: a table whose
/// groups are sorted by a column freezes each group's rows in its order;
/// a row is found by its id, a scan gives every row in row id order, and
/// batches give every row; once rows of a group are changed and deleted,
/// it is written again sorted, the changes in it.
#[test]
fn groups_are_sorted_by_their_order() {
    let (_, files) = files();
    let mut options = options(0);
    if let Some(columns) = options.columns.as_mut() {
        columns.order = Places::of(&[1]);
    }
    let table = Arc::new(RowTable::create(&files, &TYPES, options).unwrap());
    let ids: Vec<RowId> = (0..5000).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
    ended(&table, 1);
    assert!(sorted_by_text(&table), "sorted as frozen");
    assert_eq!(table.get(NO_TRANSACTION, ids[1234]).unwrap().map(|r| r[1].to_string()), Some("v1234".to_owned()));
    let in_order: Vec<String> = (0_i32..5_000_i32).map(|n| n.to_string()).collect();
    assert_eq!(scanned(&table, NO_TRANSACTION), in_order, "in row id order");
    let batched: usize = Arc::clone(&table).scan_batches(NO_TRANSACTION, &[0], None).map(|b| b.unwrap().rows).sum();
    assert_eq!(batched, 5000);
    changed(&table, (&ids[0..400], &ids[400..500]), 10);
    assert!(sorted_by_text(&table), "sorted as written again");
    assert_eq!(table.frozen.directory().unwrap().groups[0].summary.rows, 2048 - 100);
    assert_eq!(table.get(NO_TRANSACTION, ids[3]).unwrap().map(|r| r[1].to_string()), Some("v-3".to_owned()));
    assert!(table.get(NO_TRANSACTION, ids[450]).unwrap().is_none());
}

/// Verifies: [DESIGN] RULES.md D31, COLUMNSTORE.ORDER changed: groups
/// frozen in row id order record none; once the table's order names a
/// column, the maintenance thread writes each group again sorted by it,
/// a couple a call, until every one records it and holds its rows so;
/// every row is found by its id and a scan gives them in row id order, as
/// before; the order named no column again, they are written in row id
/// order.
#[test]
fn groups_are_sorted_again_when_their_order_changes() {
    let (_, files) = files();
    let table = Arc::new(RowTable::create(&files, &TYPES, options(0)).unwrap());
    let ids: Vec<RowId> = (0..7000).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
    ended(&table, 1);
    let orders =
        |t: &RowTable| -> Vec<Vec<usize>> { t.frozen.directory().unwrap().groups.iter().map(|e| e.summary.order.clone()).collect() };
    assert_eq!(orders(&table), vec![Vec::<usize>::new(); 3], "frozen in row id order");
    let mut sorted = options(0).columns.unwrap();
    sorted.order = Places::of(&[1]);
    table.frozen.reconfigure(Some(sorted));
    assert!(table.maintenance_due(), "groups to sort");
    assert!(table.maintain(1).unwrap());
    assert_eq!(orders(&table).iter().filter(|o| o.as_slice() == [1]).count(), 2, "a couple a call");
    ended(&table, 1);
    assert_eq!(orders(&table), vec![vec![1]; 3], "every group sorted");
    assert!(sorted_by_text(&table) && !table.maintenance_due());
    assert_eq!(table.get(NO_TRANSACTION, ids[4321]).unwrap().map(|r| r[1].to_string()), Some("v4321".to_owned()));
    let in_order: Vec<String> = (0_i32..7_000_i32).map(|n| n.to_string()).collect();
    assert_eq!(scanned(&table, NO_TRANSACTION), in_order, "in row id order");
    table.frozen.reconfigure(options(0).columns);
    ended(&table, 1);
    assert_eq!(orders(&table), vec![Vec::<usize>::new(); 3], "in row id order again");
    assert_eq!(scanned(&table, NO_TRANSACTION), in_order);
}

/// Verifies: [DESIGN] RULES.md D31, headers read as they are needed: a
/// table opened again holds only its groups' summaries, no header read; a
/// scan whose condition no group's summary may match, by rows or by
/// batches, reads no header and gives no frozen row; one that reads a
/// group holds its header, and gives what the table holds.
#[test]
fn headers_are_read_as_they_are_needed() {
    let (_, files) = files();
    let (table, _) = table(&files, 0, 5000);
    ended(&table, 1);
    assert_eq!(files.headers.held().1, 2, "the headers written held");
    let cold = TableFiles { headers: Arc::default(), ..files.clone() };
    let opened = Arc::new(RowTable::open(&cold, table.meta(), &TYPES, options(0)).unwrap());
    assert_eq!(cold.headers.held(), (0, 0), "summaries only");
    let directory = opened.frozen.directory().unwrap();
    assert_eq!(directory.groups.iter().map(|e| (e.summary.first, e.summary.rows)).collect::<Vec<_>>(), [(1024, 2048), (3072, 2048)]);
    let over = |n: i128| {
        Conjunction::of(Condition { columns: vec![0], probe: Probe::Compare { op: CompOp::Gt, value: Value::Exact(Decimal::new(n, 0)) } })
    };
    let rows = |c: &Conjunction| Arc::clone(&opened).scan_where(NO_TRANSACTION, &[0], c).count();
    let batched = |c: &Conjunction| Arc::clone(&opened).scan_batches(NO_TRANSACTION, &[0], Some(c)).map(|b| b.unwrap().rows).sum::<usize>();
    assert_eq!((rows(&over(4500)), batched(&over(4500))), (499, 499), "the tree's rows only");
    assert_eq!(cold.headers.held(), (0, 0), "no group may match: no header read");
    // Batches give the rows of the parts that may match, for the filter
    // to judge.
    assert_eq!(rows(&over(3000)), 1999);
    assert!((1999..5000).contains(&batched(&over(3000))));
    let (bytes, held) = cold.headers.held();
    assert!(held == 1 && bytes > 0, "the second group's header held: {bytes} bytes");
    assert_eq!(scanned(&opened, NO_TRANSACTION).len(), 5000);
    assert_eq!(cold.headers.held().1, 2);
}
