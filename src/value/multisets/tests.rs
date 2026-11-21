use super::*;
use crate::value::Decimal;

/// The whole number `n`.
fn n(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The null value.
fn null() -> Value {
    Value::Null
}

/// The row of `fields`.
fn row(fields: Vec<Value>) -> Value {
    Value::Row(fields.into())
}

/// The literals of `of`.
fn literals(of: &[Value]) -> Vec<String> {
    of.iter().map(Value::to_literal).collect()
}

/// The hash `Hash` gives `k`.
fn hash_of(k: &BagKey) -> u64 {
    let mut h = crate::hash::xxhash::Xxh3Hasher::default();
    k.hash(&mut h);
    h.finish()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR1b iii] equality of
/// multisets, MC/DC of its cases: other cardinalities false; of no null,
/// a pairing of equal pairs true, in any order, and none false; of nulls,
/// a pairing of equal or unknown pairs unknown, and none false. The
/// unknown case is at its bound when the nulls of one side exactly cover
/// what the classes leave of the other, as one null covers 2.
#[test]
fn equality_8_2() {
    assert_eq!(equal(&[n(1)], &[n(1), n(1)]), Some(false), "cardinality");
    assert_eq!(equal(&[n(1), n(2), n(2)], &[n(2), n(1), n(2)]), Some(true), "any order");
    assert_eq!(equal(&[n(1), n(2), n(2)], &[n(1), n(1), n(2)]), Some(false), "counts differ");
    assert_eq!(equal(&[n(1), null()], &[n(2), n(1)]), None, "the null pairs with 2");
    assert_eq!(equal(&[n(1), null()], &[n(2), n(3)]), Some(false), "1 pairs with nothing: the left uncovered");
    assert_eq!(equal(&[n(2), n(3)], &[n(1), null()]), Some(false), "the right uncovered");
    assert_eq!(equal(&[null(), null()], &[n(5), n(6)]), None, "nulls pair with anything");
    assert_eq!(equal(&[], &[]), Some(true));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR1b iii, GR7] a row
/// holding a null pairs with a row only where its fields' equality is not
/// false: (1, null) with (1, 2), not with (2, 2), even though its first
/// field compares first.
#[test]
fn rows_holding_nulls_8_2() {
    assert_eq!(equal(&[row(vec![n(1), null()])], &[row(vec![n(1), n(2)])]), None);
    assert_eq!(equal(&[row(vec![n(1), null()])], &[row(vec![n(2), n(2)])]), Some(false));
    assert_eq!(equal(&[row(vec![null(), n(1)])], &[row(vec![n(2), n(3)])]), Some(false), "a later field false");
}

/// Verifies: [DESIGN] RULES.md D80: the matching finds a pairing a greedy
/// one misses. Of [x, y] against [a, b], x matching a and b, y matching a
/// alone, x taking a first must give it up to y by an augmenting path.
#[test]
fn matching_augments() {
    let x = row(vec![n(1), null()]);
    let y = row(vec![null(), n(5)]);
    let (a, b) = (row(vec![n(1), n(5)]), row(vec![n(1), n(6)]));
    let (matched, (lx, wx), (ly, wy)) = matching(&[&x, &y], &[&a, &b]);
    assert_eq!((matched, lx, wx, ly, wy), (2, 2, 0, 2, 0));
    assert_eq!(equal(&[x, y], &[a, b]), None);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.17 GR3c] SUBMULTISET of no
/// fewer, MC/DC of its cases: more elements false \[i\]; each paired with
/// an equal one true \[ii\], by "no null" and "each class within the
/// other's count" each; each with an equal or unknown one unknown
/// \[iii\]; else false \[iv\].
#[test]
fn submultiset_8_17() {
    assert_eq!(submultiset(&[n(1), n(1)], &[n(1)]), Some(false), "i");
    assert_eq!(submultiset(&[n(2), n(1)], &[n(1), n(3), n(2)]), Some(true), "ii");
    assert_eq!(submultiset(&[n(1), n(1)], &[n(1), n(2)]), Some(false), "a class past the other's count");
    assert_eq!(submultiset(&[n(1), null()], &[n(3), n(1)]), None, "a null: iii");
    assert_eq!(submultiset(&[n(1), n(4)], &[n(1), null()]), None, "4 pairs with the null: iii");
    assert_eq!(submultiset(&[n(1), n(4)], &[n(1), n(5)]), Some(false), "iv");
    assert_eq!(submultiset(&[null(), null()], &[n(1)]), Some(false), "i before nulls");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15 GR1c vi] two multisets
/// are distinct if their cardinalities differ, or some value is not
/// distinct from more elements of one; nulls are not distinct from one
/// another.
#[test]
fn distinctness_8_15() {
    assert!(bags_distinct(&[n(1)], &[n(1), n(1)]).unwrap(), "cardinality");
    assert!(!bags_distinct(&[null(), n(1)], &[n(1), null()]).unwrap(), "nulls alike");
    assert!(bags_distinct(&[null(), n(1)], &[n(1), n(1)]).unwrap(), "a value of one alone");
    assert!(bags_distinct(&[n(1), n(1), n(2)], &[n(1), n(2), n(2)]).unwrap(), "each value of both, in other counts");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.50 SR2d, SR3d, SR4d, §6.51
/// SR2, §8.18 SR4] each multiset operation, ALL and DISTINCT, as many of
/// each class of elements not distinct from one another as its query
/// keeps, the first operand's first; SET one of each; IS A SET whether
/// each class has one.
#[test]
fn operations_6_50() {
    let (x, y) = ([n(1), n(1), n(2), null()], [n(1), n(3), null(), null()]);
    let of = |op, distinct| combined(op, distinct, &x, &y).unwrap();
    assert_eq!(of(BagOp::Union, false).len(), 8);
    assert_eq!(literals(&of(BagOp::Union, true)), literals(&[n(1), n(2), null(), n(3)]));
    assert_eq!(literals(&of(BagOp::Except, false)), literals(&[n(1), n(2)]));
    assert_eq!(literals(&of(BagOp::Except, true)), literals(&[n(2)]));
    assert_eq!(literals(&of(BagOp::Intersect, false)), literals(&[n(1), null()]));
    assert_eq!(literals(&of(BagOp::Intersect, true)), literals(&[n(1), null()]));
    assert_eq!(literals(&combined(BagOp::Union, true, &x, &[]).unwrap()), literals(&[n(1), n(2), null()]), "SET");
    assert!(is_set(&[n(1), n(2)]).unwrap());
    assert!(!is_set(&[null(), null()]).unwrap());
}

/// Verifies: [DESIGN] RULES.md D80: multisets equal in any order share a
/// bucket and a normalized form, which hashes alike; a null element gives
/// no bucket, but one on which a null matches a null.
#[test]
fn buckets_and_forms_any_order() {
    assert_eq!(bag_bucket(&[n(1), n(2), n(2)]), bag_bucket(&[n(2), n(1), n(2)]));
    assert_ne!(bag_bucket(&[n(1), n(2)]), bag_bucket(&[n(1), n(3)]));
    assert_eq!(bag_bucket(&[n(1), null()]), None);
    let nulls = |v: &Value| key_bucket(v, true);
    assert_eq!(null_bag_bucket(&[null(), n(1)], nulls).unwrap(), null_bag_bucket(&[n(1), null()], nulls).unwrap());
    let (NormKey::Bag(a), NormKey::Bag(b)) = (norm_bag(&[n(1), n(2), n(2)]), norm_bag(&[n(2), n(2), n(1)])) else {
        panic!("the form of a multiset is a bag")
    };
    assert_eq!(a, b);
    assert_eq!(hash_of(&a), hash_of(&b));
    let NormKey::Bag(c) = norm_bag(&[n(1), n(1), n(2)]) else { panic!("the form of a multiset is a bag") };
    assert_ne!(a, c, "the same forms, other counts");
}
