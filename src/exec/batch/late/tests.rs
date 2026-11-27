use super::*;

/// The column of the conjunct `deferred` tells the scan, if any.
fn told_column(deferred: &Deferred) -> Option<usize> {
    match deferred.told() {
        Some(Conjunct::Dynamic { column, .. }) => Some(column),
        Some(_) | None => None,
    }
}

/// Verifies: [DESIGN] RULES.md D379: a top-N's input tells its scan the
/// bound of its first key, of that key's column, its key columns read and
/// the others late; a projection of columns moves the bound to the column
/// it is of, and so does one giving a null in a column's place; beside
/// expressions that raise nothing it stays of its column, every column
/// read (RULES.md D1183); the key itself an expression, or an expression
/// that may raise, drops it, and so does a join's left operand, whose rows
/// a join may drop or repeat.
#[test]
fn a_top_bound_is_told_its_scan_where_its_rows_are_the_scans() {
    let threshold = Arc::new(Threshold::new(false));
    let topped = Deferred::all(4).topped(&[2], Some((2, Arc::clone(&threshold))));
    assert!(topped.any());
    assert_eq!(told_column(&topped), Some(2));
    assert_eq!(topped.late_of(&[0, 1, 2, 3]), vec![0, 1, 3], "the key read, the others late");
    let moved = topped.projected(&[Expr::Column(5), Expr::Column(7), Expr::Column(9), Expr::Column(1)]);
    assert_eq!(told_column(&moved), Some(9), "of the column the projection moves");
    let nulled = topped.projected(&[Expr::Column(5), Expr::Const(crate::value::Value::Null), Expr::Column(9), Expr::Column(1)]);
    assert_eq!(told_column(&nulled), Some(9), "a null given in a column's place moves no row (RULES.md D1167)");
    let computed = topped.projected(&[Expr::Column(5), Expr::Neg(Box::new(Expr::Column(7))), Expr::Column(9), Expr::Column(1)]);
    assert_eq!(told_column(&computed), Some(9), "beside an expression that raises nothing, of its column still");
    assert_eq!(computed.late_of(&[5, 9, 1]), Vec::<usize>::new(), "every column read under an expression");
    let key_made = topped.projected(&[Expr::Column(5), Expr::Column(7), Expr::Neg(Box::new(Expr::Column(9))), Expr::Column(1)]);
    assert_eq!(told_column(&key_made), None, "the key itself an expression");
    let divided = Expr::Arith(Box::new(Expr::Column(5)), crate::value::ArithOp::Div, Box::new(Expr::Column(7)));
    let raising = topped.projected(&[divided, Expr::Column(7), Expr::Column(9), Expr::Column(1)]);
    assert_eq!(told_column(&raising), None, "beside an expression that may raise");
    let joined = topped.left_of(&[JoinKey { left: 0, right: 0, nulls: false }], 4);
    assert_eq!(told_column(&joined), None, "a join's operand");
    assert_eq!(told_column(&Deferred::all(4)), None, "read whole");
    assert!(!Deferred::all(0).any());
}

/// Verifies: [DESIGN] (RULES.md D1221) a reader that finds its rows by a
/// key asks for them in any order only where it says so, the filters it
/// tells kept beside, and a projection of columns asks the same of its
/// input; what a filter, a projection that computes, runtime filters
/// judged, a top-N or a join's left operand asks of its own input is in
/// order again.
#[test]
fn any_order_is_asked_of_the_readers_operand_and_under_its_columns() {
    assert!(!Deferred::all(3).any_order() && !Deferred::default().any_order());
    assert!(!Deferred::all(3).in_any_order(false).any_order());
    let asked = Deferred::all(3).in_any_order(true);
    assert!(asked.any_order());
    assert!(asked.clone().with_told(Vec::new()).any_order(), "the filters told beside it");
    let cond = Cond::IsNull(Expr::Column(0), false);
    assert!(!asked.filtered(&cond).any_order(), "a filter's input");
    assert!(asked.projected(&[Expr::Column(1), Expr::Column(0)]).any_order(), "the input of a projection of columns");
    assert!(!Deferred::all(3).projected(&[Expr::Column(1)]).any_order(), "which asks no more than it was asked");
    assert!(!asked.projected(&[Expr::Neg(Box::new(Expr::Column(1)))]).any_order(), "a computed projection's input");
    assert!(!asked.judged(&[0]).any_order(), "under runtime filters");
    assert!(!asked.topped(&[0], None).any_order(), "a top-N's input");
    assert!(!asked.left_of(&[JoinKey { left: 0, right: 0, nulls: false }], 3).any_order(), "a join's left operand");
}
