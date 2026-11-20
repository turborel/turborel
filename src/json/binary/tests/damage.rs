//! Robustness of the stored form of a JSON value holding every kind of
//! scalar: cut short or damaged, it is refused by whatever reads it, or
//! read as some document, never a panic.

use super::*;
use crate::value::Datetime;
use crate::value::datetime::Moment;

/// An array of every kind of scalar and an object holding them again.
fn every_kind() -> Item {
    let datetime = Value::Datetime(Datetime { moment: Moment::Timestamp, utc: 86_400_000_000, zone: 60, zoned: true, precision: 3 });
    let scalars = vec![
        scalar(Value::Exact(Decimal::new(7, 0))),
        scalar(Value::Exact(Decimal::new(-125, 2))),
        scalar(Value::Exact(Decimal::new(i128::from(i64::MAX).checked_mul(1000).unwrap(), 3))),
        scalar(Value::DecFloat(DecFloat::new(15, -1, 34).unwrap())),
        scalar(Value::Approx(-0.25)),
        scalar(Value::Char("s\u{e9}".to_owned())),
        scalar(Value::Bool(true)),
        scalar(datetime),
        Item::Null,
    ];
    let members = scalars.iter().enumerate().map(|(n, v)| Member { key: format!("k{n}"), value: v.clone() }).collect();
    Item::Array(vec![Item::Array(scalars), Item::Object(members)])
}

/// That `r` is a value, or the error of a damaged document, SQLCODE -916.
fn corrupt_or_ok<T>(r: Result<T>, at: usize) {
    if let Err(e) = r {
        assert_eq!(e.sqlcode(), -916_i32, "byte {at}: {e:?}");
    }
}

/// Every reader of a node run on `node` and its children.
fn exercise(node: Node<'_>, at: usize, depth: usize) {
    corrupt_or_ok(node.kind(), at);
    corrupt_or_ok(node.len(), at);
    corrupt_or_ok(node.length(), at);
    corrupt_or_ok(node.scalar(), at);
    corrupt_or_ok(node.as_str(), at);
    corrupt_or_ok(node.member("k3"), at);
    corrupt_or_ok(node.member("zz"), at);
    corrupt_or_ok(node.element(1), at);
    corrupt_or_ok(node.item(), at);
    if depth > 2 {
        return;
    }
    if let Ok(Some(elements)) = node.elements() {
        for e in elements {
            exercise(e, at, depth.saturating_add(1));
        }
    }
    if let Ok(Some(members)) = node.members() {
        for (_, v) in members {
            exercise(v, at, depth.saturating_add(1));
        }
    }
}

/// Verifies: [DESIGN] RULES.md D89, R-ERR-2: a document of every kind of
/// scalar reads back as written; cut at any byte it is refused with
/// SQLCODE -916 as it is validated, decoded and keyed.
#[test]
fn every_document_prefix_is_refused() {
    let item = every_kind();
    let bytes = encode(&item).unwrap();
    validate(&bytes).unwrap();
    assert!(decode(&bytes).unwrap().equivalent(&item).unwrap());
    for cut in 0..bytes.len() {
        let part = &bytes[..cut];
        assert_eq!(validate(part).unwrap_err().sqlcode(), -916_i32, "cut at {cut}");
        corrupt_or_ok(decode(part), cut);
        corrupt_or_ok(document_key(part), cut);
        if let Ok(node) = root(part) {
            exercise(node, cut, 0);
        }
    }
}

/// Verifies: [DESIGN] RULES.md D89, R-ERR-2: a document of every kind of
/// scalar with any one byte set to 0x00 or 0xFF is refused with SQLCODE
/// -916 by every reader of its nodes, or read as some document, which
/// then decodes and keys; none panics.
#[test]
fn damaged_documents_are_refused_or_read() {
    let bytes = encode(&every_kind()).unwrap();
    for at in 0..bytes.len() {
        for byte in [0x00, 0xFF] {
            let mut bad = bytes.clone();
            bad[at] = byte;
            if validate(&bad).is_ok() {
                assert!(decode(&bad).is_ok() && document_key(&bad).is_ok(), "byte {at}");
            }
            corrupt_or_ok(decode(&bad), at);
            if let Ok(node) = root(&bad) {
                exercise(node, at, 0);
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D89, R-ERR-2: an object whose member count
/// is damaged to far beyond its octets is refused with SQLCODE -916 as
/// its members are read and as it is decoded. Regression: room was made
/// for every member the count named, and the allocation aborted.
#[test]
fn a_damaged_member_count_is_refused() {
    let mut bytes = stored("{\"a\":1,\"b\":2}");
    bytes[2..6].copy_from_slice(&0xFF00_0009_u32.to_le_bytes());
    assert_eq!(root(&bytes).unwrap().members().unwrap_err().sqlcode(), -916_i32);
    assert_eq!(decode(&bytes).unwrap_err().sqlcode(), -916_i32);
    assert_eq!(validate(&bytes).unwrap_err().sqlcode(), -916_i32);
}
