//! Unit tests of a record made of another's bytes: it is the record
//! encoded whole, or it is not made (RULES.md R-TEST-1, R-TEST-2).

use crate::engine::record::{RecordFormat, Spill};
use crate::error::Code;
use crate::storage::Row;
use crate::value::cast::cast;
use crate::value::{DataType, Spelled, Value};

/// A column of each kind a value is built for here, of fixed width and of
/// varying, with two values of it written as text: nine columns, so the
/// null bitmap takes two bytes, the first and the last of varying width.
fn columns() -> Vec<(DataType, [&'static str; 2])> {
    vec![
        (DataType::varying(40), ["first", "the first column, grown"]),
        (DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Numeric }, ["12.50", "-99999.99"]),
        (DataType::character(8), ["ab", "abcdefgh"]),
        (DataType::Approximate { precision: 53, spelled: Spelled::Float }, ["1.5E0", "-2.25E10"]),
        (DataType::Boolean, ["TRUE", "FALSE"]),
        (DataType::Date, ["2024-02-29", "1999-12-31"]),
        (DataType::Time { precision: 3, zoned: true }, ["10:20:30.125+02:00", "23:59:59.999-08:00"]),
        (DataType::Timestamp { precision: 6, zoned: false }, ["2024-02-29 10:20:30.000001", "1970-01-01 00:00:00"]),
        (DataType::varying(40), ["", "last   "]),
    ]
}

/// The value `text` is in a column of `data_type`.
fn value(data_type: &DataType, text: &str) -> Value {
    cast(Value::Char(text.into()), data_type).unwrap()
}

/// The format of `columns`, and the row of each column's first value.
fn fixture() -> (RecordFormat, Row) {
    let columns = columns();
    let types: Vec<DataType> = columns.iter().map(|(t, _)| t.clone()).collect();
    (RecordFormat::new(&types), columns.iter().map(|(t, texts)| value(t, texts[0])).collect())
}

fn encoded(format: &RecordFormat, row: &Row) -> Vec<u8> {
    let mut bytes = Vec::new();
    format.encode(row, &mut bytes).unwrap();
    bytes
}

/// The record of `old` patched to hold `new` in the columns `set`, from a
/// row that holds those columns of `new` alone, as an update holds it.
fn patched(format: &RecordFormat, (old, new): (&Row, &Row), set: &[usize]) -> Vec<u8> {
    let sparse: Row = new.iter().enumerate().map(|(i, v)| if set.contains(&i) { v.clone() } else { Value::Null }).collect();
    let mut bytes = Vec::new();
    assert!(format.patched(&encoded(format, old), (&sparse, set), None, &mut bytes).unwrap(), "columns {set:?}");
    bytes
}

