use std::fmt::Debug;
use std::sync::Arc;

use super::*;
use crate::error::Code;
use crate::spill::testing::{Held, areas};
use crate::value::Decimal;

fn number(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A space holding about four rows of `row(..)` before it spills, of runs
/// `broken` or not.
fn space(broken: bool) -> ListSpace {
    ListSpace {
        limit: row_bytes(&row(0)).saturating_mul(4),
        batch: 4096,
        areas: Some(areas(Some(Arc::new(Held { broken })), usize::MAX)),
        types: TypeRegistry::default(),
    }
}

fn row(n: u64) -> Row {
    vec![number(n), Value::Char(format!("row {n}"))]
}

/// `item`, as its debugging form shows it: values compare so.
fn shown<T: Debug>(item: &T) -> String {
    format!("{item:?}")
}

/// `n` items made by `item`, held in `space`.
fn held<T: Holdable>(n: u64, space: ListSpace, item: fn(u64) -> T) -> HeldRows<T> {
    let mut rows = HeldRows::new(space);
    for i in 0..n {
        rows.push(item(i)).unwrap();
    }
    rows
}

/// Every item of `rows`, read back in order, as shown.
fn read_back<T: Holdable + Debug>(rows: HeldRows<T>) -> String {
    shown(&rows.into_iter().collect::<Result<Vec<_>>>().unwrap())
}

/// Verifies: [DESIGN] RULES.md D38, `HeldRows`: rows past the space's
/// bytes are written to runs; by place, forwards and back, and read back
/// in order, once or kept, they are the rows pushed; none past the last.
#[test]
fn rows_held_past_their_bytes_are_spilled_and_read_back() {
    let mut rows = held(25, space(false), row);
    assert!(rows.spilled());
    assert_eq!(rows.len(), 25);
    for i in (0..25_u64).chain((0..25_u64).rev()) {
        let place = usize::try_from(i).unwrap();
        assert_eq!(shown(&rows.get(place).unwrap()), shown(&Some(row(i))), "place {i}");
    }
    assert!(rows.get(25).unwrap().is_none());
    let mut seen = Vec::new();
    rows.visit(&mut |r| {
        seen.push(r.clone());
        Ok(())
    })
    .unwrap();
    let all = shown(&(0..25).map(row).collect::<Vec<_>>());
    assert_eq!(shown(&seen), all);
    assert_eq!(read_back(rows), all);
    let few = held(2, space(false), row);
    assert!(!few.spilled() && !few.is_empty());
    assert!(!HeldRows::<Row>::new(space(false)).spilled());
}

/// Verifies: [DESIGN] RULES.md D38: rows with ids, an UPDATE's changes and
/// ids alone are written and read back as they were.
#[test]
fn entries_changes_and_ids_are_read_back() {
    let entry = |n: u64| (n, row(n));
    let change = |n: u64| (n, row(n), row(n + 1));
    let entries = held(25, space(false), entry);
    assert!(entries.spilled());
    assert_eq!(read_back(entries), shown(&(0..25).map(entry).collect::<Vec<_>>()));
    let changes = held(25, space(false), change);
    assert!(changes.spilled());
    assert_eq!(read_back(changes), shown(&(0..25).map(change).collect::<Vec<_>>()));
    let ids = held(2000, ListSpace { limit: 64, ..space(false) }, |n| n);
    assert!(ids.spilled());
    assert_eq!(read_back(ids), shown(&(0..2000).collect::<Vec<u64>>()));
}

/// Verifies: [DESIGN] a spilled segment that cannot be read back is
/// reported where it is read, by place or in order, and the rows after it
/// are still given; a record of another kind is SQLCODE -916, not a panic.
#[test]
fn a_run_not_read_back_is_reported() {
    let mut rows = held(25, space(true), row);
    assert_eq!(rows.get(0).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(shown(&rows.get(24).unwrap()), shown(&Some(row(24))), "still held");
    let read: Vec<Result<Row>> = rows.into_iter().collect();
    assert!(read.iter().any(|r| r.as_ref().err().map(|e| e.code) == Some(Code::Io)));
    assert_eq!(read.last().map(|r| shown(r.as_ref().unwrap())), Some(shown(&row(24))));
    let types = TypeRegistry::default();
    let record = row(1).record();
    assert_eq!(RowId::read(&record, &types).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(<(RowId, Row)>::read(&record, &types).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(<(RowId, Row, Row)>::read(&7_u64.record(), &types).err().map(|e| e.code), Some(Code::Corrupt));
    let mut longer = record.clone();
    longer.push(0);
    assert_eq!(Row::read(&longer, &types).err().map(|e| e.code), Some(Code::Corrupt), "bytes after its end");
}
