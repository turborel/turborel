use super::*;
use crate::value::Decimal;
use crate::value::types::MAX_MULTISET_CARDINALITY;

/// The whole number `n`.
fn n(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The multiset of `count` ones.
fn ones(count: usize) -> Value {
    Value::Multiset(vec![n(1); count].into())
}

/// The limit on a multiset's elements.
fn limit() -> usize {
    usize::try_from(MAX_MULTISET_CARDINALITY).unwrap()
}

/// The SQLSTATE of `r`, which must fail.
fn state(r: Result<Value>) -> String {
    r.unwrap_err().sqlstate().to_string()
}

/// The aggregate `kind` of the values `of`, read by a constructor by
/// query if `by_query`.
fn gathered(kind: Gathering, by_query: bool, of: &[Value]) -> Result<Value> {
    let mut acc = MultisetAggregate { kind, by_query }.accumulator(DataType::Boolean);
    for v in of {
        acc.update(v)?;
    }
    acc.finish()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR3b, §6.50 SR3d,
/// §6.52 GR2] a multiset is made of up to `MAX_MULTISET_CARDINALITY`
/// elements, and one past it is multiset value overflow, 2200Q: of an
/// enumeration, a MULTISET UNION, COLLECT and FUSION each.
#[test]
fn overflow_past_the_limit() {
    let t = DataType::multiset(DataType::Boolean);
    let at = vec![n(1); limit()];
    assert!(MultisetOf(t.clone()).call(&at).is_ok());
    assert_eq!(state(MultisetOf(t.clone()).call(&vec![n(1); limit() + 1])), "2200Q");
    let union = MultisetCombination(BagOp::Union, false, t);
    assert!(union.call(&[ones(limit() - 1), ones(1)]).is_ok());
    assert_eq!(state(union.call(&[ones(limit()), ones(1)])), "2200Q");
    let rows = vec![Value::Row(vec![n(1)].into()); limit() + 1];
    assert!(gathered(Gathering::Collect, false, rows.split_at(limit()).0).is_ok());
    assert_eq!(state(gathered(Gathering::Collect, false, &rows)), "2200Q");
    assert_eq!(state(gathered(Gathering::Collect, true, &rows)), "2200Q", "by query");
    assert!(gathered(Gathering::Fusion, false, &[ones(limit() - 1), ones(1)]).is_ok());
    assert_eq!(state(gathered(Gathering::Fusion, false, &[ones(limit()), ones(1)])), "2200Q");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR7d ii, §6.52 GR3b]
/// each aggregate is null of no values, but a constructor by query's
/// COLLECT the empty multiset; INTERSECTION of one multiset is it.
#[test]
fn of_no_values() {
    for kind in [Gathering::Collect, Gathering::Fusion, Gathering::Intersection] {
        assert!(gathered(kind, false, &[]).unwrap().is_null(), "{}", kind.name());
    }
    assert_eq!(gathered(Gathering::Collect, true, &[]).unwrap().to_literal(), "MULTISET[]");
    assert_eq!(gathered(Gathering::Intersection, false, &[ones(2)]).unwrap().to_literal(), "MULTISET[1, 1]");
}
