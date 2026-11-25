//! Each operator carrying only the columns read above it (RULES.md
//! D1167): a column a filter alone reads, or a join's key, given null to
//! the joins above, and every column read above kept, whether a HAVING,
//! an ORDER BY, a correlated subquery, a window or a data change delta
//! table reads it.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::rows;

/// A database of a table A of 2,000 rows, a table B of 50, whose names S
/// are green for every fifth, and a table C of a weight W of each N.
fn tables() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP CREATE TABLE A (K INTEGER, V INTEGER) CREATE TABLE B (K INTEGER, S VARCHAR(40), N CHAR(2)) \
         CREATE TABLE C (N CHAR(2), W INTEGER)",
    )
    .unwrap();
    let s = db.session("APP");
    for k in 0..2000_i32 {
        s.run(&format!("INSERT INTO A VALUES ({}, {})", k % 50, k % 7)).unwrap();
    }
    for k in 0..50_i32 {
        let colour = if k % 5 == 0 { "green" } else { "blue" };
        s.run(&format!("INSERT INTO B VALUES ({k}, 'part {k} {colour}', 'N{}')", k % 3)).unwrap();
    }
    s.run("INSERT INTO C VALUES ('N0', 10), ('N1', 20), ('N2', 30)").unwrap();
    s.commit().unwrap();
    db
}

/// The rows of A, as their K and V, joined with B.
fn joined() -> impl Iterator<Item = (i64, i64)> {
    (0..2000_i64).map(|k| (k % 50, k % 7))
}

/// The sum of A.V of the rows of A joined with each B that `kept` keeps,
/// by B.N.
fn sums(kept: impl Fn(i64) -> bool) -> [i64; 3] {
    let mut sums = [0_i64; 3];
    for (b, v) in joined() {
        if kept(b)
            && let Some(sum) = usize::try_from(b % 3).ok().and_then(|n| sums.get_mut(n))
        {
            *sum += v;
        }
    }
    sums
}

/// The physical plan EXPLAIN gives `sql`.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &Params::new()).unwrap();
    plan.split("-- physical:").nth(1).unwrap_or(&plan).to_owned()
}

/// Verifies: [DESIGN] RULES.md D1167: a column its filter alone reads, or
/// a join's key, is given null to the joins above, which EXPLAIN shows,
/// and the answers are those of the rows joined; a column read above, by
/// the grouping or by an expression, is kept.
#[test]
fn a_column_no_one_above_reads_is_given_null_to_the_joins() {
    let db = tables();
    let s = db.session("APP");
    let sql = "SELECT B.N, SUM(A.V) FROM A, B WHERE A.K = B.K AND B.S LIKE '%green%' GROUP BY B.N ORDER BY B.N";
    let plan = physical(&s, sql);
    assert!(plan.contains("Project B.K, NULL, B.N"), "{plan}");
    let green = sums(|b| b % 5 == 0);
    let expected: Vec<String> = green.iter().enumerate().map(|(n, sum)| format!("N{n},{sum}")).collect();
    assert_eq!(rows(&s, sql).unwrap(), expected);

    let chained = "SELECT C.W, SUM(A.V) FROM A, B, C WHERE A.K = B.K AND B.N = C.N GROUP BY C.W ORDER BY C.W";
    let plan = physical(&s, chained);
    assert!(plan.contains("NULL"), "{plan}");
    let all = sums(|_| true);
    let weighted: Vec<String> = all.iter().zip([10, 20, 30]).map(|(sum, w)| format!("{w},{sum}")).collect();
    assert_eq!(rows(&s, chained).unwrap(), weighted);

    let read = "SELECT B.S, COUNT(*) FROM A, B WHERE A.K = B.K AND B.S LIKE '%green%' GROUP BY B.S ORDER BY B.S";
    assert_eq!(rows(&s, read).unwrap().first().map(String::as_str), Some("part 0 green,40"));
    let computed = "SELECT B.N, MAX(CHARACTER_LENGTH(B.S)) FROM A, B WHERE A.K = B.K AND B.S LIKE '%green%' GROUP BY B.N ORDER BY B.N";
    assert_eq!(rows(&s, computed).unwrap(), ["N0,13", "N1,13", "N2,13"]);
}

/// Verifies: [DESIGN] RULES.md D1167: a column read only by a HAVING, by
/// an ORDER BY, by a correlated subquery or by a window is kept, and the
/// answers are those of every column read.
#[test]
fn a_column_read_only_above_the_joins_is_kept() {
    let db = tables();
    let s = db.session("APP");
    let all = sums(|_| true);
    let having = format!("SELECT B.N FROM A, B WHERE A.K = B.K GROUP BY B.N HAVING SUM(A.V) = {} ORDER BY B.N", all[1]);
    assert_eq!(rows(&s, &having).unwrap(), ["N1"]);

    let ordered = "SELECT B.N FROM A, B WHERE A.K = B.K AND A.V = 0 AND B.K < 3 ORDER BY B.S DESC, B.N";
    let count = |b: i64| joined().filter(|&(k, v)| k == b && v == 0).count();
    let firsts: Vec<String> = (0..3_i64).rev().flat_map(|b| std::iter::repeat_n(format!("N{b}"), count(b))).collect();
    assert_eq!(rows(&s, ordered).unwrap(), firsts);

    let correlated = "SELECT B.K, (SELECT COUNT(*) FROM A WHERE A.K = B.K AND A.V < B.K) FROM B WHERE B.S LIKE '%green%' ORDER BY B.K";
    let expected: Vec<String> =
        (0..50_i64).filter(|b| b % 5 == 0).map(|b| format!("{b},{}", joined().filter(|&(k, v)| k == b && v < b).count())).collect();
    assert_eq!(rows(&s, correlated).unwrap(), expected);

    let windowed = "SELECT DISTINCT B.N, SUM(A.V) OVER (PARTITION BY B.N) FROM A, B WHERE A.K = B.K ORDER BY B.N";
    let expected: Vec<String> = all.iter().enumerate().map(|(n, sum)| format!("N{n},{sum}")).collect();
    assert_eq!(rows(&s, windowed).unwrap(), expected);
}

/// Verifies: [DESIGN] RULES.md D1167: a row changed is written whole, and
/// a data change delta table gives the columns it reads, though the
/// change's condition alone reads one of them.
#[test]
fn a_changed_row_is_written_whole_and_given_back() {
    let db = tables();
    let s = db.session("APP");
    let changed = "SELECT K, N FROM FINAL TABLE (UPDATE B SET N = 'NX' WHERE S LIKE '%green%' AND K < 12)";
    let mut given = rows(&s, changed).unwrap();
    given.sort();
    assert_eq!(given, ["0,NX", "10,NX", "5,NX"]);
    let stored = "SELECT K, S, N FROM B WHERE K IN (0, 1, 5) ORDER BY K";
    assert_eq!(rows(&s, stored).unwrap(), ["0,part 0 green,NX", "1,part 1 blue,N1", "5,part 5 green,NX"]);
    s.run("DELETE FROM B WHERE S LIKE '%blue%' AND K < 5").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*), MIN(S) FROM B").unwrap(), ["46,part 0 green"]);
}
