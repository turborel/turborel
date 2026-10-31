//! Unit tests of batches gathered from records (`gathered.rs`).

use super::*;
use crate::engine::record::Spill;
use crate::error::Code;
use crate::value::{Decimal, Spelled};

/// The columns of the records gathered: an exact number of scale 2, a
/// CHARACTER(6), a VARCHAR(40), an approximate number.
fn types() -> Vec<DataType> {
    vec![
        DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Decimal },
        DataType::Character { length: 6 },
        DataType::Varying { length: 40 },
        DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision },
    ]
}

/// The records of `rows` in `format`, a value longer than 20 bytes kept
/// out of line, and the values so kept, each at its place.
fn encoded(format: &RecordFormat, rows: &[Vec<Value>]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut store: Vec<Vec<u8>> = Vec::new();
    let mut records = Vec::new();
    for row in rows {
        let mut out = Vec::new();
        format
            .encode_spilling(row, Spill { threshold: 20, most: 1000 }, &mut out, |_, bytes| {
                store.push(bytes.to_vec());
                Ok(u64::try_from(store.len()).unwrap().checked_sub(1).unwrap())
            })
            .unwrap();
        records.push(out);
    }
    (records, store)
}

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §8.7 GR7b]: a batch
/// gathered from records holds what the rows decoded give: exact numbers
/// as integers of their scale, of either sign; character strings as text,
/// a CHARACTER string padded to its length in characters, not ASCII too,
/// a VARCHAR as written with its trailing space; a value kept out of line
/// fetched; others as values; nulls among them all, kept from the first
/// on, none kept of a column with none; a column the table lacks null; of
/// columns asked in any order; taken, it is empty again. A field not of
/// its type is refused with SQLCODE -916.
#[test]
fn records_gather_as_their_rows_decode() {
    let format = RecordFormat::new(&types());
    let rows: Vec<Vec<Value>> = vec![
        vec![Value::Exact(Decimal::new(-150, 2)), Value::Char("ab    ".into()), Value::Char("x ".into()), Value::Approx(1.5_f64)],
        vec![Value::Exact(Decimal::new(7, 2)), Value::Null, Value::Char("y".repeat(30)), Value::Null],
        vec![Value::Null, Value::Char("éü    ".into()), Value::Null, Value::Approx(-2.0_f64)],
    ];
    let (records, store) = encoded(&format, &rows);
    assert_eq!(store.len(), 1, "the long VARCHAR out of line");
    let columns = [2_usize, 0, 1, 3, 9];
    let batch = gathered((&format, &records, &store), &columns);
    assert_eq!(batch.rows, 3);
    assert!(matches!(batch.columns[1], Column::Ints(_)) && matches!(batch.columns[0], Column::Texts(_)));
    assert!(matches!(batch.columns[2], Column::Texts(_)) && matches!(batch.columns[3], Column::Values(_)));
    for (at, &c) in columns.iter().enumerate() {
        let expected: Vec<String> = rows.iter().map(|row| format!("{:?}", row.get(c).unwrap_or(&Value::Null))).collect();
        let gathered: Vec<String> = batch.values(at).iter().map(|v| format!("{v:?}")).collect();
        assert_eq!(gathered, expected, "column {c}");
    }
    let Column::Texts(fixed) = &batch.columns[2] else { panic!("text") };
    assert_eq!(fixed.get(2), Some("éü    "), "padded by characters");
    let Column::Ints(numbers) = &batch.columns[1] else { panic!("integers") };
    assert_eq!((numbers.units.clone(), numbers.scale), (vec![-150_i64, 7_i64, 0_i64], 2_u32));
    assert_eq!(numbers.nulls, Some(vec![false, false, true]), "kept from the first null on");
    let Column::Ints(lone) = &gathered((&format, &records[..1], &store), &[0]).columns[0] else { panic!("integers") };
    assert_eq!(lone.nulls, None, "no null, no bitmap");
}

/// The batch of `columns` of `records` of `format`, values out of line
/// in `store`, gathered: the gathering then empty.
fn gathered((format, records, store): (&RecordFormat, &[Vec<u8>], &[Vec<u8>]), columns: &[usize]) -> Batch {
    let mut fetch = |r: Reference| Ok(store[usize::try_from(r.at).unwrap()].clone());
    let mut gathering = Gathering::new(&types(), columns);
    for record in records {
        gathering.push((format, record), &mut fetch).unwrap();
    }
    let batch = gathering.take();
    assert_eq!(gathering.rows(), 0, "taken, it is empty");
    batch
}

/// Verifies: [DESIGN] RULES.md D31, robustness: a record whose exact
/// number's field is not of 8 bytes, or whose string is not UTF-8, is
/// refused with SQLCODE -916 as it is gathered, as it is decoded.
#[test]
fn malformed_fields_are_refused() {
    let short = RecordFormat::new(&[DataType::Varying { length: 4 }]);
    let mut record = Vec::new();
    short.encode(&vec![Value::Char("abc".into())], &mut record).unwrap();
    let mut fetch = |r: Reference| -> Result<Vec<u8>> { panic!("fetched {r:?}") };
    // The same bytes gathered as an exact number's: a field of 3 bytes.
    let mut numbers = Gathering::new(&[DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }], &[0]);
    assert_eq!(numbers.push((&short, &record), &mut fetch).err().map(|e| e.code), Some(Code::Corrupt));
    let bad = record.iter().map(|&b| if b == b'b' { 0xFF_u8 } else { b }).collect::<Vec<u8>>();
    let mut texts = Gathering::new(&[DataType::Varying { length: 4 }], &[0]);
    assert_eq!(texts.push((&short, &bad), &mut fetch).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(short.column(&bad, 0).err().map(|e| e.code), Some(Code::Corrupt), "as decoded");
}
