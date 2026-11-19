//! Unit tests: which routines depend on what is dropped, of those whose
//! finding needs no body bound.

use super::*;
use crate::catalog::{Catalog, DataAccess, Horizon, MemoryCatalog, ParameterMode, RoutineKind, SqlParameter};
use crate::error::Code;
use crate::value::{DataType, Spelled};

/// A name in schema `auth`.
fn name(auth: &str, ident: &str) -> QualifiedName {
    QualifiedName { auth: auth.into(), ident: ident.into() }
}

/// A numeric type.
fn numeric() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }
}

/// A function of specific name `specific`, of one parameter of type
/// `parameter` and result type `returns`, whose body reads `reads` and
/// invokes `invokes`.
fn routine(
    specific: &QualifiedName,
    (parameter, returns): (DataType, DataType),
    reads: &[QualifiedName],
    invokes: &[QualifiedName],
) -> SqlRoutineDesc {
    SqlRoutineDesc {
        method: None,
        distance: None,
        name: name(&specific.auth, "F"),
        specific: specific.clone(),
        kind: RoutineKind::Function,
        parameters: vec![SqlParameter { name: "X".into(), data_type: parameter, mode: ParameterMode::In, generic: None }],
        returns: Some(returns),
        deterministic: false,
        access: DataAccess::Reads,
        null_call: false,
        text: "CREATE FUNCTION F (X NUMERIC) RETURNS NUMERIC RETURN X".into(),
        body: "RETURN X".into(),
        schema: specific.auth.clone(),
        module: None,
        ptf: None,
        reads: reads.to_vec(),
        horizon: Horizon::default().invoking(invokes.to_vec()),
        sequences: Vec::new(),
        execute: Vec::new(),
        created: 1,
        stamps: crate::catalog::Stamps::default(),
    }
}

/// The definitions with `routines` and nothing else.
fn state(routines: Vec<SqlRoutineDesc>) -> State {
    let mut catalog = MemoryCatalog::default();
    for r in routines {
        catalog.put_routine(r);
    }
    State::with_builtins(Box::new(catalog))
}

/// No routine.
const NONE: [QualifiedName; 0] = [];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.2 SR4l, GR10] the
/// routines of a schema are those whose specific names are in it, and
/// none of another.
#[test]
fn a_schemas_routines_are_those_in_it() {
    let (a, b) = (name("A", "F_1"), name("B", "F_2"));
    let st = state(vec![routine(&a, (numeric(), numeric()), &[], &[]), routine(&b, (numeric(), numeric()), &[], &[])]);
    assert_eq!(st.schema_routines_of("A"), vec![a]);
    assert_eq!(st.schema_routines_of("B"), vec![b]);
    assert_eq!(st.schema_routines_of("C"), NONE);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.59 SR4g, GR1] a
/// routine is typed by a user-defined type if a parameter's type or its
/// result type is based on it, an array of it included, and not if
/// neither is.
#[test]
fn routines_typed_by_a_type_are_found_by_parameter_or_result() {
    let distinct = DataType::distinct("A", "D", numeric());
    let array = DataType::array(distinct.clone(), 3);
    let (by_parameter, by_result, neither) = (name("A", "P_1"), name("A", "R_1"), name("A", "N_1"));
    let st = state(vec![
        routine(&by_parameter, (distinct, numeric()), &[], &[]),
        routine(&by_result, (numeric(), array), &[], &[]),
        routine(&neither, (numeric(), numeric()), &[], &[]),
    ]);
    let mut found = st.routines_typed_by(&[name("A", "D")]);
    found.sort_by(|x, y| x.ident.cmp(&y.ident));
    assert_eq!(found, vec![by_parameter, by_result]);
    assert_eq!(st.routines_typed_by(&[name("A", "E")]), NONE, "another type");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.31 SR7e, §11.33 SR5d]
/// under RESTRICT, a table or view a routine's body references is not
/// dropped, SQLCODE -103 citing the rule of a table or of a view; one no
/// body references takes no routine.
#[test]
fn a_table_a_routine_reads_is_dropped_only_with_cascade() {
    let (read, unread) = (name("A", "T"), name("A", "U"));
    let mut st = state(vec![routine(&name("A", "F_1"), (numeric(), numeric()), std::slice::from_ref(&read), &[])]);
    for (table, cited) in [(true, "§11.31 SR7e"), (false, "§11.33 SR5d")] {
        let Err(e) = st.drop_routining(&[&read], (false, table)) else { panic!("{cited} refuses it") };
        assert_eq!(e.code, Code::SyntaxRule);
        assert_eq!(e.rule.as_deref(), Some(format!("ISO/IEC 9075-2:2023 {cited}").as_str()));
    }
    for cascade in [false, true] {
        assert_eq!(st.drop_routining(&[&unread], (cascade, true)).unwrap().names, NONE);
    }
    assert!(st.catalog.routine(&name("A", "F_1")).is_some());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.62 SR4c i, GR1,
/// §4.35.2] what invokes a routine dropped includes each other routine
/// whose body invokes it, by its specific name fixed as that routine was
/// defined; not a routine dropped with it, though it invokes it, nor one
/// invoking another routine of the same name.
#[test]
fn routines_invoking_one_dropped_are_abandoned() {
    let (dropped, invoker) = (name("A", "F_1"), name("A", "G_1"));
    let (other, overload) = (name("A", "H_1"), name("A", "F_2"));
    let types = (numeric(), numeric());
    let st = state(vec![
        routine(&dropped, types.clone(), &[], std::slice::from_ref(&dropped)),
        routine(&invoker, types.clone(), &[], std::slice::from_ref(&dropped)),
        routine(&other, types.clone(), &[], std::slice::from_ref(&overload)),
        routine(&overload, types, &[], &[]),
    ]);
    let abandoned = st.invoking(std::slice::from_ref(&dropped));
    assert_eq!(abandoned.routines, vec![invoker]);
    assert!(abandoned.views.is_empty() && abandoned.triggers.is_empty() && abandoned.assertions.is_empty());
    assert!(st.without_routines(std::slice::from_ref(&dropped)).catalog.routine(&dropped).is_none());
}
