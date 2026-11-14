//! Unit tests: a sort key's rules, a key alike a select item, positions
//! and names of the result (RULES.md R-TEST-1, R-TEST-8).

use std::sync::Arc;

use super::*;
use crate::functions::{RowOf, ScalarRef};
use crate::parser::Parser;
use crate::value::ArithOp;
use crate::value::{DataType, Decimal, Value};

/// The value expression of the one sort key of `SELECT A FROM T ORDER BY
/// key`.
fn key(key: &str) -> ValueExpression {
    let c = Parser::new(&format!("SELECT A FROM T ORDER BY {key}")).unwrap().cursor_specification().unwrap();
    match c.order_by.into_iter().next().map(|s| s.key) {
        Some(SortColumn::Value(v)) => v,
        Some(SortColumn::Column(cs)) => ValueExpression::Column(cs),
        Some(SortColumn::Position(_)) | None => panic!("no value expression: {key}"),
    }
}

/// The SQLCODE class of `key`'s rules.
fn ruled(k: &str) -> Option<Code> {
    key_rules(&key(k)).err().map(|e| e.code)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26b, §6.14 SR1] a key has a
/// column reference, a window's partition column among them, and no query
/// expression, set function or next value expression.
#[test]
fn a_key_has_a_column_and_nothing_refused_7_17() {
    assert_eq!(ruled("A + 1"), None);
    assert_eq!(ruled("ROW_NUMBER() OVER (ORDER BY A)"), None);
    // MC/DC of "no column reference and no partition column": each alone.
    assert_eq!(ruled("COUNT(*) OVER (PARTITION BY A)"), None, "a partition column alone");
    assert_eq!(ruled("COUNT(*) OVER ()"), Some(Code::SyntaxRule), "neither");
    assert_eq!(ruled("COUNT(*) OVER W"), Some(Code::SyntaxRule), "a window name contains none");
    assert_eq!(ruled("1 + 1"), Some(Code::SyntaxRule));
    assert_eq!(ruled("MAX(A)"), Some(Code::SyntaxRule));
    assert_eq!(ruled("A + MAX(A)"), Some(Code::SyntaxRule), "a set function beside a column");
    assert_eq!(ruled("(SELECT A FROM T) + A"), Some(Code::SyntaxRule));
    assert_eq!(ruled("CASE WHEN EXISTS (SELECT 1 FROM T) THEN A END"), Some(Code::SyntaxRule));
    assert_eq!(ruled("NEXT VALUE FOR S"), Some(Code::SyntaxRule));
}

/// A call of `f` on the columns `columns`.
fn call(f: &ScalarRef, columns: &[usize]) -> Expr {
    Expr::Call(f.clone(), columns.iter().map(|&c| Expr::Column(c)).collect())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26 d i 6] expressions are alike
/// of the same operations on the same columns and values; a part it cannot
/// tell of, or one differing, is not.
#[test]
fn alike_expressions_7_17() {
    let (a, b) = (Expr::Column(0), Expr::Column(1));
    let sum = |x: &Expr, op, y: &Expr| Expr::Arith(Box::new(x.clone()), op, Box::new(y.clone()));
    assert!(alike(&sum(&a, ArithOp::Add, &b), &sum(&a, ArithOp::Add, &b)));
    // MC/DC of "the operator and both operands the same": each differing alone.
    assert!(!alike(&sum(&a, ArithOp::Add, &b), &sum(&a, ArithOp::Sub, &b)), "the operator");
    assert!(!alike(&sum(&a, ArithOp::Add, &b), &sum(&b, ArithOp::Add, &b)), "the left");
    assert!(!alike(&sum(&a, ArithOp::Add, &b), &sum(&a, ArithOp::Add, &a)), "the right");
    let one = |v: i128| Expr::Const(Value::Exact(Decimal::new(v, 0)));
    assert!(alike(&one(1), &one(1)));
    assert!(!alike(&one(1), &one(2)));
    assert!(!alike(&one(1), &Expr::Const(Value::Exact(Decimal::new(10, 1)))), "1 and 1.0 are of other types");
    assert!(alike(&Expr::Neg(Box::new(a.clone())), &Expr::Neg(Box::new(a.clone()))));
    assert!(!alike(&Expr::Neg(Box::new(a.clone())), &a));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26 d i 6] calls, casts and outer
/// references are alike of the same function, type or slot on alike
/// operands; a CASE is not told of.
#[test]
fn alike_calls_casts_and_outer_references_7_17() {
    let a = Expr::Column(0);
    // The same function on the same arguments, and no other.
    let f = ScalarRef(Arc::new(RowOf(DataType::row(Vec::new()))));
    let g = ScalarRef(Arc::new(RowOf(DataType::row(Vec::new()))));
    assert!(alike(&call(&f, &[0, 1]), &call(&f, &[0, 1])));
    assert!(!alike(&call(&f, &[0, 1]), &call(&g, &[0, 1])), "another function");
    assert!(!alike(&call(&f, &[0, 1]), &call(&f, &[0])), "fewer arguments");
    assert!(!alike(&call(&f, &[0, 1]), &call(&f, &[1, 0])), "other arguments");
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: crate::value::Spelled::Integer };
    let smallint = DataType::Exact { precision: 4, scale: 0, spelled: crate::value::Spelled::Smallint };
    let cast = |t: &DataType| Expr::Cast(Box::new(a.clone()), t.clone());
    assert!(alike(&cast(&integer), &cast(&integer)));
    assert!(!alike(&cast(&integer), &cast(&smallint)));
    let chosen = Expr::Case(Vec::new(), Box::new(a.clone()));
    assert!(!alike(&chosen, &chosen), "a CASE is not told of");
    assert!(alike(&Expr::Outer(2, "X".to_owned()), &Expr::Outer(2, "Y".to_owned())));
    assert!(!alike(&Expr::Outer(2, "X".to_owned()), &Expr::Outer(3, "X".to_owned())));
}

/// Verifies: [X3.135-1986 §8.3 SR6] a position is from 1 to the number of
/// columns.
#[test]
fn a_position_is_of_a_column_8_3() {
    assert_eq!(position(1, 2).ok(), Some(0));
    assert_eq!(position(2, 2).ok(), Some(1));
    assert_eq!(position(0, 2).err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(position(3, 2).err().map(|e| e.code), Some(Code::SyntaxRule));
}

/// Verifies: [DESIGN] the select items added for the sort are kept once each, in
/// the order first needed.
#[test]
fn an_added_item_is_kept_once() {
    let mut kept = Vec::new();
    assert_eq!(keep(&mut kept, 3), 0);
    assert_eq!(keep(&mut kept, 1), 1);
    assert_eq!(keep(&mut kept, 3), 0);
    assert_eq!(kept, [3, 1]);
}

/// A result column of `name`.
fn column(name: Option<&str>) -> OutColumn {
    OutColumn { name: name.map(str::to_owned), collation: None, data_type: DataType::Boolean, not_null: false, origin: None }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.1 SR10a] a name names a result column
/// only if one column alone has it.
#[test]
fn a_name_names_one_column_alone() {
    let columns = [column(Some("A")), column(None), column(Some("B")), column(Some("B"))];
    assert_eq!(named(&columns, "A"), Some(0));
    assert_eq!(named(&columns, "B"), None, "two of the name");
    assert_eq!(named(&columns, "C"), None);
    // MC/DC of "unqualified and naming a column".
    assert!(of_result_alone(&key("A + 1"), &columns));
    assert!(!of_result_alone(&key("T.A + 1"), &columns), "qualified");
    assert!(!of_result_alone(&key("C + 1"), &columns), "of no column");
}
