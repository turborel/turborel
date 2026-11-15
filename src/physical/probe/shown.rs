//! \[DESIGN\] EXPLAIN displays index lookups as SQL conditions (RULES.md D34).

use crate::index::Probe;
use crate::logical::{Cond, Expr, and_all, render_cond};
use crate::value::{CompOp, Value};

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
            let value = Expr::Const(value.clone());
            let args = if predicate.argument == 0 { vec![column, value] } else { vec![value, column] };
            vec![Cond::Value(Expr::Call(predicate.function.clone(), args))]
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
