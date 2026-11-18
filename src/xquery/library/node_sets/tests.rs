//! Tests of the operators on sequences of nodes.

use super::super::testing::{document, elements, empty, int, one, seq};
use super::*;

/// The local names of the elements of `value`, in order.
fn names(value: &Sequence) -> Vec<String> {
    value
        .items()
        .iter()
        .map(|i| match i {
            Item::Node(n) => n.tree().name(n.id()).map(|q| q.local.to_string()).unwrap_or_default(),
            Item::Atomic(_) => panic!("a node"),
        })
        .collect()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.3.2.1,
/// §15.3.3.1, §15.3.4.1] the examples of `op:union`, `op:intersect` and
/// `op:except`, in document order, each node once; an atomic value
/// err:XPTY0004.
#[test]
fn node_set_examples() {
    let doc = document("<po><item1/><item2/><item3/></po>");
    let [i1, i2, i3] = ["item1", "item2", "item3"].map(|n| one(&elements(&doc, n).remove(0)));
    let seq1 = seq(&[i1.clone(), i2.clone()]);
    let seq2 = seq(&[i2.clone(), i1.clone()]);
    let seq3 = seq(&[i3.clone(), i2.clone()]);
    assert_eq!(names(&union(&seq1, &seq1).unwrap()), ["item1", "item2"]);
    assert_eq!(names(&union(&seq2, &seq3).unwrap()), ["item1", "item2", "item3"]);
    assert_eq!(names(&union(&empty(), &seq3).unwrap()), ["item2", "item3"]);
    assert_eq!(names(&intersect(&seq1, &seq1).unwrap()), ["item1", "item2"]);
    assert_eq!(names(&intersect(&seq2, &seq3).unwrap()), ["item2"]);
    assert!(intersect(&empty(), &seq1).unwrap().items().is_empty());
    assert!(except(&seq1, &seq2).unwrap().items().is_empty());
    assert_eq!(names(&except(&seq2, &seq3).unwrap()), ["item1"]);
    assert_eq!(names(&except(&seq3, &empty()).unwrap()), ["item2", "item3"]);
    assert!(union(&int(1), &seq1).is_err());
    let other = document("<po><item1/></po>");
    let across = union(&seq1, &one(&elements(&other, "item1").remove(0))).unwrap();
    assert_eq!(across.items().len(), 3);
}
