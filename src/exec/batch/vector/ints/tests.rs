//! Unit tests of expressions of integers (`vector/ints.rs`).

use super::*;
use crate::value::Decimal;

fn exact(units: i128, scale: u32) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(units, scale)))
}

fn arith(a: Expr, op: ArithOp, b: Expr) -> Expr {
    Expr::Arith(Box::new(a), op, Box::new(b))
}

/// A batch of two columns of integers, of scales 0 and 2, nulls among
/// them, and one of values.
fn batch() -> Batch {
    let first = Ints::new(vec![3, -7, 0, 12, 0, 999], 0, Some(vec![false, false, false, false, true, false]));
    let second = Ints::new(vec![150, 25, -1, 0, 7, 0], 2, Some(vec![false, false, false, false, false, true]));
    let values = (0..6).map(|n| Value::Exact(Decimal::new(n, 0))).collect();
    Batch { columns: vec![Column::Ints(first), Column::Ints(second), Column::Values(values)], rows: 6 }
}

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.9 GR1, GR4, SR4b,
/// SR4c]: an expression of integers evaluated as integers gives the values
/// it gives evaluated of values: a column alone, a constant, a negation,
/// sums and differences of scales unlike, products, constants of either
/// scale, nested, a null of a null; of some rows, theirs. A quotient is
/// not, nor an approximate constant, a column of values, a product of
/// scales past 18, a result past 18 digits, or an expression deeper than a
/// vector's evaluation goes.
#[test]
fn integers_evaluated_as_their_values_are() {
    let batch = batch();
    let inputs = [0, 1, 2];
    let (a, b) = (Expr::Column(0), Expr::Column(1));
    let alike = [
        a.clone(),
        exact(-25, 1),
        Expr::Neg(Box::new(b.clone())),
        arith(a.clone(), ArithOp::Add, b.clone()),
        arith(b.clone(), ArithOp::Sub, a.clone()),
        arith(a.clone(), ArithOp::Mul, b.clone()),
        arith(arith(a.clone(), ArithOp::Mul, exact(5, 1)), ArithOp::Sub, exact(-3, 0)),
        arith(Expr::Neg(Box::new(a.clone())), ArithOp::Add, arith(b.clone(), ArithOp::Mul, b.clone())),
    ];
    for rows in [None, Some(&[0_usize, 2, 4, 5][..])] {
        for e in &alike {
            let evaluated = lane(e, &inputs, &batch, rows);
            assert!(evaluated.is_some(), "{e:?} as integers");
            let made = evaluated.unwrap();
            let by_values = crate::exec::batch::vector::lane(e, &inputs, &batch, rows).unwrap();
            let count = rows.map_or(batch.rows, <[usize]>::len);
            let expected: Vec<String> = (0..count).map(|n| by_values.at(n).to_string()).collect();
            let found: Vec<String> = made.values().iter().map(ToString::to_string).collect();
            assert_eq!(found, expected, "{e:?} of {rows:?}");
        }
    }
    let lone = lane(&a, &inputs, &batch, Some(&[1, 3])).unwrap();
    assert_eq!(lone.values().iter().map(ToString::to_string).collect::<Vec<_>>(), ["-7", "12"], "a column alone");
    let refused = [
        arith(a.clone(), ArithOp::Div, exact(2, 0)),
        arith(a.clone(), ArithOp::Add, Expr::Const(Value::Approx(1.5))),
        arith(Expr::Column(2), ArithOp::Add, a.clone()),
        arith(b.clone(), ArithOp::Mul, exact(1, 17)),
        arith(a.clone(), ArithOp::Mul, exact(999_999_999_999_999_999, 0)),
        arith(exact(999_999_999_999_999_999, 0), ArithOp::Add, a.clone()),
    ];
    for e in &refused {
        assert!(lane(e, &inputs, &batch, None).is_none(), "{e:?}");
    }
    let deep = (0_u32..20_u32).fold(a.clone(), |e, _| Expr::Neg(Box::new(e)));
    assert!(lane(&deep, &inputs, &batch, None).is_none(), "deeper than DEPTH");
}
