//! Range joins (RULES.md D580-D582): a join whose condition compares a
//! value of each table by order, a band or an overlap of periods among
//! them, planned as a sort of one table searched by each row of the other,
//! gives the rows the nested loop gives [X3.135-1986 §5.20 GR2b, §5.21
//! GR2; ISO/IEC 9075:1992 §7.5], in its order, nulls matching nothing
//! [X3.135-1986 §5.11 GR2], and a `<period predicate>` of two periods
//! judged as its comparisons [ISO/IEC 9075-2:2023 §8.20 GR4]. These are
//! `[DESIGN]` requirements on the plan; the standard governs only the rows.

mod periods;

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::{Rng, both_ways, exec, refused, rows, sorted};

/// Schema Q: A of 300 rows and B of 40, each of a compared value with
/// repeats and nulls, a key of five values with nulls, and a third value;
/// C of 6 rows keyed as A; D of 200 rows of two keys.
fn tables() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q
           CREATE TABLE A (X INTEGER, K INTEGER, V INTEGER)
           CREATE TABLE B (Y INTEGER, K INTEGER, W INTEGER)
           CREATE TABLE C (K INTEGER, Z INTEGER)
           CREATE TABLE D (Y INTEGER, K INTEGER)",
    )
    .unwrap();
    let s = db.session("Q");
    let mut rng = Rng::new(580);
    let value = |rng: &mut Rng, spread: usize| if rng.chance(5) { "NULL".to_owned() } else { rng.below(spread).to_string() };
    let a: Vec<String> = (0..300).map(|_| format!("({}, {}, {})", value(&mut rng, 400), value(&mut rng, 5), rng.below(7))).collect();
    let b: Vec<String> = (0..40).map(|_| format!("({}, {}, {})", value(&mut rng, 400), value(&mut rng, 5), rng.below(7))).collect();
    s.run(&format!("INSERT INTO A VALUES {}", a.join(", "))).unwrap();
    s.run(&format!("INSERT INTO B VALUES {}", b.join(", "))).unwrap();
    s.run("INSERT INTO C VALUES (0, 14), (1, 11), (2, 13), (3, 10), (4, 12), (NULL, 15)").unwrap();
    let d: Vec<String> = (0..200).map(|i| format!("({}, {})", i * 2, i % 2)).collect();
    s.run(&format!("INSERT INTO D VALUES {}", d.join(", "))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// Sets PLANNER.RANGE_JOINS to `word`.
fn range_joins(s: &Session, word: &str) {
    exec(s, &format!("SET PLANNER.RANGE_JOINS = '{word}'")).unwrap();
}

/// The physical plan EXPLAIN shows of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &Params::new()).unwrap();
    plan.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The rows of `sql`, sorted, as a range join gives them, by batch and row
/// by row, which must be the nested loop's, its plan a range join's as
/// `ranged` says, and the nested loop's when the setting is OFF. The join
/// of its operands swapped gives the rows in another order, as a hash
/// join built of the other operand does.
fn agree(s: &Session, sql: &str, ranged: bool) -> Vec<String> {
    let plan = physical(s, sql);
    assert_eq!(plan.contains("RangeJoin"), ranged, "{sql}:\n{plan}");
    let (batched, by_row) = both_ways(s, sql);
    assert_eq!(batched, by_row, "{sql}: by batch and by row");
    range_joins(s, "OFF");
    assert!(!physical(s, sql).contains("RangeJoin"), "{sql}");
    let nested = sorted(s, sql);
    range_joins(s, "ON");
    assert_eq!(by_row, nested, "{sql}: the range join and the nested loop");
    by_row
}

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.21 GR2, §5.11 GR2]; [DESIGN]
/// RULES.md D580: bands closed and open, one-sided comparisons, bounds
/// of computed values, a band within equal keys and a band with more
/// conditions give the nested loop's rows in its order, nulls matching
/// nothing; EXPLAIN shows the range join and what it is sorted on.
#[test]
fn bands_give_the_nested_loops_rows_in_its_order() {
    let db = tables();
    let s = db.session("Q");
    let select = "SELECT A.X, A.K, A.V, B.Y, B.K, B.W FROM A, B WHERE";
    for cond in [
        "A.X BETWEEN B.Y - 5 AND B.Y + 5",
        "A.X > B.Y - 5 AND A.X < B.Y + 5",
        "A.X >= B.Y AND A.X < B.Y + 3",
        "B.Y <= A.X AND B.Y + 2 > A.X",
        "A.X < B.Y",
        "A.X >= B.Y",
        "A.X BETWEEN B.Y AND B.Y",
        "A.X BETWEEN B.Y - 9 AND B.Y + 9 AND A.V <> B.W AND A.V < 5",
        "A.X - A.V BETWEEN B.Y - B.W AND B.Y + B.W",
        "A.X >= B.Y AND A.X <= B.Y + 10 AND A.X > B.W * 30",
    ] {
        let sql = format!("{select} {cond}");
        assert!(!agree(&s, &sql, true).is_empty(), "{sql}");
    }
    let plan = physical(&s, &format!("{select} A.X BETWEEN B.Y - 5 AND B.Y + 5"));
    assert!(plan.contains("RangeJoin A.X >= (B.Y - 5) AND A.X <= (B.Y + 5)  (sorted on "), "{plan}");
    let keyed = "SELECT * FROM A, D WHERE A.K = D.K AND A.X BETWEEN D.Y - 20 AND D.Y + 20";
    assert!(physical(&s, keyed).contains(" within "), "{}", physical(&s, keyed));
    assert_ne!(agree(&s, keyed, true), Vec::<String>::new());
}

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.21 GR2]; [DESIGN] RULES.md
/// D580: of a range join sorting its right operand, each left row's
/// matches come in the right's order, ties of the sorted value among
/// them, as the nested loop gives them; a right table with no row, or no
/// row a bound finds, gives none.
#[test]
fn matches_come_in_the_right_operands_order() {
    let db = tables();
    let s = db.session("Q");
    let sql = "SELECT A.X, C.K, C.Z FROM A, C WHERE A.X >= C.Z * 20";
    let plan = physical(&s, sql);
    assert!(plan.contains("sorted on (C.Z * 20)"), "{plan}");
    let joined = rows(&s, sql).unwrap();
    range_joins(&s, "OFF");
    assert_eq!(rows(&s, sql).unwrap(), joined, "{sql}");
    range_joins(&s, "ON");
    assert!(joined.len() > 30, "{}", joined.len());
    assert_eq!(agree(&s, "SELECT * FROM A, B WHERE A.X BETWEEN B.Y + 1000 AND B.Y + 2000", true), Vec::<String>::new());
    exec(&s, "DELETE FROM B").unwrap();
    assert_eq!(rows(&s, "SELECT * FROM A, B WHERE A.X BETWEEN B.Y - 5 AND B.Y + 5").unwrap(), Vec::<String>::new());
    s.rollback().unwrap();
}

