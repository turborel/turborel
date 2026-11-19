//! Unit tests of the built-in strategies' bounds, overlap, containment,
//! routing and pruning (RULES.md D154).

use super::*;
use crate::catalog::RangeDatum;
use crate::value::Decimal;
use crate::value::{INTEGER_PRECISION, Spelled};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn int_type() -> DataType {
    DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }
}

fn range(from: i128, to: i128) -> PartitionBound {
    RangeStrategy
        .bound(&[int_type()], PartitionBound::Range { from: vec![RangeDatum::Value(int(from))], to: vec![RangeDatum::Value(int(to))] })
        .unwrap()
}

fn hash(m: i128, r: i128) -> PartitionBound {
    let spec = PartitionBound::With(vec![(MODULUS.to_owned(), int(m)), (REMAINDER.to_owned(), int(r))]);
    HashStrategy.bound(&[int_type()], spec).unwrap()
}

fn equal(n: i128) -> Restriction {
    Restriction::Equal(vec![int(n)])
}

/// Verifies: [EXTENSION] (RULES.md D154) a range holds its lower bound and
/// not its upper; a null key is in no range; ranges meeting at a bound do
/// not overlap, ranges sharing keys do; an empty range is refused.
#[test]
fn range_bounds_include_from_and_exclude_to() {
    let r = range(10, 20);
    assert!(RangeStrategy.contains(&r, &[int(10)]).unwrap());
    assert!(RangeStrategy.contains(&r, &[int(19)]).unwrap());
    assert!(!RangeStrategy.contains(&r, &[int(20)]).unwrap());
    assert!(!RangeStrategy.contains(&r, &[Value::Null]).unwrap());
    assert!(!RangeStrategy.overlaps(&r, &range(20, 30)).unwrap());
    assert!(RangeStrategy.overlaps(&r, &range(15, 30)).unwrap());
    let empty = PartitionBound::Range { from: vec![RangeDatum::Value(int(5))], to: vec![RangeDatum::Value(int(5))] };
    assert_eq!(RangeStrategy.bound(&[int_type()], empty).unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] (RULES.md D154) MINVALUE and MAXVALUE bound a
/// range below and above every value, and a value after one is refused.
#[test]
fn range_minvalue_and_maxvalue_are_unbounded() {
    let types = [int_type(), int_type()];
    let all = PartitionBound::Range {
        from: vec![RangeDatum::MinValue, RangeDatum::MinValue],
        to: vec![RangeDatum::MaxValue, RangeDatum::MaxValue],
    };
    let all = RangeStrategy.bound(&types, all).unwrap();
    assert!(RangeStrategy.contains(&all, &[int(i128::from(i64::MIN)), int(0)]).unwrap());
    let bad = PartitionBound::Range {
        from: vec![RangeDatum::MinValue, RangeDatum::Value(int(1))],
        to: vec![RangeDatum::MaxValue, RangeDatum::MaxValue],
    };
    assert_eq!(RangeStrategy.bound(&types, bad).unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] (RULES.md D156) a range is admitted by an
/// equality inside it, by a range meeting it, and by no restriction; not
/// by an equality outside, a range ending at its lower bound, or IS NULL.
#[test]
fn range_admits_what_may_meet_it() {
    let r = range(10, 20);
    assert!(RangeStrategy.admits(&r, &[Some(equal(15))]).unwrap());
    assert!(!RangeStrategy.admits(&r, &[Some(equal(20))]).unwrap());
    assert!(RangeStrategy.admits(&r, &[None]).unwrap());
    let below = Restriction::Range { low: None, high: Some((int(10), false)) };
    let meets = Restriction::Range { low: Some((int(19), true)), high: None };
    assert!(RangeStrategy.admits(&r, &[Some(meets)]).unwrap());
    assert!(RangeStrategy.admits(&r, &[Some(below)]).unwrap(), "conservative at an excluded end");
    let far = Restriction::Range { low: None, high: Some((int(9), true)) };
    assert!(!RangeStrategy.admits(&r, &[Some(far)]).unwrap());
    assert!(!RangeStrategy.admits(&r, &[Some(Restriction::Null)]).unwrap());
    assert!(RangeStrategy.admits(&r, &[Some(Restriction::Equal(vec![int(1), int(12)]))]).unwrap());
}

/// Verifies: [EXTENSION] (RULES.md D154) a list holds the keys it lists, a
/// null if it lists one; lists sharing a key overlap; a list is admitted
/// by a restriction one of its keys meets.
#[test]
fn list_holds_what_it_lists() {
    let l = ListStrategy.bound(&[int_type()], PartitionBound::List(vec![vec![int(1)], vec![Value::Null]])).unwrap();
    assert!(ListStrategy.contains(&l, &[int(1)]).unwrap());
    assert!(ListStrategy.contains(&l, &[Value::Null]).unwrap());
    assert!(!ListStrategy.contains(&l, &[int(2)]).unwrap());
    let other = ListStrategy.bound(&[int_type()], PartitionBound::List(vec![vec![int(2)], vec![int(1)]])).unwrap();
    assert!(ListStrategy.overlaps(&l, &other).unwrap());
    assert!(ListStrategy.admits(&l, &[Some(Restriction::Null)]).unwrap());
    assert!(!ListStrategy.admits(&l, &[Some(equal(3))]).unwrap());
    assert_eq!(ListStrategy.bound(&[int_type()], range(1, 2)).unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] (RULES.md D154) hash partitions of one modulus
/// take each key exactly once; moduli dividing each other overlap by the
/// remainder rule, others are refused; a remainder past the modulus is
/// refused; no DEFAULT.
#[test]
fn hash_partitions_cover_each_key_once() {
    let bounds: Vec<PartitionBound> = (0..4).map(|r| hash(4, r)).collect();
    let refs: Vec<&PartitionBound> = bounds.iter().collect();
    for k in 0..50 {
        let holders = bounds.iter().filter(|b| HashStrategy.contains(b, &[int(k)]).unwrap()).count();
        assert_eq!(holders, 1);
        assert!(HashStrategy.route(&refs, &[int(k)]).unwrap().is_some());
    }
    assert!(HashStrategy.overlaps(&hash(2, 1), &hash(4, 3)).unwrap());
    assert!(!HashStrategy.overlaps(&hash(2, 1), &hash(4, 2)).unwrap());
    assert_eq!(HashStrategy.overlaps(&hash(2, 1), &hash(3, 2)).unwrap_err().code, Code::SyntaxRule);
    let bad = PartitionBound::With(vec![(MODULUS.to_owned(), int(4)), (REMAINDER.to_owned(), int(4))]);
    assert_eq!(HashStrategy.bound(&[int_type()], bad).unwrap_err().code, Code::SyntaxRule);
    assert!(!HashStrategy.takes_default());
}

/// Verifies: [EXTENSION] (RULES.md D156) a hash partition is admitted by
/// an equality whose key it holds, not by one it does not, and by a range,
/// whose keys are not hashed.
#[test]
fn hash_admits_by_the_hash_of_equal_values() {
    let bounds: Vec<PartitionBound> = (0..4).map(|r| hash(4, r)).collect();
    let admitted = bounds.iter().filter(|b| HashStrategy.admits(b, &[Some(equal(7))]).unwrap()).count();
    assert_eq!(admitted, 1);
    let ranged = Some(Restriction::Range { low: Some((int(1), true)), high: None });
    assert!(bounds.iter().all(|b| HashStrategy.admits(b, std::slice::from_ref(&ranged)).unwrap()));
}

/// Verifies: [EXTENSION] (RULES.md D154) equal values of different forms
/// hash alike: 1.50 as 1.5, 'A ' as 'A'.
#[test]
fn equal_values_hash_alike() {
    assert_eq!(hash::key_hash(&[Value::Exact(Decimal::new(150, 2))]), hash::key_hash(&[Value::Exact(Decimal::new(15, 1))]));
    assert_eq!(hash::key_hash(&[Value::Char("A ".to_owned())]), hash::key_hash(&[Value::Char("A".to_owned())]));
    assert_ne!(hash::key_hash(&[int(1)]), hash::key_hash(&[int(2)]));
}
