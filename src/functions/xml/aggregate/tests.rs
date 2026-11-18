//! Tests of XMLAGG's accumulator.

use super::*;
use crate::value::Decimal;
use crate::xml::{Atomic, AtomicType, Item};

/// An XML value of one atomic value of `lexical`.
fn atomic(lexical: &str) -> Value {
    Value::Xml(XmlValue::of(Sequence::new(vec![Item::Atomic(Atomic::new(AtomicType::String, lexical))])))
}

fn row(values: &[Value]) -> Value {
    Value::Row(values.to_vec().into())
}

/// The lexical forms of the items of `v`, an XML value.
fn lexicals(v: &Value) -> Vec<String> {
    let Value::Xml(x) = v else { panic!("an XML value, not {v:?}") };
    x.sequence().unwrap().items().iter().map(|i| if let Item::Atomic(a) = i { a.lexical.to_string() } else { String::new() }).collect()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.2 GR2c to GR2e] the values
/// not null, in the order of their keys, descending as asked, stably; the
/// null value of none.
#[test]
fn values_in_key_order_11_2() {
    let agg = XmlAggregate { orders: vec![SortOrder::of(true, None)], collations: vec![None], content: false };
    let mut acc = agg.accumulator(DataType::Xml(XmlType::Sequence));
    assert!(acc.finish().unwrap().is_null());
    for (v, k) in [(atomic("a"), 1_i128), (Value::Null, 9), (atomic("b"), 3), (atomic("c"), 3)] {
        acc.update(&row(&[v, Value::Exact(Decimal::new(k, 0))])).unwrap();
    }
    assert_eq!(lexicals(&acc.finish().unwrap()), ["b", "c", "a"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.2 GR1] [DESIGN] RULES.md D86:
/// values holding more than `MAX_STORED` octets together are a data
/// exception, XML value overflow, 2200R.
#[test]
fn values_beyond_the_limit_overflow_11_2() {
    let agg = XmlAggregate { orders: Vec::new(), collations: Vec::new(), content: true };
    let mut acc = agg.accumulator(DataType::Xml(XmlType::Sequence));
    let half = MAX_STORED.div_euclid(2);
    acc.update(&row(&[atomic(&"x".repeat(half))])).unwrap();
    let error = acc.update(&row(&[atomic(&"y".repeat(MAX_STORED.saturating_sub(half).saturating_add(1)))])).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::XML_VALUE_OVERFLOW);
}
