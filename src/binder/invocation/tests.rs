//! Unit tests of the binding of an invocation of an SQL-invoked function:
//! its subject routine, the refusals of what contains it, and its body,
//! bound over the row of its parameters (RULES.md R-TEST-1).

use std::collections::BTreeSet;

use super::body::{bind_body, chain_of};
use super::*;
use crate::ast::{CallStatement, ReturnStatement, RoutineDefinition, SqlStatement, TableName, TypeName};
use crate::binder::{Context, Invoked};
use crate::catalog::{Catalog, ColumnDesc, Horizon, MemoryCatalog, RoutineKind, SqlParameter, TableDesc, UsageDesc};
use crate::catalog::{ParameterMode, QualifiedName};
use crate::db::State;
use crate::error::{Code, SqlError};
use crate::logical::LogicalPlan;
use crate::params::Params;
use crate::parser::Parser;
use crate::value::{CharUnits, INTEGER_PRECISION, Spelled};

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }
}

/// The type `sql` spells: INTEGER, BOOLEAN or VARCHAR(10).
fn ty(sql: &str) -> DataType {
    match sql {
        "INTEGER" => integer(),
        "BOOLEAN" => DataType::Boolean,
        "VARCHAR(10)" => DataType::Varying { length: 10, units: CharUnits::Characters },
        other => panic!("no type {other} in these tests"),
    }
}

fn qn(auth: &str, ident: &str) -> QualifiedName {
    QualifiedName { auth: auth.into(), ident: ident.into() }
}

/// A column `name` of INTEGER.
fn column(name: &str) -> ColumnDesc {
    ColumnDesc { name: name.into(), data_type: integer(), not_null: false, default: None, domain: None, collation: None, generated: None }
}

/// Function S.`name` of `params`, each a name and a type's spelling,
/// returning INTEGER, deterministic and of CONTAINS SQL, its body RETURN
/// `value`; EXECUTE granted to no one.
fn function(name: &str, params: &[(&str, &str)], value: &str) -> SqlRoutineDesc {
    let parameters = params
        .iter()
        .map(|(n, t)| SqlParameter { name: (*n).into(), data_type: ty(t), mode: ParameterMode::In, generic: None, default: None })
        .collect();
    let declared: Vec<String> = params.iter().map(|(n, t)| format!("{n} {t}")).collect();
    SqlRoutineDesc {
        method: None,
        name: qn("S", name),
        specific: qn("S", name),
        kind: RoutineKind::Function,
        parameters,
        returns: Some(integer()),
        deterministic: true,
        access: DataAccess::Contains,
        null_call: false,
        text: format!("CREATE FUNCTION S.{name} ({}) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN {value}", declared.join(", ")),
        module: None,
        ptf: None,
        distance: None,
        body: format!("RETURN {value}"),
        schema: "S".into(),
        reads: Vec::new(),
        horizon: Horizon::default(),
        sequences: Vec::new(),
        execute: Vec::new(),
        created: 1,
        stamps: crate::catalog::Stamps::default(),
    }
}

/// A database of schema S, owned by S, of table T (K, P), and `routines`.
fn database(routines: Vec<SqlRoutineDesc>) -> State {
    let mut catalog = MemoryCatalog::default();
    let table = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: qn("S", "T"),
        columns: vec![column("K"), column("P")],
        constraints: Vec::new(),
        engine: "ROW".into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    catalog.create_schema("S", "S", vec![table]);
    for r in routines {
        catalog.put_routine(r);
    }
    State::with_builtins(Box::new(catalog))
}

/// The `<value expression>` `sql`.
fn expr(sql: &str) -> ValueExpression {
    Parser::for_stored(sql).unwrap().generation_expression().unwrap()
}

/// The definition of `desc` whose body is `statement`.
fn defined(desc: &SqlRoutineDesc, statement: SqlStatement) -> RoutineDefinition {
    RoutineDefinition {
        kind: desc.kind,
        name: TableName { auth: Some(desc.name.auth.clone()), ident: desc.name.ident.clone(), at: 0 },
        parameters: Vec::new(),
        returns: desc.returns.clone().map(TypeName::Data),
        specific: None,
        deterministic: desc.deterministic,
        access: desc.access,
        null_call: desc.null_call,
        new_savepoint_level: false,
        body: crate::ast::RoutineBody::Sql(Box::new(statement)),
        text: desc.text.clone(),
        body_text: desc.body.clone(),
        distance: None,
    }
}

/// The definition of `desc` whose body is RETURN `value`, or RETURN NULL.
fn returning(desc: &SqlRoutineDesc, value: Option<&str>) -> RoutineDefinition {
    defined(desc, SqlStatement::Return(ReturnStatement { value: value.map(expr), at: 0 }))
}

