//! Tests of the bound on the depth of a static type [XQueryFS §2.4.3]:
//! each walk of a type nested past `MAX_TYPE_DEPTH` is SQLCODE -900,
//! whichever of its constructors nests it, and never overflows the stack.

use std::fmt::Debug;

use super::super::functions::{Pick, pick};
use super::super::subtype::{can_promote, item_sub, map_items, subtype};
use super::super::types::{Item, MAX_TYPE_DEPTH, Occ, Type, factored, is_empty, prime, quantifier};
use super::super::values::{boolean, convert_untyped, data};
use crate::error::Result;
use crate::xml::AtomicType;
use crate::xquery::plan::{Axis, StepTest};
use crate::xquery::types::NodeKind;

/// How many times a deep type nests its constructor: past the bound.
const LEVELS: usize = MAX_TYPE_DEPTH + 8;

/// `xs:integer` within `wrap` `LEVELS` times.
fn nested(wrap: fn(Type) -> Type) -> Type {
    let mut t = Type::atomic(AtomicType::Integer);
    for _ in 0..LEVELS {
        t = wrap(t);
    }
    t
}

/// A type of each constructor nested past the bound, by its name: an
/// occurrence, a sequence of one or two, a choice, an interleaving.
fn deep() -> Vec<(&'static str, Type)> {
    vec![
        ("occurrence", nested(|t| Type::Occur(Box::new(t), Occ::Star))),
        ("sequence of one", nested(|t| Type::Seq(vec![t]))),
        ("sequence", nested(|t| Type::Seq(vec![t, Type::Item(Item::Text)]))),
        ("choice", nested(|t| Type::Choice(vec![t, Type::Item(Item::Text)]))),
        ("interleaving", nested(|t| Type::All(vec![t, Type::Item(Item::Comment)]))),
    ]
}

/// Checks `result` is the error past turborel's limits, SQLCODE -900.
fn beyond<T: Debug>(what: &str, result: Result<T>) {
    let error = result.expect_err(what);
    assert_eq!(error.sqlcode(), -900_i32, "{what}: {error}");
    assert!(error.msg.contains("deeper than turborel's limit"), "{what}: {error}");
}

/// Verifies: [DESIGN] RULES.md D106, R-CF-6: prime, quantifier, the
/// factored form, the empty check, fn:data's type, the effective boolean
/// value's check, the conversion of untyped values, an item picked and an
/// axis step of a type nested past `MAX_TYPE_DEPTH` are each SQLCODE -900.
#[test]
fn each_walk_of_a_type_too_deep_is_refused() {
    for (name, t) in deep() {
        beyond(name, prime(&t));
        beyond(name, quantifier(&t));
        beyond(name, factored(&t));
        beyond(name, is_empty(&t));
        beyond(name, data(&t));
        beyond(name, boolean(&t));
        beyond(name, convert_untyped(&t, &Type::atomic(AtomicType::Double)));
        beyond(name, map_items(&t, 0, &mut |i| Ok(Type::Item(i.clone()))));
        beyond(name, pick(&t, Pick::First));
        beyond(name, super::super::axes::step(&t, Axis::Child, &StepTest::Kind(NodeKind::Any)));
    }
}

/// Checks `result` is SQLCODE -900, or false: a type too deep is never
/// found a subtype.
fn refused_or_false(what: &str, result: Result<bool>) {
    match result {
        Ok(found) => assert!(!found, "{what}"),
        Err(error) => assert_eq!(error.sqlcode(), -900_i32, "{what}: {error}"),
    }
}

/// Verifies: [DESIGN] RULES.md D106, R-CF-6: subtyping and promotion of a
/// type nested past `MAX_TYPE_DEPTH` is SQLCODE -900, of it on the left
/// and of a choice it is in; on the right, and within a document type, a
/// choice, a sequence or an interleaving there, -900 or false, the type
/// never admitted.
#[test]
fn subtyping_a_type_too_deep_is_refused() {
    let integers = Type::occur(Type::atomic(AtomicType::Integer), Occ::Star);
    for (name, t) in deep() {
        beyond(name, subtype(&t, &integers));
        beyond(name, can_promote(&t, &integers));
        beyond(name, subtype(&Type::Choice(vec![t.clone(), Type::Item(Item::Text)]), &integers));
        refused_or_false(name, subtype(&integers, &t));
        refused_or_false(name, subtype(&t, &t));
        refused_or_false(name, can_promote(&integers, &t));
        let document = Item::Document(Box::new(t.clone()));
        refused_or_false(name, item_sub(&document, &document, 0));
        let comment = Type::Item(Item::Comment);
        let pair = Type::Seq(vec![comment.clone(), t.clone()]);
        refused_or_false(name, subtype(&comment, &Type::Choice(vec![pair, Type::Item(Item::Pi)])));
        refused_or_false(name, subtype(&comment, &Type::All(vec![t.clone(), Type::Item(Item::Pi)])));
    }
    let occurrence = nested(|t| Type::Occur(Box::new(t), Occ::Star));
    beyond("on the right", subtype(&integers, &occurrence));
    let document = Item::Document(Box::new(occurrence.clone()));
    beyond("of documents", item_sub(&document, &document, 0));
    let comment = Type::Item(Item::Comment);
    let pair = Type::Seq(vec![comment.clone(), occurrence.clone()]);
    beyond("in a choice", subtype(&comment, &Type::Choice(vec![pair, Type::Item(Item::Pi)])));
    beyond("in an interleaving", subtype(&comment, &Type::All(vec![occurrence, Type::Item(Item::Pi)])));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §8.3.2,
/// §8.4] a sequence of one type is that type:
/// a subtype of what it is, and not of what it is not.
#[test]
fn a_sequence_of_one_type_is_that_type() {
    let one = Type::Seq(vec![Type::atomic(AtomicType::Integer)]);
    assert!(subtype(&Type::atomic(AtomicType::Integer), &one).unwrap());
    assert!(subtype(&Type::atomic(AtomicType::Short), &one).unwrap());
    assert!(!subtype(&Type::atomic(AtomicType::String), &one).unwrap());
}
