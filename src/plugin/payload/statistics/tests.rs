//! Unit tests: statistics written and read back, and a plug-in's answer
//! read as the planner reads statistics (RULES.md R-TEST-1).

use super::*;
use crate::value::Decimal;

/// An exact number.
fn n(x: i128) -> Value {
    Value::Exact(Decimal::new(x, 0))
}

/// Verifies: [DESIGN] RULES.md D278, D279: a table's statistics, each
/// column's bounds, histogram and frequencies, an index's, none known,
/// and each question read back as written; a question of no kind is
/// refused.
#[test]
fn statistics_read_back_as_written() {
    let f = Frequencies { most: 3.0, median: 1.0, p90: 2.0, p99: 3.0 };
    let column =
        ColumnAnswer { distinct: 4.0, nulls: 0.25, bounds: Some((n(1), n(9))), histogram: vec![n(1), n(5), n(9)], frequencies: Some(f) };
    let t = TableAnswer { rows: 20.0, width: -1.0, refresh: 7, columns: vec![column, ColumnAnswer::default()] };
    let mut out = Vec::new();
    put_table(&mut out, Some(&t)).unwrap();
    put_table(&mut out, None).unwrap();
    let i = IndexAnswer { entries: 300.0, distinct: 100.0, pages: 1.0, refresh: 9 };
    put_index(&mut out, Some(&i));
    put_index(&mut out, None);
    for a in [Asked::Table("U.T".into()), Asked::Column("U.T".into(), "A".into()), Asked::Index("IX".into())] {
        put_asked(&mut out, &a);
    }
    let mut r = Reader::new(&out, "statistics");
    let back = table(&mut r).unwrap().unwrap();
    assert_eq!(format!("{back:?}"), format!("{t:?}"));
    assert!(table(&mut r).unwrap().is_none());
    assert_eq!(index(&mut r).unwrap(), Some(i));
    assert_eq!(index(&mut r).unwrap(), None);
    assert_eq!(asked(&mut r).unwrap(), Asked::Table("U.T".into()));
    assert_eq!(asked(&mut r).unwrap(), Asked::Column("U.T".into(), "A".into()));
    assert_eq!(asked(&mut r).unwrap(), Asked::Index("IX".into()));
    assert!(asked(&mut Reader::new(&[4], "statistics")).is_err(), "no kind 4");
}

/// Verifies: [DESIGN] RULES.md D278: a plug-in's answer is the planner's
/// statistics, where a number negative is not known, an equality's share is
/// one of the distinct values, a column beyond those answered is not
/// known; and the database's statistics told a plug-in are as kept.
#[test]
fn an_answer_is_read_as_statistics() {
    let column = ColumnAnswer { distinct: 4.0, nulls: 0.0, bounds: Some((n(0), n(3))), ..ColumnAnswer::default() };
    let t = TableAnswer { rows: 100.0, width: -1.0, refresh: 0, columns: vec![column] };
    assert_eq!((t.rows(), t.width(&[0]), t.distinct(0), t.nulls(0)), (Some(100.0_f64), None, Some(4.0_f64), Some(0.0_f64)));
    assert_eq!(t.distinct(1), None);
    let share = t.selectivity(0, CompOp::Eq, &n(2)).unwrap();
    assert!((share - 0.25_f64).abs() < 0.01_f64, "{share}");
    assert!(t.histogram(0).is_none(), "an empty histogram is none");
    let told = answer_of(&t, 2);
    assert_eq!((told.rows, told.columns.len(), told.columns[0].distinct, told.columns[1].distinct), (100.0_f64, 2, 4.0_f64, -1.0_f64));
}
