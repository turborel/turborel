//! Unit tests of log records: every kind of record read back as written,
//! and what is not a record refused (RULES.md R-TEST-1, R-TEST-2). Split
//! from record.rs (RULES.md R-SIZE-2).

use super::*;
use crate::value::types::TypeRegistry;
use crate::value::{Decimal, Value};

fn table() -> QualifiedName {
    QualifiedName { auth: "AUTH".into(), ident: "TÄBLE".into() }
}

/// Values at the edges of each kind.
fn edge_row() -> Row {
    vec![
        Value::Null,
        Value::Char(String::new()),
        Value::Char("héllo ✓".into()),
        Value::Exact(Decimal::new(i128::MIN, 0)),
        Value::Exact(Decimal::new(i128::MAX, 18)),
        Value::Approx(f64::NAN),
        Value::Approx(-0.0),
        Value::Approx(f64::INFINITY),
        Value::Approx(1.5),
    ]
}

/// One record of every kind.
fn every_kind() -> Vec<Record> {
    let change = [
        Change::Insert { table: table(), generation: 0, row: 7, after: edge_row() },
        Change::Update { table: table(), generation: u32::MAX, row: u64::MAX, before: vec![], after: edge_row() },
        Change::Delete { table: table(), generation: 7, row: 0, before: edge_row() },
        Change::Commit { at: 42 },
        Change::Abort,
        Change::Checkpoint,
        Change::StatementUndone { from: Lsn(u64::MAX) },
        Change::Stale { through: Lsn(9), index: "I".into(), row: u64::MAX, key: edge_row() },
        Change::Schema {
            sql: "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)".into(),
            owner: "A".into(),
            engines: vec![("T".into(), "rowstore".into())],
            zone: -330,
        },
        Change::Defined {
            auth: "A".into(),
            schema: "B".into(),
            zone: 780,
            sql: "CREATE TABLE T (K INTEGER)".into(),
            placed: Some(("rowstore".into(), u32::MAX)),
        },
        Change::Defined { auth: String::new(), schema: String::new(), zone: 0, sql: "GRANT SELECT ON T TO B".into(), placed: None },
        Change::CreateIndex {
            name: "I".into(),
            table: table(),
            columns: vec!["K".into(), "L".into()],
            method: "btree".into(),
            options: vec![("FILL".into(), Value::Approx(0.5)), ("SHAPE".into(), Value::Char("TALL".into()))],
        },
        Change::AlterIndex { name: "I".into(), options: vec![] },
        Change::DropIndex { name: "I".into() },
        Change::Drop { object: DropTarget::Table(table()), cascade: true },
        Change::Drop { object: DropTarget::View(table()), cascade: false },
        Change::Drop { object: DropTarget::Schema("AUTH".into()), cascade: true },
        Change::Drop { object: DropTarget::Domain(table()), cascade: false },
        Change::Drop { object: DropTarget::Assertion(table()), cascade: true },
        Change::AlterTable { table: table(), action: "ADD COLUMN \"K\" INTEGER NOT NULL".into(), schema: "S".into(), zone: -779 },
        Change::Setting { name: "LOCKS.WAIT".into(), value: Some(Value::Char("ON".into())) },
        Change::Setting { name: "OPTIMIZER.MAX_PASSES".into(), value: None },
        Change::User { name: "ANN".into(), verifier: Some(vec![0, 255, 7]) },
        Change::User { name: "ANN".into(), verifier: Some(vec![]) },
        Change::User { name: "ANN".into(), verifier: None },
        Change::Owner { name: "ANN".into() },
    ];
    change.into_iter().enumerate().map(|(n, change)| Record { tx: u64::try_from(n).unwrap().checked_add(1).unwrap(), change }).collect()
}

/// Verifies: [DESIGN] RULES.md D25, log records: every kind of record,
/// with values at the edges of every kind (empty and non-ASCII strings,
/// the extremes of exact numbers, NaN, -0.0 and infinity), reads back
/// exactly as written: its bytes, written again, are the same.
#[test]
fn every_kind_of_record_reads_back_as_written() {
    for record in every_kind() {
        let bytes = record.encode();
        let read = Record::decode(&bytes, &TypeRegistry::default()).unwrap();
        assert_eq!(read.encode(), bytes, "{record:?}");
        assert_eq!(read.tx, record.tx);
        assert_eq!(format!("{:?}", read.change), format!("{:?}", record.change));
    }
}