/// Verifies: [ISO/IEC 9075:1992 §7.5]; [DESIGN] RULES.md D580: a LEFT or
/// RIGHT outer join on a band keeps each preserved row that matches none
/// with nulls, a row of a null value among them, as the nested loop
/// does; a FULL one stays as it is.
#[test]
fn outer_joins_keep_their_unmatched_rows() {
    let db = tables();
    let s = db.session("Q");
    for sql in [
        "SELECT * FROM A LEFT JOIN B ON A.X BETWEEN B.Y - 5 AND B.Y + 5",
        "SELECT * FROM B RIGHT JOIN A ON A.X BETWEEN B.Y - 5 AND B.Y + 5",
        "SELECT * FROM A LEFT JOIN D ON A.X < D.Y - 300 AND A.K = D.K",
    ] {
        let joined = agree(&s, sql, true);
        assert!(joined.iter().any(|r| r.contains("NULL,NULL")), "{sql}");
    }
    let full = "SELECT * FROM A FULL JOIN B ON A.X BETWEEN B.Y - 5 AND B.Y + 5";
    assert!(!physical(&s, full).contains("RangeJoin"));
}

/// Verifies: [DESIGN] RULES.md D580, D303: a band among the joins of
/// three tables is a range join where the enumeration finds it cheapest,
/// and the rows are those of the greedy order and of the nested loop.
#[test]
fn enumeration_costs_range_joins() {
    let db = tables();
    let s = db.session("Q");
    let sql = "SELECT A.X, B.Y, C.Z FROM A, B, C WHERE A.X BETWEEN B.Y - 5 AND B.Y + 5 AND C.K = A.K";
    let plan = physical(&s, sql);
    assert!(plan.contains("RangeJoin") && plan.contains("join order by"), "{plan}");
    let mut joined = agree(&s, sql, true);
    joined.sort();
    exec(&s, "SET PLANNER.JOIN_ENUMERATION = 'GREEDY'").unwrap();
    assert_eq!(sorted(&s, sql), joined);
}

/// Verifies: [X3.135-1986 §5.20 GR2b]; [ISO/IEC 9075:1992 §7.5]; [DESIGN]
/// RULES.md D580, D15, D38: a condition that can raise reading both
/// tables keeps the nested loop, which raises as before; a right operand
/// past the work area is searched a part at a time, the left read again
/// for each, giving the nested loop's rows, of a LEFT join its unmatched
/// rows once; and the rows of an ORDER BY over a range join come in its
/// order.
#[test]
fn raising_conditions_keep_the_nested_loop_and_large_operands_are_parted() {
    let db = tables();
    let s = db.session("Q");
    let raising = "SELECT * FROM A, B WHERE A.X < B.Y AND A.V / (B.W - B.W) > 0";
    assert!(!physical(&s, raising).contains("RangeJoin"));
    assert_eq!(refused(&s, raising).1, "22012");
    let more: Vec<String> = (0..500).map(|i| format!("({}, {}, {})", i % 400, i % 5, i % 7)).collect();
    s.run(&format!("INSERT INTO B VALUES {}", more.join(", "))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    exec(&s, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    let joined = agree(&s, "SELECT * FROM B, A WHERE A.X BETWEEN B.Y - 1 AND B.Y + 1 AND A.K = B.K AND A.V < 2", true);
    assert_ne!(joined, Vec::<String>::new());
    let outer = agree(&s, "SELECT * FROM A LEFT JOIN B ON A.X BETWEEN B.Y - 1 AND B.Y + 1 AND B.W < 3", true);
    assert!(outer.iter().any(|r| r.ends_with("NULL,NULL,NULL")), "a LEFT join's unmatched rows");
    let ordered = "SELECT A.X, B.Y, B.W FROM A, B WHERE A.X BETWEEN B.Y - 1 AND B.Y + 1 ORDER BY A.X, B.Y, B.W";
    assert!(physical(&s, ordered).contains("RangeJoin"));
    let in_order = rows(&s, ordered).unwrap();
    assert!(in_order.len() > 100, "{}", in_order.len());
    range_joins(&s, "OFF");
    assert_eq!(rows(&s, ordered).unwrap(), in_order);
    range_joins(&s, "ON");
}
