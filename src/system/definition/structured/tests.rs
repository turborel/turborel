//! Unit tests of the Definition Schema's rows of structured types, typed
//! tables and their routines, read from a catalog that holds them as the
//! definitions would leave them (RULES.md R-TEST-1).

use std::sync::Arc;

use super::super::tables;
use crate::ast::{MethodKind, OrderingForm, RefGeneration};
use crate::catalog::{
    AttributeDesc, CastDesc, Catalog, DataAccess, MethodSpecDesc, OrderingCategory, OrderingDesc, ParameterMode, QualifiedName,
    RefRepresentation, RoutineDesc, SqlParameter, SqlRoutineDesc, StructuredDesc, TransformGroupDesc, TypedDesc, UsageDesc,
};
use crate::db::{Database, State};
use crate::system::SystemRead;
use crate::value::DataType;
use crate::value::types::UdtName;

/// The name `ident` in schema S.
fn s(ident: &str) -> QualifiedName {
    QualifiedName { auth: "S".to_owned(), ident: ident.to_owned() }
}

/// A routine of a type, its specific name `specific`.
fn routine(name: &str, specific: &str) -> RoutineDesc {
    RoutineDesc { name: name.to_owned(), specific: specific.to_owned(), execute: Vec::new() }
}

/// The attribute `name` of `t`, defined by `by`, of default `default`.
fn attribute(by: &str, name: &str, t: &DataType, default: Option<&str>) -> AttributeDesc {
    AttributeDesc {
        name: name.to_owned(),
        data_type: t.clone(),
        default: default.map(str::to_owned),
        collation: None,
        defined_by: s(by),
        observer: routine(name, &format!("{by}_{name}_OBSERVER")),
        mutator: routine(name, &format!("{by}_{name}_MUTATOR")),
    }
}

/// The instance method specification `name` of specific name `specific`.
fn method(name: &str, specific: &str, parameters: Vec<SqlParameter>, returns: &DataType) -> MethodSpecDesc {
    MethodSpecDesc {
        name: name.to_owned(),
        specific: s(specific),
        kind: MethodKind::Instance,
        overrides: None,
        parameters,
        returns: returns.clone(),
        self_as_result: false,
        deterministic: false,
        access: DataAccess::Contains,
        null_call: false,
        created: 1,
    }
}

/// An input parameter `name` of type `t`.
fn parameter(name: &str, t: &DataType) -> SqlParameter {
    SqlParameter { name: name.to_owned(), data_type: t.clone(), mode: ParameterMode::In, generic: None }
}

/// A structured type `name`, of no attribute, method or supertype.
fn structured(name: &str, reference: RefRepresentation) -> StructuredDesc {
    StructuredDesc {
        name: s(name),
        supertype: None,
        supertypes: Vec::new(),
        instantiable: true,
        is_final: false,
        attributes: Vec::new(),
        methods: Vec::new(),
        reference,
        ref_casts: None,
        constructor: Some(routine(name, &format!("{name}_CONSTRUCTOR"))),
        usage: Vec::new(),
        under: Vec::new(),
    }
}

