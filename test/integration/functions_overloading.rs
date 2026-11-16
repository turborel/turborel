//! Overloaded scalar function plug-ins `[EXTENSION]` (RULES.md D74), several
//! registered under one name, each declaring the types of its parameters,
//! the one a call invokes chosen by its arguments' types, after the
//! functions of the session's default schema.

use std::sync::{Arc, Mutex};

use turborel::db::Database;
use turborel::error::{Code, Result};
use turborel::functions::{Accumulator, AggregateFunction, ScalarFunction};
use turborel::params::ParameterType;
use turborel::settings::Configurable;
use turborel::value::{BIGINT_PRECISION, DOUBLE_PRECISION, DataType, Decimal, INTEGER_PRECISION, MAX_EXACT_PRECISION, Spelled, Value};

use crate::common::{Noted, code, function_calls, rows, setup, whole};

/// The longest result a `Tagged` gives.
const TAGGED_LENGTH: u32 = 40;

/// A scalar function of a name that returns its tag and the argument it
/// was given, so a test sees which function of the name ran and the value
/// it saw. It keeps the argument types `return_type` was given last.
struct Tagged {
    name: &'static str,
    parameters: Option<Vec<DataType>>,
    tag: &'static str,
    seen: Arc<Mutex<Vec<DataType>>>,
    noted: Option<Noted>,
}

impl Tagged {
    /// The function `name` of parameters `parameters`, if it declares them,
    /// returning `tag`.
    fn new(name: &'static str, parameters: Option<Vec<DataType>>, tag: &'static str) -> Tagged {
        Tagged { name, parameters, tag, seen: Arc::new(Mutex::new(Vec::new())), noted: None }
    }

    /// The argument types `return_type` was given last, shared with it.
    fn seen(&self) -> Arc<Mutex<Vec<DataType>>> {
        Arc::clone(&self.seen)
    }
}

impl ScalarFunction for Tagged {
    fn name(&self) -> &str {
        self.name
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        self.noted.as_ref().map(|n| n as &dyn Configurable)
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        *self.seen.lock().unwrap() = args.to_vec();
        Ok(DataType::varying(TAGGED_LENGTH))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let shown: Vec<String> = args.iter().map(ToString::to_string).collect();
        Ok(Value::Char(format!("{}:{}", self.tag, shown.join(","))))
    }
    fn parameter_types(&self) -> Option<Vec<DataType>> {
        self.parameters.clone()
    }
}

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }
}

/// BIGINT.
fn bigint() -> DataType {
    DataType::Exact { precision: BIGINT_PRECISION, scale: 0, spelled: Spelled::Bigint }
}

/// DOUBLE PRECISION.
fn double() -> DataType {
    DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision }
}

/// HALF of one INTEGER, tagged INT.
fn half_integer() -> Tagged {
    Tagged::new("HALF", Some(vec![integer()]), "INT")
}

/// HALF of one DOUBLE PRECISION, tagged DOUBLE.
fn half_double() -> Tagged {
    Tagged::new("HALF", Some(vec![double()]), "DOUBLE")
}

/// The SQLCODE class of registering `f` in `db`, which must fail.
fn refused(db: &Database, f: Tagged) -> Code {
    db.register_scalar_function(Box::new(f)).unwrap_err().code
}

