//! Assertions of a query's rows checked from the rows a statement changed
//! [ISO/IEC 9075:1992 §7.5, §7.7, §11.34 GR2] (RULES.md D51): NOT EXISTS
//! and MAX bounded of inner joins, of a table joined to itself, raising
//! errors; NOT EXISTS and MAX bounded of a grouping, of null keys, rows
//! leaving a group, and set functions that eliminate nulls; immediate and
//! deferred, and deferred counts, greatest reached and unequal. Each answers as the same assertion checked whole, which `AND
//! 1 = 1` makes it: the same statements and commits fail, with the same
//! SQLCODE, and the same warnings are raised.

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{Rng, exec, rows};

/// Schema A: tables T and U (K INTEGER NOT NULL PRIMARY KEY, G INTEGER, N
/// INTEGER), each indexed by G; T of rows K 1 to 12, G K mod 4, N K * 3;
/// U of rows K 1 to 12, G K mod 3, N K * 2.
fn schema() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, G INTEGER, N INTEGER)
           CREATE TABLE U (K INTEGER NOT NULL PRIMARY KEY, G INTEGER, N INTEGER)",
    )
    .unwrap();
    let s = db.session("A");
    for k in 1..=12 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {}, {})", k % 4, k * 3)).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({k}, {}, {})", k % 3, k * 2)).unwrap();
    }
    s.commit().unwrap();
    s.run("CREATE INDEX T_G ON T (G)").unwrap();
    s.run("CREATE INDEX U_G ON U (G)").unwrap();
    (db, s)
}

/// The assertions of `asserted`, made in `s` and committed: checked whole
/// if `whole`, by a conjunct `1 = 1` of no form.
fn assert_all(s: &Session, asserted: &[(&str, &str, &str)], whole: bool) {
    for (name, condition, attributes) in asserted {
        let condition = if whole { format!("{condition} AND 1 = 1") } else { (*condition).to_owned() };
        s.run(&format!("CREATE ASSERTION {name} CHECK ({condition}) {attributes}")).unwrap();
    }
    s.commit().unwrap();
}

/// Two databases of `schema`, of the assertions `asserted`: the first
/// checks them from changes where their forms allow, the second whole.
fn twins(asserted: &[(&str, &str, &str)]) -> ((Database, Session), (Database, Session)) {
    let (changed, whole) = (schema(), schema());
    assert_all(&changed.1, asserted, false);
    assert_all(&whole.1, asserted, true);
    (changed, whole)
}

/// `sql`'s outcome in `a` and in `b`, which must be the same, warnings
/// among it: the outcome.
fn same(a: &Session, b: &Session, sql: &str, step: usize) -> Result<Completion, Code> {
    let (checked, whole) = (exec(a, sql), exec(b, sql));
    assert_eq!(checked, whole, "step {step}: {sql}");
    assert_eq!(a.warning(), b.warning(), "step {step}: {sql}");
    checked
}

