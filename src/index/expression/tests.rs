use super::*;

/// An exact type of `precision` digits, `scale` after the point.
fn exact(precision: u32, scale: u32) -> DataType {
    DataType::Exact { precision, scale, spelled: Spelled::Numeric }
}

/// Verifies: [EXTENSION] [X3.135-1986 §5.9 SR4, GR6a] RULES.md D92:
/// a sum, difference or product of exact operands is admitted only if its
/// every value fits `MAX_EXACT_PRECISION` digits, a quotient never, and
/// an operand not exact never. Boundaries: two INTEGERs' sum (11 digits)
/// fits; a sum needing 19 digits does not, one needing 18 does; a product
/// of 9 and 9 digits fits, of 10 and 9 does not.
#[test]
fn exact_arithmetic_is_admitted_only_where_no_row_overflows() {
    let int = exact(10, 0);
    assert_eq!(exact_arith(Some(&int), ArithOp::Add, Some(&int)), Some(exact(11, 0)));
    assert_eq!(exact_arith(Some(&int), ArithOp::Sub, Some(&exact(4, 2))), Some(exact(13, 2)));
    assert_eq!(exact_arith(Some(&exact(17, 0)), ArithOp::Add, Some(&int)), Some(exact(18, 0)));
    assert_eq!(exact_arith(Some(&exact(18, 0)), ArithOp::Add, Some(&int)), None);
    assert_eq!(exact_arith(Some(&exact(9, 0)), ArithOp::Mul, Some(&exact(9, 0))), Some(exact(18, 0)));
    assert_eq!(exact_arith(Some(&int), ArithOp::Mul, Some(&exact(9, 0))), None);
    assert_eq!(exact_arith(Some(&int), ArithOp::Div, Some(&int)), None);
    assert_eq!(exact_arith(Some(&int), ArithOp::Add, Some(&DataType::Boolean)), None);
    assert_eq!(exact_arith(None, ArithOp::Add, Some(&int)), None);
}
