//! The chunks of a column pinned, byte for byte, as they were written of
//! fields of a vector each before a column's fields were made one buffer
//! (RULES.md D1148); and fields of records given as they are.

use std::borrow::Cow;

use super::*;
use crate::hash::xxhash::xxh3_64;
use crate::value::datetime::Moment;
use crate::value::{Datetime, Decimal, Spelled};

const WIDE: DataType = DataType::Exact { precision: 30, scale: 2, spelled: Spelled::Numeric };
const DOUBLE: DataType = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
const STAMP: DataType = DataType::Timestamp { precision: 6, zoned: true };

fn stamp(n: i64) -> Value {
    Value::Datetime(Datetime { moment: Moment::Timestamp, utc: n, zone: 60, zoned: true, precision: 6 })
}

/// Columns of each encoding: their type and values.
fn columns() -> Vec<(DataType, Vec<Value>)> {
    let null_every = |n: usize, every: usize, v: Value| if n.checked_rem(every) == Some(3) { Value::Null } else { v };
    vec![
        (
            INTEGER,
            (0..3000_usize)
                .map(|n| {
                    null_every(n, 97, int(i128::try_from(n.checked_div(40).unwrap().wrapping_add((n % 7).wrapping_mul(1000))).unwrap()))
                })
                .collect(),
        ),
        (INTEGER, (0..3000_i128).map(|n| int(n.checked_div(500).unwrap())).collect()),
        (INTEGER, (0..500).map(|_| int(42)).collect()),
        (WIDE, (0..1000_i128).map(|n| Value::Exact(Decimal::new(n.wrapping_mul(1_000_000_007), 2))).collect()),
        (DOUBLE, (0..2000_u32).map(|n| null_every(usize::try_from(n).unwrap(), 31, Value::Approx(f64::from(n) * 0.25))).collect()),
        (DOUBLE, (0..500_u32).map(|n| Value::Approx(f64::from(n).sqrt())).collect()),
        (TEXT, (0..2000_usize).map(|n| Value::Char(["alpha", "beta", "gamma"][n % 3].into())).collect()),
        (TEXT, (0..2000_usize).map(|n| null_every(n, 50, Value::Char(format!("text {n} {}", n.wrapping_mul(7919)).into()))).collect()),
        (CHAR, (0..800_usize).map(|n| null_every(n, 9, Value::Char(format!("c{}", n % 37).into()))).collect()),
        (DataType::Boolean, (0..900_usize).map(|n| null_every(n, 5, Value::Bool(n % 3 == 0))).collect()),
        (STAMP, (0..1500_i64).map(|n| stamp(n.wrapping_mul(1_000_003))).collect()),
    ]
}

/// The digest of the chunks of every column of `columns`, each cut for
/// pages of 300 bytes and of 4000, compressed and not, each as made by
/// `fields` of its type and values.
fn digest(fields: impl Fn(&DataType, &[Value]) -> Fields<'static>) -> u64 {
    let mut all = Vec::new();
    for (t, values) in columns() {
        let made = fields(&t, &values);
        for capacity in [300_usize, 4000] {
            for compress in [true, false] {
                for (from, count, chunk) in made.chunks(&t, (capacity, compress)).unwrap() {
                    all.extend_from_slice(&u64::try_from(from).unwrap().to_le_bytes());
                    all.extend_from_slice(&u64::try_from(count).unwrap().to_le_bytes());
                    all.extend_from_slice(&chunk);
                }
            }
        }
    }
    xxh3_64(&all)
}

/// The digest `digest` gave of fields of a vector each.
const PINNED: u64 = 6_117_703_719_257_929_298;

/// Verifies: [DESIGN] RULES.md D31, D1144, D1148: the chunks of columns of
/// every encoding, of integers narrow and wide, approximate numbers,
/// texts few and many, CHARACTER strings, truth values and timestamps
/// with time zone, nulls among them, are the chunks fields of a vector
/// each wrote, byte for byte.
#[test]
fn chunks_are_pinned_byte_for_byte() {
    assert_eq!(digest(|t, values| Fields::of(t, values.iter()).unwrap()), PINNED);
}

/// Verifies: [DESIGN] RULES.md D1148: fields taken as they are of the
/// records `encode_record` writes of each value give the chunks fields of
/// the values do, byte for byte, of every encoding; and their width is
/// found of the records, a fixed column's chunks fixed.
#[test]
fn fields_of_records_are_fields_of_their_values() {
    let of_records = |t: &DataType, values: &[Value]| Fields::from_records(t, Cow::Owned(records(t, values)));
    assert_eq!(digest(of_records), PINNED);
    let ints: Vec<Value> = (0..20).map(int).chain([Value::Null]).collect();
    let fixed = of_records(&INTEGER, &ints).chunk(&INTEGER, 0, (ints.len(), false)).unwrap();
    assert_eq!(fixed.first(), Some(&FIXED), "a width found of the records");
    assert_eq!(fixed, Fields::of(&INTEGER, ints.iter()).unwrap().chunk(&INTEGER, 0, (ints.len(), false)).unwrap());
}