/// The assertions of the random test: each name, condition and attributes.
const ASSERTED: [(&str, &str, &str); 16] = [
    ("PAIRED", "NOT EXISTS (SELECT * FROM T, U WHERE T.G = U.G AND T.N + U.N > 150)", ""),
    ("SELFED", "NOT EXISTS (SELECT * FROM T A1, T A2 WHERE A1.G = A2.G AND A1.K < A2.K AND A1.N = A2.N AND A1.N > 40)", ""),
    ("JOINED", "NOT EXISTS (SELECT T.K FROM T JOIN U ON T.K = U.K WHERE T.N > U.N + 60)", ""),
    ("ONCE", "NOT EXISTS (SELECT DISTINCT T.G FROM T, U WHERE T.G = U.N AND T.N > 85)", ""),
    ("CAPPED", "(SELECT MAX(T.N + U.N) FROM T, U WHERE T.K = U.K) <= 160", ""),
    ("DIVIDED", "NOT EXISTS (SELECT * FROM T, U WHERE T.G = U.G AND 100 / (U.N - 7) > 100)", ""),
    ("CROWD", "NOT EXISTS (SELECT G FROM T GROUP BY G HAVING COUNT(*) > 7)", ""),
    ("PAIRS", "NOT EXISTS (SELECT G FROM U WHERE G < 3 GROUP BY G HAVING COUNT(*) < 2)", ""),
    ("SUMMED", "NOT EXISTS (SELECT G FROM T GROUP BY G HAVING SUM(N) > 400)", ""),
    ("TOPPED", "(SELECT MAX(C) FROM (SELECT G, COUNT(*) FROM U GROUP BY G) AS X (G, C)) <= 8", ""),
    ("AVERAGED", "NOT EXISTS (SELECT G FROM T GROUP BY G HAVING AVG(N) > 70 AND COUNT(*) > 1)", ""),
    ("LATEJOIN", "NOT EXISTS (SELECT * FROM T, U WHERE T.K = U.G AND T.N < U.N - 80)", "DEFERRABLE INITIALLY DEFERRED"),
    ("LATEGROUP", "NOT EXISTS (SELECT G FROM U WHERE G >= 3 GROUP BY G HAVING SUM(N) > 300)", "DEFERRABLE INITIALLY DEFERRED"),
    ("LATECOUNT", "(SELECT COUNT(*) FROM T WHERE N > 50) <= 6", "DEFERRABLE INITIALLY DEFERRED"),
    ("LATEREACH", "(SELECT MAX(N) FROM U WHERE G < 5) >= 10", "DEFERRABLE INITIALLY DEFERRED"),
    ("LATEAPART", "(SELECT MIN(N) FROM T WHERE G = 1) <> 30", "DEFERRABLE INITIALLY DEFERRED"),
];

