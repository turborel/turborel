//! Direct batch calls preserve argument positions, row order and errors.

use std::sync::{Arc, Mutex};

use super::*;
use crate::error::{Code, Result, fail};
use crate::exec::{NoSubqueries, broken, eval_cond};
use crate::functions::ScalarFunction;
use crate::storage::Column;
use crate::value::lob::fixtures::Temp;
use crate::value::lob::{CHUNK_OCTETS, Lob, LobKind};
use crate::value::{DataType, Decimal, Spelled};

struct Function(Arc<Mutex<Vec<Vec<String>>>>);

struct Borrowed;

impl ScalarFunction for Borrowed {
    fn name(&self) -> &'static str {
        "BORROWED_BATCH"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    fn call(&self, _: &[Value]) -> Result<Value> {
        fail(Code::Api, "[DESIGN] this test requires borrowed batch dispatch")
    }

    fn call_borrowed(&self, args: &[&Value], _temp: Option<&dyn TempLobs>) -> Result<Value> {
        for arg in args {
            let Value::Row(values) = arg else { return fail(Code::Api, "[DESIGN] not the shared argument") };
            if Arc::strong_count(values) != 1 {
                return fail(Code::Api, "[DESIGN] the batch evaluator cloned a shared argument");
            }
        }
        Ok(Value::Bool(true))
    }
}

/// Verifies: [DESIGN] D31: batch scalar calls borrow constant and column
/// values, including repeated references and rows, without copying shared
/// payloads. Selected rows retain their input order.
#[test]
fn batch_scalar_arguments_borrow_shared_values() {
    let batch = Batch { columns: vec![Column::from(vec![Value::Row(Arc::from([Value::Null]))])], rows: 1 };
    let args = vec![Expr::Column(4), Expr::Const(Value::Row(Arc::from([Value::Null]))), Expr::Column(4)];
    let cond = Cond::Compare(Expr::Call(ScalarRef(Arc::new(Borrowed)), args), CompOp::Eq, Expr::Const(Value::Bool(true)));
    let (kept, error) = Call::of(&cond, &[4]).unwrap().in_order(&batch, &[0, 0], None);
    assert!(error.is_none());
    assert_eq!(kept, vec![0, 0]);
}

impl ScalarFunction for Function {
    fn name(&self) -> &'static str {
        "BATCH_CALL"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer })
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.0.lock().map_err(|_| broken("the test argument log is poisoned"))?.push(args.iter().map(ToString::to_string).collect());
        let mut sum = 0_i128;
        for (position, arg) in args.iter().enumerate() {
            let Value::Exact(value) = arg else { return Ok(Value::Null) };
            if value.units < 0 {
                return fail(Code::Data, "[DESIGN] a test argument is negative");
            }
            let weight = i128::try_from(position.saturating_add(1)).map_err(|_| broken("a test argument position is too large"))?;
            let term = value.units.checked_mul(weight).ok_or_else(|| broken("a test product overflowed"))?;
            sum = sum.checked_add(term).ok_or_else(|| broken("a test sum overflowed"))?;
        }
        Ok(number(sum))
    }

    fn deterministic(&self) -> bool {
        false
    }
}

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn by_rows(cond: &Cond, batch: &Batch, layout: &[usize], rows: &[usize]) -> Vec<usize> {
    rows.iter()
        .copied()
        .filter(|&n| {
            let mut row = vec![Value::Null; 5];
            for (at, &column) in layout.iter().enumerate() {
                row[column] = batch.value(at, n).unwrap().clone();
            }
            eval_cond(cond, &row, &NoSubqueries).unwrap() == Truth::True
        })
        .collect()
}

/// Verifies: [DESIGN] D15, D31: comparing a scalar result directly over
/// batch columns agrees with row evaluation for every comparison and
/// either operand order, zero through five arguments, repeated columns,
/// constants and NULLs. Calls occur once for each selected row in order,
/// even for a function that does not promise determinism.
#[test]
fn batch_calls_preserve_arguments_comparisons_and_selection() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let function = ScalarRef(Arc::new(Function(Arc::clone(&log))));
    let batch = Batch {
        columns: vec![Column::from(vec![number(7), Value::Null, number(4)]), Column::from(vec![number(2), number(3), number(9)])],
        rows: 3,
    };
    let layout = [4, 1];
    for count in 0..=5 {
        let args = [Expr::Column(4), Expr::Const(number(3)), Expr::Column(1), Expr::Column(4), Expr::Const(Value::Null)];
        let expr = Expr::Call(function.clone(), args[..count].to_vec());
        for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge] {
            for reverse in [false, true] {
                let constant = Expr::Const(number(7));
                let cond = if reverse { Cond::Compare(constant, op, expr.clone()) } else { Cond::Compare(expr.clone(), op, constant) };
                for rows in [&[][..], &[0, 1, 2][..], &[2, 0][..]] {
                    log.lock().unwrap().clear();
                    let (actual, error) = Call::of(&cond, &layout).unwrap().in_order(&batch, rows, None);
                    assert!(error.is_none());
                    let called = log.lock().unwrap().clone();
                    log.lock().unwrap().clear();
                    let expected = by_rows(&cond, &batch, &layout, rows);
                    assert_eq!(actual, expected);
                    assert_eq!(*log.lock().unwrap(), called);
                    assert_eq!(called.len(), rows.len());
                }
            }
        }
    }
}

