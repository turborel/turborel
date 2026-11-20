//! Unit tests of a comparison of a row with a subquery of no quantifier
//! (`eval::quantified` of `Quantifier::One`).

use super::super::super::eval;
use super::*;
use crate::error::Code;
use crate::value::Decimal;

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [X3.135-1986 §5.11 GR1, GR2] a comparison with a subquery
/// of no quantifier compares with its one row: of no row it is unknown;
/// of more than one row it is a cardinality violation, SQLCODE -811; an
/// error reading the rows is the comparison's.
#[test]
fn a_comparison_of_no_quantifier_takes_one_row() {
    let x = [number(1)];
    let one = |rows: Vec<Result<Row>>| eval::quantified(&x, (CompOp::Eq, Quantifier::One), rows);
    assert_eq!(one(vec![Ok(vec![number(1)])]).unwrap(), Truth::True);
    assert_eq!(one(vec![Ok(vec![number(2)])]).unwrap(), Truth::False);
    assert_eq!(one(Vec::new()).unwrap(), Truth::Unknown);
    assert_eq!(one(vec![Ok(vec![number(1)]), Ok(vec![number(1)])]).unwrap_err().code, Code::Cardinality);
    let failed = crate::error::SqlError::new(Code::Corrupt, "a row not read");
    assert_eq!(one(vec![Err(failed)]).unwrap_err().code, Code::Corrupt);
}
