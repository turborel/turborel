//! Scalar and aggregate function plug-ins `[EXTENSION]`.

use turborel::db::Database;
use turborel::error::{Code, Result, fail};
use turborel::functions::{Accumulator, AggregateFunction, ScalarFunction};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, MAX_EXACT_PRECISION, Value};

use crate::common::{code, rows, setup};
use turborel::value::Spelled;

struct Twice;
impl ScalarFunction for Twice {
    fn name(&self) -> &'static str {
        "TWICE"
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        match args {
            [DataType::Exact { scale, .. }] => {
                Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: *scale, spelled: Spelled::Numeric })
            }
            _ => fail(Code::SyntaxRule, "TWICE takes one exact number"),
        }
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        Ok(match &args[0] {
            Value::Exact(d) => Value::Exact(d.checked_add(*d)?),
            _ => Value::Null,
        })
    }
}

/// Replaces the standard COUNT, to show set functions are plugins too.
struct CountTimesTen;
struct Acc(i128);
impl AggregateFunction for CountTimesTen {
    fn name(&self) -> &'static str {
        "COUNT"
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
    }
    fn accumulator(&self, _r: DataType) -> Box<dyn Accumulator> {
        Box::new(Acc(0))
    }
}
impl Accumulator for Acc {
    fn update(&mut self, _v: &Value) -> Result<()> {
        self.0 += 10;
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(self.0, 0)))
    }
}

/// `setup()` with the scalar function TWICE registered.
fn with_twice() -> Database {
    let db = setup();
    db.register_scalar_function(Box::new(Twice)).unwrap();
    db
}

/// Verifies: [EXTENSION] a function call is a syntax error unless extensions are enabled.
#[test]
fn function_calls_need_extensions_enabled() {
    let db = with_twice();
    let s = db.session("APP");
    // Off by default: <identifier>( is not SQL-86.
    assert_eq!(code(&s, "SELECT TWICE(ID) FROM T"), Code::Syntax);
}

