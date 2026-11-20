//! Unit tests of what a read's conjuncts say of a partition key's parts
//! (RULES.md D156): comparisons either way round, IN lists, null tests,
//! ORs of them, and conjuncts narrowing one another.

use std::sync::Arc;

use super::*;
use crate::logical::InSet;
use crate::value::{Decimal, INTEGER_PRECISION, Spelled};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

const INTEGER: DataType = DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer };

/// Column `c` compared by `op` with the integer `n`.
fn cmp(c: usize, op: CompOp, n: i128) -> Cond {
    Cond::Compare(Expr::Column(c), op, Expr::Const(int(n)))
}

/// Column `c` IN the integers `ns`.
fn among(c: usize, ns: &[i128]) -> Cond {
    Cond::In(Expr::Column(c), Arc::new(InSet::of(ns.iter().map(|n| int(*n)).collect())))
}

/// What `conds` say of the one part, column 0, as text.
fn of(conds: &[Cond]) -> String {
    format!("{:?}", restrictions(conds, &[Some(0)], &[INTEGER])[0])
}

/// The text of a range from `low` to `high`, each with whether included.
fn range(low: Option<(i128, bool)>, high: Option<(i128, bool)>) -> String {
    format!("{:?}", Some(Restriction::Range { low: low.map(|(n, i)| (int(n), i)), high: high.map(|(n, i)| (int(n), i)) }))
}

/// The text of a list of `ns`.
fn equal(ns: &[i128]) -> String {
    format!("{:?}", Some(Restriction::Equal(ns.iter().map(|n| int(*n)).collect())))
}

/// Verifies: [EXTENSION] RULES.md D156: each comparison of the part with a
/// constant, either way round, is its range or list; `<>`, a comparison
/// with another column or of two constants restricts nothing; a
/// comparison with the null value admits nothing.
#[test]
fn comparisons_restrict_a_part() {
    assert_eq!(of(&[cmp(0, CompOp::Le, 5)]), range(None, Some((5, true))));
    assert_eq!(of(&[cmp(0, CompOp::Gt, 5)]), range(Some((5, false)), None));
    assert_eq!(of(&[cmp(0, CompOp::Eq, 5)]), equal(&[5]));
    let flipped = Cond::Compare(Expr::Const(int(5)), CompOp::Gt, Expr::Column(0));
    assert_eq!(of(&[flipped]), range(None, Some((5, false))), "5 > c is c < 5");
    assert_eq!(of(&[Cond::Compare(Expr::Const(int(5)), CompOp::Eq, Expr::Column(1))]), "None");
    assert_eq!(of(&[cmp(0, CompOp::Ne, 5)]), "None");
    assert_eq!(of(&[cmp(1, CompOp::Eq, 5)]), "None", "another column");
    assert_eq!(of(&[Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Column(1))]), "None");
    assert_eq!(of(&[Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(Value::Null))]), equal(&[]));
    assert_eq!(of(&[Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(Value::Approx(1.5)))]), equal(&[]), "no integer");
}

/// Verifies: [EXTENSION] RULES.md D156: IN lists its values, of another
/// column nothing; IS NULL is the null restriction, IS NOT NULL none; an
/// OR of lists is both lists, an OR with anything else nothing.
#[test]
fn lists_nulls_and_ors() {
    assert_eq!(of(&[among(0, &[1, 2])]), equal(&[1, 2]));
    assert_eq!(of(&[among(1, &[1, 2])]), "None");
    assert_eq!(of(&[Cond::IsNull(Expr::Column(0), false)]), format!("{:?}", Some(Restriction::Null)));
    assert_eq!(of(&[Cond::IsNull(Expr::Column(0), true)]), "None");
    let lists = Cond::Or(Box::new(cmp(0, CompOp::Eq, 1)), Box::new(among(0, &[3])));
    assert_eq!(of(&[lists]), equal(&[1, 3]));
    let mixed = Cond::Or(Box::new(cmp(0, CompOp::Eq, 1)), Box::new(cmp(0, CompOp::Lt, 0)));
    assert_eq!(of(&[mixed]), "None");
}

/// Verifies: [EXTENSION] RULES.md D156: conjuncts narrow one another: two
/// ranges to the tighter of each end, an end met by both included only if
/// both include it; a list to its values within the other restriction;
/// null with null is null, null with anything else admits nothing.
#[test]
fn conjuncts_narrow_each_other() {
    assert_eq!(of(&[cmp(0, CompOp::Ge, 2), cmp(0, CompOp::Gt, 4), cmp(0, CompOp::Lt, 9)]), range(Some((4, false)), Some((9, false))));
    assert_eq!(of(&[cmp(0, CompOp::Le, 9), cmp(0, CompOp::Lt, 9)]), range(None, Some((9, false))));
    assert_eq!(of(&[cmp(0, CompOp::Ge, 3), cmp(0, CompOp::Ge, 3)]), range(Some((3, true)), None));
    assert_eq!(of(&[among(0, &[1, 5, 9]), cmp(0, CompOp::Ge, 5), cmp(0, CompOp::Le, 8)]), equal(&[5]));
    assert_eq!(of(&[cmp(0, CompOp::Gt, 1), among(0, &[1, 2])]), equal(&[2]));
    assert_eq!(of(&[among(0, &[1, 2]), among(0, &[2, 3])]), equal(&[2]));
    let null = Cond::IsNull(Expr::Column(0), false);
    assert_eq!(of(&[null.clone(), null.clone()]), format!("{:?}", Some(Restriction::Null)));
    assert_eq!(of(&[null.clone(), cmp(0, CompOp::Eq, 1)]), equal(&[]));
    assert_eq!(of(&[among(0, &[1]), null]), equal(&[]));
    assert_eq!(of(&[among(0, &[1]), Cond::IsNull(Expr::Column(0), true)]), equal(&[1]), "a conjunct restricting nothing");
}

/// Verifies: [EXTENSION] RULES.md D156: an expression part, given as no
/// column, is restricted by nothing; each column part by its own
/// conjuncts.
#[test]
fn each_part_by_its_own_column() {
    let found = restrictions(&[cmp(0, CompOp::Eq, 1), cmp(1, CompOp::Lt, 7)], &[Some(1), None, Some(0)], &[INTEGER, INTEGER, INTEGER]);
    let text: Vec<String> = found.iter().map(|r| format!("{r:?}")).collect();
    assert_eq!(text, [range(None, Some((7, false))), "None".to_owned(), equal(&[1])]);
}