/// The body of `desc`, `definition`'s, bound in `st` as S's statement.
fn body_of(st: &State, desc: &SqlRoutineDesc, definition: &RoutineDefinition) -> Result<FunctionBody> {
    let params = Params::new();
    bind_function_body(Context::new(st, "S", &params), desc, definition)
}

/// The function an invocation invokes, bound, if one of its schema.
fn done(called: Called) -> Option<(Expr, DataType)> {
    match called {
        Called::Done(invoked) => Some(invoked),
        Called::Pass(_) => None,
    }
}

/// The invocation `sql`, a `<routine invocation>`, bound in `st` under
/// `auth`, for `purpose`.
fn invoked(st: &State, (auth, purpose): (&str, Purpose), sql: &str) -> Result<Option<(Expr, DataType)>> {
    let params = Params::new();
    let cx = Context { purpose, schema: "S", ..Context::new(st, auth, &params) };
    let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    let (schema, name, args) = call(sql);
    binder.invocation((schema.as_deref(), &name), &args, Scope::Rows("no set function")).and_then(passed_on)
}

/// `done`, a refusal an invocation passes on as its error.
fn passed_on(called: Called) -> Result<Option<(Expr, DataType)>> {
    match called {
        Called::Pass(Passed { refusal: Some(e), .. }) => Err(e),
        other @ (Called::Done(_) | Called::Pass(_)) => Ok(done(other)),
    }
}

/// The `<routine invocation>` `sql`: its schema, name and arguments.
fn call(sql: &str) -> (Option<String>, String, Vec<ValueExpression>) {
    let ValueExpression::Call { schema, name, args, .. } = expr(sql) else { panic!("no call: {sql}") };
    (schema, name, args)
}

/// The error of `r`, which failed.
fn failure<T>(r: Result<T>) -> SqlError {
    r.err().expect("expected a failure")
}

/// The outer expressions of the body of `b`.
fn outer(b: &FunctionBody) -> Vec<Expr> {
    b.body.as_ref().expect("a body").outer.clone()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.6 SR8, §16.2] a parameter
/// referenced by its name alone is an outer reference of the body, filled
/// by the column of its ordinal position in the row of the parameters;
/// the body reads nothing, of CONTAINS SQL.
#[test]
fn parameter_is_an_outer_reference_of_its_position() {
    let f = function("F", &[("A", "INTEGER"), ("B", "INTEGER")], "B + 1");
    let st = database(vec![]);
    let b = body_of(&st, &f, &returning(&f, Some("B + 1"))).unwrap();
    assert!(matches!(outer(&b).as_slice(), [Expr::Column(1)]));
    assert_eq!(b.access, DataAccess::Contains);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.6 SR8] a parameter is
