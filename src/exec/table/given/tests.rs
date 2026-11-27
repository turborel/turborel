//! Unit tests of a table's groups given as a batch (RULES.md D1123): each
//! column holds, of each group, the value the group's finished row holds,
//! in vectors of the lanes' types, and the groups' order is told by their
//! first rows.

use super::super::{GroupTable, Key, Seed, hash_values};
use crate::functions::Registry;
use crate::logical::{AggCall, Expr};
use crate::storage::Column;
use crate::value::codec::put_value;
use crate::value::datetime::Moment;
use crate::value::{DataType, Datetime, Decimal, Spelled, Value};

/// The type of an INTEGER column.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// The exact number of `units` at `scale`.
fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// A timestamp of `utc`, displaced by `zone` minutes.
fn instant(utc: i64, zone: i16) -> Value {
    Value::Datetime(Datetime { moment: Moment::Timestamp, utc, zone, zoned: true, precision: 0 })
}

/// A call of the built-in set function `name` of an INTEGER column, or
/// of rows (COUNT(*)) if not `of_column`.
fn call(name: &str, of_column: bool) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let arg = of_column.then_some(INTEGER);
    let result = func.0.result_type(arg.clone()).unwrap();
    AggCall { func, arg: arg.map(|_| Expr::Column(0)), distinct: false, result, filter: None, distribution: None, label: name.to_owned() }
}

/// The keys of three groups, a column of each type a lane holds: integers,
/// texts, approximate numbers, datetimes, truth values, values of two
/// types, and nulls alone; the second group's null in every typed lane.
fn keys() -> Vec<Vec<Value>> {
    let text = |s: &str| Value::Char(s.into());
    vec![
        vec![exact(1, 0), text("ab  "), Value::Approx(-0.0), instant(60_000_000, 120), Value::Bool(true), exact(5, 0), Value::Null],
        vec![Value::Null, Value::Null, Value::Null, Value::Null, Value::Null, text("x"), Value::Null],
        vec![exact(-3, 0), text(""), Value::Approx(2.5), instant(0, 0), Value::Bool(false), exact(150, 2), Value::Null],
    ]
}

/// A table of the groups of `keys`, each computing COUNT(*) and SUM, the
/// `n`th group's first row `firsts[n]`, its count `n + 2` and its sum 7
/// times that.
fn table(firsts: &[u64]) -> GroupTable {
    let mut table = GroupTable::new(7, &[call("COUNT", false), call("SUM", true)], &Seed::fixed(0x5EED), usize::MAX);
    for ((n, key), &first) in (2_i128..).zip(&keys()).zip(firsts) {
        let g = table.found_or_made(hash_values(table.seed(), key), Key::Values(key), first);
        let mut total = Vec::new();
        put_value(&mut total, &exact(n.saturating_mul(7), 0));
        table.merge_written(g, (first, &[n.to_le_bytes().to_vec(), total])).unwrap();
    }
    table
}

/// Verifies: [DESIGN] (RULES.md D1123) a table's groups given as a batch
/// hold, of each group and column, the value the group's finished row
/// holds, a null where it holds one, a text with its trailing spaces, and
/// each group's first row's number beside them; integers, texts,
/// approximate numbers and counts in vectors of their types, no value
/// made of them.
#[test]
fn groups_given_as_a_batch_hold_what_their_rows_hold() {
    let expected = table(&[4, 9, 30]).finished().unwrap();
    let (batch, firsts) = table(&[4, 9, 30]).into_batch().unwrap();
    assert_eq!((batch.rows, batch.columns.len()), (3, 9));
    assert_eq!(firsts, expected.iter().map(|g| g.0).collect::<Vec<_>>());
    for (g, (_, row)) in expected.iter().enumerate() {
        for (c, value) in row.iter().enumerate() {
            assert_eq!(format!("{:?}", batch.value(c, g).unwrap()), format!("{value:?}"), "group {g}, column {c}");
        }
    }
    let kinds: Vec<&str> = batch
        .columns
        .iter()
        .map(|column| match column {
            Column::Ints(_) => "ints",
            Column::Texts(_) => "texts",
            Column::Floats(_) => "floats",
            Column::Values(_) => "values",
            Column::Nulls(_) => "nulls",
            Column::Datetimes(_) | Column::Bools(_) | Column::Late(_) => "other",
        })
        .collect();
    assert_eq!(kinds, ["ints", "texts", "floats", "values", "values", "values", "nulls", "ints", "values"]);
}

/// Verifies: [DESIGN] (RULES.md D1123) a table tells whether its groups
/// are in the order of their first rows: those made in that order are,
/// one made before a group of an earlier first row is not.
#[test]
fn a_table_tells_whether_its_groups_are_in_first_row_order() {
    assert!(table(&[4, 9, 30]).in_order());
    assert!(table(&[4, 4, 4]).in_order(), "equal numbers are in order");
    assert!(!table(&[4, 30, 9]).in_order());
    assert!(GroupTable::new(1, &[], &Seed::fixed(1), usize::MAX).in_order(), "no group is in order");
}

/// Verifies: [DESIGN] (RULES.md D1256) one set function's results given
/// as a column hold what the groups' finished rows hold of it, a group's
/// row made alone is its finished row, the first rows' numbers are told,
/// and finishing the other set functions for their errors raises none
/// where none fails; a set function the table does not compute is
/// refused (SQLCODE -910).
#[test]
fn one_result_column_and_one_row_are_the_finished_rows() {
    let table = table(&[4, 9, 30]);
    let expected = table.finished().unwrap();
    for agg in 0..2 {
        let batch = crate::storage::Batch { columns: vec![table.results_of(agg).unwrap()], rows: 3 };
        for (g, (_, row)) in expected.iter().enumerate() {
            assert_eq!(format!("{:?}", batch.value(0, g).unwrap()), format!("{:?}", &row[agg + 7]), "group {g}, set function {agg}");
        }
    }
    assert!(matches!(table.results_of(0).unwrap(), Column::Ints(_)), "counts stay integers");
    for (g, (first, row)) in expected.iter().enumerate() {
        assert_eq!(format!("{:?}", table.row(g).unwrap()), format!("{row:?}"));
        assert_eq!(table.firsts().get(g), Some(first));
    }
    assert!(table.checked_but(&[0]).is_ok() && table.checked_but(&[]).is_ok());
    assert_eq!(table.results_of(2).unwrap_err().sqlcode(), -910_i32);
}