/// A random statement changing T or U, from `rng`.
fn change(rng: &mut Rng) -> String {
    let table = if rng.chance(50) { "T" } else { "U" };
    let (k, n) = (rng.below(40), rng.below(100));
    let g = if rng.chance(10) { "NULL".to_owned() } else { rng.below(7).to_string() };
    let value = if rng.chance(10) { "NULL".to_owned() } else { n.to_string() };
    match rng.below(10) {
        0..=3 => format!("INSERT INTO {table} VALUES ({k}, {g}, {value})"),
        4 => format!("UPDATE {table} SET N = {value} WHERE K = {k}"),
        5 => format!("UPDATE {table} SET G = {g} WHERE K BETWEEN {k} AND {}", k + rng.below(4)),
        6 => format!("UPDATE {table} SET N = N + {} WHERE G = {}", rng.below(20), rng.below(7)),
        7 => format!("DELETE FROM {table} WHERE K = {k}"),
        8 => format!("DELETE FROM {table} WHERE G = {} AND K > {k}", rng.below(7)),
        _ => format!("UPDATE {table} SET N = 7 WHERE K = {k}"),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §6.5 GR1b, §7.5, §7.7
/// GR1, §11.34 GR2, §14.3 GR4]; [DESIGN] RULES.md D15, D51, R-TEST-5:
/// random transactions of random statements under assertions of inner
/// joins, a table joined to itself, DISTINCT, MAX bounded of a join,
/// errors raised, groupings of null keys and of set functions eliminating
/// nulls, MAX bounded of a grouping, and deferred ones, checked from their
/// changes, answer as the same assertions checked whole: each statement
/// and each commit the same outcome, its SQLCODE and warning, at
/// SERIALIZABLE and at READ COMMITTED.
#[test]
fn queries_checked_from_changes_answer_as_checked_whole() {
    let ((changed, a), (whole, b)) = twins(&ASSERTED);
    let mut rng = Rng::new(0x5EED_D051_0000_0003);
    let (mut refused, mut done, mut rolled_back) = (0, 0, 0);
    let mut step = 0;
    for _ in 0..300 {
        if rng.chance(30) {
            same(&a, &b, "SET TRANSACTION ISOLATION LEVEL READ COMMITTED", step).unwrap();
        }
        for _ in 0..=rng.below(3) {
            step += 1;
            let sql = change(&mut rng);
            match same(&a, &b, &sql, step) {
                Ok(_) => done += 1,
                Err(_) => refused += 1,
            }
        }
        step += 1;
        if rng.chance(15) {
            let _ = same(&a, &b, "SET CONSTRAINTS ALL IMMEDIATE", step);
        }
        if same(&a, &b, "COMMIT", step).is_err() {
            rolled_back += 1;
            a.rollback().unwrap();
            b.rollback().unwrap();
        }
    }
    assert!(refused > 60 && done > 200 && rolled_back > 5, "{refused} refused, {done} done, {rolled_back} rolled back");
    for table in ["T", "U"] {
        let sql = format!("SELECT K, G, N FROM {table} ORDER BY K");
        assert_eq!(rows(&a, &sql), rows(&b, &sql), "{table}");
    }
    drop((changed, whole));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5, §11.34 GR2]; [DESIGN]
/// RULES.md D51: an assertion of a join is broken by a row of either
/// table, inserted or updated, that joins one of the other, a table joined
/// to itself by a row of it alone; not by a row that joins none, nor by a
/// row deleted.
#[test]
fn a_join_is_broken_by_a_row_of_either_table_7_5() {
    let (_db, s) = schema();
    s.run("CREATE ASSERTION PAIRED CHECK (NOT EXISTS (SELECT * FROM T, U WHERE T.G = U.G AND T.N + U.N > 150))").unwrap();
    s.run("CREATE ASSERTION SELFED CHECK (NOT EXISTS (SELECT * FROM T A1, T A2 WHERE A1.G = A2.G AND A1.K < A2.K AND A1.N = A2.N))")
        .unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (20, 1, 140)"), Err(Code::Constraint), "joins U's G 1");
    assert_eq!(exec(&s, "INSERT INTO U VALUES (20, 2, 130)"), Err(Code::Constraint), "joins T's G 2");
    assert_eq!(exec(&s, "UPDATE U SET N = 140 WHERE K = 3"), Err(Code::Constraint), "updated");
    assert_eq!(exec(&s, "INSERT INTO T VALUES (21, 0, 12)"), Err(Code::Constraint), "joins itself: K 4's N");
    s.run("INSERT INTO T VALUES (20, 9, 140)").unwrap();
    s.run("INSERT INTO T VALUES (21, 1, 13)").unwrap();
    s.run("DELETE FROM U WHERE K < 4").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM U").unwrap(), ["9"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §3.1.3h, §7.7 GR1, §7.8,
/// §11.34 GR2]; [DESIGN] RULES.md D51: an assertion of a grouping is
/// broken by a row that makes its group's HAVING true, entering it, or
/// leaving it, deleted or updated to another; rows of a null key are one
/// group; a group no row is in any more is none.
#[test]
fn a_grouping_is_broken_by_a_group_changed_7_7() {
    let (_db, s) = schema();
    s.run("CREATE ASSERTION CROWD CHECK (NOT EXISTS (SELECT G FROM T GROUP BY G HAVING COUNT(*) > 3))").unwrap();
    s.run("CREATE ASSERTION PAIRS CHECK (NOT EXISTS (SELECT G FROM U WHERE G < 3 GROUP BY G HAVING COUNT(*) < 3))").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (20, 1, 0)"), Err(Code::Constraint), "a fourth of G 1");
    s.run("INSERT INTO T VALUES (20, NULL, 0), (21, NULL, 0), (22, NULL, 0)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (23, NULL, 0)"), Err(Code::Constraint), "a fourth of the nulls");
    s.run("DELETE FROM U WHERE K = 3").unwrap();
    assert_eq!(exec(&s, "DELETE FROM U WHERE K = 6"), Err(Code::Constraint), "two left of G 0");
    s.run("UPDATE U SET G = 5 WHERE K = 1").unwrap();
    assert_eq!(exec(&s, "UPDATE U SET G = 5 WHERE K = 4"), Err(Code::Constraint), "two left of G 1");
    s.run("UPDATE U SET G = 5 WHERE G = 1").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM U WHERE G = 1").unwrap(), ["0"], "a group no more");
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, §11.34 GR2];
/// [DESIGN] RULES.md D51: of a grouping whose set function's argument may
/// be null, the warning of a null eliminated is raised as the whole check
/// raises it, of a group changed or of one not.
#[test]
fn a_grouping_warns_as_checked_whole_6_5() {
    let asserted = [("SUMMED", "NOT EXISTS (SELECT G FROM T GROUP BY G HAVING SUM(N) > 400)", "")];
    let ((_x, a), (_y, b)) = twins(&asserted);
    for (step, sql) in [
        "INSERT INTO T VALUES (20, 1, 5)",
        "INSERT INTO T VALUES (21, 2, NULL)",
        "UPDATE T SET N = 6 WHERE K = 20",
        "COMMIT",
        "UPDATE T SET N = 6 WHERE K = 1",
        "INSERT INTO T VALUES (22, 2, 300)",
        "UPDATE T SET N = NULL WHERE K = 22",
    ]
    .into_iter()
    .enumerate()
    {
        let _ = same(&a, &b, sql, step);
    }
    assert_eq!(exec(&a, "INSERT INTO T VALUES (23, 3, 1)"), exec(&b, "INSERT INTO T VALUES (23, 3, 1)"));
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "a null in group 2, not changed");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §14.3 GR4];
/// [DESIGN] RULES.md D51: a deferred assertion of a join, or of a
/// grouping, is checked at commit, or as it is made immediate, from every
/// row written while it was deferred: of a grouping, a row deleted or
/// moved to another group too.
#[test]
fn a_deferred_query_is_checked_from_the_rows_written_14_3() {
    let (_db, s) = schema();
    s.run("CREATE ASSERTION LATEJOIN CHECK (NOT EXISTS (SELECT * FROM T, U WHERE T.K = U.G AND T.N > 100)) DEFERRABLE INITIALLY DEFERRED")
        .unwrap();
    s.run("CREATE ASSERTION PAIRS CHECK (NOT EXISTS (SELECT G FROM U WHERE G < 3 GROUP BY G HAVING COUNT(*) < 3)) DEFERRABLE INITIALLY DEFERRED")
        .unwrap();
    s.commit().unwrap();
    s.run("UPDATE T SET N = 200 WHERE K = 1").unwrap();
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::CommitFailed), "T 1 joins U's G 1");
    s.run("UPDATE T SET N = 200 WHERE K = 1").unwrap();
    s.run("UPDATE U SET G = 7 WHERE G = 1").unwrap();
    s.commit().unwrap();
    s.run("DELETE FROM U WHERE K = 3").unwrap();
    s.run("DELETE FROM U WHERE K = 6").unwrap();
    assert_eq!(exec(&s, "SET CONSTRAINTS PAIRS IMMEDIATE"), Err(Code::Constraint), "two left of G 0");
    s.run("INSERT INTO U VALUES (30, 0, 0)").unwrap();
    s.run("UPDATE U SET G = 0 WHERE K = 5 OR K = 8").unwrap();
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::CommitFailed), "two left of G 2, K 5 and 8 moved away");
    s.run("DELETE FROM U WHERE K = 9").unwrap();
    s.run("INSERT INTO U VALUES (31, 0, 0)").unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28, §11.34 GR2]; [DESIGN]