/// Verifies: [DESIGN] D15, D31: a failed call retains the successful
/// prefix, reports the original error and never evaluates a later row.
/// Empty selections never call, including a failing constant argument.
#[test]
fn batch_calls_stop_at_the_first_error_after_the_successful_prefix() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let function = ScalarRef(Arc::new(Function(Arc::clone(&log))));
    for failed in 0..3 {
        let mut values = vec![number(1), number(1), number(1)];
        values[failed] = number(-1);
        let batch = Batch { columns: vec![Column::from(values)], rows: 3 };
        let call = Expr::Call(function.clone(), vec![Expr::Const(number(2)), Expr::Column(0)]);
        let cond = Cond::Compare(call, CompOp::Gt, Expr::Const(number(0)));
        log.lock().unwrap().clear();
        let (kept, error) = Call::of(&cond, &[0]).unwrap().in_order(&batch, &[0, 1, 2], None);
        assert_eq!(kept, (0..failed).collect::<Vec<_>>());
        assert_eq!(error.unwrap().code, Code::Data);
        assert_eq!(log.lock().unwrap().len(), failed.saturating_add(1));
    }
    let call = Expr::Call(function, vec![Expr::Const(number(-1))]);
    let cond = Cond::Compare(call, CompOp::Eq, Expr::Const(number(0)));
    log.lock().unwrap().clear();
    let (kept, error) = Call::of(&cond, &[]).unwrap().in_order(&Batch::empty(0), &[], None);
    assert!(kept.is_empty() && error.is_none());
    assert!(log.lock().unwrap().is_empty());
}

/// Verifies: [DESIGN] D15, D31: only a direct scalar comparison with
/// constant or present column arguments uses batch calls. Other
/// expressions retain the general evaluator and its error ordering.
#[test]
fn batch_calls_decline_arguments_that_need_expression_evaluation() {
    let function = ScalarRef(Arc::new(Function(Arc::new(Mutex::new(Vec::new())))));
    for arg in [Expr::Column(7), Expr::Neg(Box::new(Expr::Const(number(2))))] {
        let expr = Expr::Call(function.clone(), vec![arg]);
        let cond = Cond::Compare(expr, CompOp::Eq, Expr::Const(number(0)));
        assert!(Call::of(&cond, &[0]).is_none());
    }
    assert!(Call::of(&Cond::Const(Truth::True), &[]).is_none());
    let column = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(number(0)));
    assert!(Call::of(&column, &[0]).is_none());
}

/// Verifies: [DESIGN] D31, D79: a direct batch scalar comparison gives
/// UPPER the temporary store needed for a result longer than one chunk
/// and reads that result for comparison, as the row evaluator does.
#[test]
fn borrowed_batch_preserves_temporary_large_objects() {
    let function = crate::functions::Registry::with_set_functions().scalar("UPPER").unwrap();
    let length = CHUNK_OCTETS.saturating_add(1);
    let batch = Batch { columns: vec![Column::from(vec![Value::Lob(Lob::of_text(&"a".repeat(length)))])], rows: 1 };
    let call = Expr::Call(function, vec![Expr::Column(0)]);
    let constant = Expr::Const(Value::Lob(Lob::of_text(&"A".repeat(length))));
    let cond = Cond::Compare(call, CompOp::Eq, constant);
    let temp = Temp::default();
    let (kept, error) = Call::of(&cond, &[0]).unwrap().in_order(&batch, &[0], Some(&temp));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(kept, vec![0]);
    assert_eq!(temp.made(), 1);
}

/// Verifies: [DESIGN] D15, D31, D79: a large-object read error in a
/// scalar result comparison retains the successful prefix and the same
/// error as row evaluation, with either operand order. An unreadable
/// reference must not become an unknown comparison and silently disappear.
#[test]
fn borrowed_batch_reports_large_object_comparison_errors() {
    let kind = DataType::Lob { kind: LobKind::Character, length: 10, units: crate::value::CharUnits::Characters };
    let function = ScalarRef(Arc::new(crate::functions::FieldOf(kind, 0)));
    let good = Value::Lob(Lob::of_text("X"));
    let bad = Value::Lob(Lob::of_reference(LobKind::Character, 42, (1, 1)));
    let values: Vec<_> = [good.clone(), bad, good.clone()].into_iter().map(|value| Value::Row(Arc::from([value]))).collect();
    let batch = Batch { columns: vec![Column::from(values.clone())], rows: 3 };
    for reversed in [false, true] {
        let call = Expr::Call(function.clone(), vec![Expr::Column(0)]);
        let constant = Expr::Const(good.clone());
        let cond = if reversed { Cond::Compare(constant, CompOp::Eq, call) } else { Cond::Compare(call, CompOp::Eq, constant) };
        let (kept, error) = Call::of(&cond, &[0]).unwrap().in_order(&batch, &[0, 1, 2], None);
        assert_eq!(kept, vec![0]);
        assert_eq!(error.unwrap().code, Code::Api);
        assert_eq!(eval_cond(&cond, &vec![values[1].clone()], &NoSubqueries).unwrap_err().code, Code::Api);
    }
}
