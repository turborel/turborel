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

/// OCTET_LENGTH of column `c`.
fn octets(c: usize) -> Expr {
    Expr::Call(crate::functions::Registry::with_set_functions().scalar(kw::OCTET_LENGTH).unwrap(), vec![Expr::Column(c)])
}

/// Column `c` compared by `op` with `text`, both unpadded, as the binder
/// writes a comparison under a collation that orders strings by octets.
fn unpadded(c: usize, op: crate::value::CompOp, text: &str) -> Cond {
    let by_octets = crate::value::collation::unpadded_scalar();
    let constant = Expr::Const(crate::value::Value::Char(text.into()));
    Cond::Compare(Expr::sort_key(Expr::Column(c), &by_octets), op, Expr::sort_key(constant, &by_octets))
}

/// A reader that reads the text of column 0 alone of rows 4 wide.
fn measuring() -> Deferred {
    Deferred::all(4).measuring(&[0], 4)
}

/// Verifies: [DESIGN] RULES.md D1267, which columns a scan is asked for
/// measured: one its reader reads only as the operand of OCTET_LENGTH,
/// alone or within an expression, with no operator that drops rows and
/// under a filter that compares it unpadded with the empty string, by any
/// operator, alone or beside a conjunct of another column. It is read,
/// and not measured, if the reader reads its text too, or takes its
/// length in characters, or a filter compares it unpadded with another
/// constant, compares it padded, or holds the comparison under an OR.
/// MC/DC of a column measured: its text read by the reader against not,
/// and by a filter against not.
#[test]
fn a_column_read_for_its_length_alone_is_asked_measured() {
    use crate::value::{CompOp, Decimal, Value};
    let other = || Cond::Compare(Expr::Column(0), CompOp::Gt, Expr::Const(Value::Exact(Decimal::new(3, 0))));
    let plus_one = Expr::Arith(Box::new(octets(1)), crate::value::ArithOp::Add, Box::new(Expr::Const(Value::Exact(Decimal::new(1, 0)))));
    assert_eq!(read_as_text(&[&octets(1), &Expr::Column(0)], &[]), [0]);
    assert_eq!(read_as_text(&[&plus_one, &Expr::Column(0)], &[]), [0], "within an expression");
    assert_eq!(read_as_text(&[&octets(1), &Expr::Column(1)], &[]), [1], "its text read too");
    let characters = Expr::Call(crate::functions::Registry::with_set_functions().scalar(kw::CHAR_LENGTH).unwrap(), vec![Expr::Column(1)]);
    assert_eq!(read_as_text(&[&characters], &[]), [1], "characters are counted of the text");
    let asked = measuring();
    assert!(asked.any());
    assert_eq!((asked.measured_of(&[0, 1]), asked.late_of(&[0, 1])), (vec![1], Vec::new()), "measured though no operator drops rows");
    let kept = |cond: &Cond| asked.filtered(cond).measured_of(&[0, 1]);
    for op in [CompOp::Ne, CompOp::Eq, CompOp::Gt, CompOp::Ge, CompOp::Lt, CompOp::Le] {
        assert_eq!(kept(&unpadded(1, op, "")), [1], "{op:?} the empty string is told by its length");
    }
    assert_eq!(kept(&Cond::And(Box::new(unpadded(1, CompOp::Ne, "")), Box::new(other()))), [1]);
    let padded = Cond::Compare(Expr::Column(1), CompOp::Ne, Expr::Const(Value::Char("".into())));
    let either = Cond::Or(Box::new(unpadded(1, CompOp::Ne, "")), Box::new(other()));
    assert_eq!(kept(&unpadded(1, CompOp::Ne, "a")), Vec::<usize>::new(), "another constant: the filter reads its text");
    assert_eq!(kept(&padded), Vec::<usize>::new(), "padded: the filter reads its text");
    assert_eq!(kept(&either), Vec::<usize>::new(), "under an OR: the filter reads its text");
}

/// Verifies: [DESIGN] RULES.md D1267, a measured column through the
/// operators between its reader and its scan: a projection of columns
/// moves it, unless it gives the column twice and one is read; a
/// projection that computes measures what it reads for length alone and
/// nothing it reads as text; a join's left operand and runtime filters
/// measure none, and a reader told nothing measures none. MC/DC of a
/// column moved: every place it is given measured against one read.
#[test]
fn a_measured_column_is_moved_by_projections_and_dropped_by_joins() {
    use crate::value::{Decimal, Value};
    let none = Vec::<usize>::new();
    let asked = measuring();
    let moved = asked.projected(&[Expr::Column(5), Expr::Column(7), Expr::Column(9), Expr::Column(1)]);
    assert_eq!(moved.measured_of(&[1, 5, 7, 9]), [1, 7, 9], "each moved to a column measured");
    let twice = asked.projected(&[Expr::Column(7), Expr::Column(7), Expr::Column(9), Expr::Column(1)]);
    assert_eq!(twice.measured_of(&[1, 7, 9]), [1, 9], "given twice, once to a column read");
    let plus_one = Expr::Arith(Box::new(octets(1)), crate::value::ArithOp::Add, Box::new(Expr::Const(Value::Exact(Decimal::new(1, 0)))));
    let computed = Deferred::all(2).projected(&[octets(2), Expr::Column(0), plus_one]);
    assert_eq!(computed.measured_of(&[0, 1, 2]), [1, 2], "a projection that computes measures what it reads for length alone");
    assert_eq!(Deferred::all(2).projected(&[octets(2), Expr::Column(2)]).measured_of(&[0, 2]), none, "its text read beside");
    assert_eq!(asked.left_of(&[], 4).measured_of(&[0, 1]), none, "a join's left operand");
    assert_eq!(asked.judged(&[0]).measured_of(&[0, 1]), none, "under runtime filters");
    assert_eq!(Deferred::all(4).measured_of(&[0, 1]), none, "a reader that told nothing");
    assert_eq!(Deferred::default().measured_of(&[0, 1]), none);
}
