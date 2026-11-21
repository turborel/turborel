//! \[DESIGN\] The types of a logical plan's output columns, as far as the
//! plan tells them (RULES.md D275): a base table's columns', a literal's,
//! a CAST's, a set function's, a function's result, and arithmetic's by
//! the rules the binder types it by [X3.135-1986 §5.9 SR4, SR5]. What the
//! plan does not tell is not known (none). The form of a plan and the plan
//! of a form are typed by these alone, so a rewrite keeps its output's
//! types where they are known.

use crate::db::State;
use crate::error::Result;
use crate::logical::{Expr, LogicalPlan};
use crate::parser::Parser;
use crate::value::{ArithOp, DOUBLE_PRECISION, DataType, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Spelled, Value};

/// A column's type, or none if not known.
pub type Typed = Option<DataType>;

/// `t` as the plan form writes it: empty if not known.
pub fn text(t: &Typed) -> String {
    t.as_ref().map(ToString::to_string).unwrap_or_default()
}

/// The type `text` writes, none if it is empty.
///
/// # Errors
///
/// SQLCODE -102 if it is no `<data type>`.
pub fn parsed(text: &str) -> Result<Typed> {
    if text.is_empty() {
        return Ok(None);
    }
    Parser::new(text)?.plan_data_type().map(Some)
}

/// Whether two types are the same, as the form writes them.
pub fn same(a: &Typed, b: &Typed) -> bool {
    text(a) == text(b)
}

/// The radix of decimal digits.
const RADIX: u128 = 10;

/// The decimal digits of `units`, at least one.
fn digits(units: i128) -> u32 {
    let mut n = units.unsigned_abs();
    let mut d = 1_u32;
    while n >= RADIX {
        n = n.checked_div(RADIX).unwrap_or(0);
        d = d.saturating_add(1);
    }
    d
}

/// The type of the literal `v`, as the binder types one; none of a value
/// of another kind, which the form shows opaque.
pub fn of_value(v: &Value) -> Typed {
    match v {
        Value::Bool(_) => Some(DataType::Boolean),
        Value::Exact(d) => Some(DataType::Exact { precision: digits(d.units).max(d.scale), scale: d.scale, spelled: Spelled::Numeric }),
        Value::Approx(_) => Some(DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision }),
        Value::Char(s) => Some(DataType::character(u32::try_from(s.chars().count()).unwrap_or(u32::MAX).max(1))),
        Value::Binary(b) => Some(DataType::Binary { length: u32::try_from(b.bytes().len()).unwrap_or(u32::MAX).max(1) }),
        Value::Null
        | Value::DecFloat(_)
        | Value::Lob(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => None,
    }
}

/// [X3.135-1986 §5.9 SR4, SR5; ISO/IEC 9075-2:2023 §6.30 SR1a] The type of
/// `op` of numbers of types `a` and `b`; none if either is not known or
/// not a number, as datetime arithmetic is not typed here.
pub fn of_arithmetic(op: ArithOp, a: &Typed, b: &Typed) -> Typed {
    let (a, b) = (a.as_ref()?, b.as_ref()?);
    if !a.is_numeric() || !b.is_numeric() {
        return None;
    }
    Some(match (a, b) {
        (DataType::Exact { scale: s1, .. }, DataType::Exact { scale: s2, .. }) => {
            let scale = match op {
                ArithOp::Mul => s1.checked_add(*s2).filter(|s| *s <= MAX_EXACT_PRECISION)?,
                ArithOp::Add | ArithOp::Sub | ArithOp::Div => *s1.max(s2),
            };
            DataType::Exact { precision: MAX_EXACT_PRECISION, scale, spelled: Spelled::Numeric }
        }
        (DataType::DecFloat { .. }, _) | (_, DataType::DecFloat { .. }) => DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION },
        _ => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float },
    })
}

/// The type of `e` over a row of columns of `input`'s types.
pub fn of_expr(e: &Expr, input: &[Typed]) -> Typed {
    match e {
        Expr::Column(i) => input.get(*i).cloned().flatten(),
        Expr::Const(v) => of_value(v),
        Expr::Neg(x) => of_expr(x, input).filter(DataType::is_numeric),
        Expr::Arith(a, op, b) => of_arithmetic(*op, &of_expr(a, input), &of_expr(b, input)),
        Expr::Call(f, args) => {
            let types = args.iter().map(|a| of_expr(a, input)).collect::<Option<Vec<_>>>()?;
            f.0.return_type(&types).ok()
        }
        Expr::Cast(_, t) => Some(t.clone()),
        Expr::Truth(_) => Some(DataType::Boolean),
        Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_)
        | Expr::Warned(..) => None,
    }
}

/// The types of `plan`'s output columns, its inputs' being `inner`.
pub fn of_node(plan: &LogicalPlan, inner: &[Vec<Typed>], st: &State) -> Vec<Typed> {
    let unknown = || vec![None; plan.width()];
    let first = inner.first().cloned().unwrap_or_default();
    match plan {
        LogicalPlan::Scan { table, located, .. } => match st.catalog.table(table) {
            Some(desc) => {
                let mut types: Vec<Typed> = desc.columns.iter().map(|c| Some(c.data_type.clone())).collect();
                types.extend(located.then_some(None));
                types
            }
            None => unknown(),
        },
        LogicalPlan::Filter { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::SetOp { .. } => first,
        LogicalPlan::Product { .. } | LogicalPlan::OuterJoin { .. } => inner.concat(),
        LogicalPlan::Aggregate { groups, aggs, .. } => {
            let mut types: Vec<Typed> = groups.iter().map(|g| first.get(*g).cloned().flatten()).collect();
            types.extend(aggs.iter().map(|a| Some(a.result.clone())));
            types
        }
        LogicalPlan::Project { exprs, .. } => exprs.iter().map(|e| of_expr(e, &first)).collect(),
        LogicalPlan::Values { types, .. } => types.iter().cloned().map(Some).collect(),
        LogicalPlan::System { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => unknown(),
    }
}
