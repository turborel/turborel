//! Unit tests of the record format: rows read back as written, any column
//! alone, values out of line, and malformed records refused (RULES.md
//! R-TEST-1, R-TEST-2). Split from record.rs (RULES.md R-SIZE-2).

use super::*;

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// Ten columns of every type, so the null bitmap takes two bytes.
fn types() -> Vec<DataType> {
    let mut t = vec![
        DataType::Exact { precision: 18, scale: 0 },
        DataType::Character { length: 5 },
        DataType::Approximate { precision: 53 },
        DataType::Exact { precision: 6, scale: 2 },
        DataType::Character { length: 3 },
    ];
    t.extend(t.clone());
    t
}

fn row() -> Row {
    let mut r = vec![
        exact(-999_999_999_999_999_999, 0),
        Value::Char("ab   ".into()),
        Value::Approx(-1.5),
        exact(12345, 2),
        Value::Char("é  ".into()),
    ];
    r.extend([Value::Null, Value::Char("     ".into()), Value::Approx(f64::INFINITY), Value::Null, Value::Char("xyz".into())]);
    r
}

fn shown(row: &Row) -> Vec<String> {
    row.iter().map(|v| format!("{v:?}")).collect()
}

/// Verifies: [DESIGN] RULES.md D31, record format: a row of every type,
/// NULLs among them, reads back as written, whole, column by column, or
/// only the columns asked for (the others NULL); character strings are
/// stored without their trailing spaces and read back padded to their
/// column's length.
#[test]
fn rows_read_back_as_written() {
    let format = RecordFormat::new(&types());
    let mut bytes = Vec::new();
    format.encode(&row(), &mut bytes).unwrap();
    assert_eq!(shown(&format.decode(&bytes).unwrap()), shown(&row()));
    for (i, value) in row().iter().enumerate() {
        assert_eq!(format!("{:?}", format.column(&bytes, i).unwrap()), format!("{value:?}"), "column {i}");
    }
    let some = format.decode_columns(&bytes, &[1, 3]).unwrap();
    assert_eq!(some.iter().filter(|v| !v.is_null()).count(), 2);
    assert_eq!((format!("{:?}", some[1]), format!("{:?}", some[3])), (format!("{:?}", row()[1]), format!("{:?}", row()[3])));
    // Header, two bytes of bitmap, six numbers, four ends, and "ab", "é"
    // and "xyz": no trailing spaces stored.
    assert_eq!(bytes.len(), 1 + 2 + 6 * 8 + 4 * 4 + 2 + 2 + 3);
    let empty = RecordFormat::new(&[]);
    let mut none = Vec::new();
    empty.encode(&vec![], &mut none).unwrap();
    assert!(empty.decode(&none).unwrap().is_empty());
}

/// Verifies: [DESIGN] RULES.md D31, record format: an exact value is
/// stored at its column's scale, a value of a smaller scale scaled up;
/// one with digits beyond the scale, more digits than 64 bits hold, of
/// another type, or a row of the wrong number of values, is refused
/// with SQLCODE -910.
#[test]
fn values_must_be_of_their_columns_types() {
    let format = RecordFormat::new(&[DataType::Exact { precision: 6, scale: 2 }]);
    let mut bytes = Vec::new();
    format.encode(&vec![exact(5, 0)], &mut bytes).unwrap();
    assert_eq!(format!("{:?}", format.column(&bytes, 0).unwrap()), format!("{:?}", exact(500, 2)));
    for bad in [exact(1, 3), exact(i128::MAX, 0), Value::Char("1".into()), Value::Approx(1.0)] {
        assert_eq!(format.encode(&vec![bad.clone()], &mut Vec::new()).err().map(|e| e.code), Some(Code::Api), "{bad:?}");
    }
    assert_eq!(format.encode(&vec![], &mut Vec::new()).err().map(|e| e.code), Some(Code::Api));
    let chars = RecordFormat::new(&[DataType::Character { length: 2 }]);
    assert_eq!(chars.encode(&vec![Value::Exact(Decimal::new(1, 0))], &mut Vec::new()).err().map(|e| e.code), Some(Code::Api));
}

/// Values out of line, kept in `store` by the index of their place in it.
fn spill(format: &RecordFormat, row: &Row, spill: Spill, store: &mut Vec<(usize, Vec<u8>)>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    format.encode_spilling(row, spill, &mut bytes, |column, value| {
        let at = store.len();
        store.push((column, value.to_vec()));
        Ok(u64::try_from(at).unwrap())
    })?;
    Ok(bytes)
}

