//! \[EXTENSION\] The key parts of an index that are expressions (RULES.md
//! D92): which bound `<value expression>`s may be one, the
//! canonical form a query's expression is matched by, and the value of one
//! of a row.
//!
//! A key part is an expression only if it is deterministic, reads its
//! table's row alone and raises no exception condition of any row: an
//! entry's key is then the value a query computes of the same row, no
//! write fails for an index it does not name, and every row has its key,
//! so a lookup by one hides no error a scan would raise (RULES.md D15).
//! An expression's calls promise it (`ScalarFunction::index_safe`); exact
//! arithmetic is judged by the precision and scale of its operands, a sum
//! of two INTEGERs fitting `MAX_EXACT_PRECISION` digits; anything else that
//! can raise, a division, a CAST, approximate arithmetic, is refused.

use std::fmt;

use crate::catalog::TableDesc;
use crate::error::{Code, Result, SqlError};
use crate::exec::{NoSubqueries, eval};
use crate::logical::{Expr, expr_columns, render_expr};
use crate::storage::Row;
use crate::value::collation::SORT_KEY;
use crate::value::{ArithOp, DataType, MAX_EXACT_PRECISION, Spelled, Value};

/// An argument of a call in a key expression, as the called function
/// judges whether the call can raise (`ScalarFunction::index_safe`).
#[derive(Debug, Clone, Copy)]
pub struct KeyArgument<'a> {
    /// Its type, if the analysis knows it.
    pub data_type: Option<&'a DataType>,
    /// Its value, if it is a constant.
    pub constant: Option<&'a Value>,
}

/// A key part's expression, bound over its table's row and admitted: what
/// it reads and its canonical form.
pub struct BoundKey {
    /// The expression, over the table's row.
    expr: Expr,
    /// Its declared type, the key part's.
    data_type: DataType,
    /// The positions of the columns it reads, in order, each once.
    reads: Vec<usize>,
    /// Its canonical form (`canonical`).
    canonical: String,
    /// Whether it is never null (`never_null`).
    not_null: bool,
}

impl fmt::Debug for BoundKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical)
    }
}

/// \[DESIGN\] The canonical form of `e`, by which a query's expression is
/// the key part's: its tree written out, each function by the name it
/// shows, which of an SQL/JSON operator spells every clause, explicit or
/// implicit (`functions/json.rs`), each constant with its type, each
/// column by its position. Two expressions of one form compute the same
/// value of every row.
pub fn canonical(e: &Expr) -> String {
    format!("{e:?}")
}

impl BoundKey {
    /// The key part `expr`, of type `data_type`, over the row of `desc`,
    /// if it is admitted.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a part of it can raise an exception condition or
    /// is not deterministic, named in the message.
    pub fn new(expr: Expr, data_type: DataType, desc: &TableDesc) -> Result<BoundKey> {
        if let Err(part) = never_raises(&expr, desc) {
            let names: Vec<String> = desc.columns.iter().map(|c| c.name.clone()).collect();
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!(
                    "[EXTENSION] {} can raise an error or is not deterministic, so it is no key part of an index (RULES.md D92)",
                    render_expr(part, &names)
                ),
            ));
        }
        let mut reads = Vec::new();
        expr_columns(&expr, &mut reads);
        reads.sort_unstable();
        reads.dedup();
        let not_null = never_null(&expr, desc);
        Ok(BoundKey { canonical: canonical(&expr), expr, data_type, reads, not_null })
    }

    /// Its value of `row`.
    ///
    /// # Errors
    ///
    /// Only a limit of this implementation, as SQLCODE -900 of JSON text
    /// nested too deep: the expression raises no exception condition.
    pub fn value_of(&self, row: &Row) -> Result<Value> {
        eval(&self.expr, row, &NoSubqueries)
    }

    /// The expression.
    pub fn expr(&self) -> &Expr {
        &self.expr
    }

    /// Its declared type.
    pub fn data_type(&self) -> &DataType {
        &self.data_type
    }

    /// The columns it reads.
    pub fn reads(&self) -> &[usize] {
        &self.reads
    }

    /// Its canonical form.
    pub fn canonical(&self) -> &str {
        &self.canonical
    }

    /// Whether its value of no row is null.
    pub fn not_null(&self) -> bool {
        self.not_null
    }
}

/// Whether `e`, over the row of `desc`, is null of no row: a column NOT
/// NULL, a constant not null, or a negation, sum, difference or product
/// of such [X3.135-1986 §6.9 GR2], which is null only of a null operand.
/// Of anything else, as a function's value, not known.
fn never_null(e: &Expr, desc: &TableDesc) -> bool {
    if let Expr::Column(at) = e {
        return desc.columns.get(*at).is_some_and(|c| c.not_null);
    }
    if let Expr::Const(v) = e {
        return !matches!(v, Value::Null);
    }
    if let Expr::Neg(x) = e {
        return never_null(x, desc);
    }
    if let Expr::Arith(a, _, b) = e {
        return never_null(a, desc) && never_null(b, desc);
    }
    false
}

