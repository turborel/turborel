//! Assertions checked from the rows a statement changed, of the forms
//! beyond `NOT EXISTS` and COUNT(*) [ISO/IEC 9075:1992 §6.5, §11.34 GR2]
//! (RULES.md D51): COUNT, COUNT DISTINCT, MAX and MIN, equal or unequal
//! too, of arguments null or never null, a view's projection, and deferred
//! forms of no row passing from the rows its transaction wrote. Each answers as the same assertion checked whole, which `AND 1 =
//! 1` makes it: the same statements fail, with the same SQLCODE, and the
//! same warnings are raised.

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{Rng, exec, rows};

/// Schema A: table T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER, M INTEGER
/// NOT NULL, S VARCHAR(6)) of rows K 1 to 10, N K, M K * 5; table U (K
/// INTEGER, R INTEGER REFERENCES T ON DELETE CASCADE) of rows K 1 to 10, R
/// K; view W (K, D, M), D N * 2, of T's rows K < 35, not merged into a
/// query naming it; and view V (K, N, M) of T's rows K < 20, merged.
fn schema() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER, M INTEGER NOT NULL, S VARCHAR(6))
           CREATE TABLE U (K INTEGER, R INTEGER REFERENCES T ON DELETE CASCADE)
           CREATE VIEW W (K, D, M) AS SELECT K, N * 2, M FROM T WHERE K < 35
           CREATE VIEW V AS SELECT K, N, M FROM T WHERE K < 20",
    )
    .unwrap();
    let s = db.session("A");
    for k in 1..=10 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {k}, {}, 'S')", k * 5)).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({k}, {k})")).unwrap();
    }
    s.commit().unwrap();
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
const ASSERTED: [(&str, &str, &str); 19] = [
    ("COUNTED", "(SELECT COUNT(M) FROM T WHERE N > 20) <= 9", ""),
    ("REJECTED", "(SELECT COUNT(N) FROM T WHERE N > 30) < 7", ""),
    ("CEILING", "(SELECT MAX(M) FROM T WHERE K < 30) < 95", ""),
    ("FLOOR", "(SELECT MIN(N - M) FROM T WHERE N IS NOT NULL) >= -60", ""),
    ("REACHED", "(SELECT MAX(M) FROM T WHERE K < 12) > 20", ""),
    ("LOWEST", "16 > (SELECT MIN(DISTINCT M) FROM T WHERE K > 2 AND K < 15)", ""),
    ("VIEWED", "NOT EXISTS (SELECT * FROM W WHERE D > 90 AND M < 10)", ""),
    ("VIEWMAX", "(SELECT MAX(D) FROM W WHERE D > 0) < 140", ""),
    ("DIVIDED", "NOT EXISTS (SELECT * FROM V WHERE 50 / (M - 11) > 30)", ""),
    ("RAISED", "(SELECT MAX(100 / (N - 13)) FROM T WHERE N > 0) < 60", ""),
    ("LATER", "NOT EXISTS (SELECT * FROM T WHERE N > M + 15)", "DEFERRABLE INITIALLY DEFERRED"),
    ("PEAK", "(SELECT MIN(M) FROM T WHERE K < 4) = 5", ""),
    ("APART", "(SELECT MAX(N) FROM T WHERE K < 25 AND N < 13) <> 11", ""),
    ("KINDS", "(SELECT COUNT(DISTINCT N) FROM T WHERE N < 9) <= 8", ""),
    ("SPREAD", "(SELECT COUNT(DISTINCT M) FROM T WHERE M < 30) >= 4", ""),
    ("LEAST", "(SELECT MIN(N) FROM T) >= 0", ""),
    ("NULLS", "(SELECT COUNT(N) FROM T WHERE K > 5) < 30", ""),
    (
        "PAIRED",
        "NOT EXISTS (SELECT * FROM T WHERE N > 45 AND M < 5) AND (SELECT MAX(M) FROM T WHERE K > 35) < 95",
        "DEFERRABLE INITIALLY DEFERRED",
    ),
    // Checked whole: a SUM.
    ("TOTAL", "(SELECT SUM(M) FROM T) < 3000", ""),
];