/// Verifies: [DESIGN] RULES.md D915: a record patched in one column is,
/// byte for byte, the record of the row after encoded whole. Of each kind
/// of column, of fixed width and of varying: a value made another, a
/// value made NULL and NULL made a value, the column before it NULL or
/// not; of the first column, of one in the middle and of the last; and of
/// a varying column grown, shrunk, emptied and filled.
#[test]
fn a_record_patched_in_one_column_is_the_record_encoded() {
    let (format, base) = fixture();
    for (i, (data_type, texts)) in columns().iter().enumerate() {
        let (a, b) = (value(data_type, texts[0]), value(data_type, texts[1]));
        for (from, to) in [(&a, &b), (&b, &a), (&a, &Value::Null), (&Value::Null, &a), (&Value::Null, &Value::Null), (&b, &b)] {
            for neighbour_null in [false, true] {
                let mut old = base.clone();
                if let (true, Some(before)) = (neighbour_null, i.checked_sub(1)) {
                    old[before] = Value::Null;
                }
                old[i] = from.clone();
                let mut new = old.clone();
                new[i] = to.clone();
                assert_eq!(patched(&format, (&old, &new), &[i]), encoded(&format, &new), "column {i}: {from:?} to {to:?}");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D915: several columns set at once, of
/// fixed width and of varying, named in any order and one named twice,
/// give the record encoded whole; a patch appends after what the buffer
/// holds; and columns not set are never read of the row, which holds NULL
/// in them.
#[test]
fn a_record_patched_in_several_columns_is_the_record_encoded() {
    let (format, base) = fixture();
    let mut new = base.clone();
    for (i, (data_type, texts)) in columns().iter().enumerate() {
        new[i] = value(data_type, texts[1]);
    }
    new[4] = Value::Null;
    for set in [vec![0, 8], vec![8, 0, 2], vec![1, 3, 4, 5, 6, 7], vec![2, 2, 1], (0..9).collect::<Vec<usize>>(), vec![]] {
        let mut after = base.clone();
        for &i in &set {
            after[i] = new[i].clone();
        }
        assert_eq!(patched(&format, (&base, &new), &set), encoded(&format, &after), "columns {set:?}");
    }
    let mut out = vec![7_u8, 7];
    assert!(format.patched(&encoded(&format, &base), (&new, &[2]), None, &mut out).unwrap());
    let mut after = base.clone();
    after[2] = new[2].clone();
    assert_eq!(out, [vec![7_u8, 7], encoded(&format, &after)].concat());
}

/// Verifies: [DESIGN] RULES.md D915: a record is not made of one that
/// keeps a value out of line, of one of another format, such as a sealed
/// record, of one cut short or with bytes past its last value, or where
/// the record made would not be kept whole, a value past the threshold or
/// the record past the most a row may take; one exactly at each limit is
/// made. The buffer is then as it was.
#[test]
fn a_record_that_cannot_be_patched_is_refused() {
    let (format, base) = fixture();
    let whole = encoded(&format, &base);
    let mut longer = base.clone();
    longer[0] = Value::Char("x".repeat(30));
    let refused = |old: &[u8], row: &Row, spill: Option<Spill>| {
        let mut out = vec![9_u8];
        let made = format.patched(old, (row, &[0]), spill, &mut out).unwrap();
        assert!(made || out == [9_u8], "nothing of a record refused is left");
        !made
    };
    let mut spilled = Vec::new();
    format.encode_spilling(&longer, Spill { threshold: 10, most: 1000 }, &mut spilled, |_, _| Ok(4)).unwrap();
    assert!(refused(&spilled, &base, None), "a value out of line");
    let mut sealed = whole.clone();
    sealed[0] = 1;
    assert!(refused(&sealed, &base, None), "another format");
    assert!(refused(whole.split_last().unwrap().1, &base, None), "cut short");
    assert!(refused(&[whole.clone(), vec![0]].concat(), &base, None), "bytes past its last value");
    assert!(refused(&whole, &longer, Some(Spill { threshold: 29, most: 1000 })), "a value past the threshold");
    assert!(!refused(&whole, &longer, Some(Spill { threshold: 30, most: 1000 })), "a value at the threshold");
    let most = encoded(&format, &longer).len();
    assert!(refused(&whole, &longer, Some(Spill { threshold: 100, most: most.saturating_sub(1) })), "a record past the most");
    assert!(!refused(&whole, &longer, Some(Spill { threshold: 100, most })), "a record at the most");
}

/// Verifies: [DESIGN] RULES.md D915: a value set that is not of its
/// column's type, a row not of the format's width and a column the
/// format has not are refused with SQLCODE -910, the buffer as it was.
#[test]
fn a_patch_of_the_wrong_values_is_an_error() {
    let (format, base) = fixture();
    let whole = encoded(&format, &base);
    let mut wrong = base.clone();
    wrong[1] = Value::Char("x".into());
    wrong[8] = Value::Bool(true);
    let mut out = vec![9_u8];
    for (row, set) in [(&wrong, vec![1]), (&wrong, vec![0, 8]), (&base, vec![9]), (&base[..8].to_vec(), vec![0])] {
        assert_eq!(format.patched(&whole, (row, &set), None, &mut out).err().map(|e| e.code), Some(Code::Api), "columns {set:?}");
        assert_eq!(out, [9_u8]);
    }
}