/// referenced too as the routine's name and its own, F.A.
#[test]
fn parameter_is_referenced_qualified_by_the_routine_name() {
    let f = function("F", &[("A", "INTEGER"), ("B", "INTEGER")], "F.A");
    let st = database(vec![]);
    let b = body_of(&st, &f, &returning(&f, Some("F.A"))).unwrap();
    assert!(matches!(outer(&b).as_slice(), [Expr::Column(0)]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.6 SR4, SR8] a column of a
/// table of the body's own FROM clause hides a parameter of its name, the
/// most local scope first; a parameter no column hides is still an outer
/// reference, through the subquery; a body reading a table possibly reads
/// SQL-data.
#[test]
fn column_of_the_body_hides_a_parameter() {
    let hidden = "(SELECT MAX(P) FROM S.T)";
    let f = function("F", &[("P", "INTEGER")], hidden);
    let st = database(vec![]);
    let of_column = body_of(&st, &f, &returning(&f, Some(hidden))).unwrap();
    assert!(matches!(outer(&of_column).as_slice(), []), "P is T's column");
    assert_eq!(of_column.access, DataAccess::Reads);
    let seen = "(SELECT COUNT(*) FROM S.T WHERE K = A)";
    let g = function("G", &[("A", "INTEGER")], seen);
    let of_parameter = body_of(&st, &g, &returning(&g, Some(seen))).unwrap();
    assert!(matches!(outer(&of_parameter).as_slice(), [Expr::Column(0)]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §16.2 SR2] RETURN NULL is a
/// null of the result type, CAST (NULL AS RDT).
#[test]
fn return_null_is_of_the_result_type() {
    let f = function("F", &[], "NULL");
    let st = database(vec![]);
    let b = body_of(&st, &f, &returning(&f, None)).unwrap();
    let LogicalPlan::Values { types, .. } = &b.body.unwrap().plan else { panic!("a VALUES") };
    assert_eq!(types, &vec![integer()]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §16.2 SR4] a returned value
/// not assignable to the result type is refused (SQLCODE -103); one of
/// another numeric type is not.
#[test]
fn return_value_not_assignable_is_refused() {
    let f = function("F", &[], "TRUE");
    let st = database(vec![]);
    let e = failure(body_of(&st, &f, &returning(&f, Some("TRUE"))));
    assert_eq!((e.code, e.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §16.2 SR4")));
    assert!(body_of(&st, &f, &returning(&f, Some("1.5"))).is_ok());
}

/// Verifies: [DESIGN] a body that is no `<return statement>` is bound as
/// none, which raises its exception as the function is invoked.
#[test]
fn body_of_no_return_statement_is_none() {
    let f = function("F", &[], "NULL");
    let st = database(vec![]);
    let call = SqlStatement::Call(CallStatement {
        name: TableName { auth: None, ident: "P".into(), at: 0 },
        arguments: Vec::new(),
        names: Vec::new(),
        dynamic: Vec::new(),
    });
    let b = body_of(&st, &f, &defined(&f, call)).unwrap();
    assert!(b.body.is_none());
    assert_eq!(b.access, DataAccess::Contains);
}

/// Verifies: [DESIGN] the functions whose bodies are being bound are what
/// a list of invoked routines holds before its entry of no schema; a
/// statement's own list holds none; a function in that chain is refused
/// again (SQLCODE -103).
#[test]
fn chain_of_bodies_being_bound() {
    let list = Invoked::new(vec![qn("S", "F"), qn("", ""), qn("S", "G")]);
    assert_eq!(chain_of(Some(&list)), vec![qn("S", "F")]);
    let own = Invoked::new(vec![qn("S", "G")]);
    assert_eq!(chain_of(Some(&own)), Vec::new());
    assert_eq!(chain_of(None), Vec::new());
    let f = function("F", &[], "1");
    let st = database(vec![]);
    let params = Params::new();
    let cx = Context::new(&st, "S", &params);
    let e = failure(bind_body(cx, (&f, &returning(&f, Some("1"))), vec![qn("S", "F")]));
    assert_eq!(e.code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR5f] of a database of
/// no routine, or of none of the name, an invocation is none of a routine
/// of an SQL routine body: a distinct type's or a registered function is
/// tried next.
#[test]
fn no_routine_of_the_name_is_none() {
    let none = database(vec![]);
    assert!(!none.catalog.any_routines());
    assert!(invoked(&none, ("S", Purpose::Query), "S.F(1)").unwrap().is_none());
    let other = database(vec![function("G", &[], "1")]);
    assert!(invoked(&other, ("S", Purpose::Query), "S.F(1)").unwrap().is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10a] an invocation of
/// more or fewer arguments than the function has parameters is refused
/// (SQLCODE -103).
#[test]
fn arity_mismatch_is_refused() {
    let st = database(vec![function("F", &[("P", "INTEGER")], "P")]);
    let more = failure(invoked(&st, ("S", Purpose::Query), "S.F(1, 2)"));
    assert_eq!((more.code, more.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §9.18 SR10a")));
    let fewer = failure(invoked(&st, ("S", Purpose::Query), "F()"));
    assert_eq!(fewer.code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10d] an argument not
/// assignable to its parameter's declared type is refused (SQLCODE -103).
#[test]
fn argument_not_assignable_is_refused() {
    let st = database(vec![function("F", &[("P", "INTEGER")], "P")]);
    let e = failure(invoked(&st, ("S", Purpose::Query), "S.F(TRUE)"));
    assert_eq!((e.code, e.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §9.18 SR10d")));
}

/// Procedure S.`name` of no parameters, as `function` makes a function.
fn procedure(name: &str) -> SqlRoutineDesc {
    SqlRoutineDesc {
        kind: RoutineKind::Procedure,
        returns: None,
        access: DataAccess::Reads,
        text: format!("CREATE PROCEDURE S.{name} () READS SQL DATA SELECT 1 INTO N FROM T"),
        body: "SELECT 1 INTO N FROM T".into(),
        ..function(name, &[], "1")
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR6] an authorization
/// identifier that holds no EXECUTE on the procedure is refused (SQLCODE
/// -104); the owner of its schema, one granted it and any, if PUBLIC is,
/// hold it.
#[test]
fn execute_is_required() {
    let owned = database(vec![procedure("P")]);
    let params = Params::new();
    let name = TableName { auth: Some("S".into()), ident: "P".into(), at: 0 };
    let as_of = |auth: &'static str, db: &State| resolve_procedure(Context::new(db, auth, &params), &name, &[]);
    let e = failure(as_of("R", &owned));
    assert_eq!((e.code, e.rule.as_deref()), (Code::Privilege, Some("ISO/IEC 9075-2:2023 §9.18 SR6")));
    assert!(as_of("S", &owned).is_ok());
    let mut granted = procedure("P");
    granted.execute.push(UsageDesc { grantor: "S".into(), grantee: Some("R".into()), grantable: false });
    let to_r = database(vec![granted.clone()]);
    assert!(as_of("R", &to_r).is_ok());
    assert_eq!(failure(as_of("Q", &to_r)).code, Code::Privilege);
    granted.execute = vec![UsageDesc { grantor: "S".into(), grantee: None, grantable: false }];
    let to_public = database(vec![granted]);
    assert!(as_of("Q", &to_public).is_ok());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR9b, SR9e, SR9c] a
/// name of no schema is of the statement's default schema; of another,
/// there is no procedure of the name (SQLCODE -103); a function is no
/// procedure; a CALL of another number of arguments is refused, naming
/// the numbers the procedures of the name have. The procedure found is
/// noted among those the statement invokes, and is the catalog's own,
/// shared, not a copy.
#[test]
fn unqualified_name_is_of_the_default_schema() {
    let st = database(vec![procedure("P"), function("F", &[], "1")]);
    let params = Params::new();
    let name = TableName { auth: None, ident: "P".into(), at: 0 };
    let noted = Invoked::default();
    let cx = Context { schema: "S", invoked: Some(&noted), ..Context::new(&st, "S", &params) };
    let found = resolve_procedure(cx, &name, &[]).unwrap();
    assert_eq!(found.specific, qn("S", "P"));
    assert!(Arc::ptr_eq(&found, &st.catalog.routine_shared(&qn("S", "P")).unwrap()), "shared");
    assert_eq!(*noted.borrow(), vec![qn("S", "P")]);
    let elsewhere = Context { schema: "R", ..cx };
    let absent = failure(resolve_procedure(elsewhere, &name, &[]));
    assert_eq!((absent.code, absent.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §9.18 SR9e")));
    let function = TableName { auth: None, ident: "F".into(), at: 0 };
    assert_eq!(failure(resolve_procedure(cx, &function, &[])).rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.18 SR9e"));
    let more = failure(resolve_procedure(cx, &name, &[None, None]));
    assert_eq!(more.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.18 SR9b"));
    assert!(more.msg.contains("have 0"), "{}", more.msg);
}

/// A database of function S.F, of `access`, deterministic if
/// `deterministic`.
fn of_kind(access: DataAccess, deterministic: bool) -> State {
    let f = function("F", &[], "1");
    database(vec![SqlRoutineDesc { deterministic, access, ..f }])
}

/// The rule the invocation of S.F() bound for `purpose` in `st` breaks.
fn refused(st: &State, purpose: Purpose) -> Option<String> {
    failure(invoked(st, ("S", purpose), "S.F()")).rule
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR25, §11.9 SR6,
/// §11.47 SR8, §11.4 SR10e] a function that possibly modifies SQL-data is
/// refused in a query, a constraint and a generation expression (SQLCODE
/// -103).
#[test]
fn modifying_function_is_refused_everywhere() {
    let st = of_kind(DataAccess::Modifies, true);
    assert_eq!(refused(&st, Purpose::Query).as_deref(), Some("ISO/IEC 9075-2:2023 §7.17 SR25, §11.49 SR11"));
    assert_eq!(refused(&st, Purpose::Constraint).as_deref(), Some("ISO/IEC 9075-2:2023 §11.9 SR6, §11.47 SR8"));
    assert_eq!(refused(&st, Purpose::UnderConstraint).as_deref(), Some("ISO/IEC 9075-2:2023 §11.9 SR6, §11.47 SR8"));
    assert_eq!(refused(&st, Purpose::Generation).as_deref(), Some("ISO/IEC 9075-2:2023 §11.4 SR10e, §4.35.2"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.9 CR2, CR3, §9.16 SR1am]
/// a constraint refuses a function that is not deterministic, and one
/// that possibly reads SQL-data (SQLCODE -103); a query takes both.
#[test]
fn constraint_refuses_nondeterministic_and_reading_functions() {
    let nondeterministic = of_kind(DataAccess::Contains, false);
    assert_eq!(refused(&nondeterministic, Purpose::Constraint).as_deref(), Some("ISO/IEC 9075-2:2023 §9.16 SR1am, §11.9 CR2"));
    let reading = of_kind(DataAccess::Reads, true);
    assert_eq!(refused(&reading, Purpose::Constraint).as_deref(), Some("ISO/IEC 9075-2:2023 §11.9 CR3"));
    let both = of_kind(DataAccess::Reads, false);
    let params = Params::new();
    let cx = Context { schema: "S", ..Context::new(&both, "S", &params) };
    let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    assert!(binder.permitted(both.catalog.routine(&qn("S", "F")).unwrap()).is_ok());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.4 SR10d, SR10e] a
/// generation expression refuses a function that is not deterministic,
/// and one that possibly reads SQL-data (SQLCODE -103).
#[test]
fn generation_refuses_nondeterministic_and_reading_functions() {
    let nondeterministic = of_kind(DataAccess::Contains, false);
    assert_eq!(refused(&nondeterministic, Purpose::Generation).as_deref(), Some("ISO/IEC 9075-2:2023 §11.4 SR10d, §9.16 SR1am"));
    let reading = of_kind(DataAccess::Reads, true);
    assert_eq!(refused(&reading, Purpose::Generation).as_deref(), Some("ISO/IEC 9075-2:2023 §11.4 SR10e"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10, §10.4] an
/// invocation binds its arguments, the name and type of each parameter,
/// its body and its result type, the function's deterministic and
/// null-call characteristics; it is noted among the routines the
/// statement invokes.
#[test]
fn invocation_is_bound() {
    let f = function("F", &[("P", "INTEGER")], "P + 1");
    let st = database(vec![SqlRoutineDesc { null_call: true, ..f }]);
    let params = Params::new();
    let noted = Invoked::default();
    let cx = Context { schema: "S", invoked: Some(&noted), ..Context::new(&st, "S", &params) };
    let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    let (schema, name, args) = call("F(2)");
    let (e, t) = done(binder.invocation((schema.as_deref(), &name), &args, Scope::Rows("no set function")).unwrap()).unwrap();
    assert_eq!(t, integer());
    let Expr::Invoked(i) = e else { panic!("an invocation") };
    assert_eq!((i.args.len(), i.params.clone(), i.result.clone()), (1, vec![("P".to_owned(), integer())], integer()));
    assert!(i.null_call && i.deterministic);
    assert!(matches!(i.body.as_ref().unwrap().outer.as_slice(), [Expr::Column(0)]));
    assert_eq!(*noted.borrow(), vec![qn("S", "F")]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16 GR1, §4.35.2] an
/// invocation of a function that is not deterministic is not
/// deterministic, as an expression of it is not.
#[test]
fn nondeterministic_function_makes_its_invocation_so() {
    let f = function("F", &[], "1");
    let not_so = database(vec![SqlRoutineDesc { deterministic: false, ..f.clone() }]);
    let (of_not_so, _) = invoked(&not_so, ("S", Purpose::Query), "S.F()").unwrap().unwrap();
    assert!(!super::super::dependencies::deterministic(&of_not_so));
    let so = database(vec![f]);
    let (of_so, _) = invoked(&so, ("S", Purpose::Query), "S.F()").unwrap().unwrap();
    assert!(super::super::dependencies::deterministic(&of_so));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 GR1, §4.35.2] a body
/// invoking a function that possibly reads SQL-data possibly reads it;
/// the functions it invokes are noted. A function that possibly modifies
/// SQL-data is refused in a body, as in a query (SQLCODE -103).
#[test]
fn body_access_is_that_of_the_functions_it_invokes() {
    let g = function("G", &[], "(SELECT COUNT(*) FROM S.T)");
    let reader = SqlRoutineDesc { access: DataAccess::Reads, ..g.clone() };
    let inner = function("H", &[], "1");
    let caller = function("F", &[], "S.G() + S.H()");
    let reads = database(vec![reader, inner, caller.clone()]);
    let params = Params::new();
    let noted = Invoked::default();
    let cx = Context { invoked: Some(&noted), ..Context::new(&reads, "S", &params) };
    let b = bind_function_body(cx, &caller, &returning(&caller, Some("S.G() + S.H()"))).unwrap();
    assert_eq!(b.access, DataAccess::Reads);
    assert_eq!(*noted.borrow(), vec![qn("S", "G"), qn("S", "H")]);
    let writer = SqlRoutineDesc { access: DataAccess::Modifies, ..g };
    let modifies = database(vec![writer, caller.clone()]);
    let e = failure(body_of(&modifies, &caller, &returning(&caller, Some("S.G()"))));
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §7.17 SR25, §11.49 SR11"));
}
