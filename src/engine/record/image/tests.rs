//! Unit tests of a record's values as a log record holds them (RULES.md
//! R-TEST-1, R-TEST-2).

use crate::engine::record::{RecordFormat, Spill};
use crate::error::Code;
use crate::storage::{AfterImage, Row};
use crate::value::cast::cast;
use crate::value::{DataType, Spelled, Value};

/// A column of each kind a value is built for here, with a value of it
/// written as text.
fn columns() -> Vec<(DataType, &'static str)> {
    vec![
        (DataType::varying(40), "varying, its spaces kept  "),
        (DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Numeric }, "-99999.99"),
        (DataType::character(8), "ab"),
        (DataType::character(6), "é and"),
        (DataType::Approximate { precision: 53, spelled: Spelled::Float }, "-2.25E10"),
        (DataType::Boolean, "TRUE"),
        (DataType::Date, "2024-02-29"),
        (DataType::Time { precision: 3, zoned: true }, "10:20:30.125+02:00"),
        (DataType::Timestamp { precision: 6, zoned: false }, "2024-02-29 10:20:30.000001"),
        (DataType::Exact { precision: 30, scale: 0, spelled: Spelled::Numeric }, "123456789012345678"),
        (DataType::varying(40), ""),
    ]
}

fn fixture() -> (RecordFormat, Row) {
    let columns = columns();
    let types: Vec<DataType> = columns.iter().map(|(t, _)| t.clone()).collect();
    let row = columns.iter().map(|(t, text)| cast(Value::Char((*text).into()), t).unwrap()).collect();
    (RecordFormat::new(&types), row)
}

/// Verifies: [DESIGN] RULES.md D916, D475: the image made of a record is
/// the image of the row the record holds, the values the codec writes of
/// the row decoded, byte for byte, with the places of the columns set: of
/// each kind of column, a CHARACTER string given its spaces back, of one
/// byte a character and of more, a VARCHAR as it is, and nulls among
/// them; an image written into again holds the later row alone.
#[test]
fn a_records_image_is_its_rows() {
    let (format, base) = fixture();
    let mut nulled = base.clone();
    for i in [0, 2, 5, 9] {
        nulled[i] = Value::Null;
    }
    let (mut made, mut of_row) = (AfterImage::new(), AfterImage::new());
    for (row, set) in [(&base, vec![0, 3, 10]), (&nulled, vec![1, 2]), (&base, vec![])] {
        let mut record = Vec::new();
        format.encode(row, &mut record).unwrap();
        format.image(&record, &set, &mut made).unwrap();
        of_row.put_row(&format.decode(&record).unwrap(), &set);
        assert_eq!(made.parts(), of_row.parts(), "columns {set:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D916: no image is made of a record of
/// another format, of one that keeps a value out of line, or of one whose
/// string is not UTF-8 or is longer than its column: each is refused with
/// SQLCODE -916.
#[test]
fn a_malformed_records_image_is_refused() {
    let (format, base) = fixture();
    let mut image = AfterImage::new();
    let mut record = Vec::new();
    format.encode(&base, &mut record).unwrap();
    let mut sealed = record.clone();
    sealed[0] = 1;
    assert_eq!(format.image(&sealed, &[], &mut image).err().map(|e| e.code), Some(Code::Corrupt), "another format");
    let mut spilled = Vec::new();
    format.encode_spilling(&base, Spill { threshold: 10, most: 1000 }, &mut spilled, |_, _| Ok(4)).unwrap();
    assert_eq!(format.image(&spilled, &[], &mut image).err().map(|e| e.code), Some(Code::Corrupt), "a value out of line");
    let strings = RecordFormat::new(&[DataType::character(2), DataType::varying(9)]);
    let wide = RecordFormat::new(&[DataType::character(9), DataType::varying(9)]);
    let mut long = Vec::new();
    wide.encode(&vec![Value::Char(format!("{:<9}", "abc")), Value::Char("v".into())], &mut long).unwrap();
    assert_eq!(strings.image(&long, &[], &mut image).err().map(|e| e.code), Some(Code::Corrupt), "longer than its column");
    for at in [long.len().saturating_sub(1), long.len().saturating_sub(2)] {
        let mut broken = long.clone();
        broken[at] = 0xFF;
        assert_eq!(wide.image(&broken, &[], &mut image).err().map(|e| e.code), Some(Code::Corrupt), "not UTF-8 at {at}");
    }
}