/// Schema S of tables PEOPLE, typed of PERSON, its BOSS of REF(PERSON)
/// SCOPE PEOPLE, and STAFF, typed of EMPLOYEE, a subtable of PEOPLE;
/// SELECT on PEOPLE and USAGE on PERSON granted to R. PERSON has
/// attributes NAME and BOSS and methods GREETING and RENAMED, SELF AS
/// RESULT, of a body; EMPLOYEE, final, not instantiable, of a derived
/// representation, inherits them, adds PAY, overrides GREETING and has a
/// static MAKE; POINT, of a user-defined representation, is ordered FULL
/// BY MAP, has a transform group G, and a cast from INTEGER by MAKE_POINT,
/// AS ASSIGNMENT.
fn state() -> State {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE PEOPLE (ID INTEGER, NAME CHARACTER VARYING(20), BOSS INTEGER)
           CREATE TABLE STAFF (ID INTEGER, NAME CHARACTER VARYING(20), BOSS INTEGER, PAY INTEGER)
           CREATE FUNCTION MAKE_POINT (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X
           GRANT SELECT ON PEOPLE TO R",
    )
    .unwrap();
    let mut st = (*db.snapshot().unwrap()).clone();
    let mut c = st.catalog.duplicate();
    let people = c.table(&s("PEOPLE")).unwrap().clone();
    let (integer, name) = (people.columns[0].data_type.clone(), people.columns[1].data_type.clone());
    let mut person = structured("PERSON", RefRepresentation::System);
    let person_type = person.data_type();
    let boss = DataType::reference(person.structured_type(), Some(UdtName::new("S", "PEOPLE")));
    person.attributes = vec![attribute("PERSON", "NAME", &name, Some("'X'")), attribute("PERSON", "BOSS", &boss, None)];
    let renamed = MethodSpecDesc { self_as_result: true, ..method("RENAMED", "RENAMED_1", vec![parameter("N", &name)], &person_type) };
    person.methods = vec![method("GREETING", "GREETING_1", Vec::new(), &name), renamed];
    person.usage = vec![UsageDesc { grantor: "S".to_owned(), grantee: Some("R".to_owned()), grantable: false }];
    let mut employee = structured("EMPLOYEE", RefRepresentation::Derived(vec!["NAME".to_owned()]));
    employee.supertype = Some(s("PERSON"));
    employee.supertypes = vec![UdtName::new("S", "PERSON")];
    (employee.instantiable, employee.is_final, employee.constructor) = (false, true, None);
    employee.attributes = [person.attributes.clone(), vec![attribute("EMPLOYEE", "PAY", &integer, None)]].concat();
    let greeting = MethodSpecDesc { overrides: Some(s("GREETING_1")), ..method("GREETING", "GREETING_2", Vec::new(), &name) };
    let make =
        MethodSpecDesc { kind: MethodKind::Static, ..method("MAKE", "MAKE_1", vec![parameter("N", &integer)], &employee.data_type()) };
    employee.methods = vec![greeting, make];
    let mut point = structured("POINT", RefRepresentation::User(integer.clone()));
    point.is_final = true;
    point.attributes = vec![attribute("POINT", "X", &integer, None)];
    let point_type = point.data_type();
    let maker = c.routines_named(&s("MAKE_POINT"))[0].clone();
    c.put_routine(renamed_body(&maker, person_type, &name));
    c.put_cast(CastDesc { source: integer, target: point_type, function: maker.specific.clone(), assignment: true });
    c.put_ordering(OrderingDesc { udt: s("PERSON"), form: OrderingForm::Equals, category: OrderingCategory::State(None) });
    c.put_ordering(OrderingDesc { udt: s("POINT"), form: OrderingForm::Full, category: OrderingCategory::Map(s("POINT_X_OBSERVER")) });
    let group = TransformGroupDesc { name: "G".to_owned(), to_sql: Some(s("T_IN")), from_sql: Some(s("T_OUT")) };
    c.put_transforms(&s("POINT"), vec![group]);
    for t in [person, employee, point] {
        c.put_structured_type(t);
    }
    make_typed(&mut *c, boss);
    st.catalog = Arc::from(c);
    st
}

/// The body of PERSON's method RENAMED, of type `person`, of a parameter
/// N of type `name`: a routine like `maker`, made after it, of a method's
/// own text, so its body binds as the Definition Schema's tables of usage
/// bind it.
fn renamed_body(maker: &SqlRoutineDesc, person: DataType, name: &DataType) -> SqlRoutineDesc {
    let mut body = maker.clone();
    (body.name, body.specific, body.created) = (s("RENAMED"), s("RENAMED_1"), maker.created.saturating_add(1));
    body.text = "CREATE INSTANCE METHOD RENAMED (N CHARACTER VARYING(20)) RETURNS PERSON FOR PERSON RETURN SELF".to_owned();
    "RETURN SELF".clone_into(&mut body.body);
    body.parameters = vec![parameter("SELF", &person), parameter("N", name)];
    (body.returns, body.method) = (Some(person), Some((s("PERSON"), MethodKind::Instance)));
    body
}

