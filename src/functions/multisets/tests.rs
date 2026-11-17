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

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.26 SR1, §7.11 GR1, §6.51
/// SR2] ELEMENT is null of a null or empty multiset, its element of one,
/// and 21000 of more; SET is null of a null multiset; a value not a
/// multiset is refused (SQLCODE -910).
#[test]
fn element_and_set() {
    let t = DataType::multiset(DataType::Boolean);
    let element = ElementOfMultiset(DataType::Boolean);
    assert_eq!((element.name(), element.return_type(&[]).unwrap()), (ELEMENT, DataType::Boolean));
    assert!(element.call(&[Value::Null]).unwrap().is_null());
    assert!(element.call(&[ones(0)]).unwrap().is_null());
    assert_eq!(element.call(&[ones(1)]).unwrap().to_literal(), "1");
    assert_eq!(state(element.call(&[ones(2)])), "21000");
    assert_eq!(element.call(&[n(1)]).unwrap_err().code, Code::Api, "not a multiset");
    let set = SetOf(t.clone());
    assert_eq!((set.name(), set.return_type(&[]).unwrap()), (SET, t.clone()));
    assert!(set.call(&[Value::Null]).unwrap().is_null());
    assert_eq!(set.call(&[ones(3)]).unwrap().to_literal(), "MULTISET[1]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.50, §6.31] a multiset
/// constructed is of its type; each MULTISET UNION, EXCEPT and INTERSECT,
/// ALL or DISTINCT, is null if an operand is, of its operands' type, and
/// named as written.
#[test]
fn combinations_named_as_written() {
    let t = DataType::multiset(DataType::Boolean);
    let made = MultisetOf(t.clone());
    assert_eq!((made.name(), made.return_type(&[]).unwrap()), (MULTISET, t.clone()));
    let mut names = Vec::new();
    for op in [BagOp::Union, BagOp::Except, BagOp::Intersect] {
        for distinct in [false, true] {
            let c = MultisetCombination(op, distinct, t.clone());
            assert!(c.call(&[Value::Null, ones(1)]).unwrap().is_null() && c.call(&[ones(1), Value::Null]).unwrap().is_null());
            assert_eq!(c.return_type(&[]).unwrap(), t);
            names.push(c.name().to_owned());
        }
    }
    assert_eq!(
        names,
        [
            "MULTISET UNION ALL",
            "MULTISET UNION DISTINCT",
            "MULTISET EXCEPT ALL",
            "MULTISET EXCEPT DISTINCT",
            "MULTISET INTERSECT ALL",
            "MULTISET INTERSECT DISTINCT"
        ]
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.16 GR3] MEMBER is false
/// of an empty multiset, null if either is, true of a value equal to an
/// element, else null if an element is, else false. MC/DC of MEMBER's
/// decision: (empty, null) pairs by {empty, one null element} and
/// {no element null, one null element}.
#[test]
fn member_predicate() {
    let bag = |e: Vec<Value>| Value::Multiset(e.into());
    let truth = |v: Value| v.to_literal();
    assert_eq!((MemberOf.name(), MemberOf.return_type(&[]).unwrap()), (MEMBER, DataType::Boolean));
    assert_eq!(truth(MemberOf.call(&[Value::Null, bag(vec![])]).unwrap()), "FALSE", "empty");
    assert_eq!(truth(MemberOf.call(&[Value::Null, ones(1)]).unwrap()), "NULL", "a null value");
    assert_eq!(truth(MemberOf.call(&[n(1), Value::Null]).unwrap()), "NULL", "a null multiset");
    assert_eq!(truth(MemberOf.call(&[n(1), bag(vec![n(2), n(1)])]).unwrap()), "TRUE");
    assert_eq!(truth(MemberOf.call(&[n(1), bag(vec![Value::Null, n(2)])]).unwrap()), "NULL", "a null element");
    assert_eq!(truth(MemberOf.call(&[n(1), bag(vec![n(2)])]).unwrap()), "FALSE");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.17 GR3, §8.18 SR4]
/// SUBMULTISET is true of an empty one, null if either is, false of more
/// elements; IS A SET is null of a null multiset.
#[test]
fn submultiset_and_set_predicates() {
    let bag = |e: Vec<Value>| Value::Multiset(e.into());
    let truth = |v: Value| v.to_literal();
    assert_eq!((SubmultisetOf.name(), SubmultisetOf.return_type(&[]).unwrap()), (SUBMULTISET, DataType::Boolean));
    assert_eq!(truth(SubmultisetOf.call(&[bag(vec![]), Value::Null]).unwrap()), "TRUE", "empty");
    assert_eq!(truth(SubmultisetOf.call(&[ones(1), Value::Null]).unwrap()), "NULL");
    assert_eq!(truth(SubmultisetOf.call(&[Value::Null, ones(1)]).unwrap()), "NULL");
    assert_eq!(truth(SubmultisetOf.call(&[ones(2), ones(1)]).unwrap()), "FALSE", "more elements");
    assert_eq!(truth(SubmultisetOf.call(&[ones(1), ones(2)]).unwrap()), "TRUE");
    assert_eq!((IsASet.name(), IsASet.return_type(&[]).unwrap()), (SET_PREDICATE, DataType::Boolean));
    assert!(IsASet.call(&[Value::Null]).unwrap().is_null());
    assert_eq!(truth(IsASet.call(&[ones(2)]).unwrap()), "FALSE");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR7k, SR7m, SR7n]
/// COLLECT's type is a multiset of its argument's, FUSION's and
/// INTERSECTION's their multiset argument's; FUSION of a value not a
/// multiset is refused (SQLCODE -103); one bound without its argument, or
/// given a value of another form, is refused (SQLCODE -910).
#[test]
fn aggregate_types_and_arguments() {
    let row = DataType::row(vec![crate::value::types::Field { name: "X".into(), data_type: DataType::Boolean, unnamed: false }]);
    let collect = MultisetAggregate { kind: Gathering::Collect, by_query: false };
    assert_eq!(collect.name(), COLLECT);
    assert_eq!(collect.result_type(Some(row)).unwrap(), DataType::multiset(DataType::Boolean));
    assert_eq!(collect.result_type(Some(DataType::row(vec![]))).unwrap_err().code, Code::Api, "no field");
    assert_eq!(collect.result_type(Some(DataType::Boolean)).unwrap_err().code, Code::Api);
    let fusion = MultisetAggregate { kind: Gathering::Fusion, by_query: false };
    let t = DataType::multiset(DataType::Boolean);
    assert_eq!((fusion.name(), fusion.result_type(Some(t.clone())).unwrap()), (FUSION, t));
    assert_eq!(fusion.result_type(Some(DataType::Boolean)).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(fusion.result_type(None).unwrap_err().code, Code::Api);
    for kind in [Gathering::Collect, Gathering::Fusion, Gathering::Intersection] {
        assert_eq!(gathered(kind, false, &[Value::Bool(true)]).unwrap_err().code, Code::Api, "{}", kind.name());
    }
    assert_eq!(gathered(Gathering::Intersection, false, &[ones(3), ones(2), ones(4)]).unwrap().to_literal(), "MULTISET[1, 1]");
}
