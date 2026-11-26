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
/// it is of, and so does one giving a null in a column's place. One of an
/// expression drops it, and so does a join's left operand, whose rows a
/// join may drop or repeat.
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
    assert_eq!(told_column(&computed), None, "a projection of an expression");
    let joined = topped.left_of(&[JoinKey { left: 0, right: 0, nulls: false }], 4);
    assert_eq!(told_column(&joined), None, "a join's operand");
    assert_eq!(told_column(&Deferred::all(4)), None, "read whole");
    assert!(!Deferred::all(0).any());
}
