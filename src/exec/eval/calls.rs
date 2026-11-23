//! Argument evaluation has the same order, NULLs and first error for
//! stack-backed unary/binary lists and longer heap-backed lists.

use crate::functions::ScalarRef;
use std::sync::{Arc, Mutex};

use super::*;
use crate::functions::ScalarFunction;
use crate::value::lob::fixtures::Temp;
use crate::value::lob::{CHUNK_OCTETS, Lob, TempLobs};
use crate::value::{DataType, Decimal, Spelled};

struct Function {
    value: Option<i128>,
    seen: Arc<Mutex<Vec<i128>>>,
}

impl ScalarFunction for Function {
    fn name(&self) -> &'static str {
        "EVALUATED_ARGUMENTS"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer })
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(value) = self.value else { return Ok(Value::Row(Arc::from(args))) };
        self.seen.lock().map_err(|_| broken("the test argument log is poisoned"))?.push(value);
        if value < 0 {
            return fail(Code::Data, "[DESIGN] an argument failed");
        }
        Ok(Value::Exact(Decimal::new(value, 0)))
    }
}

/// Verifies: [DESIGN] scalar argument storage preserves zero, one, two
/// and longer argument lists, including NULL and nested calls. A failing
/// argument stops evaluation before every later argument and before the
/// enclosing call, regardless of which storage path its arity uses.
#[test]
fn scalar_argument_storage_preserves_order_nulls_and_first_error() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let function = |value| ScalarRef(Arc::new(Function { value, seen: Arc::clone(&seen) }));
    let argument = |n| Expr::Call(function(Some(n)), Vec::new());
    for count in 0..=5 {
        let args = (0..count).map(argument).collect::<Vec<_>>();
        let expr = Expr::Call(function(None), args.clone());
        let expected = (0..count).collect::<Vec<_>>();
        let Value::Row(result) = eval(&expr, &Vec::new(), &NoSubqueries).unwrap() else { panic!("not a collected argument list") };
        assert_eq!(
            result.iter().map(ToString::to_string).collect::<Vec<_>>(),
            expected.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
        assert_eq!(*seen.lock().unwrap(), expected);
        seen.lock().unwrap().clear();
        for failed in 0..args.len() {
            let mut args = args.clone();
            args[failed] = argument(-1);
            let failing = Expr::Call(function(None), args);
            assert_eq!(eval(&failing, &Vec::new(), &NoSubqueries).unwrap_err().code, Code::Data);
            let reached = (0..i128::try_from(failed).unwrap()).chain([-1]).collect::<Vec<_>>();
            assert_eq!(*seen.lock().unwrap(), reached);
            seen.lock().unwrap().clear();
        }
        let nulls = Expr::Call(function(None), vec![Expr::Const(Value::Null); usize::try_from(count).unwrap()]);
        let Value::Row(nulled) = eval(&nulls, &Vec::new(), &NoSubqueries).unwrap() else { panic!("not a collected NULL list") };
        assert_eq!(nulled.len(), usize::try_from(count).unwrap());
        assert!(nulled.iter().all(Value::is_null));
    }
}

struct Borrowed;

impl ScalarFunction for Borrowed {
    fn name(&self) -> &'static str {
        "BORROWED"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    fn call(&self, _: &[Value]) -> Result<Value> {
        fail(Code::Api, "[DESIGN] this test requires borrowed dispatch")
    }

    fn call_borrowed(&self, args: &[&Value], _temp: Option<&dyn TempLobs>) -> Result<Value> {
        for arg in args {
            let Value::Row(values) = arg else { return fail(Code::Api, "[DESIGN] not the shared argument") };
            if Arc::strong_count(values) != 1 {
                return fail(Code::Api, "[DESIGN] the evaluator cloned a shared argument");
            }
        }
        Ok(Value::Bool(true))
    }
}

/// Verifies: [DESIGN] direct unary and binary scalar arguments borrow
/// columns and constants, including repeated references, without cloning
/// their shared storage. Missing columns retain the ordinary API error.
#[test]
fn direct_scalar_arguments_borrow_shared_values() {
    let function = ScalarRef(Arc::new(Borrowed));
    let row = vec![Value::Row(Arc::from([Value::Null]))];
    for args in [vec![Expr::Column(0)], vec![Expr::Column(0), Expr::Column(0)]] {
        assert!(matches!(eval(&Expr::Call(function.clone(), args), &row, &NoSubqueries).unwrap(), Value::Bool(true)));
    }
    for count in [1_usize, 2] {
        let args = (0..count).map(|_| Expr::Const(Value::Row(Arc::from([Value::Null])))).collect();
        assert!(matches!(eval(&Expr::Call(function.clone(), args), &row, &NoSubqueries).unwrap(), Value::Bool(true)));
    }
    for args in [vec![Expr::Column(1)], vec![Expr::Column(0), Expr::Column(1)], vec![Expr::Column(1), Expr::Column(0)]] {
        assert_eq!(eval(&Expr::Call(function.clone(), args), &row, &NoSubqueries).unwrap_err().code, Code::Api);
    }
}