/// A random statement changing T or U, from `rng`.
fn change(rng: &mut Rng) -> String {
    let (k, n, m) = (rng.below(40), rng.below(50), rng.below(100));
    let value = if rng.chance(15) { "NULL".to_owned() } else { n.to_string() };
    match rng.below(10) {
        0..=2 => format!("INSERT INTO T VALUES ({k}, {value}, {m}, 'S')"),
        3 => format!("UPDATE T SET N = {value} WHERE K = {k}"),
        4 => format!("UPDATE T SET M = {m} WHERE K BETWEEN {k} AND {}", k + rng.below(6)),
        5 => format!("UPDATE T SET N = N + {} WHERE K BETWEEN {k} AND {}", rng.below(9), k + rng.below(6)),
        6 => format!("DELETE FROM T WHERE K = {k} OR M = {m}"),
        7 => format!("INSERT INTO U VALUES ({k}, {})", rng.below(12) + 1),
        8 => format!("UPDATE T SET M = M + {} WHERE K = {k}", rng.below(60)),
        _ => format!("DELETE FROM T WHERE K < {}", rng.below(14)),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §6.5 GR1b, GR2b iii,
/// §11.34 GR2, §14.3 GR4]; [DESIGN] RULES.md D15, D51, R-TEST-5: random
/// transactions of random statements under assertions of COUNT, COUNT
/// DISTINCT, MAX and MIN, equal and unequal too, of values null or never
/// null, through views, raising errors, deferred, and a SUM, checked from
/// their changes, answer as the same assertions
/// checked whole: each statement and each commit the same outcome, its
/// SQLCODE and warning, at SERIALIZABLE and at READ COMMITTED.
#[test]
fn forms_checked_from_changes_answer_as_checked_whole() {
    let ((changed, a), (whole, b)) = twins(&ASSERTED);
    let mut rng = Rng::new(0x5EED_D051_0000_0002);
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
        if same(&a, &b, "COMMIT", step).is_err() {
            rolled_back += 1;
            a.rollback().unwrap();
            b.rollback().unwrap();
        }
    }
    assert!(refused > 60 && done > 200 && rolled_back > 5, "{refused} refused, {done} done, {rolled_back} rolled back");
    assert_eq!(rows(&a, "SELECT K, N, M FROM T ORDER BY K"), rows(&b, "SELECT K, N, M FROM T ORDER BY K"));
    drop((changed, whole));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, GR2a, §11.34 GR2];
/// [DESIGN] RULES.md D51: COUNT of a column NOT NULL, or of one its filter
/// rejects null of, is held from the change, raising no warning; MAX of
/// one that may be null raises the warning of a null eliminated once one
/// is, from the change of a null, else checked whole, as it does.
#[test]
fn a_set_function_warns_as_checked_whole_6_5() {
    let asserted = [
        ("FEW", "(SELECT COUNT(M) FROM T WHERE M > 40) <= 3", ""),
        ("PART", "(SELECT COUNT(N) FROM T WHERE N > 8) < 3", ""),
        ("CAPPED", "(SELECT MAX(N) FROM T) < 100", ""),
    ];
    let ((_x, a), (_y, b)) = twins(&asserted);
    same(&a, &b, "INSERT INTO T VALUES (11, 5, 45, 'S')", 1).unwrap();
    assert_eq!(a.warning(), None, "no null");
    assert_eq!(same(&a, &b, "INSERT INTO T VALUES (12, 9, 46, 'S')", 2), Err(Code::Constraint), "four over 40");
    assert_eq!(same(&a, &b, "INSERT INTO T VALUES (12, 11, 5, 'S')", 3), Err(Code::Constraint), "three over 8");
    same(&a, &b, "UPDATE T SET M = 0 WHERE K = 10", 4).unwrap();
    same(&a, &b, "INSERT INTO T VALUES (12, NULL, 46, 'S')", 5).unwrap();
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "a null eliminated, checked whole");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR2b i, iii, §11.34 GR2];
/// [DESIGN] RULES.md D51: MAX below a bound and MIN above one are broken
/// by a row past it, inserted or updated, a null not counting, and hold of
/// no row, the result null; MAX above a bound holds if a row changed is
/// past it, is broken by a row put into a set that was empty, and checks a
/// change of its greatest whole.
#[test]
fn a_greatest_or_least_is_held_to_its_bound_6_5() {
    let asserted = [
        ("CEILING", "(SELECT MAX(M) FROM T) < 90", ""),
        ("FLOOR", "0 <= (SELECT MIN(N) FROM T WHERE N IS NOT NULL)", ""),
        ("REACHED", "(SELECT MAX(M) FROM T WHERE K > 20) >= 30", ""),
    ];
    let ((_x, a), (_y, b)) = twins(&asserted);
    let cases: [(&str, Option<Code>); 12] = [
        ("INSERT INTO T VALUES (11, NULL, 90, 'S')", Some(Code::Constraint)),
        ("INSERT INTO T VALUES (11, NULL, 89, 'S')", None),
        ("UPDATE T SET M = 95 WHERE K = 3", Some(Code::Constraint)),
        ("UPDATE T SET N = -1 WHERE K = 3", Some(Code::Constraint)),
        ("UPDATE T SET N = NULL WHERE K = 3", None),
        ("INSERT INTO T VALUES (21, 1, 29, 'S')", Some(Code::Constraint)),
        ("INSERT INTO T VALUES (21, 1, 30, 'S')", None),
        ("INSERT INTO T VALUES (22, 1, 5, 'S')", None),
        ("UPDATE T SET M = 6 WHERE K = 21", Some(Code::Constraint)),
        ("INSERT INTO T VALUES (23, 1, 40, 'S')", None),
        ("DELETE FROM T WHERE K = 21", None),
        ("DELETE FROM T", None),
    ];
    for (step, (sql, expected)) in cases.into_iter().enumerate() {
        assert_eq!(same(&a, &b, sql, step).err(), expected, "{sql}");
        a.commit().unwrap();
        b.commit().unwrap();
    }
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T"), Ok(vec!["0".to_owned()]), "of no row, every one holds");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR2b iii, §6.12, §11.34
/// GR2]; [DESIGN] RULES.md D15, D51: an argument that overflows or
/// divides by zero, and a view's column that does, raise the error as the
/// whole check does, though the row is past no bound or the view's column
/// is not compared; a SUM that overflows is checked whole.
#[test]
fn a_change_raises_what_the_whole_check_raises_6_5() {
    let asserted = [
        ("POWER", "(SELECT MAX(N * N * N * N * N) FROM T WHERE N > 0) < 1000000000000", ""),
        ("TOTAL", "(SELECT SUM(M * 500000000) FROM T) > 0", ""),
    ];
    let ((_x, a), (_y, b)) = twins(&asserted);
    for (s, whole) in [(&a, ""), (&b, " AND 1 = 1")] {
        s.run("CREATE VIEW X (K, Q) AS SELECT K, 100 / (M - 7) FROM T").unwrap();
        s.run(&format!("CREATE ASSERTION QUOTIENT CHECK (NOT EXISTS (SELECT K FROM X WHERE K > 1000){whole})")).unwrap();
        s.commit().unwrap();
    }
    let cases: [(&str, Option<Code>); 6] = [
        ("INSERT INTO T VALUES (11, 2000000000, 8, 'S')", Some(Code::Data)),
        ("INSERT INTO T VALUES (11, 1, 7, 'S')", Some(Code::Data)),
        ("INSERT INTO T VALUES (11, 1, 8, 'S')", None),
        ("INSERT INTO T VALUES (12, 1, 1900000000, 'S')", None),
        ("INSERT INTO T VALUES (13, 1, 1900000000, 'S')", Some(Code::Data)),
        ("UPDATE T SET M = 7 WHERE K = 1", Some(Code::Data)),
    ];
    for (step, (sql, expected)) in cases.into_iter().enumerate() {
        assert_eq!(same(&a, &b, sql, step).err(), expected, "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §14.2 GR3, §14.3 GR4,
/// GR5a]; [DESIGN] RULES.md D51: a deferred `NOT EXISTS` is checked, when
/// made immediate or at commit, from the rows written while it was
/// deferred, as each is then: a row mended or deleted before breaks
/// nothing; one that still breaks it fails SET CONSTRAINTS, or rolls the
/// commit back; and a deferrable one made deferred within its transaction
/// alike.
#[test]
fn a_deferred_assertion_is_checked_from_its_rows_14_3() {
    let asserted = [
        ("LATER", "NOT EXISTS (SELECT * FROM T WHERE N > M)", "DEFERRABLE INITIALLY DEFERRED"),
        ("SOMETIME", "NOT EXISTS (SELECT * FROM T WHERE N < 0)", "DEFERRABLE"),
    ];
    let ((_x, a), (_y, b)) = twins(&asserted);
    let script: [(&str, Option<Code>); 18] = [
        ("INSERT INTO T VALUES (11, 99, 1, 'S')", None),
        ("UPDATE T SET N = 1 WHERE K = 11", None),
        ("UPDATE T SET N = 60 WHERE K = 2", None),
        ("DELETE FROM T WHERE K = 2", None),
        ("COMMIT", None),
        ("INSERT INTO T VALUES (12, 99, 1, 'S')", None),
        ("SET CONSTRAINTS LATER IMMEDIATE", Some(Code::Constraint)),
        ("UPDATE T SET M = 99 WHERE K = 12", None),
        ("SET CONSTRAINTS LATER IMMEDIATE", None),
        ("UPDATE T SET N = 50 WHERE K = 3", Some(Code::Constraint)),
        ("SET CONSTRAINTS LATER DEFERRED", None),
        ("UPDATE T SET N = 50 WHERE K = 3", None),
        ("COMMIT", Some(Code::CommitFailed)),
        ("INSERT INTO T VALUES (13, -1, 1, 'S')", Some(Code::Constraint)),
        ("SET CONSTRAINTS SOMETIME DEFERRED", None),
        ("INSERT INTO T VALUES (13, -1, 1, 'S')", None),
        ("UPDATE T SET N = 0 WHERE K = 13", None),
        ("COMMIT", None),
    ];
    for (step, (sql, expected)) in script.into_iter().enumerate() {
        assert_eq!(same(&a, &b, sql, step).err(), expected, "{sql}");
    }
    let kept = rows(&a, "SELECT K FROM T WHERE K > 10 ORDER BY K");
    assert_eq!(kept, Ok(vec!["11".to_owned(), "13".to_owned()]), "the second transaction rolled back");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28, §14.1 GR5]; [DESIGN]
/// RULES.md D29, D51: a greatest no row of a change passes is held from
/// the change only in a transaction that reads at one snapshot. Read at a
/// snapshot of each statement, the rows others deleted meanwhile are gone
/// from it, with the transaction's own, whole; at one snapshot the change
/// is checked alone, and the transaction fails at commit, what it read for
/// its first check changed.
#[test]
fn a_greatest_read_at_each_statement_is_checked_whole_6_5() {
    for level in ["READ COMMITTED", "SERIALIZABLE"] {
        let (db, s) = schema();
        s.run("CREATE ASSERTION REACHED CHECK ((SELECT MAX(M) FROM T WHERE K < 12) > 40)").unwrap();
        s.commit().unwrap();
        let other = db.session("A");
        exec(&s, &format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
        s.run("INSERT INTO T VALUES (11, 11, 10, 'S')").unwrap();
        s.run("INSERT INTO T VALUES (30, 30, 10, 'S')").unwrap();
        other.run("DELETE FROM T WHERE K < 11").unwrap();
        other.commit().unwrap();
        // Of no row under 12.
        let unchanged = exec(&s, "UPDATE T SET S = 'X' WHERE K = 30");
        if level == "READ COMMITTED" {
            assert_eq!(unchanged, Err(Code::Constraint), "{level}: its own row alone, at 10");
        } else {
            assert!(unchanged.is_ok(), "{level}: {unchanged:?}");
            assert!(s.commit().is_err(), "{level}: T changed since its first check");
        }
        s.rollback().unwrap();
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34, §11.35 GR1, §14.3
/// GR4]; [DESIGN] RULES.md D51: the rows a deferred assertion's checks
/// wait for are checked by the assertion as it is when they are made:
/// whole, once, if it was made again of another form; not at all if it
/// was dropped.
#[test]
fn a_deferred_assertion_made_again_is_checked_as_it_is_14_3() {
    let (_db, s) = schema();
    let later = "CREATE ASSERTION LATER CHECK (NOT EXISTS (SELECT * FROM T WHERE N > M)) DEFERRABLE INITIALLY DEFERRED";
    s.run(later).unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (11, 99, 1, 'S')").unwrap();
    s.run("INSERT INTO T VALUES (12, 99, 1, 'S')").unwrap();
    s.run("DROP ASSERTION LATER").unwrap();
    s.run("CREATE ASSERTION LATER CHECK ((SELECT COUNT(*) FROM T) < 13) DEFERRABLE INITIALLY DEFERRED").unwrap();
    s.commit().unwrap();
    s.run("DROP ASSERTION LATER").unwrap();
    s.run(later.replace("N > M", "N > M + 200").as_str()).unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (13, 300, 1, 'S')").unwrap();
    s.run("DROP ASSERTION LATER").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec!["13".to_owned()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR2b iii, §8.2 GR1a,
/// §11.34 GR2]; [DESIGN] RULES.md D51: MAX equal to a bound is broken by a
/// row past it, holds of a row changed reaching it, and checks the rest
/// whole; MIN unequal to one holds of a row changed past it, or of rows
/// only added on its side, and checks a row leaving whole.
#[test]
fn a_greatest_or_least_equal_or_unequal_to_a_bound_6_5() {
    let asserted = [("PEAK", "(SELECT MAX(M) FROM T WHERE K > 20) = 50", ""), ("APART", "(SELECT MIN(M) FROM T WHERE K < 20) <> 15", "")];
    let ((_x, a), (_y, b)) = twins(&asserted);
    let cases: [(&str, Option<Code>); 12] = [
        ("INSERT INTO T VALUES (21, 1, 40, 'S')", Some(Code::Constraint)),
        ("INSERT INTO T VALUES (21, 1, 50, 'S')", None),
        ("INSERT INTO T VALUES (22, 1, 51, 'S')", Some(Code::Constraint)),
        ("INSERT INTO T VALUES (22, 1, 30, 'S')", None),
        ("UPDATE T SET M = 49 WHERE K = 21", Some(Code::Constraint)),
        ("DELETE FROM T WHERE K = 21", Some(Code::Constraint)),
        ("DELETE FROM T WHERE K = 1", None),
        ("DELETE FROM T WHERE K = 2", Some(Code::Constraint)),
        ("INSERT INTO T VALUES (11, 1, 20, 'S')", None),
        ("UPDATE T SET M = 15 WHERE K = 2", Some(Code::Constraint)),
        ("INSERT INTO T VALUES (12, 1, 15, 'S')", None),
        ("UPDATE T SET M = 3 WHERE K = 12", None),
    ];
    for (step, (sql, expected)) in cases.into_iter().enumerate() {
        assert_eq!(same(&a, &b, sql, step).err(), expected, "{sql}");
        a.commit().unwrap();
        b.commit().unwrap();
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, GR2, §11.34 GR2];
/// [DESIGN] RULES.md D51: COUNT DISTINCT holds of rows changed keeping
/// their values, checks a value changed whole, and an argument that may be
/// null raises the warning of a null eliminated as the whole check does,
/// from the change when it has a null.
#[test]
fn a_count_of_distinct_values_answers_as_checked_whole_6_5() {
    let asserted =
        [("KINDS", "(SELECT COUNT(DISTINCT N) FROM T WHERE N < 20) <= 10", ""), ("VARIED", "(SELECT COUNT(DISTINCT N) FROM T) >= 10", "")];
    let ((_x, a), (_y, b)) = twins(&asserted);
    let cases: [(&str, Option<Code>, Option<&str>); 7] = [
        ("INSERT INTO T VALUES (11, 5, 1, 'S')", None, None),
        ("INSERT INTO T VALUES (12, 15, 1, 'S')", Some(Code::Constraint), None),
        ("UPDATE T SET N = 15 WHERE K = 11", Some(Code::Constraint), None),
        ("UPDATE T SET M = 7 WHERE K < 5", None, None),
        ("UPDATE T SET N = NULL WHERE K = 11", None, Some("01003")),
        ("DELETE FROM T WHERE K = 10", Some(Code::Constraint), None),
        ("INSERT INTO T VALUES (13, 20, 1, 'S')", None, Some("01003")),
    ];
    for (step, (sql, expected, warned)) in cases.into_iter().enumerate() {
        assert_eq!(same(&a, &b, sql, step).err(), expected, "{sql}");
        assert_eq!(a.warning().map(SqlState::as_str), warned, "{sql}");
        a.commit().unwrap();
        b.commit().unwrap();
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §6.5 GR1b, §14.2
/// GR3, §14.3 GR4]; [DESIGN] RULES.md D15, D51: a deferred assertion of
/// forms of no row passing, MAX bounded among them, is checked from the
/// rows written while it was deferred; one whose steps can raise an error,
/// or whose argument may be null, whole, raising what the whole check
/// raises.
#[test]
fn a_deferred_assertion_of_several_forms_is_checked_from_its_rows_14_3() {
    let asserted = [
        (
            "PAIRED",
            "NOT EXISTS (SELECT * FROM T WHERE N > M) AND (SELECT MAX(M) FROM T WHERE K > 10) < 100",
            "DEFERRABLE INITIALLY DEFERRED",
        ),
        ("RAISING", "NOT EXISTS (SELECT * FROM T WHERE N < 0) AND (SELECT MAX(M) FROM T WHERE 60 / (N - 20) > 0) < 90", "DEFERRABLE"),
        ("NULLED", "(SELECT MIN(N) FROM T) > -10", "DEFERRABLE"),
    ];
    let ((_x, a), (_y, b)) = twins(&asserted);
    let script: [(&str, Option<Code>); 16] = [
        ("INSERT INTO T VALUES (11, 1, 120, 'S')", None),
        ("UPDATE T SET M = 60 WHERE K = 11", None),
        ("COMMIT", None),
        ("INSERT INTO T VALUES (12, 99, 1, 'S')", None),
        ("SET CONSTRAINTS PAIRED IMMEDIATE", Some(Code::Constraint)),
        ("DELETE FROM T WHERE K = 12", None),
        ("INSERT INTO T VALUES (12, 1, 100, 'S')", None),
        ("COMMIT", Some(Code::CommitFailed)),
        ("SET CONSTRAINTS RAISING DEFERRED", None),
        ("INSERT INTO T VALUES (13, -1, 1, 'S')", None),
        ("INSERT INTO T VALUES (14, 20, 95, 'S')", None),
        ("SET CONSTRAINTS RAISING IMMEDIATE", Some(Code::Data)),
        ("ROLLBACK", None),
        ("SET CONSTRAINTS NULLED DEFERRED", None),
        ("UPDATE T SET N = NULL WHERE K = 1", None),
        ("SET CONSTRAINTS NULLED IMMEDIATE", None),
    ];
    for (step, (sql, expected)) in script.into_iter().enumerate() {
        assert_eq!(same(&a, &b, sql, step).err(), expected, "{sql}");
    }
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "checked whole, a null eliminated");
}
