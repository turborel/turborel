//! Unit tests of the expressions of user-defined types and references
//! [ISO/IEC 9075-2:2023 §6.4, §6.18 to §6.24, §6.40], and of the `<type
//! predicate>` [§8.19]: each form read into its syntax tree, and the forms
//! a method is not, which stay what they were.

use crate::ast::{Composite, Predicate, SearchCondition, StructuredExpression, TypeName, ValueExpression};
use crate::lexer::Tok;
use crate::parser::Parser;

/// The value expression `text` is, read whole.
fn value(text: &str) -> ValueExpression {
    let mut parser = Parser::new(text).unwrap();
    let e = parser.value_expression().unwrap_or_else(|e| panic!("{text}: {e:?}"));
    assert_eq!(*parser.peek(), Tok::End, "{text}: not read whole");
    e
}

/// The structured expression `text` is.
fn structured(text: &str) -> StructuredExpression {
    let ValueExpression::Structured(s, _) = value(text) else { panic!("{text}: no structured expression") };
    *s
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.18] a `<direct invocation>` of a
/// method with arguments, of a column chain of three identifiers, of a
/// parenthesized value and of another invocation.
#[test]
fn method_invocations_6_18() {
    let StructuredExpression::Method { subject, as_type, name, args } = structured("T.C.M(1, 2)") else { panic!("no method") };
    assert!(matches!(subject, ValueExpression::Column(c) if c.column == "C" && c.qualifier.is_some()));
    assert!(as_type.is_none());
    assert_eq!((name.as_str(), args.len()), ("M", 2));
    let StructuredExpression::Method { subject: nested, name: m, args: none, .. } = structured("(X).M()") else { panic!("no method") };
    assert!(matches!(nested, ValueExpression::Nested(_)));
    assert_eq!((m.as_str(), none.len()), ("M", 0));
    let StructuredExpression::Method { subject: inner, name: n, .. } = structured("NEW PT(1).M(2).N(3)") else { panic!("no method") };
    assert_eq!(n, "N");
    assert!(matches!(inner, ValueExpression::Structured(s, _) if matches!(*s, StructuredExpression::Method { .. })));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.18, §10.4, §6.16] the forms that stay
/// what they were: A.B( a routine invocation of a schema, A.B a column, and
/// (X).F a field reference, which the binder takes for a method or an
/// attribute of a structured type.
#[test]
fn chains_and_calls_stay_6_18() {
    assert!(matches!(value("S.F(1)"), ValueExpression::Call { schema: Some(s), .. } if s == "S"));
    assert!(matches!(value("A.B"), ValueExpression::Column(_)));
    assert!(matches!(value("(X).F"), ValueExpression::Composite(c, _) if matches!(*c, Composite::Field(..))));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.18] a `<generalized invocation>`,
/// with and without arguments.
#[test]
fn generalized_invocation_6_18() {
    let StructuredExpression::Method { as_type, name, args, .. } = structured("(X AS S.PT).NORM(1)") else { panic!("no method") };
    assert!(matches!(as_type, Some(TypeName::Domain(t)) if t.ident == "PT"));
    assert_eq!((name.as_str(), args.len()), ("NORM", 1));
    assert!(matches!(structured("(X AS PT).NORM"), StructuredExpression::Method { as_type: Some(_), .. }));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.19, §6.20] a static method
/// invocation, of a qualified type and without arguments, and NEW; NEW
/// and a correlation name stay a column and its name.
#[test]
fn static_methods_and_new_6_19() {
    let StructuredExpression::Static { udt, name, args } = structured("S.PT::ORIGIN(0)") else { panic!("no static") };
    assert_eq!((udt.auth.as_deref(), udt.ident.as_str(), name.as_str(), args.len()), (Some("S"), "PT", "ORIGIN", 1));
    assert!(matches!(structured("PT::ORIGIN"), StructuredExpression::Static { args: none, .. } if none.is_empty()));
    let StructuredExpression::New { udt: made, args: given } = structured("NEW PT(1, 'A')") else { panic!("no new") };
    assert_eq!((made.ident.as_str(), given.len()), ("PT", 2));
    assert!(matches!(value("NEW.X"), ValueExpression::Column(_)));
    let mut parser = Parser::new("NEW X").unwrap();
    assert!(matches!(parser.value_expression().unwrap(), ValueExpression::Column(_)));
    assert_eq!(*parser.peek(), Tok::Identifier("X".to_owned()));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.24, §6.23, §6.40] TREAT, DEREF, and
/// SPECIFICTYPE with and without ( ), of a column chain and of a value.
#[test]
fn treat_deref_specifictype_6_24() {
    assert!(matches!(structured("TREAT(X AS EMP)"), StructuredExpression::Treat { target: TypeName::Domain(_), .. }));
    assert!(matches!(structured("TREAT(R AS REF(EMP))"), StructuredExpression::Treat { target: TypeName::Reference(..), .. }));
    assert!(matches!(structured("DEREF(R)"), StructuredExpression::Deref(_)));
    assert!(matches!(structured("T.C.SPECIFICTYPE"), StructuredExpression::SpecificType(ValueExpression::Column(_))));
    assert!(matches!(structured("C.SPECIFICTYPE()"), StructuredExpression::SpecificType(ValueExpression::Column(_))));
    assert!(matches!(structured("(X).SPECIFICTYPE"), StructuredExpression::SpecificType(ValueExpression::Nested(_))));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.4] CURRENT_DEFAULT_TRANSFORM_GROUP
/// and CURRENT_TRANSFORM_GROUP_FOR_TYPE of a qualified type.
#[test]
fn transform_group_values_6_4() {
    assert!(matches!(structured("CURRENT_DEFAULT_TRANSFORM_GROUP"), StructuredExpression::CurrentDefaultTransformGroup));
    assert!(matches!(
        structured("CURRENT_TRANSFORM_GROUP_FOR_TYPE S.PT"),
        StructuredExpression::CurrentTransformGroupForType(t) if t.ident == "PT"
    ));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.21] -> of an attribute, a dereference
/// operation, and -> of a method with arguments, a method reference.
#[test]
fn dereference_and_method_reference_6_21() {
    let StructuredExpression::Dereference { subject, attribute } = structured("R->NAME") else { panic!("no dereference") };
    assert!(matches!(subject, ValueExpression::Column(_)));
    assert_eq!(attribute, "NAME");
    let StructuredExpression::MethodReference { name, args, .. } = structured("T.R -> AGE(2026)") else { panic!("no reference") };
    assert_eq!((name.as_str(), args.len()), ("AGE", 1));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.19] IS OF and IS NOT OF, of ONLY
/// types and of others.
#[test]
fn type_predicate_8_19() {
    let mut parser = Parser::new("X IS NOT OF (ONLY EMP, S.MGR)").unwrap();
    let SearchCondition::Predicate(p, _) = parser.search_condition().unwrap() else { panic!("no predicate") };
    let Predicate::Type { types, not, .. } = *p else { panic!("no type predicate") };
    assert!(not);
    assert_eq!(types.0.iter().map(|(only, _)| *only).collect::<Vec<_>>(), [true, false]);
    let mut of = Parser::new("X IS OF (EMP)").unwrap();
    assert!(matches!(of.search_condition().unwrap(), SearchCondition::Predicate(q, _) if matches!(*q, Predicate::Type { not: false, .. })));
}
