//! Unit tests of the hash index: equality probes with values of the
//! column's type and not, a comparison with NULL, estimates, counted pairs,
//! entries too long, reopening by locator, and a dropped index's pages
//! freed (RULES.md R-TEST-1, R-TEST-2). Split from hash.rs (RULES.md
//! R-SIZE-4).

use super::*;
use crate::value::Decimal;

const MONEY: DataType = DataType::Exact { precision: 6, scale: 2 };
const CODE: DataType = DataType::Character { length: 3 };

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

fn eq(value: Value) -> Probe {
    Probe::Compare { op: CompOp::Eq, value }
}

/// The rows `probe` finds in `index`, sorted.
fn found(index: &dyn Index, probe: &Probe) -> Vec<RowId> {
    let mut rows: Vec<RowId> = index.search(probe).unwrap().into_iter().map(|(_, row)| row).collect();
    rows.sort_unstable();
    rows
}

/// Verifies: [X3.135-1986 §5.11 GR2, GR3]; [DESIGN] `index/hash.rs`: on a
/// DECIMAL(6,2) column, a value of another scale but equal (10.5), and one
/// of a finer scale (10.555), and an approximate one, find exactly the rows
/// SQL finds equal; on a CHAR(3) column, a string with trailing spaces
/// finds its key, a longer one none; a comparison with NULL finds none.
/// Estimates are exact for one key, and count every entry for all.
#[test]
fn probes_find_what_sql_finds_equal() {
    let method = HashMethod::in_memory();
    let money = method.create(&[MONEY]).unwrap();
    for (row, units) in (1..).zip([1_050, 1_050, 1_055, 2_000]) {
        money.insert(Key(vec![exact(units, 2)]), row).unwrap();
    }
    assert_eq!(found(&*money, &eq(exact(105, 1))), vec![1, 2], "10.5 = 10.50");
    assert_eq!(found(&*money, &eq(exact(10_555, 3))), Vec::<RowId>::new(), "10.555 is no DECIMAL(6,2)");
    assert_eq!(found(&*money, &eq(Value::Approx(20.0))), vec![4], "20E0 = 20.00");
    assert_eq!(found(&*money, &eq(Value::Null)), Vec::<RowId>::new());
    assert_eq!(found(&*money, &Probe::All), vec![1, 2, 3, 4]);
    assert_eq!(money.estimate(&eq(exact(1_050, 2))), Some(2));
    assert_eq!(money.estimate(&eq(exact(10_555, 3))), Some(0));
    assert_eq!(money.estimate(&eq(Value::Null)), Some(0));
    assert_eq!(money.estimate(&Probe::All), Some(4));
    assert_eq!(money.estimate(&eq(Value::Approx(20.0))), None, "every entry checked: not cheap");
    let code = method.create(&[CODE]).unwrap();
    code.insert(Key(vec![Value::Char("AB".into())]), 7).unwrap();
    assert_eq!(found(&*code, &eq(Value::Char("AB ".into()))), vec![7]);
    assert_eq!(found(&*code, &eq(Value::Char("ABCD".into()))), Vec::<RowId>::new());
    let prefix = Probe::Prefix { equal: vec![Value::Char("AB".into())], then: None };
    assert_eq!(found(&*code, &prefix), vec![7]);
}

/// Verifies: [DESIGN] `Index`, counted pairs: a pair inserted twice is
/// found once and stays until removed twice; `rows` gives each row once; a
/// key too long for a page is refused (SQLCODE -900).
#[test]
fn pairs_are_counted() {
    let method = HashMethod::in_memory();
    let index = method.create(&[CODE]).unwrap();
    let k = Key(vec![Value::Char("K".into())]);
    for row in [1, 1, 2] {
        index.insert(k.clone(), row).unwrap();
    }
    assert_eq!(index.rows(&k).unwrap(), vec![1, 2]);
    assert_eq!(found(&*index, &eq(Value::Char("K".into()))), vec![1, 2]);
    index.remove(&k, 1).unwrap();
    assert_eq!(index.rows(&k).unwrap(), vec![1, 2], "one entry of row 1 left");
    index.remove(&k, 1).unwrap();
    index.remove(&k, 9).unwrap();
    assert_eq!(index.rows(&k).unwrap(), vec![2]);
    let long = method.create(&[DataType::Character { length: 9_000 }]).unwrap();
    let refused = long.insert(Key(vec![Value::Char("x".repeat(9_000))]), 1).err().map(|e| e.code);
    assert_eq!(refused, Some(Code::Unsupported));
}

/// Verifies: [DESIGN] `IndexMethod::open`, `checkpoint`, `Index::discard`:
/// an index checkpointed is opened again by its locator with its entries;
/// a locator that is no page number is refused (SQLCODE -916); a dropped
/// index's pages are freed when it goes, so its table can be opened no more.
#[test]
fn indexes_reopen_and_are_freed() {
    let method = HashMethod::in_memory();
    let index = method.create(&[MONEY]).unwrap();
    for row in 0..500 {
        index.insert(Key(vec![exact(i128::from(row), 0)]), row).unwrap();
    }
    method.checkpoint().unwrap();
    let locator = index.locator();
    let again = method.open(&[MONEY], &locator).unwrap().unwrap();
    assert_eq!(found(&*again, &eq(exact(123, 0))), vec![123]);
    assert_eq!(method.open(&[MONEY], b"no").err().map(|e| e.code), Some(Code::Corrupt));
    drop(again);
    index.discard();
    drop(index);
    assert!(method.open(&[MONEY], &locator).is_err(), "its meta page is freed");
}
