//! Unit tests of a sort's first rows taken of a grouping's partitions
//! under the bound they share (RULES.md D1338), and of groups read
//! where they lie with no state made of them (RULES.md D1337).

use std::sync::atomic::Ordering;

use super::super::super::top::Top;
use super::super::How;
use super::super::wide::Widen;
use super::{NONE, first_rows_of, table};
use crate::error::Code;
use crate::exec::Db;
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;
use crate::value::{Decimal, Value};

/// Verifies: [DESIGN] RULES.md D1338, D1241 the partitions of a grouping
/// share the bound of the first key of a sort's first rows picked by a
/// set function, of tables and of wide partitions. Ten groups took ten
/// rows each and every other group one, and the sort asks the seven of
/// the greatest counts. Told a bound of ten before any partition runs,
/// as another partition's rows would set it, the partitions keep those
/// ten groups alone, each the row it keeps under no bound, and no group
/// of one row. Told none, their own sorts raise it to ten, the seventh
/// greatest count of all, and the ten are among the rows they keep.
#[test]
fn the_partitions_share_the_bound_of_a_sorts_first_rows() {
    let (_db, a) = table();
    let sql =
        "SELECT G, COUNT(*), SUM(V), MAX(K) FROM (SELECT CASE WHEN K < 100 THEN MOD(K, 10) ELSE K END AS G, K, V FROM T) AS Q GROUP BY G";
    let ten = Value::Exact(Decimal::new(10, 0));
    let shown = |value: Option<&Value>| format!("{value:?}");
    let of_ten = |rows: &[Row]| {
        let mut tens: Vec<String> =
            rows.iter().filter(|row| shown(row.get(1)) == shown(Some(&ten))).map(|row| format!("{row:?}")).collect();
        tens.sort();
        tens
    };
    let alone = |functions: usize, db: &Db| Top::by_function((1, functions), (0, 7), (db, NO_TRANSACTION));
    let never = How { forced: NONE, widen: Widen::past(usize::MAX), ..How::default() };
    let wide = How { forced: NONE, widen: Widen::past(5).parted(8), ..How::default() };
    let (unbounded, _) = first_rows_of(&a, sql, &never, alone).unwrap();
    let tens = of_ten(&unbounded);
    assert!(tens.len() == 10 && unbounded.len() > 100, "{} of {} rows", tens.len(), unbounded.len());
    for (how, lying) in [(&never, "tables"), (&wide, "wide partitions")] {
        let (kept, top) = first_rows_of(&a, sql, how, |functions, db| alone(functions, db).bounded(&vec![ten.clone(); 7])).unwrap();
        assert_eq!((kept.len(), of_ten(&kept)), (10, tens.clone()), "told the bound, of {lying}");
        assert_eq!(shown(top.bound().as_ref()), shown(Some(&ten)), "the bound told, of {lying}");
        let (theirs, raised) = first_rows_of(&a, sql, how, |functions, db| alone(functions, db).bounded(&[])).unwrap();
        assert_eq!(of_ten(&theirs), tens, "raising the bound themselves, of {lying}");
        assert_eq!(shown(raised.bound().as_ref()), shown(Some(&ten)), "the bound raised, of {lying}");
    }
    assert!(wide.widen.merged.load(Ordering::Relaxed) > 0, "no partition went wide");
}

/// Verifies: [DESIGN] RULES.md D1337, D1256 a group read where it lies,
/// its whole state at one place of one Made, gives the result and
/// raises the error the table merged of the same Mades gives and
/// raises. Each group is one entry of five rows of one morsel or of
/// three, the two kinds turn about in each Made. By a count, a sum and
/// a greatest value that raise nothing, the first rows of wide
/// partitions are those of tables. With one group two of whose values
/// sum past the digits an exact number has, within the 128 bits a
/// total is kept in, a group the sort by the greatest key does not
/// pick, the statement raises data exception either way, as the sum
/// is finished of every group for its error.
#[test]
fn a_group_read_where_it_lies_gives_and_raises_what_the_table_merged_of_it_does() {
    const HUGE: &str = "60000000000000000000000000000000000000";
    let (db, a) = table();
    a.run("CREATE TABLE U (K INTEGER NOT NULL, X NUMERIC(38, 0) NOT NULL, Y INTEGER NOT NULL) USING columnstore").unwrap();
    let values: Vec<String> = (0_i64..4000).map(|k| format!("({k}, {}, {})", if k < 2 { HUGE } else { "1" }, k.rem_euclid(7))).collect();
    a.run(&format!("INSERT INTO U VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    let grouped = |summed: &str| {
        let keyed = "K - MOD(K, 8) + CASE WHEN MOD(K, 8) < 5 THEN 0 ELSE 5 END";
        format!("SELECT G, COUNT(*), SUM({summed}), MAX(K) FROM (SELECT {keyed} AS G, K, {summed} FROM U) AS Q GROUP BY G")
    };
    let by_greatest = |functions: usize, run: &Db| Top::by_function((1, functions), (2, 7), (run, NO_TRANSACTION));
    let never = How { forced: NONE, apart: NONE, widen: Widen::past(usize::MAX), ..How::default() };
    let wide = How { forced: NONE, apart: NONE, widen: Widen::past(5).parted(8), ..How::default() };
    let shown = |how: &How, by: usize| {
        let top = move |functions: usize, run: &Db| Top::by_function((1, functions), (by, 7), (run, NO_TRANSACTION));
        let mut rows: Vec<String> = first_rows_of(&a, &grouped("Y"), how, top).unwrap().0.iter().map(|row| format!("{row:?}")).collect();
        rows.sort();
        rows
    };
    for by in 0..3 {
        assert_eq!(shown(&wide, by), shown(&never, by), "by the set function at {by}");
    }
    for (how, lying) in [(&never, "tables"), (&wide, "wide partitions")] {
        let raised = first_rows_of(&a, &grouped("X"), how, by_greatest).map(drop).map_err(|e| e.code);
        assert_eq!(raised, Err(Code::Data), "a group not picked whose sum is past its type's precision, of {lying}");
    }
    assert!(wide.widen.merged.load(Ordering::Relaxed) > 0, "no partition went wide");
}
