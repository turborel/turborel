//! Unit tests of a table as it was before a statement: its rows found by
//! their values through lookups made once, as comparing each would find
//! them, and the rows read for it. Split from db/integrity/before.rs
//! (RULES.md R-SIZE-2).

use super::*;
use crate::db::Database;
use crate::value::{Decimal, Spelled};

/// Rows of the referenced table, of one key on A.
const ROWS: usize = 200;
/// Rows of Q as it was in `before`: `ROWS`, and two with a null.
const ALL: usize = 202;

fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// Q (A, B) as it was: `ROWS` rows (1, B), ids B; then (1, NULL) and
/// (NULL, 7).
fn before() -> Before {
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    let space = ListSpace::default();
    let mut rows = HeldRows::new(space.clone());
    for b in 0..ROWS {
        rows.push((u64::try_from(b).unwrap(), vec![int(1), int(i64::try_from(b).unwrap())])).unwrap();
    }
    rows.push((1000_u64, vec![int(1), Value::Null])).unwrap();
    rows.push((1001_u64, vec![Value::Null, int(7)])).unwrap();
    let name = QualifiedName { auth: "A".to_owned(), ident: "Q".to_owned() };
    Before { table: name, rows, types: vec![integer, integer], lookups: Vec::new(), map: SpillMap::new(space.clone()), places: NONE, space }
}

/// The rows of `before` that `values` matched, sorted, and the rows of it
/// read to find them.
fn matched(before: &mut Before, values: &[Value]) -> (Vec<RowId>, usize) {
    let was = READS.with(std::cell::Cell::get);
    let mut found: Vec<RowId> = before.matched(&[0, 1], values).unwrap().into_iter().map(Result::unwrap).collect();
    found.sort_unstable();
    (found, READS.with(std::cell::Cell::get).checked_sub(was).unwrap())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR3b i, GR4]; [DESIGN]
/// RULES.md D38: the rows of a table as it was that referencing values
/// matched, those not null equal, are found through a lookup of the
/// columns they are not null in, made by one pass over its rows the first
/// time and read no more after; a value not exactly of its column's type
/// (an approximate number for an INTEGER) compares with each row, finding
/// the same; values all null match none.
#[test]
fn lookups_find_what_comparing_each_finds() {
    let mut q = before();
    assert_eq!(matched(&mut q, &[int(1), int(5)]), (vec![5_u64], ALL), "made: one pass");
    assert_eq!(matched(&mut q, &[int(1), int(6)]), (vec![6_u64], 0_usize), "made before");
    assert_eq!(matched(&mut q, &[int(2), int(6)]), (vec![], 0_usize), "no such key");
    let (on_a, reads) = matched(&mut q, &[int(1), Value::Null]);
    assert_eq!((on_a.len(), on_a.last()), (ROWS.checked_add(1).unwrap(), Some(&1000_u64)), "a lookup of A alone");
    assert_eq!(reads, ALL);
    assert_eq!(matched(&mut q, &[Value::Null, int(7)]), (vec![7_u64, 1001_u64], ALL));
    assert_eq!(matched(&mut q, &[int(1), Value::Approx(5.0_f64)]), (vec![5_u64], ALL), "compared");
    assert_eq!(matched(&mut q, &[int(1), Value::Approx(5.5_f64)]), (vec![], ALL));
    assert_eq!(matched(&mut q, &[Value::Null, Value::Null]), (vec![], 0_usize), "none");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR3b i, GR4]; [DESIGN]
/// RULES.md D38: a table as it was with a value not exactly of its
/// column's type (an approximate number in an INTEGER column) makes no
/// lookup of that column: each row is compared, each time, finding what
/// equals.
#[test]
fn a_value_not_of_its_type_makes_no_lookup() {
    let mut q = before();
    q.rows.push((2000_u64, vec![int(1), Value::Approx(3.0_f64)])).unwrap();
    let all = ALL.checked_add(1).unwrap();
    assert_eq!(matched(&mut q, &[int(1), int(3)]), (vec![3_u64, 2000_u64], all.checked_mul(2).unwrap()), "a pass, then compared");
    assert_eq!(matched(&mut q, &[int(1), int(4)]), (vec![4_u64], all), "compared again");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR3b, GR4, GR5b];
/// [DESIGN] RULES.md D38. Regression test (R-TEST-7): MATCH PARTIAL read
/// the referenced table as it was once for each matching row of each row
/// deleted, its rows times theirs. Every row of Q deleted, each of
/// `ROWS` rows of R matching one and three rows (1, NULL) matching all,
/// cascading, reads Q as it was a few times over, not once a row; R ends
/// with its row of nulls.
#[test]
fn match_partial_reads_the_table_before_a_few_times() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE Q (A INTEGER, B INTEGER, UNIQUE (A, B))
           CREATE TABLE R (A INTEGER, B INTEGER,
             FOREIGN KEY (A, B) REFERENCES Q (A, B) MATCH PARTIAL ON DELETE CASCADE)",
    )
    .unwrap();
    let s = db.session("A");
    let referenced: Vec<String> = (0..ROWS).map(|b| format!("(1, {b})")).collect();
    s.run(&format!("INSERT INTO Q VALUES {}", referenced.join(", "))).unwrap();
    s.run(&format!("INSERT INTO R VALUES (1, NULL), (1, NULL), (1, NULL), (NULL, NULL), {}", referenced.join(", "))).unwrap();
    let was = READS.with(std::cell::Cell::get);
    s.run("DELETE FROM Q").unwrap();
    let reads = READS.with(std::cell::Cell::get).checked_sub(was).unwrap();
    assert!(reads <= ROWS.checked_mul(3).unwrap(), "{reads} rows read of Q as it was");
    let mut left = s.open_cursor("SELECT A, B FROM R").unwrap();
    assert_eq!(format!("{:?}", left.fetch().unwrap()), format!("{:?}", Some(vec![Value::Null, Value::Null])));
    assert!(left.fetch().unwrap().is_none(), "R's other rows deleted");
}
