//! Unit tests of the type relations the rules of user-defined types
//! compare by.

use super::*;
use crate::value::types::UdtName;

/// The structured type `name` of schema S, its supertypes `supers`.
fn structured(name: &str, supers: &[&str]) -> DataType {
    DataType::structured(UdtName::new("S", name), supers.iter().map(|s| UdtName::new("S", s)).collect())
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.9.3.1] a structured type is a subtype
/// of itself and of each proper supertype, not of a proper subtype; any
/// other type of itself alone.
#[test]
fn subtypes_are_of_their_supertypes_4_9_3_1() {
    let (a, b) = (structured("A", &[]), structured("B", &["A"]));
    assert!(is_subtype(&b, &a));
    assert!(is_subtype(&a, &a));
    assert!(!is_subtype(&a, &b));
    assert!(is_subtype(&DataType::Boolean, &DataType::Boolean));
    assert!(!is_subtype(&DataType::Boolean, &DataType::Date));
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.1.4] structured types are compatible
/// with themselves alone, a subtype with no supertype; predefined types
/// as they compare. MC/DC: both structured (equal, not); one structured;
/// neither.
#[test]
fn compatibility_of_types_4_1_4() {
    let (a, b) = (structured("A", &[]), structured("B", &["A"]));
    assert!(compatible(&a, &a));
    assert!(!compatible(&a, &b));
    assert!(!compatible(&a, &DataType::Boolean));
    assert!(compatible(&DataType::character(1), &DataType::varying(4)));
    assert!(!all_compatible(std::slice::from_ref(&a), &[]));
    assert!(all_compatible(std::slice::from_ref(&a), std::slice::from_ref(&a)));
}
