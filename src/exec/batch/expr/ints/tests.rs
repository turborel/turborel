//! Unit tests of exact arithmetic over integers (`ints.rs`).

use super::*;

/// Integers of `units` at `scale`, no null.
fn ints(units: Vec<i64>, scale: u32) -> Ints {
    Ints::new(units, scale, None)
}

/// Verifies: [X3.135-1986 §5.9 SR4b, SR4c] a sum is at the greater
/// scale, each operand scaled to it; a product at the sum of the scales;
/// a quotient not computed over integers; a product whose scale passes 38
/// none.
#[test]
fn scales_are_those_decimals_give() {
    let (a, b) = (ints(vec![150, -25], 2), ints(vec![3, 4], 0));
    let sum = arith(ArithOp::Add, (&Exact::Column(&a), &Exact::Column(&b)), 2).unwrap();
    assert_eq!((sum.units.clone(), sum.scale), (vec![450, 375], 2));
    let difference = arith(ArithOp::Sub, (&Exact::Column(&b), &Exact::Const(5, 1)), 2).unwrap();
    assert_eq!((difference.units.clone(), difference.scale), (vec![25, 35], 1));
    let product = arith(ArithOp::Mul, (&Exact::Column(&a), &Exact::Const(3, 1)), 2).unwrap();
    assert_eq!((product.units.clone(), product.scale), (vec![450, -75], 3));
    assert!(arith(ArithOp::Div, (&Exact::Column(&a), &Exact::Column(&b)), 2).is_none());
    let fine = arith(ArithOp::Mul, (&Exact::Column(&a), &Exact::Const(1, 17)), 2).unwrap();
    assert_eq!((fine.units.clone(), fine.scale), (vec![150, -25], 19), "scale 19");
    assert!(arith(ArithOp::Mul, (&Exact::Column(&a), &Exact::Const(1, 37)), 2).is_none(), "scale 39");
}

/// Verifies: [X3.135-1986 §5.9 GR1, GR6a] a null is null of a null, its
/// slot 0, its overflow no flag; one of a row not null gives none, past
/// 64 bits or of 19 digits, which the rows compute in 128 bits.
#[test]
fn the_flag_counts_rows_not_null() {
    let nulls = Some(Mask::from_bools(&[false, true]));
    let a = Ints::new(vec![1, i64::MAX], 0, nulls);
    let made = arith(ArithOp::Add, (&Exact::Column(&a), &Exact::Const(1, 0)), 2).unwrap();
    assert_eq!(made.units, vec![2, 0], "the null's slot 0");
    assert!(made.is_null(1) && !made.is_null(0));
    let big = ints(vec![1, 999_999_999_999_999_999], 0);
    assert!(arith(ArithOp::Add, (&Exact::Column(&big), &Exact::Const(1, 0)), 2).is_none(), "19 digits");
    assert!(arith(ArithOp::Mul, (&Exact::Column(&big), &Exact::Const(i64::MAX, 0)), 2).is_none(), "past 64 bits");
    assert!(negated(&ints(vec![i64::MIN], 0)).is_none());
}

/// Verifies: [DESIGN] RULES.md D31: a result of two operands known as
/// runs is known as runs, its ends those of either; of a constant, the
/// column's.
#[test]
fn runs_of_both_are_kept() {
    let a = ints(vec![1, 1, 2, 2], 0).with_runs(vec![2, 4]);
    let b = ints(vec![5, 6, 6, 6], 0).with_runs(vec![1, 4]);
    let made = arith(ArithOp::Add, (&Exact::Column(&a), &Exact::Column(&b)), 4).unwrap();
    assert_eq!(made.units, vec![6, 7, 8, 8]);
    assert_eq!(made.runs(), Some(&[1_usize, 2, 4][..]));
    let of_constant = arith(ArithOp::Mul, (&Exact::Column(&a), &Exact::Const(3, 0)), 4).unwrap();
    assert_eq!(of_constant.runs(), Some(&[2_usize, 4][..]));
    assert_eq!(arith(ArithOp::Add, (&Exact::Column(&a), &Exact::Column(&ints(vec![0; 4], 0))), 4).unwrap().runs(), None);
}
