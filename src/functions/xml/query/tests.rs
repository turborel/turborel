//! Tests of the functions of XMLQUERY, XMLEXISTS and XMLTABLE as functions:
//! their names and types, a call outside a statement, and the arguments
//! the binder never gives them.

use std::sync::Arc;

use super::{Evaluation, TableColumn, XmlExistsOf, XmlFilter, XmlQueryOf, XmlRowsOf, XmlTableOf};
use crate::error::{Code, SqlState};
use crate::functions::ScalarFunction;
use crate::value::xml::{XmlType, XmlValue};
use crate::value::{DataType, Value};
use crate::xml::{Atomic, AtomicType, Item, Moment, Sequence, Shape};
use crate::xquery::{Collations, Environment, Library, Query};

/// The evaluation of `text`, of a context item first if `context`.
fn evaluation(text: &str, context: bool) -> Evaluation {
    let (library, collations) = (Library::builtin(), Collations::new());
    let env = Environment {
        namespaces: Vec::new(),
        default_element: None,
        variables: Vec::new(),
        library: &library,
        collations: &collations,
        schemas: Vec::new(),
        importable: Vec::new(),
        modules: Vec::new(),
        linked: Vec::new(),
        static_typing: false,
        passed: crate::xquery::Passed::default(),
    };
    let query = Arc::new(Query::compile(text, &env).unwrap());
    let now = Moment::parse("2026-11-17T10:30:00Z", Shape::DateTime).unwrap().unwrap();
    Evaluation { query, context, one_node: false, now, timezone: 0 }
}

/// An XML value of the integers `numbers`.
fn integers(numbers: &[&str]) -> Value {
    let items = numbers.iter().map(|n| Item::Atomic(Atomic::new(AtomicType::Integer, *n))).collect();
    Value::Xml(XmlValue::of(Sequence::new(items)))
}

/// `v` as `Debug` shows it.
fn shown(v: &Value) -> String {
    format!("{v:?}")
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR1, GR4, §8.4 GR2] [DESIGN]
/// RULES.md D106: XMLQUERY and XMLEXISTS called outside a statement, by
/// their names and of their types: a copy of the value BY VALUE; Unknown of
/// a null context item; a context item of more than one item SQLSTATE
/// 2200V; a value passed not XML SQLCODE -910.
#[test]
fn query_and_exists_as_functions() {
    let query = XmlQueryOf { evaluation: evaluation("1, 2", false), by_value: true, null_on_empty: false };
    assert_eq!((query.name(), query.return_type(&[]).unwrap()), ("XMLQUERY", DataType::Xml(XmlType::Sequence)));
    assert!(shown(&query.call(&[]).unwrap()).starts_with("Xml("));
    let exists = XmlExistsOf(evaluation(".", true));
    assert_eq!((exists.name(), exists.return_type(&[]).unwrap()), ("XMLEXISTS", DataType::Boolean));
    assert_eq!(shown(&exists.call(&[Value::Null]).unwrap()), "Null");
    assert_eq!(shown(&exists.call(&[integers(&["1"])]).unwrap()), "Bool(true)");
    assert_eq!(exists.call(&[integers(&["1", "2"])]).unwrap_err().state, Some(SqlState::INVALID_XQUERY_CONTEXT_ITEM));
    assert_eq!(exists.call(&[Value::Bool(true)]).unwrap_err().code, Code::Api);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.1 SR4h, GR1] [DESIGN] RULES.md
/// D106: XMLTABLE called outside a statement, by its name and of its type:
/// a row of each item, its ordinal; no row of a null context item.
#[test]
fn table_as_a_function() {
    let array = DataType::Boolean;
    let table =
        XmlTableOf { rows: evaluation("1 to 2", false), arguments: 0, columns: vec![TableColumn::Ordinality], array: array.clone() };
    assert_eq!((table.name(), table.return_type(&[]).unwrap()), ("XMLTABLE", array.clone()));
    assert_eq!(shown(&table.call(&[]).unwrap()).matches("Row(").count(), 2);
    let of_context = XmlTableOf { rows: evaluation(".", true), arguments: 1, columns: Vec::new(), array };
    assert_eq!(shown(&of_context.call(&[Value::Null]).unwrap()), "Array([])");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §7.1 SR4h, §8.4 GR2]
/// RULES.md D106: XMLTABLE's row pattern as a filter of the rows it is of
/// is XMLEXISTS of it: true of a value it gives an item of, unknown of the
/// null value; its reduction an implied filter's, exact and raising no
/// error of a value of one node passed, neither of a sequence's.
#[test]
fn row_pattern_as_a_filter() {
    let rows = XmlRowsOf(evaluation(".", true));
    assert_eq!((rows.name(), rows.return_type(&[]).unwrap(), rows.in_statement()), ("XMLTABLE", DataType::Boolean, true));
    assert_eq!(shown(&rows.call(&[integers(&["1"])]).unwrap()), "Bool(true)");
    assert_eq!(shown(&rows.call(&[Value::Null]).unwrap()), "Null");
    assert!(rows.xml_filter().is_none(), "of a query of no lookup");
    for one_node in [true, false] {
        let implied = XmlRowsOf(Evaluation { one_node, ..evaluation("//a", true) });
        let Some(XmlFilter::Implied(reduction)) = implied.xml_filter() else { panic!("{one_node}") };
        assert_eq!((reduction.raise_free, reduction.exact), (one_node, one_node));
        let exists = XmlExistsOf(Evaluation { one_node, ..evaluation("//a", true) });
        assert_eq!(exists.raise_free(), one_node, "a sequence's value may fail its query");
    }
}
