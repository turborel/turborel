//! Declared types in a Description's and a Prepared's tail (RULES.md
//! D396).

use turborel::value::{DataType, Spelled};

use super::{FAILED, kind_of, reply_round_trip};
use crate::{Column, Declared, Reply};

/// A column C of `t`, its declared type told if `told`.
fn column(t: DataType, told: bool) -> Column {
    let c = Column { name: Some("C".into()), data_type: t, not_null: false, declared: None };
    if told { c.with_declared() } else { c }
}

/// The body of `reply`, encoded.
fn body(reply: &Reply) -> Vec<u8> {
    let mut bytes = Vec::new();
    reply.encode(&mut bytes);
    bytes.split_off(crate::HEADER)
}

/// Verifies: [DESIGN] RULES.md D396: each name a number's type is
/// declared with is told, one tag holding several: SMALLINT, INTEGER,
/// BIGINT, DECIMAL and NUMERIC of tag 2, REAL, DOUBLE PRECISION and FLOAT
/// of tag 3; a distinct type is USER-DEFINED and its name; any other type
/// is its own name.
#[test]
fn every_declared_name_is_told() {
    let exact = |p, s, spelled| DataType::Exact { precision: p, scale: s, spelled };
    let approximate = |p, spelled| DataType::Approximate { precision: p, spelled };
    let cases = [
        (exact(5, 0, Spelled::Smallint), "SMALLINT", "SMALLINT"),
        (exact(10, 0, Spelled::Integer), "INTEGER", "INTEGER"),
        (exact(18, 0, Spelled::Bigint), "BIGINT", "BIGINT"),
        (exact(9, 2, Spelled::Decimal), "DECIMAL", "DECIMAL(9,2)"),
        (exact(9, 2, Spelled::Numeric), "NUMERIC", "NUMERIC(9,2)"),
        (approximate(24, Spelled::Real), "REAL", "REAL"),
        (approximate(53, Spelled::DoublePrecision), "DOUBLE PRECISION", "DOUBLE PRECISION"),
        (approximate(53, Spelled::Float), "FLOAT", "FLOAT(53)"),
        (DataType::distinct("S", "MONEY", exact(9, 2, Spelled::Decimal)), "USER-DEFINED", "S.MONEY"),
        (DataType::Date, "DATE", "DATE"),
    ];
    for (t, name, written) in cases {
        assert_eq!(Declared::of(&t), Declared { name: name.into(), written: written.into() });
    }
}

/// Verifies: [DESIGN] RULES.md D396: a Description or a Prepared with no
/// declared type is its first form, byte for byte, and one with them is
/// the same bytes then tag 19, a count and each column's name and text;
/// both read back as written; a column told reads back told.
#[test]
fn declared_types_follow_the_first_form() {
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    for told in [false, true] {
        let columns = vec![column(integer.clone(), told), column(DataType::Boolean, told)];
        reply_round_trip(&Reply::Description(columns.clone()));
        reply_round_trip(&Reply::Prepared { statement: 1, parameters: Vec::new(), columns });
    }
    let plain = body(&Reply::Description(vec![column(integer.clone(), false)]));
    let told = body(&Reply::Description(vec![column(integer.clone(), true)]));
    let tail = [&[19_u8, 1, 0, 0, 0, 7, 0, 0, 0][..], b"INTEGER", &[7, 0, 0, 0], b"INTEGER"].concat();
    assert_eq!(told, [plain.clone(), tail].concat());
    let Reply::Description(read) = Reply::decode(b'D', &told).unwrap() else { panic!("no Description") };
    assert_eq!(read[0].type_written(), "INTEGER");
    let Reply::Description(untold) = Reply::decode(b'D', &plain).unwrap() else { panic!("no Description") };
    assert_eq!((untold[0].declared.clone(), untold[0].type_written()), (None, "NUMERIC(10,0)".to_owned()));
}

/// Verifies: [DESIGN] RULES.md D396, D42: declared types of another count
/// than the columns', or a tag the message does not take, fail the
/// connection; a column without one beside one told is told as its type
/// says.
#[test]
fn declared_types_out_of_count_are_refused() {
    let columns = vec![column(DataType::Boolean, true), column(DataType::Date, false)];
    let Reply::Description(read) = Reply::decode(b'D', &body(&Reply::Description(columns))).unwrap() else { panic!("no Description") };
    assert_eq!(read[1].declared, Some(Declared::of(&DataType::Date)));
    let one = body(&Reply::Description(vec![column(DataType::Boolean, true)]));
    // The tail is its tag, a count and two strings of 7 octets: 27 octets.
    let head = &one[..one.len() - 27];
    let mut more = one.clone();
    more[one.len() - 26] = 2;
    let refused = |b: &[u8]| Reply::decode(b'D', b).map_err(|e| kind_of(&e)).err();
    assert_eq!(refused(&[head, &[19, 0, 0, 0, 0]].concat()), Some(FAILED), "no declared type for a column");
    assert_eq!(refused(&[head, &[18]].concat()), Some(FAILED), "a tag not taken");
    assert_eq!(refused(&more), Some(FAILED), "a count past what follows");
}