/// Verifies: [EXTENSION] a `ScalarFunction` plug-in is called in value
/// expressions, also inside a set function and a correlated subquery.
#[test]
fn a_scalar_plug_in_is_called_in_expressions() {
    let db = with_twice();
    let mut s = db.session("APP");
    s.enable_extensions(true);
    assert_eq!(rows(&s, "SELECT TWICE(AMT) FROM T WHERE ID = 1"), Ok(vec!["21.00".into()]));
    assert_eq!(rows(&s, "SELECT SUM(TWICE(ID)) FROM T"), Ok(vec!["12".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE TWICE(U.ID) = TWICE(T.ID))"), Ok(vec!["1".into()]));
}

/// Verifies: [EXTENSION] a `ScalarFunction` plug-in refuses argument types it does not take.
#[test]
fn a_scalar_plug_in_checks_its_argument_types() {
    let db = with_twice();
    let mut s = db.session("APP");
    s.enable_extensions(true);
    assert_eq!(code(&s, "SELECT TWICE(NAME) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] calling a function that is not registered is a syntax rule error.
#[test]
fn an_unregistered_function_is_refused() {
    let db = with_twice();
    let mut s = db.session("APP");
    s.enable_extensions(true);
    assert_eq!(code(&s, "SELECT MISSING(ID) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] an `AggregateFunction` plug-in can replace a standard set function.
#[test]
fn a_set_function_can_be_replaced_by_a_plug_in() {
    let db = setup();
    db.register_aggregate_function(Box::new(CountTimesTen)).unwrap();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec!["30".into()]));
}

/// Always returns 7 but declares itself not deterministic, so constant
/// folding must leave its calls alone.
struct NonDeterministicConstant;
impl ScalarFunction for NonDeterministicConstant {
    fn name(&self) -> &'static str {
        "SEQ"
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
    }
    fn call(&self, _: &[Value]) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(7, 0)))
    }
    fn deterministic(&self) -> bool {
        false
    }
}

/// Verifies: [DESIGN] only deterministic functions are folded
#[test]
fn only_deterministic_functions_are_folded() {
    let db = setup();
    db.register_scalar_function(Box::new(NonDeterministicConstant)).unwrap();
    db.register_scalar_function(Box::new(Twice)).unwrap();
    let mut s = db.session("APP");
    s.enable_extensions(true);
    let plan = s.explain("SELECT TWICE(2), SEQ(1) FROM T", &Params::new()).unwrap();
    let physical = plan.split("-- physical:").nth(1).unwrap();
    assert!(physical.contains("Project 4, SEQ(1)"), "{plan}");
}

/// A scalar function that only has a name, for registration tests.
struct NamedScalar(&'static str);
impl ScalarFunction for NamedScalar {
    fn name(&self) -> &str {
        self.0
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
    }
    fn call(&self, _: &[Value]) -> Result<Value> {
        Ok(Value::Null)
    }
}

/// An aggregate function that only has a name, for registration tests.
struct NamedAggregate(&'static str);
impl AggregateFunction for NamedAggregate {
    fn name(&self) -> &str {
        self.0
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
    }
    fn accumulator(&self, _r: DataType) -> Box<dyn Accumulator> {
        Box::new(Acc(0))
    }
}

/// Verifies: [EXTENSION] one name cannot be both a scalar and an aggregate
/// function: the second registration is refused with SQLCODE -910, in either
/// order, and for the built-in set functions too.
///
/// Regression test: with F registered as
/// both, `SELECT F(X)` bound F as a scalar but counted it as an aggregate.
#[test]
fn a_name_is_either_scalar_or_aggregate() {
    let db = setup();
    db.register_scalar_function(Box::new(NamedScalar("F"))).unwrap();
    assert_eq!(db.register_aggregate_function(Box::new(NamedAggregate("F"))).map_err(|e| e.code), Err(Code::Api));
    db.register_aggregate_function(Box::new(NamedAggregate("G"))).unwrap();
    assert_eq!(db.register_scalar_function(Box::new(NamedScalar("G"))).map_err(|e| e.code), Err(Code::Api));
    assert_eq!(db.register_scalar_function(Box::new(NamedScalar("COUNT"))).map_err(|e| e.code), Err(Code::Api));
    let mut s = db.session("APP");
    s.enable_extensions(true);
    assert_eq!(rows(&s, "SELECT F(ID) FROM T WHERE ID = 1"), Ok(vec!["NULL".to_owned()]));
}

/// A function that breaks its contract: it declares CHARACTER(6) and returns
/// ten characters.
struct TooLong;
impl ScalarFunction for TooLong {
    fn name(&self) -> &'static str {
        "LONG"
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Character { length: 6 })
    }
    fn call(&self, _: &[Value]) -> Result<Value> {
        Ok(Value::Char("TENLETTERS".into()))
    }
}

/// Verifies: [X3.135-1986 §8.12 GR6, §7.3 GR3b]; [EXTENSION] a function whose
/// value is longer than its declared type gets a data exception, not a panic,
/// and the failed statement has no effect.
///
/// Regression test: padding the value
/// subtracted past zero and panicked.
#[test]
fn a_value_longer_than_its_declared_type_is_a_data_exception() {
    let db = setup();
    db.register_scalar_function(Box::new(TooLong)).unwrap();
    let mut s = db.session("APP");
    s.enable_extensions(true);
    let update = s.execute("UPDATE T SET NAME = LONG(NAME) WHERE ID = 1", &mut Params::new());
    assert_eq!(update.map_err(|e| e.code), Err(Code::Data));
    assert_eq!(rows(&s, "SELECT NAME FROM T WHERE ID = 1"), Ok(vec!["ANN   ".to_owned()]));
}

/// Verifies: [EXTENSION] an extension aggregate called by name takes exactly
/// one argument.
#[test]
fn extension_aggregates_take_one_argument() {
    let db = setup();
    db.register_aggregate_function(Box::new(NamedAggregate("TALLY"))).unwrap();
    let mut s = db.session("APP");
    s.enable_extensions(true);
    assert_eq!(rows(&s, "SELECT TALLY(ID) FROM T"), Ok(vec!["30".into()]));
    assert_eq!(code(&s, "SELECT TALLY(ID, ID) FROM T"), Code::SyntaxRule);
}