/// Makes PEOPLE typed of PERSON, its BOSS of `boss`, and STAFF typed of
/// EMPLOYEE, a subtable of PEOPLE.
fn make_typed(c: &mut dyn Catalog, boss: DataType) {
    let mut people = c.table(&s("PEOPLE")).unwrap().clone();
    people.columns[2].data_type = boss;
    let system = RefGeneration::System;
    people.typed = Some(TypedDesc { udt: s("PERSON"), self_referencing: "ID".to_owned(), generation: system, supertable: None });
    c.replace_table(people);
    let mut staff = c.table(&s("STAFF")).unwrap().clone();
    let derived = RefGeneration::Derived;
    let supertable = Some(s("PEOPLE"));
    staff.typed = Some(TypedDesc { udt: s("EMPLOYEE"), self_referencing: "ID".to_owned(), generation: derived, supertable });
    c.replace_table(staff);
}

/// The columns `columns` of the rows of the Definition Schema's table
/// `name`, as user S reads it, each row's values joined by bars, those
/// whose first value picked is `first` alone if it is not empty.
fn picked(st: &State, name: &str, columns: &[&str], first: &str) -> Vec<String> {
    let table = tables().into_iter().find(|t| t.name() == name).unwrap();
    let desc = table.columns();
    let at: Vec<usize> = columns.iter().map(|c| desc.iter().position(|d| d.name == *c).unwrap()).collect();
    let read = SystemRead { table, user: "S".into(), own: crate::settings::Own::default() };
    let shown = |r: Vec<crate::value::Value>| at.iter().map(|i| r[*i].to_string().trim_end().to_owned()).collect::<Vec<_>>();
    let rows = read.rows(st).unwrap().into_iter().map(shown);
    rows.filter(|r| first.is_empty() || r[0] == first).map(|r| r.join("|")).collect()
}

