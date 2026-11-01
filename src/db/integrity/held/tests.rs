//! Unit tests of `db/integrity/held.rs`: each effect and each check
//! written and read back as it was; a record not one refused (RULES.md
//! R-TEST-1, R-TEST-2).

use super::*;
use crate::error::Code;
use crate::value::Value;

/// `S.<ident>`.
fn named(ident: &str) -> QualifiedName {
    QualifiedName { auth: "S".to_owned(), ident: ident.to_owned() }
}

/// A row of two values, one null.
fn old() -> Vec<Value> {
    vec![Value::Char("old".into()), Value::Null]
}

/// `item` as its record gives it back, and that record.
fn again<T: Holdable>(item: &T) -> (T, Vec<u8>) {
    let record = item.record();
    (T::read(&record, &TypeRegistry::default()).unwrap(), record)
}

/// Verifies: [DESIGN] RULES.md D38, `Holdable for Effect`: each effect a
/// statement did is read back from its record as it was, and counts its
/// old row in its bytes; a record of no effect, or with bytes past it, is
/// SQLCODE -916.
#[test]
fn effects_are_read_back() {
    let effects = [
        Effect::Inserted { table: named("T"), id: 7 },
        Effect::Updated { table: named("T"), id: 8, old: old(), columns: vec![0, 1] },
        Effect::Deleted { table: named("U"), id: 9, old: old() },
    ];
    for effect in &effects {
        let (read, record) = again(effect);
        assert_eq!(read.record(), record);
        assert!(effect.bytes() > record.len().saturating_sub(64), "{}", effect.bytes());
    }
    assert!(effects[1].bytes() > effects[0].bytes(), "the old row counted");
    let (Effect::Updated { table, id, old: was, columns }, _) = again(&effects[1]) else { panic!("not updated") };
    assert_eq!((table, id, was.len(), columns), (named("T"), 8_u64, 2_usize, vec![0_usize, 1_usize]));
    let mut unknown = effects[0].record();
    unknown[0] = 9_u8;
    assert_eq!(Effect::read(&unknown, &TypeRegistry::default()).err().map(|e| e.code), Some(Code::Corrupt));
    let mut longer = effects[0].record();
    longer.push(0_u8);
    assert_eq!(Effect::read(&longer, &TypeRegistry::default()).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D38, `Holdable for Check`: each check of a
/// constraint is read back from its record as it was, a NO ACTION check's
/// row id or none among them; a record of no check is SQLCODE -916.
#[test]
fn checks_are_read_back() {
    let checks = [
        Check::Row { table: named("T"), constraint: named("C"), id: 1 },
        Check::Unique { table: named("T"), constraint: named("K"), id: 2 },
        Check::Table { table: named("T"), constraint: named("C2") },
        Check::Referenced { child: named("U"), constraint: named("F"), old: old(), id: Some(3) },
        Check::Referenced { child: named("U"), constraint: named("F"), old: old(), id: None },
        Check::Assertion { constraint: named("A") },
        Check::AssertedRow { table: named("T"), constraint: named("A"), id: 4, before: None },
        Check::AssertedRow { table: named("T"), constraint: named("A"), id: 5, before: Some(old()) },
    ];
    for check in &checks {
        let (read, record) = again(check);
        assert_eq!(read.record(), record);
        assert_eq!((read.constraint(), read.table()), (check.constraint(), check.table()));
        assert!(check.bytes() > 0);
    }
    for (at, id) in [(3_usize, Some(3_u64)), (4_usize, None)] {
        let (Check::Referenced { id: read, .. }, _) = again(&checks[at]) else { panic!("not referenced") };
        assert_eq!(read, id);
    }
    assert!(matches!(again(&checks[1]).0, Check::Unique { id: 2, .. }));
    assert!(matches!(again(&checks[0]).0, Check::Row { id: 1, .. }));
    asserted_rows_are_read_back(&checks[5..]);
    let mut unknown = checks[2].record();
    unknown[0] = 9_u8;
    assert_eq!(Check::read(&unknown, &TypeRegistry::default()).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Of an assertion's check, and of its rows', `checks`: each row's read
/// back with its table and id, and the row before it, if kept, each name
/// and row counted among its bytes.
fn asserted_rows_are_read_back(checks: &[Check]) {
    let (Check::AssertedRow { table, id, before, .. }, _) = again(&checks[1]) else { panic!("not an asserted row") };
    assert_eq!((table, id, before.is_none()), (named("T"), 4_u64, true));
    assert!(checks[1].bytes() > checks[0].bytes(), "its table's name counted");
    let (Check::AssertedRow { id: kept, before: was, .. }, _) = again(&checks[2]) else { panic!("not an asserted row") };
    assert_eq!((kept, format!("{was:?}")), (5_u64, format!("{:?}", Some(old()))));
    assert!(checks[2].bytes() > checks[1].bytes(), "the row before counted");
}
