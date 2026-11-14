use super::*;
use crate::error::SqlError;
use crate::exec::NoSubqueries;
use crate::logical::Fetch;
use crate::spill::testing::{Held, areas};
use crate::value::types::TypeRegistry;
use crate::value::{DecFloat, Decimal};

/// A whole number, as a constant.
fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// The rows of `values` in their order, each one column of its value.
fn rows(values: &[i128]) -> RowIter {
    let made: Vec<Result<Row>> = values.iter().map(|&i| Ok(vec![Value::Exact(Decimal::new(i, 0))])).collect();
    Box::new(made.into_iter())
}

/// A FETCH FIRST of `count`, a percentage if `percent`, of ONLY.
fn fetch(count: Expr, percent: bool) -> Fetch {
    Fetch { count, percent, ties: None }
}

/// `fetch`, of WITH TIES by the one column ascending.
fn tied(count: Expr, percent: bool) -> Fetch {
    Fetch { ties: Some(vec![SortKey::ascending(0)]), ..fetch(count, percent) }
}

/// The first column of each row `limit` keeps of the rows of `values`, or
/// the SQLSTATE of its error.
fn kept_of(limit: &Limit, values: &[i128]) -> std::result::Result<Vec<i128>, String> {
    kept_in(limit, values, ListSpace::default())
}

/// `kept_of`, a percentage's rows held in `space`.
fn kept_in(limit: &Limit, values: &[i128], space: ListSpace) -> std::result::Result<Vec<i128>, String> {
    let state = |e: SqlError| e.sqlstate().as_str().to_owned();
    let limited = kept(limit, &NoSubqueries, rows(values), space).map_err(state)?;
    limited.map(|r| r.as_ref().map(number).map_err(Clone::clone)).collect::<Result<Vec<_>>>().map_err(state)
}

/// A space holding a few rows before it spills, to runs `broken` or not.
fn small(broken: bool) -> ListSpace {
    let few = 256;
    ListSpace { limit: few, batch: few, areas: Some(areas(Some(Arc::new(Held { broken })), usize::MAX)), types: TypeRegistry::default() }
}

/// The number the one column of `row` holds.
fn number(row: &Row) -> i128 {
    let Some(Value::Exact(d)) = row.first() else { panic!("not a number: {row:?}") };
    d.units
}

/// The numbers 0 to `n` - 1.
fn upto(n: i128) -> Vec<i128> {
    (0..n).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] OFFSET passes over its
/// count of rows, all of them past the last, none of 0; FETCH FIRST keeps
/// at most its count after; both, in order; neither, every row.
#[test]
fn the_rows_offset_and_fetch_first_keep_7_17() {
    let limit = |offset: Option<i128>, count: Option<i128>| Limit { offset: offset.map(int), fetch: count.map(|n| fetch(int(n), false)) };
    assert_eq!(kept_of(&limit(Some(2), None), &upto(5)), Ok(vec![2, 3, 4]));
    assert_eq!(kept_of(&limit(Some(0), None), &upto(3)), Ok(vec![0, 1, 2]));
    assert_eq!(kept_of(&limit(Some(9), None), &upto(3)), Ok(vec![]), "GR6c i");
    assert_eq!(kept_of(&limit(None, Some(2)), &upto(5)), Ok(vec![0, 1]));
    assert_eq!(kept_of(&limit(None, Some(9)), &upto(3)), Ok(vec![0, 1, 2]), "GR7d");
    assert_eq!(kept_of(&limit(Some(1), Some(2)), &upto(5)), Ok(vec![1, 2]));
    assert_eq!(kept_of(&limit(Some(4), Some(2)), &upto(5)), Ok(vec![4]));
    assert_eq!(kept_of(&limit(None, None), &upto(2)), Ok(vec![0, 1]));
    let past = i128::from(u64::MAX) * 4;
    assert_eq!(kept_of(&limit(Some(past), Some(past)), &upto(2)), Ok(vec![]), "a count past usize::MAX");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR6b, GR7c] an offset below 0 is