/// Verifies: [DESIGN] RULES.md D37, `SystemTable`: of structured types and
/// typed tables too, each table of the Definition Schema gives rows of its
/// columns' degree and types, none null in a NOT NULL column, and every
/// view of the Information Schema binds.
#[test]
fn every_table_keeps_the_contract_of_structured_types() {
    let st = state();
    for table in tables() {
        let read = SystemRead { table, user: "S".into(), own: crate::settings::Own::default() };
        read.rows(&st).unwrap();
    }
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.61 Description 2 to 10;
/// ISO/IEC 9075-2:2023 §4.13] USER_DEFINED_TYPES shows each structured
/// type, instantiable and final or not, of the ordering the catalog holds
/// or NONE, and its reference type's representation, of a user-defined
/// one REF_DTD_IDENTIFIER and its type's descriptor. MC/DC of the
/// ordering: one held, of a routine or not, and none.
#[test]
fn structured_types_are_described() {
    let columns = [
        "USER_DEFINED_TYPE_NAME",
        "USER_DEFINED_TYPE_CATEGORY",
        "IS_INSTANTIABLE",
        "IS_FINAL",
        "ORDERING_FORM",
        "ORDERING_CATEGORY",
        "ORDERING_ROUTINE_NAME",
        "REFERENCE_TYPE",
        "SOURCE_DTD_IDENTIFIER",
        "REF_DTD_IDENTIFIER",
        "DATA_TYPE",
    ];
    assert_eq!(
        picked(&state(), "USER_DEFINED_TYPES", &columns, ""),
        [
            "EMPLOYEE|STRUCTURED|NO|YES|NONE|NULL|NULL|DERIVED|NULL|NULL|NULL",
            "PERSON|STRUCTURED|YES|NO|EQUALS|STATE|NULL|SYSTEM GENERATED|NULL|NULL|NULL",
            "POINT|STRUCTURED|YES|YES|FULL|MAP|POINT_X_OBSERVER|USER GENERATED|NULL|0|INTEGER",
        ]
    );
    let supertypes = ["USER_DEFINED_TYPE_NAME", "SUPERTYPE_SCHEMA", "SUPERTYPE_NAME"];
    assert_eq!(picked(&state(), "DIRECT_SUPERTYPES", &supertypes, ""), ["EMPLOYEE|S|PERSON"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.6 Description 1 to 6] ATTRIBUTES
/// shows a subtype's attributes, inherited first, at their ordinal
/// positions; a default as written; whether a derived representation names
/// one; and a reference type's referenced type and scope.
#[test]
fn attributes_are_described() {
    let columns = [
        "UDT_NAME",
        "ATTRIBUTE_NAME",
        "ORDINAL_POSITION",
        "DTD_IDENTIFIER",
        "ATTRIBUTE_DEFAULT",
        "IS_DERIVED_REFERENCE_ATTRIBUTE",
        "DATA_TYPE",
        "USER_DEFINED_TYPE_NAME",
        "SCOPE_NAME",
    ];
    assert_eq!(
        picked(&state(), "ATTRIBUTES", &columns, "EMPLOYEE"),
        [
            "EMPLOYEE|NAME|1|1|'X'|YES|CHARACTER VARYING|NULL|NULL",
            "EMPLOYEE|BOSS|2|2|NULL|NO|REF|PERSON|PEOPLE",
            "EMPLOYEE|PAY|3|3|NULL|NO|INTEGER|NULL|NULL",
        ]
    );
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.29, §6.30; ISO/IEC 9075-2:2023
/// §11.51 SR10g] METHOD_SPECIFICATIONS shows the observer and mutator of
/// each attribute a type defines, then its own specifications, static,
/// overriding or neither, of LANGUAGE SQL and so of no IS_DETERMINISTIC;
/// METHOD_SPECIFICATION_PARAMETERS each one's unaugmented parameters.
#[test]
fn method_specifications_are_described() {
    let columns = [
        "UDT_NAME",
        "METHOD_NAME",
        "SPECIFIC_NAME",
        "IS_STATIC",
        "IS_OVERRIDING",
        "IS_CONSTRUCTOR",
        "DTD_IDENTIFIER",
        "METHOD_LANGUAGE",
        "IS_DETERMINISTIC",
        "SQL_DATA_ACCESS",
        "IS_NULL_CALL",
        "DATA_TYPE",
    ];
    assert_eq!(
        picked(&state(), "METHOD_SPECIFICATIONS", &columns, "EMPLOYEE"),
        [
            "EMPLOYEE|PAY|EMPLOYEE_PAY_OBSERVER|NO|NO|NO|M1|SQL|NULL|CONTAINS SQL|YES|INTEGER",
            "EMPLOYEE|PAY|EMPLOYEE_PAY_MUTATOR|NO|NO|NO|M2|SQL|NULL|CONTAINS SQL|NO|USER-DEFINED",
            "EMPLOYEE|GREETING|GREETING_2|NO|YES|NO|M3|SQL|NULL|CONTAINS SQL|NO|CHARACTER VARYING",
            "EMPLOYEE|MAKE|MAKE_1|YES|NO|NO|M4|SQL|NULL|CONTAINS SQL|NO|USER-DEFINED",
        ]
    );
    assert_eq!(picked(&state(), "METHOD_SPECIFICATIONS", &columns, "PERSON").len(), 6, "NAME's, BOSS's, GREETING, RENAMED");
    let parameters = ["SPECIFIC_NAME", "ORDINAL_POSITION", "DTD_IDENTIFIER", "PARAMETER_MODE", "IS_RESULT", "PARAMETER_NAME", "DATA_TYPE"];
    assert_eq!(picked(&state(), "METHOD_SPECIFICATION_PARAMETERS", &parameters, "MAKE_1"), ["MAKE_1|1|1|IN|NO|N|INTEGER"]);
    assert_eq!(
        picked(&state(), "METHOD_SPECIFICATION_PARAMETERS", &parameters, "EMPLOYEE_PAY_MUTATOR"),
        ["EMPLOYEE_PAY_MUTATOR|1|1|IN|NO|ATTR|INTEGER"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 GR4, GR5, §12.3] UDT_PRIVILEGES
/// shows TYPE USAGE and UNDER on each structured type, the owner's of
/// "_SYSTEM", grantable, and each granted; TRANSFORMS each function of a
/// transform group the catalog holds [§11.67].
#[test]
fn type_privileges_and_transforms_are_described() {
    let columns = ["GRANTOR", "GRANTEE", "UDT_NAME", "PRIVILEGE_TYPE", "IS_GRANTABLE"];
    let shown = picked(&state(), "UDT_PRIVILEGES", &columns, "");
    let person: Vec<&String> = shown.iter().filter(|r| r.contains("|PERSON|")).collect();
    assert_eq!(person, ["_SYSTEM|S|PERSON|TYPE USAGE|YES", "S|R|PERSON|TYPE USAGE|NO", "_SYSTEM|S|PERSON|UNDER|YES"]);
    assert!(shown.contains(&"_SYSTEM|S|EMPLOYEE|UNDER|YES".to_owned()), "S holds UNDER on PERSON grantably: {shown:?}");
    let transforms = ["UDT_NAME", "SPECIFIC_NAME", "GROUP_NAME", "TRANSFORM_TYPE"];
    assert_eq!(picked(&state(), "TRANSFORMS", &transforms, ""), ["POINT|T_OUT|G|FROM SQL", "POINT|T_IN|G|TO SQL"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.40 Description 4, 5, 16, 17;
/// ISO/IEC 9075-2:2023 §11.51 GR3c, §11.52 GR5, GR6] ROUTINES shows each
/// observer and mutator an INSTANCE METHOD of its type, the constructor
/// function a FUNCTION of none, an SQL-invoked method's body of its kind
/// and type, and a cast function's cast, AS ASSIGNMENT. MC/DC of
/// IS_USER_DEFINED_CAST: a function of a cast, a function of none.
#[test]
fn routines_of_structured_types_are_described() {
    let columns = [
        "ROUTINE_NAME",
        "SPECIFIC_NAME",
        "ROUTINE_TYPE",
        "UDT_NAME",
        "DATA_TYPE",
        "IS_DETERMINISTIC",
        "IS_NULL_CALL",
        "IS_USER_DEFINED_CAST",
        "IS_IMPLICITLY_INVOCABLE",
    ];
    let st = state();
    assert_eq!(
        picked(&st, "ROUTINES", &columns, "PAY"),
        [
            "PAY|EMPLOYEE_PAY_OBSERVER|INSTANCE METHOD|EMPLOYEE|INTEGER|YES|YES|NO|NULL",
            "PAY|EMPLOYEE_PAY_MUTATOR|INSTANCE METHOD|EMPLOYEE|USER-DEFINED|YES|NO|NO|NULL",
        ]
    );
    assert_eq!(picked(&st, "ROUTINES", &columns, "PERSON"), ["PERSON|PERSON_CONSTRUCTOR|FUNCTION|NULL|USER-DEFINED|NO|NO|NO|NULL"]);
    assert_eq!(picked(&st, "ROUTINES", &columns, "RENAMED"), ["RENAMED|RENAMED_1|INSTANCE METHOD|PERSON|USER-DEFINED|NO|NO|NO|NULL"]);
    let cast = picked(&st, "ROUTINES", &columns, "MAKE_POINT");
    assert!(cast.len() == 1 && cast[0].ends_with("|FUNCTION|NULL|INTEGER|NO|NO|YES|YES"), "{cast:?}");
    let parameters = ["SPECIFIC_NAME", "ORDINAL_POSITION", "IS_RESULT", "PARAMETER_NAME", "DATA_TYPE"];
    assert_eq!(
        picked(&st, "PARAMETERS", &parameters, "RENAMED_1"),
        ["RENAMED_1|1|YES|SELF|USER-DEFINED", "RENAMED_1|2|NO|N|CHARACTER VARYING"]
    );
    assert_eq!(
        picked(&st, "PARAMETERS", &parameters, "PERSON_NAME_MUTATOR"),
        ["PERSON_NAME_MUTATOR|1|YES|SELF|USER-DEFINED", "PERSON_NAME_MUTATOR|2|NO|ATTR|CHARACTER VARYING"]
    );
    let privileges = ["SPECIFIC_NAME", "GRANTOR", "GRANTEE", "PRIVILEGE_TYPE", "IS_GRANTABLE"];
    assert_eq!(picked(&st, "ROUTINE_PRIVILEGES", &privileges, "POINT_X_OBSERVER"), ["POINT_X_OBSERVER|_SYSTEM|S|EXECUTE|YES"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.20, §6.22, §6.50 Description
/// 3, 5, 6, 8] TABLES shows a typed table's self-referencing column, how
/// its values are made and its type; COLUMNS which column is
/// self-referencing; DIRECT_SUPERTABLES a subtable's direct supertable.
#[test]
fn typed_tables_are_described() {
    let st = state();
    let columns = ["TABLE_NAME", "SELF_REFERENCING_COLUMN_NAME", "REFERENCE_GENERATION", "USER_DEFINED_TYPE_NAME", "IS_TYPED"];
    let mut shown: Vec<String> = ["PEOPLE", "STAFF"].iter().flat_map(|t| picked(&st, "TABLES", &columns, t)).collect();
    shown.sort();
    assert_eq!(shown, ["PEOPLE|ID|SYSTEM GENERATED|PERSON|YES", "STAFF|ID|DERIVED|EMPLOYEE|YES"]);
    assert_eq!(picked(&st, "TABLES", &columns, "TABLES"), ["TABLES|NULL|NULL|NULL|NO"], "the Information Schema's view");
    let self_referencing = ["TABLE_NAME", "COLUMN_NAME", "IS_SELF_REFERENCING"];
    assert_eq!(picked(&st, "COLUMNS", &self_referencing, "PEOPLE"), ["PEOPLE|ID|YES", "PEOPLE|NAME|NO", "PEOPLE|BOSS|NO"]);
    assert_eq!(picked(&st, "DIRECT_SUPERTABLES", &["TABLE_NAME", "SUPERTABLE_NAME"], ""), ["STAFF|PEOPLE"]);
    let scope = ["TABLE_OR_DOMAIN_NAME", "COLUMN_NAME", "DATA_TYPE", "USER_DEFINED_TYPE_NAME", "SCOPE_SCHEMA", "SCOPE_NAME"];
    let described = picked(&st, "DATA_TYPE_DESCRIPTOR", &scope, "PEOPLE");
    assert!(described.contains(&"PEOPLE|BOSS|REF|PERSON|S|PEOPLE".to_owned()), "{described:?}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 GR7, §12.2 GR6] TABLE_METHOD_PRIVILEGES
/// shows, of each SELECT on a typed table, one on each method of its type
/// and its supertypes, of the same grantor, grantee and grantability.
#[test]
fn table_method_privileges_are_described() {
    let columns = ["GRANTOR", "GRANTEE", "TABLE_NAME", "SPECIFIC_NAME", "IS_GRANTABLE"];
    let shown = picked(&state(), "TABLE_METHOD_PRIVILEGES", &columns, "");
    assert_eq!(shown.len(), 22, "PEOPLE's 6 methods to S and R, STAFF's 10 to S: {shown:?}");
    for row in ["_SYSTEM|S|PEOPLE|RENAMED_1|YES", "S|R|PEOPLE|GREETING_1|NO", "_SYSTEM|S|STAFF|PERSON_NAME_OBSERVER|YES"] {
        assert!(shown.contains(&row.to_owned()), "{row}: {shown:?}");
    }
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.32 Description 1, 2]
/// REFERENCED_TYPES shows the referenced type of each reference type: of a
/// column's, of an attribute's, of a method specification's result type
/// and of a routine's parameter's.
#[test]
fn referenced_types_are_described() {
    let columns = ["OBJECT_NAME", "OBJECT_TYPE", "REFERENCE_TYPE_IDENTIFIER", "DTD_IDENTIFIER", "DATA_TYPE", "USER_DEFINED_TYPE_NAME"];
    let shown = picked(&state(), "REFERENCED_TYPES", &columns, "");
    for row in [
        "PEOPLE|TABLE|3|3.1|USER-DEFINED|PERSON",
        "PERSON|USER-DEFINED TYPE|2|2.1|USER-DEFINED|PERSON",
        "PERSON|USER-DEFINED TYPE|M3|M3.1|USER-DEFINED|PERSON",
        "PERSON_BOSS_MUTATOR|USER-DEFINED TYPE|1|1.1|USER-DEFINED|PERSON",
        "PERSON_BOSS_MUTATOR|ROUTINE|2|2.1|USER-DEFINED|PERSON",
    ] {
        assert!(shown.contains(&row.to_owned()), "{row}: {shown:?}");
    }
}
