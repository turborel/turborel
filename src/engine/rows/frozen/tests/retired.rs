//! Unit tests of the values a row keeps out of line, read while a freeze
//! takes its entry out of the tree (`freeze.rs`, RULES.md D1140): rows
//! read by batch and by id on six threads, many rounds of rows
//! written and frozen meanwhile.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use super::*;
use crate::storage::Batch;

/// A key and a value kept out of line, as the pages are of 4096 bytes.
const LONG: [DataType; 2] = [DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, DataType::varying(4000)];

/// Row `n`, its value long, and made of `n` alone.
fn long(n: i128) -> Row {
    vec![Value::Exact(Decimal::new(n, 0)), Value::Char(format!("{n:0>1500}").into())]
}

/// Whether `key` and `value` are a row `long` makes.
fn whole(key: &Value, value: &Value) -> bool {
    key.to_string().parse::<i128>().is_ok_and(|n| long(n).get(1).is_some_and(|v| v.to_string() == value.to_string()))
}

/// The rows of `batches` that are not whole, and every error, counted.
fn broken(batches: impl Iterator<Item = crate::error::Result<Batch>>) -> usize {
    let mut broken = 0_usize;
    for batch in batches {
        let Ok(mut batch) = batch else {
            broken = broken.saturating_add(1);
            continue;
        };
        if batch.resolve(None, &crate::kernels::Portable).is_err() {
            broken = broken.saturating_add(1);
            continue;
        }
        broken =
            broken.saturating_add((0..batch.rows).filter(|&r| !whole(&batch.cell(0, r).to_value(), &batch.cell(1, r).to_value())).count());
    }
    broken
}

/// Reads `table` by batch, or the last group's worth of rows written by
/// id, until `done`: how many reads went wrong. Rows are given ids one
/// after another from `first`, `written` of them so far.
fn reading(table: &Arc<RowTable>, (first, written): (RowId, &AtomicU64), done: &AtomicBool, by_batch: bool) -> usize {
    let mut wrong = 0_usize;
    while !done.load(Ordering::Acquire) {
        wrong = wrong.saturating_add(if by_batch {
            broken(Arc::clone(table).scan_batches(NO_TRANSACTION, &[0, 1], None, &[]))
        } else {
            let end = first.saturating_add(written.load(Ordering::Acquire));
            let read = |id: RowId| table.get(NO_TRANSACTION, id).is_ok_and(|r| r.is_some_and(|r| whole(&r[0], &r[1])));
            (end.saturating_sub(2048).max(first)..end).filter(|&id| !read(id)).count()
        });
    }
    wrong
}

/// One round: rows written and frozen, group by group, while six threads
/// read them: how many reads went wrong.
fn round() -> usize {
    let (_, files) = files();
    let table = Arc::new(RowTable::create(&files, &LONG, options(0)).unwrap());
    let first = table.insert(NO_TRANSACTION, long(0)).unwrap();
    let written = AtomicU64::new(1);
    let done = AtomicBool::new(false);
    let (shared, counted, ended) = (&table, &written, &done);
    thread::scope(|s| {
        let readers =
            [true, false, true, false, true, false].map(|by_batch| s.spawn(move || reading(shared, (first, counted), ended, by_batch)));
        for g in 0..12_i128 {
            // Pages the last freeze freed are taken again by these.
            for n in g.saturating_mul(2048).max(1)..g.saturating_add(1).saturating_mul(2048) {
                assert_eq!(table.insert(NO_TRANSACTION, long(n)).unwrap(), first.saturating_add(n.unsigned_abs().try_into().unwrap()));
                written.fetch_add(1, Ordering::Release);
            }
            while table.maintain(1, true).unwrap() {}
        }
        done.store(true, Ordering::Release);
        readers.into_iter().map(|r| r.join().unwrap()).sum()
    })
}

/// Verifies: [DESIGN] RULES.md D1140, a value out of line freed only once
/// no read may still fetch it, under load. Rows whose values are kept out
/// of line are read by batch and by id on six threads while a freeze
/// takes their entries out of the tree, and rows written meanwhile take
/// again the pages freed; every read gives each row whole, over as many
/// rounds as TURBOREL_RETIRED_ROUNDS says, 4 if unset. The window it hunts
/// is a few microseconds wide, so the tests by step below are the ones
/// that fail without the fix.
#[test]
fn a_value_out_of_line_outlives_its_readers() {
    let rounds: usize = std::env::var("TURBOREL_RETIRED_ROUNDS").ok().and_then(|r| r.parse().ok()).unwrap_or(4);
    let wrong: usize = (0..rounds).map(|_| round()).sum();
    assert_eq!(wrong, 0, "reads that went wrong over {rounds} rounds");
}

/// The record of row `id`'s entry in the tree, copied out as a read
/// copies one whose values are kept out of line.
fn copied(table: &RowTable, id: RowId) -> Vec<u8> {
    let entry = table.tree.get(&key(id)).unwrap().unwrap();
    crate::engine::rows::version::Version::parse(&entry).unwrap().record.to_vec()
}

/// A freeze put step by step, a read made between its group's place in
/// the directory and its entries taken out: by row id, which holds the
/// directory as it begins, or by a scan, which takes it, the group's bits
/// not yet marked. Each copies a row's entry, then fetches its value once
/// the entries are out and rows written after take pages again: whether
/// it is whole. Once the read lets the directory go, the next freeze tried
/// frees the values' pages.
fn copied_then_frozen(by_scan: bool) -> bool {
    let (_, files) = files();
    let table = Arc::new(RowTable::create(&files, &LONG, options(0)).unwrap());
    let ids: Vec<RowId> = (0..2100).map(|n| table.insert(NO_TRANSACTION, long(n)).unwrap()).collect();
    let columns = table.frozen.columns().unwrap();
    let frozen = table.eligible(None, 1, 2048).unwrap();
    let made = table.made(table.written(&frozen, &columns).unwrap()).unwrap();
    let put = table.put((None, Some(made))).unwrap();
    let held = if by_scan { Some(table.frozen.directory().unwrap()) } else { table.frozen.pinned() };
    let record = copied(&table, ids[9]);
    table.marked(put, frozen).unwrap();
    assert_eq!(entries(&table), 52, "the frozen rows' entries taken out");
    for n in 2100..2400 {
        table.insert(NO_TRANSACTION, long(n)).unwrap();
    }
    let whole_after = table.decode(ids[9], &record, None).is_ok_and(|row| whole(&row[0], &row[1]));
    drop(held);
    table.retire_unread().unwrap();
    assert!(crate::sync::lock_builtin(&table.frozen.freeing).is_empty(), "freed once it is done");
    whole_after
}

/// Verifies: [DESIGN] RULES.md D1140, step by step, a read by row id: it
/// fetches the value of the entry it copied whole after the freeze took
/// the entry out, and the value's page is freed once it is done. A
/// regression test (R-TEST-7) that fails if a read by row id holds no
/// directory, or a freeze frees a value while one is held: the read then
/// fetches a page not allocated, or another row's.
#[test]
fn a_read_by_row_id_fetches_its_value_after_the_freeze() {
    assert!(copied_then_frozen(false));
}

/// Verifies: [DESIGN] RULES.md D1140, step by step, a scan that took the
/// directory before the group's bits were marked: it fetches the value of
/// the entry it copied whole after the freeze took the entry out. A
/// regression test (R-TEST-7) that fails if a freeze frees a value as
/// soon as no reader holds a directory from before its group.
#[test]
fn a_scan_fetches_its_value_after_the_freeze() {
    assert!(copied_then_frozen(true));
}
