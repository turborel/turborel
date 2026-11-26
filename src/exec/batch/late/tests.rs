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

/// Verifies: [DESIGN] RULES.md D1096: an inner join gives late the columns
/// of its right operand's rows none above reads, once an operator above
/// drops rows, each as the operand's own column; a column a filter above
/// reads is read; read whole with no row dropped above, none; and the left
/// operand of a join above drops rows, so the join below gives them late.
#[test]
fn a_join_gives_late_the_right_columns_none_above_reads() {
    assert_eq!(Deferred::all(5).right_of(2), Vec::<usize>::new(), "no row dropped above");
    let cond = Cond::Compare(Expr::Column(3), crate::value::CompOp::Eq, Expr::Const(crate::value::Value::Null));
    let filtered = Deferred::all(5).filtered(&cond);
    assert_eq!(filtered.right_of(2), vec![0, 2], "column 3 read by the filter");
    let above = Deferred::all(7).left_of(&[JoinKey { left: 1, right: 0, nulls: false }], 5);
    assert_eq!(above.right_of(2), vec![0, 1, 2], "a join above, its key the left's column 1");
    assert_eq!(above.right_of(5), Vec::<usize>::new(), "past the left's columns, none");
}