/// Verifies: [DESIGN] RULES.md D31, values out of line: a character
/// value longer than the threshold is stored out of line, the record
/// holding a reference (where, how long, and its first 16 bytes, cut at
/// a character's start); one at the threshold stays in line; reading
/// the row fetches the value and pads it again, reading other columns
/// fetches nothing; `outside` lists the references; a reader without a
/// store is refused with SQLCODE -910, and a fetched value of another
/// length with SQLCODE -916.
#[test]
fn long_values_go_out_of_line() {
    let format = RecordFormat::new(&[
        DataType::Exact { precision: 4, scale: 0 },
        DataType::Character { length: 100 },
        DataType::Character { length: 100 },
    ]);
    let long = format!("{}{}", "é".repeat(10), "x".repeat(30));
    let row = vec![exact(1, 0), Value::Char(format!("{long:<100}")), Value::Char(format!("{:<100}", "short"))];
    let mut store = Vec::new();
    let bytes = spill(&format, &row, Spill { threshold: 5, most: 1000 }, &mut store).unwrap();
    assert_eq!(store, [(1, long.as_bytes().to_vec())], "\"short\" is at the threshold, so stays");
    // Header, bitmap, a number, two ends; the reference: 8 + 4 bytes, and
    // 16 bytes cut to 8 two-byte characters; then "short".
    assert_eq!(bytes.len(), 1 + 1 + 8 + 2 * 4 + 12 + 16 + 5);
    let mut fetch = |r: Reference| Ok(store[usize::try_from(r.at).unwrap()].1.clone());
    assert_eq!(shown(&format.decode_with(&bytes, &mut fetch).unwrap()), shown(&row));
    let mut none = |r: Reference| -> Result<Vec<u8>> { panic!("fetched {r:?}") };
    assert_eq!(format!("{:?}", format.decode_columns_with(&bytes, &[0, 2], &mut none).unwrap()[2]), format!("{:?}", row[2]));
    assert_eq!(format.outside(&bytes).unwrap(), [Reference { column: 1, at: 0, length: long.len() }]);
    assert_eq!(format.decode(&bytes).err().map(|e| e.code), Some(Code::Api));
    let mut short = |_: Reference| Ok(b"too short".to_vec());
    assert_eq!(format.column_with(&bytes, 1, &mut short).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D31, values out of line: a record longer
/// than the most a row may take has its longest values moved out of
/// line, longest first, until it fits, though they are under the
/// threshold; values no longer than a reference are never moved; a row
/// that cannot fit even so is refused with SQLCODE -900; a store's
/// error is the encoding's.
#[test]
fn rows_too_long_move_their_longest_values_out() {
    let format =
        RecordFormat::new(&[DataType::Character { length: 200 }, DataType::Character { length: 200 }, DataType::Character { length: 20 }]);
    let row = vec![Value::Char("a".repeat(100)), Value::Char("b".repeat(150)), Value::Char("c".repeat(20))];
    let mut store = Vec::new();
    let bytes = spill(&format, &row, Spill { threshold: 1000, most: 180 }, &mut store).unwrap();
    assert_eq!(store.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [1], "the longest, 150, was enough");
    assert!(bytes.len() <= 180);
    store.clear();
    spill(&format, &row, Spill { threshold: 1000, most: 100 }, &mut store).unwrap();
    assert_eq!(store.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [0, 1], "then the next longest; 20 bytes never");
    let refused = spill(&format, &row, Spill { threshold: 1000, most: 60 }, &mut Vec::new());
    assert_eq!(refused.err().map(|e| e.code), Some(Code::Unsupported));
    let failing =
        format.encode_spilling(&row, Spill { threshold: 10, most: 1000 }, &mut Vec::new(), |_, _| Err(SqlError::new(Code::Io, "disk")));
    assert_eq!(failing.err().map(|e| e.code), Some(Code::Io));
}

/// Verifies: [DESIGN] RULES.md D31, values out of line: a malformed
/// reference (shorter than where and how long, or with a prefix longer
/// than 16 bytes or than its value) is refused with SQLCODE -916.
#[test]
fn malformed_references_are_refused() {
    assert!(reference(&[0; 11], 0).is_err());
    assert!(reference(&[0; 12 + 17], 0).is_err());
    let mut short_value = vec![0; 8];
    short_value.extend_from_slice(&3_u32.to_le_bytes());
    short_value.extend_from_slice(b"four");
    assert_eq!(reference(&short_value, 0).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(prefix("ab".as_bytes()), b"ab");
}

/// Verifies: [DESIGN] RULES.md D31, record format: bytes that are not a
/// record of the format are refused with SQLCODE -916: too short, of
/// another header, an end past the record, characters that are not
/// UTF-8 or longer than their column, or a column that does not exist.
#[test]
fn malformed_records_are_refused() {
    let format = RecordFormat::new(&[DataType::Exact { precision: 4, scale: 0 }, DataType::Character { length: 2 }]);
    let mut bytes = Vec::new();
    format.encode(&vec![exact(7, 0), Value::Char("ab".into())], &mut bytes).unwrap();
    let corrupt = |b: &[u8]| format.decode(b).err().map(|e| e.code);
    assert_eq!(corrupt(&bytes[..bytes.len() - 3]), Some(Code::Corrupt), "too short");
    let mut header = bytes.clone();
    header[0] = 9;
    assert_eq!(corrupt(&header), Some(Code::Corrupt));
    let mut end = bytes.clone();
    let ends = 1 + 1 + 8;
    end[ends] = 200;
    assert_eq!(corrupt(&end), Some(Code::Corrupt), "an end past the record");
    let mut utf8 = bytes.clone();
    let last = utf8.len() - 1;
    utf8[last] = 0xFF;
    assert_eq!(corrupt(&utf8), Some(Code::Corrupt));
    let mut long = bytes.clone();
    long[ends] = 3;
    long.push(b'c');
    assert_eq!(corrupt(&long), Some(Code::Corrupt), "three characters in a column of two");
    assert_eq!(format.column(&bytes, 2).err().map(|e| e.code), Some(Code::Corrupt));
}
