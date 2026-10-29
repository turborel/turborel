//! Unit tests of filters judged over vectors (`judge.rs`).

use super::*;
use crate::value::{ArithOp, Decimal};

fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn compare(a: Expr, op: CompOp, b: Expr) -> Cond {
    Cond::Compare(a, op, b)
}

fn and(a: Cond, b: Cond) -> Cond {
    Cond::And(Box::new(a), Box::new(b))
}

fn or(a: Cond, b: Cond) -> Cond {
    Cond::Or(Box::new(a), Box::new(b))
}

/// Verifies: [DESIGN] RULES.md D31, filters by batch: the conjuncts
/// comparing a column the batch holds with a constant, a constant first
/// turned around, testing it for null, or testing it IN or NOT IN a list
/// of constants are judged over vectors; the others, an OR of two columns,
/// and those of a column the batch lacks, are the rest; a filter that can
/// raise an error is all rest.
#[test]
fn conjuncts_judged_over_vectors() {
    let list = or(
        or(compare(Expr::Column(4), CompOp::Eq, int(1)), compare(int(2), CompOp::Eq, Expr::Column(4))),
        compare(Expr::Column(4), CompOp::Eq, int(3)),
    );
    let two = or(compare(Expr::Column(4), CompOp::Eq, int(1)), compare(Expr::Column(2), CompOp::Eq, int(2)));
    let cond = and(
        and(compare(Expr::Column(2), CompOp::Lt, int(5)), compare(int(3), CompOp::Lt, Expr::Column(4))),
        and(
            and(compare(Expr::Column(9), CompOp::Eq, int(1)), Cond::IsNull(Expr::Column(2), true)),
            and(and(list.clone(), Cond::Not(Box::new(list))), two),
        ),
    );
    let (judged, rest) = split(&cond, &[4, 2]);
    let shown: Vec<String> = judged.iter().map(|j| format!("{j:?}")).collect();
    assert_eq!(shown.len(), 5, "{shown:?}");
    assert!(shown[1].contains("Compare(0, Gt"), "turned around: {shown:?}");
    assert!(shown[2].starts_with("IsNull(1, true)"), "{shown:?}");
    assert!(shown[3].starts_with("In(0,") && shown[4].ends_with("true)"), "{shown:?}");
    assert!(matches!(rest, Some(Cond::And(..))), "the column the batch lacks, and the OR of two columns");
    let raising = and(
        compare(Expr::Column(2), CompOp::Lt, int(5)),
        compare(Expr::Arith(Box::new(int(1)), ArithOp::Div, Box::new(Expr::Column(2))), CompOp::Gt, int(0)),
    );
    let (none_judged, all_rest) = split(&raising, &[2]);
    assert!(none_judged.is_empty() && all_rest.is_some(), "it can raise");
    let (one, left) = split(&compare(Expr::Column(2), CompOp::Eq, int(1)), &[2]);
    assert_eq!((one.len(), left.is_none()), (1, true), "nothing left");
}

/// Verifies: [X3.135-1986 §5.13, §5.15, §5.18] what each conjunct judged
/// over vectors keeps, as a row's judgment does: IS NULL a null, never
/// unknown; IN a value equal to one of the list, not a null; NOT IN a
/// value equal to none, not a null, nor any value if the list holds a
/// null, every comparison then unknown or false.
#[test]
fn what_each_conjunct_keeps() {
    let list = vec![number(1), number(2)];
    assert!(Judged::IsNull(0, false).keeps(&Value::Null) && !Judged::IsNull(0, false).keeps(&number(1)));
    assert!(Judged::IsNull(0, true).keeps(&number(1)) && !Judged::IsNull(0, true).keeps(&Value::Null));
    let within = Judged::In(0, list.clone(), false);
    assert!(within.keeps(&number(2)) && !within.keeps(&number(3)) && !within.keeps(&Value::Null));
    let outside = Judged::In(0, list, true);
    assert!(outside.keeps(&number(3)) && !outside.keeps(&number(1)) && !outside.keeps(&Value::Null));
    let nulled = Judged::In(0, vec![number(1), Value::Null], true);
    assert!(!nulled.keeps(&number(3)), "a null in the list: unknown");
    assert!(Judged::Compare(0, CompOp::Ge, number(2)).keeps(&number(2)));
}