/// `setup()` with HALF(INTEGER) and HALF(DOUBLE PRECISION) registered.
fn halves() -> Database {
    let db = setup();
    db.register_scalar_function(Box::new(half_integer())).unwrap();
    db.register_scalar_function(Box::new(half_double())).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D74 [ISO/IEC 9075-2:2023 §9.18 SR10, §9.6]
/// two scalar functions of one name, each declaring its parameter types,
/// both register; a call invokes the one its argument's type chooses, an
/// exact argument the INTEGER one and an approximate the DOUBLE PRECISION
/// one; an argument of a CHARACTER type, which neither takes, is refused
/// with SQLCODE -103.
#[test]
fn a_call_invokes_the_overload_its_arguments_choose() {
    let db = halves();
    let s = db.session("APP");
    function_calls(&s);
    assert_eq!(rows(&s, "SELECT HALF(4) FROM T WHERE ID = 1"), Ok(vec!["INT:4".to_owned()]));
    assert_eq!(rows(&s, "SELECT HALF(ID) FROM T WHERE ID = 2"), Ok(vec!["INT:2".to_owned()]));
    assert_eq!(rows(&s, "SELECT HALF(4E0) FROM T WHERE ID = 1"), Ok(vec!["DOUBLE:4E0".to_owned()]));
    assert_eq!(code(&s, "SELECT HALF('AB') FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT HALF(NAME) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] RULES.md D74 [ISO/IEC 9075-2:2023 §11.60 SR20b] a
/// second function of a name registers only if both declare their
/// parameter types and no call could invoke both. One declaring none
/// beside one declaring them is refused with SQLCODE -910, in either
/// order, as is one of the same types and one of BIGINT beside INTEGER,
/// exact numbers being of one rank. The first goes on being called.
#[test]
fn an_overload_not_told_apart_is_refused() {
    let db = setup();
    db.register_scalar_function(Box::new(half_integer())).unwrap();
    assert_eq!(refused(&db, Tagged::new("HALF", None, "NONE")), Code::Api, "undeclared after declared");
    assert_eq!(refused(&db, Tagged::new("HALF", Some(vec![integer()]), "SAME")), Code::Api, "the same types");
    assert_eq!(refused(&db, Tagged::new("HALF", Some(vec![bigint()]), "BIG")), Code::Api, "an exact type of one rank");
    db.register_scalar_function(Box::new(Tagged::new("PLAIN", None, "PLAIN"))).unwrap();
    assert_eq!(refused(&db, Tagged::new("PLAIN", Some(vec![integer()]), "INT")), Code::Api, "declared after undeclared");
    let s = db.session("APP");
    function_calls(&s);
    assert_eq!(rows(&s, "SELECT HALF(4), PLAIN(5) FROM T WHERE ID = 1"), Ok(vec!["INT:4,PLAIN:5".to_owned()]));
    assert_eq!(code(&s, "SELECT HALF(4E0) FROM T WHERE ID = 1"), Code::SyntaxRule, "no DOUBLE one");
}

/// Verifies: [EXTENSION] RULES.md D74 [ISO/IEC 9075-2:2023 §9.18 SR10, §9.2]
/// an argument of a function declaring its parameter types is stored in
/// its parameter's type. A SMALLINT column and an exact literal of scale 1
/// reach HALF(INTEGER) as INTEGER values, and its `return_type` is given
/// INTEGER.
#[test]
fn an_argument_is_stored_in_its_declared_type() {
    let db = setup();
    let half = half_integer();
    let seen = half.seen();
    db.register_scalar_function(Box::new(half)).unwrap();
    let s = db.session("APP");
    s.run("CREATE TABLE S (V SMALLINT)").unwrap();
    s.run("INSERT INTO S VALUES (7)").unwrap();
    function_calls(&s);
    assert_eq!(rows(&s, "SELECT HALF(V) FROM S"), Ok(vec!["INT:7".to_owned()]));
    assert_eq!(*seen.lock().unwrap(), vec![integer()], "the declared type, not SMALLINT");
    assert_eq!(rows(&s, "SELECT HALF(2.0) FROM T WHERE ID = 1"), Ok(vec!["INT:2".to_owned()]));
    assert_eq!(*seen.lock().unwrap(), vec![integer()], "the declared type, not DECIMAL(2,1)");
}

/// Verifies: [EXTENSION] RULES.md D74 [ISO/IEC 9075:1992 §17.6 GR5;
/// ISO/IEC 9075-2:2023 §9.6] a `?` argument of the one function of its
/// name takes that function's parameter type; of two overloads, which a
/// `?` cannot choose between, the statement is refused as ambiguous with
/// SQLCODE -103.
#[test]
fn a_dynamic_parameter_takes_the_one_overloads_type() {
    let db = setup();
    db.register_scalar_function(Box::new(half_integer())).unwrap();
    let s = db.session("APP");
    function_calls(&s);
    let one = s.prepare("SELECT HALF(?) FROM T WHERE ID = 1").unwrap();
    assert!(matches!(one.parameters(), [ParameterType::Of(DataType::Exact { precision: INTEGER_PRECISION, scale: 0, .. })]));
    let mut c = s.open_prepared(&one, &[Value::Exact(Decimal::new(3, 0))]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("INT:3".to_owned()));
    drop(c);
    let two = halves();
    let t = two.session("APP");
    function_calls(&t);
    assert_eq!(t.prepare("SELECT HALF(?) FROM T WHERE ID = 1").map(|_| ()).map_err(|e| e.code), Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] RULES.md D74 [ISO/IEC 9075-2:2023 §9.18 SR10b,
/// SR10h iv] an SQL-invoked function of the default schema that takes
/// the arguments is invoked before a registered one of its name; one that
/// does not take them, of an approximate argument or of a DATE parameter
/// given a number, leaves the call to the registered function that does.
#[test]
fn a_schema_function_comes_before_a_registered_one() {
    let db = halves();
    db.register_scalar_function(Box::new(Tagged::new("F", Some(vec![integer()]), "F"))).unwrap();
    let s = db.session("APP");
    s.run("CREATE FUNCTION HALF (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 100").unwrap();
    s.run("CREATE FUNCTION F (X DATE) RETURNS INTEGER CONTAINS SQL RETURN 0").unwrap();
    s.commit().unwrap();
    function_calls(&s);
    assert_eq!(rows(&s, "SELECT HALF(4) FROM T WHERE ID = 1"), Ok(vec!["400".to_owned()]), "the schema's");
    assert_eq!(rows(&s, "SELECT HALF(4E0) FROM T WHERE ID = 1"), Ok(vec!["DOUBLE:4E0".to_owned()]), "the registered one");
    assert_eq!(rows(&s, "SELECT F(1) FROM T WHERE ID = 1"), Ok(vec!["F:1".to_owned()]), "not hidden by F(DATE)");
}

/// Verifies: [EXTENSION] RULES.md D74 [ISO/IEC 9075-2:2023 §9.18 SR6, SR10h iv]
/// an SQL-invoked function of the default schema on which the session
/// holds no EXECUTE is passed over for a registered function that takes
/// the arguments; if no registered one takes them, the call is refused
/// with the schema function's SQLCODE -104.
#[test]
fn a_function_not_executable_leaves_the_call_to_a_registered_one() {
    let db = halves();
    let a = db.session("APP");
    a.run("CREATE FUNCTION HALF (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 100").unwrap();
    a.run("CREATE FUNCTION HALF (X CHARACTER(3)) RETURNS INTEGER CONTAINS SQL RETURN 0").unwrap();
    a.run("GRANT SELECT ON T TO B").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    b.run("SET SCHEMA 'APP'").unwrap();
    function_calls(&b);
    function_calls(&a);
    assert_eq!(rows(&a, "SELECT HALF(4) FROM T WHERE ID = 1"), Ok(vec!["400".to_owned()]), "the owner's");
    assert_eq!(rows(&b, "SELECT HALF(4) FROM T WHERE ID = 1"), Ok(vec!["INT:4".to_owned()]), "no EXECUTE");
    assert_eq!(code(&b, "SELECT HALF('ABC') FROM T WHERE ID = 1"), Code::Privilege);
}

/// Verifies: [EXTENSION] RULES.md D74, D36 two overloads of a name that
/// each declare a setting under the name share it. Both register, and each
/// is told the setting's default and each change of it.
#[test]
fn overloads_share_their_settings() {
    let db = setup();
    let (first, second) = (Noted::new("HALF.DEPTH"), Noted::new("HALF.DEPTH"));
    let mut int = half_integer();
    int.noted = Some(first.clone());
    let mut dbl = half_double();
    dbl.noted = Some(second.clone());
    db.register_scalar_function(Box::new(int)).unwrap();
    db.register_scalar_function(Box::new(dbl)).unwrap();
    assert_eq!((first.told(), second.told()), (1, 1), "the default");
    db.set_setting("HALF.DEPTH", &whole(4)).unwrap();
    assert_eq!((first.told(), second.told()), (4, 4), "one setting, told to both");
}

/// An aggregate TALLY, the number of values it is given.
struct Tally;

/// The count a `Tally` has made.
struct Counted(i128);

impl AggregateFunction for Tally {
    fn name(&self) -> &'static str {
        "TALLY"
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(Counted(0))
    }
}

impl Accumulator for Counted {
    fn update(&mut self, _v: &Value) -> Result<()> {
        self.0 += 1;
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(self.0, 0)))
    }
}

/// Verifies: [EXTENSION] RULES.md D74 a call of a registered aggregate's
/// name is the aggregate's unless a function of the default schema of the
/// name has as many parameters as it has arguments: an SQL-invoked
/// function TALLY of two parameters leaves TALLY(ID) to the aggregate,
/// and is invoked by TALLY(1, 2); one of one parameter takes the name,
/// so TALLY(ID) of a DATE parameter is refused (SQLCODE -103), as which
/// it is is told before its argument is bound.
#[test]
fn a_registered_aggregate_is_hidden_only_by_a_function_of_its_arity() {
    let db = setup();
    db.register_aggregate_function(Box::new(Tally)).unwrap();
    let s = db.session("APP");
    s.run("CREATE FUNCTION TALLY (A INTEGER, B INTEGER) RETURNS INTEGER CONTAINS SQL RETURN A + B").unwrap();
    s.commit().unwrap();
    function_calls(&s);
    let counted = rows(&s, "SELECT COUNT(*) FROM T");
    assert_eq!(rows(&s, "SELECT TALLY(ID) FROM T"), counted, "the aggregate");
    assert_eq!(rows(&s, "SELECT TALLY(1, 2) FROM T WHERE ID = 1"), Ok(vec!["3".to_owned()]), "the schema's");
    s.run("CREATE FUNCTION TALLY (D DATE) RETURNS INTEGER CONTAINS SQL RETURN 0").unwrap();
    s.commit().unwrap();
    assert_eq!(code(&s, "SELECT TALLY(ID) FROM T"), Code::SyntaxRule);
}