/// Verifies: [DESIGN] RULES.md D25, log records: the format is fixed: a
/// commit record is version, kind, transaction and commit timestamp,
/// little-endian.
#[test]
fn the_format_is_fixed() {
    let bytes = Record { tx: 0x0102, change: Change::Commit { at: 0x0A0B } }.encode();
    assert_eq!(bytes, [&[RECORD_VERSION, COMMIT][..], &0x0102_u64.to_le_bytes(), &0x0A0B_u64.to_le_bytes()].concat());
}

/// Verifies: [DESIGN] RULES.md D25, log records: bytes that are not a
/// whole record are refused with SQLCODE -916, never misread: a record
/// cut at any byte, one with a byte after its end, an unknown version,
/// kind of record, value or object dropped, a string that is not UTF-8, a
/// time zone displacement out of range, and a count of values far beyond
/// what follows (refused without allocating for it).
#[test]
fn what_is_not_a_record_is_refused() {
    let corrupt = |bytes: &[u8]| Record::decode(bytes, &TypeRegistry::default()).err().map(|e| e.code);
    for record in every_kind() {
        let bytes = record.encode();
        for cut in 0..bytes.len() {
            assert_eq!(corrupt(&bytes[..cut]), Some(Code::Corrupt), "{record:?} cut at {cut}");
        }
        assert_eq!(corrupt(&[&bytes[..], &[0]].concat()), Some(Code::Corrupt), "a byte after the end");
    }
    let abort = Record { tx: 1, change: Change::Abort }.encode();
    let mut version = abort.clone();
    version[0] = 9;
    assert!(Record::decode(&version, &TypeRegistry::default()).unwrap_err().msg.contains("version 9"));
    let mut kind = abort.clone();
    kind[1] = 99;
    assert_eq!(corrupt(&kind), Some(Code::Corrupt));
    let mut object = Record { tx: 1, change: Change::Drop { object: DropTarget::Schema("S".into()), cascade: false } }.encode();
    object[10] = 9;
    assert_eq!(corrupt(&object), Some(Code::Corrupt), "an unknown kind of object dropped");
    let mut insert = Record { tx: 1, change: Change::Insert { table: table(), generation: 0, row: 1, after: vec![Value::Null] } }.encode();
    let tag = insert.len() - 1;
    insert[tag] = 77;
    assert_eq!(corrupt(&insert), Some(Code::Corrupt), "an unknown kind of value");
    let schema = Record { tx: 1, change: Change::Schema { sql: "ab".into(), owner: String::new(), engines: vec![], zone: 0 } }.encode();
    let mut not_utf8 = schema.clone();
    // "ab", then its engines' count, then its displacement.
    let last = not_utf8.len() - 5 - 16;
    not_utf8[last] = 0xFF;
    assert_eq!(corrupt(&not_utf8), Some(Code::Corrupt));
    let mut far = schema.clone();
    let zone = far.len() - 16;
    far[zone..].copy_from_slice(&i128::MAX.to_le_bytes());
    assert_eq!(corrupt(&far), Some(Code::Corrupt), "a displacement out of range");
    let mut huge = Record { tx: 1, change: Change::Delete { table: table(), generation: 0, row: 1, before: vec![] } }.encode();
    let count = huge.len() - 4;
    huge[count..].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(corrupt(&huge), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D57, `KINDS`: it lists every kind of record
/// and only those: each tag `Record::decode` reads as a kind (any but those
/// it refuses as of no kind) is listed, once, in tag order; and the
/// records of every kind, each of a kind `Change::kind` gives, cover the
/// list, so each is read back as written.
#[test]
fn kinds_are_listed() {
    let known = |tag: u8| match Record::decode(&[&[RECORD_VERSION, tag][..], &1_u64.to_le_bytes()].concat(), &TypeRegistry::default()) {
        Ok(_) => true,
        Err(e) => !e.msg.contains("no kind of record has tag"),
    };
    let read: Vec<u8> = (0_u8..=u8::MAX).filter(|&tag| known(tag)).collect();
    assert_eq!(read, KINDS.to_vec(), "the tags decode reads, in order, are KINDS");
    let mut written: Vec<u8> = every_kind().iter().map(|r| r.change.kind()).collect();
    written.sort_unstable();
    written.dedup();
    assert_eq!(written, KINDS.to_vec(), "every kind is read back as written");
}