struct WithTemp(Temp);

impl Subqueries for WithTemp {
    fn rows(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<SubqueryRows> {
        NoSubqueries.rows(subquery, outer, limit)
    }

    fn temp_lobs(&self) -> Option<&dyn TempLobs> {
        Some(&self.0)
    }
}

/// Verifies: [DESIGN] D79: borrowed and evaluated scalar arguments keep
/// the statement's temporary large-object store. UPPER of more than one
/// chunk succeeds and writes there; dispatch through `call` would fail.
#[test]
fn borrowed_scalar_preserves_temporary_large_objects() {
    let function = crate::functions::Registry::with_set_functions().scalar("UPPER").unwrap();
    let length = CHUNK_OCTETS.saturating_add(1);
    let row = vec![Value::Lob(Lob::of_text(&"a".repeat(length)))];
    let nested = Expr::Call(function.clone(), vec![Expr::Column(0)]);
    for arg in [Expr::Column(0), Expr::Const(row[0].clone()), nested] {
        let context = WithTemp(Temp::default());
        let result = eval(&Expr::Call(function.clone(), vec![arg]), &row, &context).unwrap();
        let Value::Lob(result) = result else { panic!("UPPER did not return a large object") };
        assert_eq!(result.reader().read(0, u64::try_from(length).unwrap()).unwrap(), vec![b'A'; length]);
        assert!(context.0.made() > 0);
    }
}

/// Verifies: [DESIGN] RULES.md D910. An operand that is a slot is read
/// where the statement holds it only if its value is one of the slot's
/// type; any other is cast to that type as the slot is read [ISO/IEC
/// 9075:1992 §17.9 GR5f], in an operator, a comparison and a call alike,
/// and an outer reference is read as it is held.
#[test]
fn an_operand_slot_not_of_its_type_is_cast() {
    use crate::exec::Framed;
    use crate::logical::{Cond, Slot};
    use crate::value::{ArithOp, CompOp};
    let tenths = DataType::Exact { precision: 4, scale: 1, spelled: Spelled::Decimal };
    let slot = |index| Expr::Dynamic(Slot { index, data_type: tenths.clone(), at: 0 });
    let exact = |units, scale| Value::Exact(Decimal::new(units, scale));
    // Slot 0 is to be cast, slot 1 is of its type, slot 2 has no value of it.
    let frame = [exact(5, 0), exact(25, 1), Value::Char("x".to_owned())];
    let outer = [exact(7, 0)];
    let framed = Framed { inner: &NoSubqueries, frame: &frame, outer: &outer };
    let none = Vec::new();
    let sum = |a, b| eval(&Expr::Arith(Box::new(a), ArithOp::Add, Box::new(b)), &none, &framed);
    assert_eq!(sum(slot(0), Expr::Const(exact(1, 0))).unwrap().to_string(), "6.0");
    assert_eq!(sum(slot(1), Expr::Outer(0, "O".to_owned())).unwrap().to_string(), "9.5");
    assert_eq!(sum(slot(2), slot(1)).unwrap_err().sqlstate().as_str(), "22018");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let collect = ScalarRef(Arc::new(Function { value: None, seen }));
    let Value::Row(args) = eval(&Expr::Call(collect, vec![slot(0), slot(1)]), &none, &framed).unwrap() else { panic!("no row") };
    assert_eq!(args.iter().map(ToString::to_string).collect::<Vec<_>>(), ["5.0", "2.5"]);
    let less = Cond::Compare(slot(2), CompOp::Lt, slot(1));
    assert_eq!(eval_cond(&less, &none, &framed).unwrap_err().sqlstate().as_str(), "22018");
    let equal = Cond::Compare(slot(0), CompOp::Eq, Expr::Const(exact(50, 1)));
    assert_eq!(eval_cond(&equal, &none, &framed).unwrap(), Truth::True);
}
