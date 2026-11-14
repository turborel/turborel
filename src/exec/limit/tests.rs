use super::*;
use crate::error::SqlError;
use crate::exec::NoSubqueries;
use crate::value::Decimal;

/// A whole number, as a constant.
fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// The rows 0 to `n` - 1, each one column of its number.
fn rows(n: i128) -> RowIter {
    Box::new((0..n).map(|i| Ok(vec![Value::Exact(Decimal::new(i, 0))])))
}

/// The first column of each row `limit` keeps of `n` rows, or the SQLSTATE
/// of its error.
fn kept(limit: &Limit, n: i128) -> std::result::Result<Vec<i128>, String> {
    let (skip, left) = counts(limit, &NoSubqueries).map_err(|e| e.sqlstate().as_str().to_owned())?;
    Ok(Limited { rows: rows(n), skip, left }.map(number).collect())
}

/// The number the one column of `row` holds.
fn number(row: Result<Row>) -> i128 {
    let row = row.unwrap();
    let Some(Value::Exact(d)) = row.first() else { panic!("not a number: {row:?}") };
    d.units
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] OFFSET passes over its
/// count of rows, all of them past the last, none of 0; FETCH FIRST keeps
/// at most its count after; both, in order; neither, every row.
#[test]
fn the_rows_offset_and_fetch_first_keep_7_17() {
    let limit = |offset: Option<i128>, fetch: Option<i128>| Limit { offset: offset.map(int), fetch: fetch.map(int) };
    assert_eq!(kept(&limit(Some(2), None), 5), Ok(vec![2, 3, 4]));
    assert_eq!(kept(&limit(Some(0), None), 3), Ok(vec![0, 1, 2]));
    assert_eq!(kept(&limit(Some(9), None), 3), Ok(vec![]), "GR6c i");
    assert_eq!(kept(&limit(None, Some(2)), 5), Ok(vec![0, 1]));
    assert_eq!(kept(&limit(None, Some(9)), 3), Ok(vec![0, 1, 2]), "GR7d");
    assert_eq!(kept(&limit(Some(1), Some(2)), 5), Ok(vec![1, 2]));
    assert_eq!(kept(&limit(Some(4), Some(2)), 5), Ok(vec![4]));
    assert_eq!(kept(&limit(None, None), 2), Ok(vec![0, 1]));
    let past = i128::from(u64::MAX) * 4;
    assert_eq!(kept(&limit(Some(past), Some(past)), 2), Ok(vec![]), "a count past usize::MAX");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR6b, GR7c] an offset below 0 is
/// SQLSTATE 2201X, a fetch first count below 1 2201W, either null as well.
#[test]
fn a_count_not_a_row_count_is_refused_7_17() {
    let refused = |offset: Option<Expr>, fetch: Option<Expr>| kept(&Limit { offset, fetch }, 3);
    assert_eq!(refused(Some(int(-1)), None), Err("2201X".to_owned()));
    assert_eq!(refused(None, Some(int(0))), Err("2201W".to_owned()));
    assert_eq!(refused(None, Some(int(-5))), Err("2201W".to_owned()));
    assert_eq!(refused(Some(Expr::Const(Value::Null)), None), Err("2201X".to_owned()));
    assert_eq!(refused(None, Some(Expr::Const(Value::Null))), Err("2201W".to_owned()));
    assert_eq!(refused(Some(int(0)), Some(int(1))), Ok(vec![0]));
}

/// Verifies: [DESIGN] RULES.md R-ERR-1, an error of the input passes on
/// in its place, neither passed over nor counted, and no row past those
/// kept is read.
#[test]
fn an_error_passes_on_and_no_row_past_is_read() {
    let failing: RowIter =
        Box::new([Ok(vec![Value::Null]), Err(SqlError::new(Code::Data, "x")), Ok(vec![Value::Null]), Ok(vec![Value::Null])].into_iter());
    let got: Vec<bool> = Limited { rows: failing, skip: 1, left: Some(2) }.map(|r| r.is_ok()).collect();
    assert_eq!(got, vec![false, true, true]);
    let endless: RowIter = Box::new(std::iter::repeat_with(|| Ok(vec![Value::Null])));
    assert_eq!(Limited { rows: endless, skip: 3, left: Some(2) }.count(), 2);
}