/// What the analysis knows of an expression that raises nothing: its
/// type, if it knows it.
type Typed = Option<DataType>;

/// Whether `e`, over the row of `desc`, is deterministic and raises no
/// exception condition of any row: its type if so, as far as known; else
/// the part of it that may.
fn never_raises<'e>(e: &'e Expr, desc: &TableDesc) -> std::result::Result<Typed, &'e Expr> {
    match e {
        Expr::Column(at) => desc.columns.get(*at).map(|c| Some(c.data_type.clone())).ok_or(e),
        Expr::Const(v) => Ok(constant_type(v)),
        // Negation of a number raises nothing: exact values are symmetric.
        Expr::Neg(x) => never_raises(x, desc)?.filter(DataType::is_numeric).map(Some).ok_or(e),
        Expr::Arith(a, op, b) => {
            let (left, right) = (never_raises(a, desc)?, never_raises(b, desc)?);
            exact_arith(left.as_ref(), *op, right.as_ref()).map(Some).ok_or(e)
        }
        Expr::Call(f, args) => {
            if !f.0.deterministic() || f.0.uses_context(args.len()) {
                return Err(e);
            }
            let typed = args.iter().map(|a| never_raises(a, desc)).collect::<std::result::Result<Vec<Typed>, _>>()?;
            let constant = |a: &'e Expr| if let Expr::Const(v) = a { Some(v) } else { None };
            let arguments: Vec<KeyArgument<'_>> =
                typed.iter().zip(args).map(|(t, a)| KeyArgument { data_type: t.as_ref(), constant: constant(a) }).collect();
            if !f.0.index_safe(&arguments) {
                return Err(e);
            }
            let all_typed: Option<Vec<DataType>> = typed.into_iter().collect();
            Ok(all_typed.and_then(|t| f.0.return_type(&t).ok()))
        }
        // [ISO/IEC 9075:1992 §6.9 GR2] only through its conditions and results.
        Expr::Case(whens, otherwise) => {
            for (c, result) in whens {
                if c.can_raise() {
                    return Err(e);
                }
                never_raises(result, desc)?;
            }
            never_raises(otherwise, desc)
        }
        Expr::SortKey(x, _) => never_raises(x, desc).map(|_| Some(SORT_KEY)),
        Expr::Truth(c) => {
            if c.can_raise() {
                Err(e)
            } else {
                Ok(Some(DataType::Boolean))
            }
        }
        Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::Warned(..)
        | Expr::Subquery(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_)
        | Expr::Outer(..)
        | Expr::Dynamic(_) => Err(e),
    }
}

/// The type of the constant `v`, where the analysis needs it: of an exact
/// number, its digits and scale; of a character string, its length.
fn constant_type(v: &Value) -> Typed {
    match v {
        Value::Exact(d) => Some(DataType::Exact { precision: d.digits().max(d.scale), scale: d.scale, spelled: Spelled::Numeric }),
        Value::Char(s) => u32::try_from(s.chars().count()).ok().map(DataType::varying),
        Value::Bool(_) => Some(DataType::Boolean),
        Value::Null
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => None,
    }
}

/// [X3.135-1986 §5.9 SR4] The type of `a op b`, of exact numeric operands
/// of these types, if its every value fits `MAX_EXACT_PRECISION` digits,
/// so no row can make it overflow [GR6a]: a sum or difference has a digit
/// before the point more than the wider operand, at the larger scale
/// \[SR4b\]; a product the digits of both, at the sum of their scales
/// \[SR4c\]. A quotient can divide by zero \[GR5\], so it is none.
fn exact_arith(a: Option<&DataType>, op: ArithOp, b: Option<&DataType>) -> Option<DataType> {
    let (Some(DataType::Exact { precision: pa, scale: sa, .. }), Some(DataType::Exact { precision: pb, scale: sb, .. })) = (a, b) else {
        return None;
    };
    let (ia, ib) = (pa.saturating_sub(*sa), pb.saturating_sub(*sb));
    let (whole, scale) = match op {
        ArithOp::Add | ArithOp::Sub => (ia.max(ib).saturating_add(1), (*sa).max(*sb)),
        ArithOp::Mul => (ia.saturating_add(ib), sa.saturating_add(*sb)),
        ArithOp::Div => return None,
    };
    let precision = whole.saturating_add(scale);
    (precision <= MAX_EXACT_PRECISION).then_some(DataType::Exact { precision, scale, spelled: Spelled::Numeric })
}

// Unit tests: which exact arithmetic can overflow (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
