//! Unit tests: a partitioning strategy's bounds and restrictions, as
//! protocol 1 carries them, read back as written, and a form no bound or
//! restriction has, or one cut short, refused (RULES.md D169, R-TEST-1,
//! R-TEST-2).

use super::*;
use crate::error::SqlState;
use crate::plugin::errors::VIOLATION;
use crate::plugin::payload::reader;
use crate::value::{Binary, Decimal};

/// An exact value of `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A bound of each form: DEFAULT, a range of each kind of end, a list
/// and a WITH.
fn bounds() -> [PartitionBound; 4] {
    [
        PartitionBound::Default,
        PartitionBound::Range { from: vec![RangeDatum::MinValue, RangeDatum::Value(int(1))], to: vec![RangeDatum::MaxValue] },
        PartitionBound::List(vec![vec![int(1)], vec![Value::Null]]),
        PartitionBound::With(vec![("MODULUS".into(), int(4)), ("REMAINDER".into(), int(1))]),
    ]
}

/// A restriction of each form, and none.
fn restrictions() -> [Option<Restriction>; 5] {
    [
        None,
        Some(Restriction::Equal(vec![int(2), Value::Char("x".into())])),
        Some(Restriction::Range { low: Some((int(1), true)), high: None }),
        Some(Restriction::Range { low: None, high: Some((int(9), false)) }),
        Some(Restriction::Null),
    ]
}

/// That `bytes`, cut short at any byte, is refused by `read` as a
/// malformed message received, SQLSTATE 08006.
fn cut_short<T>(bytes: &[u8], read: impl Fn(&mut Reader<'_>) -> Result<T>) {
    for end in 0..bytes.len() {
        let state = read(&mut reader(&bytes[..end])).err().map(|e| e.sqlstate());
        assert_eq!(state, Some(SqlState::CONNECTION_FAILURE), "cut at {end} of {}", bytes.len());
    }
}

/// Verifies: [EXTENSION] RULES.md D169: a bound of each form, ranges of
/// MINVALUE, MAXVALUE and values among them, is read back as written, and
/// cut short at any byte refused.
#[test]
fn each_bound_is_read_as_written() {
    for b in bounds() {
        let mut bytes = Vec::new();
        put_bound(&mut bytes, &b).unwrap();
        let back = bound(&mut reader(&bytes)).unwrap();
        assert_eq!(format!("{back:?}"), format!("{b:?}"));
        cut_short(&bytes, bound);
    }
}

/// Verifies: [EXTENSION] RULES.md D169: a restriction of each form, an
/// equality, ranges open below or above, inclusive or not, IS NULL, and
/// none, is read back as written, and cut short at any byte refused.
#[test]
fn each_restriction_is_read_as_written() {
    for r in restrictions() {
        let mut bytes = Vec::new();
        put_restriction(&mut bytes, r.as_ref()).unwrap();
        let back = restriction(&mut reader(&bytes)).unwrap();
        assert_eq!(format!("{back:?}"), format!("{r:?}"));
        cut_short(&bytes, restriction);
    }
}

/// Verifies: [EXTENSION] RULES.md D169, D172: a bound, an end of a range
/// or a restriction of a form none has is refused, SQLSTATE 38T03; and a
/// value no protocol 1 carries, a reference, is refused as a bound or a
/// restriction is written.
#[test]
fn forms_no_bound_has_are_refused() {
    assert_eq!(bound(&mut reader(&[4])).unwrap_err().sqlstate(), VIOLATION);
    assert_eq!(bound(&mut reader(&[1, 1, 0, 0, 0, 3])).unwrap_err().sqlstate(), VIOLATION, "an end of form 3");
    assert_eq!(restriction(&mut reader(&[4])).unwrap_err().sqlstate(), VIOLATION);
    let reference = Value::Reference(Binary::new(vec![1]));
    let refused = [
        PartitionBound::Range { from: vec![RangeDatum::Value(reference.clone())], to: Vec::new() },
        PartitionBound::Range { from: Vec::new(), to: vec![RangeDatum::Value(reference.clone())] },
        PartitionBound::List(vec![vec![reference.clone()]]),
        PartitionBound::With(vec![("MODULUS".into(), reference.clone())]),
    ];
    for b in refused {
        assert_eq!(put_bound(&mut Vec::new(), &b).unwrap_err().sqlstate(), VIOLATION, "{b:?}");
    }
    let ends = [
        Restriction::Equal(vec![reference.clone()]),
        Restriction::Range { low: Some((reference.clone(), true)), high: None },
        Restriction::Range { low: None, high: Some((reference, true)) },
    ];
    for r in ends {
        assert_eq!(put_restriction(&mut Vec::new(), Some(&r)).unwrap_err().sqlstate(), VIOLATION, "{r:?}");
    }
}