/// RULES.md D29, D51: rows of two transactions that join only each other
/// break an assertion of the join: the second to commit fails, whichever
/// way it is checked, at each level. Read at a snapshot of each statement,
/// a row committed before the statement is joined by it.
#[test]
fn rows_of_two_transactions_that_join_fail_one_4_28() {
    for whole in ["", " AND 1 = 1"] {
        for level in ["SERIALIZABLE", "READ COMMITTED"] {
            let (db, s) = schema();
            s.run(&format!("CREATE ASSERTION PAIRED CHECK (NOT EXISTS (SELECT * FROM T, U WHERE T.G = U.G AND T.N + U.N > 150){whole})"))
                .unwrap();
            s.commit().unwrap();
            let other = db.session("A");
            for session in [&s, &other] {
                exec(session, &format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
            }
            s.run("INSERT INTO T VALUES (20, 9, 100)").unwrap();
            other.run("INSERT INTO U VALUES (20, 9, 100)").unwrap();
            other.commit().unwrap();
            assert!(s.commit().is_err(), "{level}{whole}: U's row joins its own");
            s.rollback().unwrap();
            exec(&s, &format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
            let seen = exec(&s, "INSERT INTO T VALUES (21, 9, 100)");
            assert_eq!(seen, Err(Code::Constraint), "{level}{whole}: U's row committed");
            s.rollback().unwrap();
        }
    }
}

/// Verifies: [DESIGN] RULES.md D38, D51: a change of more rows than
/// MEMORY.WORK_BYTES holds, of a join's table or of a grouping's, is
/// checked whole: the same outcome.
#[test]
fn a_change_past_the_work_area_is_checked_whole() {
    let asserted = [
        ("PAIRED", "NOT EXISTS (SELECT * FROM T, U WHERE T.G = U.G AND T.N + U.N > 5000)", ""),
        ("CROWD", "NOT EXISTS (SELECT G FROM T GROUP BY G HAVING COUNT(*) > 2000)", ""),
    ];
    let ((_x, a), (_y, b)) = twins(&asserted);
    same(&a, &b, "SET MEMORY.WORK_BYTES = 65536", 0).unwrap();
    let many = |from: usize, n: usize| (from..from + n).map(|k| format!("({k}, {}, 1)", k % 3)).collect::<Vec<_>>().join(", ");
    assert!(same(&a, &b, &format!("INSERT INTO T VALUES {}", many(100, 3000)), 1).is_ok());
    assert_eq!(same(&a, &b, &format!("INSERT INTO T VALUES {}", many(4000, 3000)), 2), Err(Code::Constraint), "a group past 2000");
    assert_eq!(same(&a, &b, "UPDATE T SET N = 5000 WHERE K > 100", 3), Err(Code::Constraint), "a pair past 5000");
    same(&a, &b, "COMMIT", 4).unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.4, §11.9 AR1, §11.34 AR2];
/// [DESIGN] RULES.md D51: an assertion reading a derived table needs
/// REFERENCES on the tables its query reads, not on the derived table,
/// whose name is no `<table name>`.
#[test]
fn a_derived_tables_columns_need_no_references_11_9() {
    let (db, s) = schema();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let b = db.session("B");
    let derived = "CREATE ASSERTION B.TOPPED CHECK ((SELECT MAX(C) FROM (SELECT G, COUNT(*) FROM A.U GROUP BY G) AS X (G, C)) <= 8)";
    assert_eq!(exec(&b, derived), Err(Code::Privilege), "none on U");
    s.run("GRANT REFERENCES ON U TO B").unwrap();
    s.commit().unwrap();
    b.run(derived).unwrap();
    b.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §14.3 GR4];
/// [DESIGN] RULES.md D51: a deferred count is checked at commit from each
/// row as it was before its first change while deferred: a row brought
/// into the count by one statement and out by another moves it not at
/// all, so a row inserted past the bound breaks it.
#[test]
fn a_deferred_count_takes_each_row_once_14_3() {
    let (_db, s) = schema();
    s.run("CREATE ASSERTION LATECOUNT CHECK ((SELECT COUNT(*) FROM T WHERE N > 30) <= 2) DEFERRABLE INITIALLY DEFERRED").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE N > 30").unwrap(), ["2"], "at the bound");
    s.run("UPDATE T SET N = 40 WHERE K = 1").unwrap();
    s.run("UPDATE T SET N = 3 WHERE K = 1").unwrap();
    s.run("INSERT INTO T VALUES (20, 0, 50)").unwrap();
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::CommitFailed), "three past 30");
    s.run("UPDATE T SET N = 40 WHERE K = 1").unwrap();
    s.run("UPDATE T SET N = 3 WHERE K = 1").unwrap();
    s.commit().unwrap();
    s.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    s.run("DELETE FROM T WHERE K = 12").unwrap();
    s.run("INSERT INTO T VALUES (21, 0, 50)").unwrap();
    s.commit().unwrap();
}
