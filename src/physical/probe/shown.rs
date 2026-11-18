//! \[DESIGN\] EXPLAIN displays index lookups as SQL conditions (RULES.md D34).

use crate::functions::JsonOperator;
use crate::geometry::SpatialOperation;
use crate::index::{Predicate, Probe};
use crate::logical::{Cond, Expr, and_all, render_cond};
use crate::value::{CompOp, Decimal, Value};

/// Renders the lookup with table column names so EXPLAIN shows which
/// condition the index serves. Function calls retain their argument order.
pub(in crate::physical) fn render_probe(probe: &Probe, columns: &[usize], names: &[String]) -> String {
    let compared = |at: usize, op: CompOp, value: &Value| {
        Cond::Compare(Expr::Column(columns.get(at).copied().unwrap_or_default()), op, Expr::Const(value.clone()))
    };
    let equalities = |equal: &[Value]| equal.iter().enumerate().map(|(at, v)| compared(at, CompOp::Eq, v)).collect::<Vec<_>>();
    let conds = match probe {
        Probe::All => Vec::new(),
        Probe::Columns(each) => each.iter().map(|c| compared(c.column, c.op, &c.value)).collect(),
        Probe::Predicate { predicate, value } => {
            let column = Expr::Column(columns.get(predicate.column).copied().unwrap_or_default());
            let Some(argument) = predicate.argument else { return json_call(predicate, column, value, names) };
            let value = Expr::Const(value.clone());
            let args = if argument == 0 { vec![column, value] } else { vec![value, column] };
            let call = Expr::Call(predicate.function.clone(), args);
            let condition = if predicate.function.0.spatial_operation() == Some(SpatialOperation::Intersects) {
                Cond::Compare(call, CompOp::Eq, Expr::Const(Value::Exact(Decimal::new(1, 0))))
            } else {
                Cond::Value(call)
            };
            vec![condition]
        }
        Probe::Compare { op, value } => vec![compared(0, *op, value)],
        Probe::Prefix { equal, then } => {
            let next = then.as_ref().map(|(op, value)| compared(equal.len(), *op, value));
            equalities(equal).into_iter().chain(next).collect()
        }
        Probe::Range { equal, low, high } => {
            let next = [compared(equal.len(), low.0, &low.1), compared(equal.len(), high.0, &high.1)];
            equalities(equal).into_iter().chain(next).collect()
        }
    };
    and_all(conds).map_or_else(|| "all keys, in order".to_string(), |c| render_cond(&c, names))
}

/// \[EXTENSION\] A json index's lookup of a call of an SQL/JSON operator
/// or JSON_CONTAINS shown as its condition: the call of the column and the
/// values the probe holds, its PASSING arguments' and, of JSON_VALUE, the
/// value it is compared equal to.
fn json_call(predicate: &Predicate, column: Expr, value: &Value, names: &[String]) -> String {
    let mut fields: Vec<Expr> = match value {
        Value::Row(fields) => fields.iter().cloned().map(Expr::Const).collect(),
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Lob(_)
        | Value::Json(_)
        | Value::Extension(_) => vec![Expr::Const(value.clone())],
    };
    let operator = predicate.function.0.json_probe().map(|p| p.operator);
    let compared = (operator == Some(JsonOperator::Value)).then(|| fields.pop()).flatten();
    let call = Expr::Call(predicate.function.clone(), std::iter::once(column).chain(fields).collect());
    let cond = match compared {
        Some(other) => Cond::Compare(call, CompOp::Eq, other),
        None => Cond::Value(call),
    };
    render_cond(&cond, names)
}
