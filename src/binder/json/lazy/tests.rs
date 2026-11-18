//! Tests of the CASE a JSON_VALUE of a DEFAULT that can raise is bound
//! as: its parts, of each ON ERROR behavior.

use super::*;
use crate::value::Spelled;

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: crate::value::INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4c iii, GR5] the ON EMPTY
/// branch: of ERROR ON ERROR the cast alone, whose exception is raised;
/// of NULL ON ERROR or DEFAULT ON ERROR a CASE whose WHEN is whether the
/// cast fails, its THEN the null value or ON ERROR's value. MC/DC of
/// `on_error`: each of its three values.
#[test]
fn the_on_empty_branch_replaces_a_failed_cast_as_on_error_says() {
    let raw = Expr::Column(0);
    let cast = Expr::Cast(Box::new(Expr::Column(0)), integer());
    let error = Expr::Const(Value::Exact(Decimal::new(7, 0)));
    assert!(matches!(on_empty_branch(raw.clone(), cast.clone(), (&integer(), ValueSlot::Error), None), Expr::Cast(..)));
    let Expr::Case(whens, otherwise) = on_empty_branch(raw.clone(), cast.clone(), (&integer(), ValueSlot::Null), None) else {
        panic!("a CASE")
    };
    assert!(matches!(whens.as_slice(), [(Cond::Value(Expr::Call(..)), Expr::Const(Value::Null))]));
    assert!(matches!(*otherwise, Expr::Cast(..)));
    let Expr::Case(defaulted, _) = on_empty_branch(raw, cast, (&integer(), ValueSlot::Default), Some(error)) else { panic!("a CASE") };
    let [(_, Expr::Const(Value::Exact(d)))] = defaulted.as_slice() else { panic!("one WHEN of ON ERROR's value") };
    assert_eq!(d.units, 7);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4c iii, GR5d] the call that
/// gives the value when no DEFAULT is taken has each DEFAULT ERROR, and
/// keeps ERROR and NULL; a WHEN compares the behavior taken to a code.
#[test]
fn a_default_is_error_where_it_is_not_taken() {
    assert_eq!(unless(ValueSlot::Default), ValueSlot::Error);
    assert_eq!(unless(ValueSlot::Error), ValueSlot::Error);
    assert_eq!(unless(ValueSlot::Null), ValueSlot::Null);
    let Cond::Compare(Expr::Column(3), CompOp::Eq, Expr::Const(Value::Exact(d))) = is(&Expr::Column(3), TAKEN_EMPTY) else {
        panic!("a comparison")
    };
    assert_eq!(d.units, TAKEN_EMPTY);
}
