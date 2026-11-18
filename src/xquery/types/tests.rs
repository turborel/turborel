//! Tests of the types of queries.

use std::sync::Arc;

use super::{ItemKind, SchemaType, XDT_NAMESPACE};
use crate::xml::{Atomic, AtomicType, Item, QName, XS_NAMESPACE};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §2.5.1, §2.5.4] the in-scope schema types by name,
/// atomic or not, and derives-from by the built-in hierarchy.
#[test]
fn schema_types_and_derivation() {
    let named = |uri, local| SchemaType::named(Some(uri), local);
    assert_eq!(named(XS_NAMESPACE, "integer"), Some(SchemaType::Atomic(AtomicType::Integer)));
    assert_eq!(named(XDT_NAMESPACE, "untypedAtomic"), Some(SchemaType::Atomic(AtomicType::UntypedAtomic)));
    assert_eq!(named(XS_NAMESPACE, "untypedAtomic"), None);
    assert_eq!(named(XS_NAMESPACE, "IDREFS"), Some(SchemaType::List("IDREFS")));
    assert!(!SchemaType::List("IDREFS").is_atomic());
    assert_eq!(named(XDT_NAMESPACE, "untyped"), Some(SchemaType::Untyped));
    assert_eq!(SchemaType::named(None, "integer"), None);
    let byte = SchemaType::Atomic(AtomicType::Byte);
    assert!(byte.derives_from(&SchemaType::Atomic(AtomicType::Integer)));
    assert!(byte.derives_from(&SchemaType::AnyType));
    assert!(!SchemaType::Atomic(AtomicType::Integer).derives_from(&byte));
    assert!(SchemaType::Untyped.derives_from(&SchemaType::AnyType));
    assert!(!SchemaType::Untyped.derives_from(&SchemaType::AnyAtomicType));
    for (t, shown) in [
        (SchemaType::AnyType, "xs:anyType"),
        (SchemaType::Untyped, "xdt:untyped"),
        (SchemaType::AnySimpleType, "xs:anySimpleType"),
        (SchemaType::AnyAtomicType, "xdt:anyAtomicType"),
        (SchemaType::List("NMTOKENS"), "xs:NMTOKENS"),
        (SchemaType::Atomic(AtomicType::Date), "xs:date"),
    ] {
        assert_eq!(t.to_string(), shown);
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §2.5.4.2]
/// RULES.md D106: a value of a user-defined atomic type matches the
/// built-in types above the one its type restricts, as a value of that
/// type would, and no others; a value of a built-in type matches the
/// same.
#[test]
fn values_of_user_defined_types_match_built_in_types() {
    let name = Arc::new(QName { prefix: None, uri: Some("urn:t".into()), local: "small".into() });
    let small = Item::Atomic(Atomic::new(AtomicType::Int, "5").annotated(Some(name)));
    let plain = Item::Atomic(Atomic::new(AtomicType::Int, "5"));
    for item in [&small, &plain] {
        let of = |t| ItemKind::Atomic(SchemaType::Atomic(t)).matches(item);
        assert!(of(AtomicType::Int) && of(AtomicType::Integer) && !of(AtomicType::Short) && !of(AtomicType::String));
        assert!(ItemKind::Atomic(SchemaType::AnyAtomicType).matches(item));
    }
}