/// SQLSTATE 2201X, a fetch first count below 1 2201W, either null as well.
#[test]
fn a_count_not_a_row_count_is_refused_7_17() {
    let refused = |offset: Option<Expr>, count: Option<Expr>| kept_of(&Limit { offset, fetch: count.map(|c| fetch(c, false)) }, &upto(3));
    assert_eq!(refused(Some(int(-1)), None), Err("2201X".to_owned()));
    assert_eq!(refused(None, Some(int(0))), Err("2201W".to_owned()));
    assert_eq!(refused(None, Some(int(-5))), Err("2201W".to_owned()));
    assert_eq!(refused(Some(Expr::Const(Value::Null)), None), Err("2201X".to_owned()));
    assert_eq!(refused(None, Some(Expr::Const(Value::Null))), Err("2201W".to_owned()));
    assert_eq!(refused(Some(int(0)), Some(int(1))), Ok(vec![0]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR7d i] WITH TIES keeps the peers
/// of the last row its count keeps, those after it only, and no more; of
/// OFFSET, the peers of the rows passed over stay passed over.
#[test]
fn with_ties_keeps_the_peers_of_the_last_row_7_17() {
    let ties = |offset: Option<i128>, n: i128| Limit { offset: offset.map(int), fetch: Some(tied(int(n), false)) };
    let values = [1, 2, 2, 2, 3, 3, 4];
    assert_eq!(kept_of(&ties(None, 2), &values), Ok(vec![1, 2, 2, 2]));
    assert_eq!(kept_of(&ties(None, 1), &values), Ok(vec![1]));
    assert_eq!(kept_of(&ties(None, 5), &values), Ok(vec![1, 2, 2, 2, 3, 3]));
    assert_eq!(kept_of(&ties(Some(2), 1), &values), Ok(vec![2, 2]), "the peer passed over stays so");
    assert_eq!(kept_of(&ties(None, 9), &values), Ok(values.to_vec()));
    let only = Limit { offset: None, fetch: Some(fetch(int(2), false)) };
    assert_eq!(kept_of(&only, &values), Ok(vec![1, 2]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR7b ii, GR7c, NOTE 328] a
/// percentage keeps CEILING ( FFP * LOCT / 100.0E0 ) rows, LOCT the rows
/// before OFFSET; of an exact, approximate or DECFLOAT percentage; one of
/// 0, below or null SQLSTATE 2201W; past 100, every row.
#[test]
fn a_percentage_keeps_its_ceiling_of_the_rows_7_17() {
    let percent = |offset: Option<i128>, p: Value| Limit { offset: offset.map(int), fetch: Some(fetch(Expr::Const(p), true)) };
    let exact = |units: i128, scale: u32| Value::Exact(Decimal::new(units, scale));
    assert_eq!(kept_of(&percent(None, exact(25, 0)), &upto(10)), Ok(vec![0, 1, 2]), "CEILING(2.5)");
    assert_eq!(kept_of(&percent(None, exact(1, 1)), &upto(10)), Ok(vec![0]), "0.1 percent, CEILING(0.01)");
    assert_eq!(kept_of(&percent(None, Value::Approx(50.0)), &upto(10)), Ok(upto(5)));
    assert_eq!(kept_of(&percent(Some(8), exact(30, 0)), &upto(10)), Ok(vec![8, 9]), "of the rows before OFFSET");
    assert_eq!(kept_of(&percent(None, exact(500, 0)), &upto(4)), Ok(upto(4)));
    // 10.0000000000000000000000000001 percent of 100 rows, exactly; as a
    // DOUBLE PRECISION, 10.
    let decfloat = DecFloat::new(100_000_000_000_000_000_000_000_000_001, -28, 34).unwrap();
    assert_eq!(kept_of(&percent(None, Value::DecFloat(decfloat)), &upto(100)), Ok(upto(11)), "CEILING(10.000...01)");
    for refused in [exact(0, 0), exact(-5, 0), Value::Approx(-0.5), Value::Null] {
        assert_eq!(kept_of(&percent(None, refused.clone()), &upto(10)), Err("2201W".to_owned()), "{refused}");
    }
    assert_eq!(kept_of(&percent(None, exact(10, 0)), &[]), Err("2201W".to_owned()), "of no rows, CEILING(0)");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR7b ii; RULES.md D38] a
/// percentage's rows held past their space spill, and are given back in
/// order; a run that cannot be written or read back fails the query.
#[test]
fn a_percentages_rows_spill_past_their_space_7_17() {
    let limit = Limit { offset: Some(int(100)), fetch: Some(tied(int(10), true)) };
    assert_eq!(kept_in(&limit, &upto(2000), small(false)), Ok((100..300).collect()));
    assert!(kept_in(&limit, &upto(2000), small(true)).is_err(), "of runs that fail");
}

/// Verifies: [DESIGN] RULES.md R-ERR-1, an error of the input passes on
/// in its place, neither passed over nor counted, and no row past those
/// kept is read.
#[test]
fn an_error_passes_on_and_no_row_past_is_read() {
    let failing: RowIter =
        Box::new([Ok(vec![Value::Null]), Err(SqlError::new(Code::Data, "x")), Ok(vec![Value::Null]), Ok(vec![Value::Null])].into_iter());
    let got: Vec<bool> = Limited { rows: failing, skip: 1, left: Some(2), ties: None }.map(|r| r.is_ok()).collect();
    assert_eq!(got, vec![false, true, true]);
    let endless: RowIter = Box::new(std::iter::repeat_with(|| Ok(vec![Value::Null])));
    assert_eq!(Limited { rows: endless, skip: 3, left: Some(2), ties: None }.count(), 2);
    let grouped: RowIter = Box::new((0..).map(|i: i128| Ok(vec![Value::Exact(Decimal::new(i.div_euclid(3), 0))])));
    let ties = Some(Ties { keys: vec![SortKey::ascending(0)], last: None });
    assert_eq!(Limited { rows: grouped, skip: 0, left: Some(4), ties }.count(), 6, "the peers of 1, then no more read");
}
