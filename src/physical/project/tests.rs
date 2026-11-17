//! Column composition preserves values, multiplicity and evaluation errors.

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::error::{Code, Result};
use crate::exec::execute;
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;
use crate::value::{ArithOp, Decimal, Value};
use std::sync::Arc;

fn selected(exprs: Vec<Expr>, input: Plan) -> Plan {
    Plan::Project { exprs, detail: String::new(), input: Box::new(input) }
}

fn fields(columns: &[usize]) -> Vec<Expr> {
    columns.iter().copied().map(Expr::Column).collect()
}

fn values() -> Plan {
    let row = vec![Value::Null, Value::Char("left".into()), Value::Char("right".into())];
    Plan::Values { rows: vec![row.into_iter().map(Expr::Const).collect()], detail: String::new() }
}

fn rows(plan: Plan) -> Result<Vec<Row>> {
    let state = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
    execute(plan, &state, NO_TRANSACTION).collect()
}

/// Verifies: [DESIGN] D31: combining several column selections preserves
/// order, repeated outputs, NULLs and empty projections, and removes each
/// intermediate selection before execution.
#[test]
fn column_composition_preserves_values_and_multiplicity() {
    let intermediate = ["right", "left", "right", "NULL", "right"];
    for output in [vec![], vec![0, 1, 2, 3, 4], vec![4, 3, 4, 1, 0], vec![3, 3]] {
        let input = selected(fields(&[3, 2, 0, 1, 3]), selected(fields(&[2, 0, 1, 2]), values()));
        let (exprs, input) = collapsed(fields(&output), input);
        assert!(matches!(input, Plan::Values { .. }));
        let actual = rows(selected(exprs, input)).unwrap();
        assert_eq!(actual.len(), 1);
        let expected: Vec<_> = output.iter().map(|&at| intermediate[at]).collect();
        assert_eq!(actual[0].iter().map(ToString::to_string).collect::<Vec<_>>(), expected);
    }
    let (exprs, input) = collapsed(Vec::new(), selected(Vec::new(), values()));
    assert!(matches!(input, Plan::Values { .. }));
    assert_eq!(rows(selected(exprs, input)).unwrap()[0].len(), 0);
}

/// Verifies: [DESIGN] D15, D31: a column selection cannot remove an unused
/// computed expression. Its division-by-zero error still reaches the
/// reader, while a computed outer projection is left to its evaluator.
#[test]
fn computed_expressions_remain_evaluated() {
    let number = |n| Box::new(Expr::Const(Value::Exact(Decimal::new(n, 0))));
    let failure = Expr::Arith(number(1), ArithOp::Div, number(0));
    for output in [vec![], vec![Expr::Column(0)]] {
        let input = selected(vec![Expr::Column(0), failure.clone()], values());
        let (exprs, input) = collapsed(output, input);
        assert!(matches!(input, Plan::Project { .. }));
        assert_eq!(rows(selected(exprs, input)).unwrap_err().code, Code::Data);
    }
    let (exprs, input) = collapsed(vec![Expr::Const(Value::Null)], selected(fields(&[2, 0]), values()));
    assert!(matches!(input, Plan::Project { .. }));
    assert_eq!(rows(selected(exprs, input)).unwrap()[0][0].to_string(), "NULL");
}

/// Verifies: [DESIGN] D31: an invalid reference in a column selection is
/// retained for the executor to report SQLCODE -910, rather than indexing
/// the mapping or silently removing the missing output.
#[test]
fn missing_columns_remain_errors() {
    for through in [vec![], vec![0, 1]] {
        let (exprs, input) = collapsed(fields(&[2]), selected(fields(&through), values()));
        assert!(matches!(input, Plan::Project { .. }));
        assert_eq!(rows(selected(exprs, input)).unwrap_err().code, Code::Api);
    }
}
