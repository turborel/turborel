//! Assertions checked from the rows a statement changed [ISO/IEC 9075:1992
//! §4.10.1, §11.34 GR2] (RULES.md D51): `NOT EXISTS` over one table's
//! rows, and a count of them compared with a constant, each of conditions
//! of the row alone; every other form, a deferred count, and a count read
//! below one snapshot, checked whole. Whichever way, a statement fails
//! exactly when its condition is false after it. The further forms are
//! tested in `assertion_forms.rs`.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{Rng, exec, rows};
use crate::functions::CountTimesTen;

/// Schema A: table T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER, S
/// VARCHAR(6)) of rows K 1 to 10, N K, S 'S' and K; and table U (K
/// INTEGER, R INTEGER REFERENCES T ON DELETE CASCADE), of rows K 1 to 10,
/// R K.
fn schema() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER, S VARCHAR(6))
           CREATE TABLE U (K INTEGER, R INTEGER REFERENCES T ON DELETE CASCADE)",
    )
    .unwrap();
    let s = db.session("A");
    for k in 1..=10 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {k}, 'S{k}')")).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({k}, {k})")).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2];
/// [DESIGN] RULES.md D51: an assertion that no row of T has a condition of
/// the row is broken by a row inserted or updated to have it, by a
/// referential action too, and not by changes of other rows.
#[test]
fn no_row_may_have_the_condition_11_34() {
    let (_db, s) = schema();
    s.run("CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT K FROM T WHERE N > 100 AND S LIKE 'S%'))").unwrap();
    s.run("CREATE ASSERTION LINKED CHECK (NOT EXISTS (SELECT * FROM U WHERE R IS NULL AND K < 5))").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (11, 101, 'S11')"), Err(Code::Constraint), "inserted");
    assert_eq!(exec(&s, "UPDATE T SET N = N * 50 WHERE K = 3"), Err(Code::Constraint), "updated");
    s.run("INSERT INTO T VALUES (11, 101, 'X11')").unwrap();
    s.run("UPDATE T SET N = N + 1 WHERE K < 5").unwrap();
    s.run("DELETE FROM T WHERE K = 9").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM U").unwrap(), ["9"], "deleted by the referential action");
    s.commit().unwrap();
    assert_eq!(exec(&s, "UPDATE U SET R = NULL WHERE K = 2"), Err(Code::Constraint));
    s.run("UPDATE U SET R = NULL WHERE K = 7").unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2];
/// [DESIGN] RULES.md D51: an assertion bounding a count of T's rows is
/// broken by a change that moves the count past the bound, whichever side
/// of the comparison the count is on, and not by one that moves it back or
/// leaves it; each conjunct of an AND is held.
#[test]
fn a_count_is_held_to_its_bound_11_34() {
    let (_db, s) = schema();
    s.run("CREATE ASSERTION FEW CHECK ((SELECT COUNT(*) FROM T WHERE N > 5) <= 6 AND 4 < (SELECT COUNT(*) FROM T))").unwrap();
    s.run("CREATE ASSERTION SAME CHECK ((SELECT COUNT(*) FROM U WHERE R < 4) = 3)").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (11, 11, 'S')").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (12, 12, 'S')"), Err(Code::Constraint), "seven rows over 5");
    assert_eq!(exec(&s, "UPDATE T SET N = N + 5 WHERE K = 1"), Err(Code::Constraint), "a row moved into the count");
    s.run("UPDATE T SET N = N - 10 WHERE K = 11").unwrap();
    s.run("UPDATE T SET S = 'X'").unwrap();
    s.run("DELETE FROM T WHERE K > 6").unwrap();
    assert_eq!(exec(&s, "DELETE FROM T WHERE K < 3"), Err(Code::Constraint), "four rows left");
    assert_eq!(exec(&s, "UPDATE U SET R = 1 WHERE K = 5"), Err(Code::Constraint), "four rows under 4");
    s.run("UPDATE U SET R = 2 WHERE K = 3").unwrap();
    s.run("UPDATE U SET K = K + 100 WHERE R < 4").unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §10.6 GR3, §14.3 GR4];
/// [DESIGN] RULES.md D51: a deferred assertion of a count is checked whole
/// when it is made immediate, or at commit: a statement that breaks it may
/// be mended by another before.
#[test]
fn a_deferred_assertion_is_checked_whole_14_3() {
    let (_db, s) = schema();
    s.run("CREATE ASSERTION FEW CHECK ((SELECT COUNT(*) FROM T) <= 10) DEFERRABLE INITIALLY DEFERRED").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (11, 11, 'S')").unwrap();
    s.run("DELETE FROM T WHERE K = 1").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (12, 12, 'S')").unwrap();
    s.run("UPDATE T SET N = 0 WHERE K = 2").unwrap();
    assert_eq!(exec(&s, "SET CONSTRAINTS FEW IMMEDIATE"), Err(Code::Constraint));
    assert_eq!(exec(&s, "COMMIT"), Err(Code::CommitFailed));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2]; [DESIGN] RULES.md
/// D51: an assertion of a COUNT a plug-in replaced is checked whole: what
/// the standard COUNT does tells nothing of it.
#[test]
fn a_count_replaced_is_checked_whole_11_34() {
    let (db, s) = schema();
    s.run("CREATE ASSERTION FEW CHECK ((SELECT COUNT(*) FROM T) <= 50)").unwrap();
    s.commit().unwrap();
    db.register_aggregate_function(Box::new(CountTimesTen)).unwrap();
    assert_eq!(exec(&s, "DELETE FROM T WHERE K = 1"), Err(Code::Constraint), "ninety, counted ten times each");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28, §14.1 GR5]; [DESIGN]
/// RULES.md D29, D51: a count is held from a change only in a transaction
/// that reads at one snapshot. Read at a snapshot of each statement, a
/// count others raised meanwhile is seen with the transaction's own rows,
/// whole; at one snapshot the change is checked alone, and the transaction
/// fails at commit, what it read for its first check changed.
#[test]
fn a_count_read_at_each_statement_is_checked_whole_11_34() {
    for level in ["READ COMMITTED", "SERIALIZABLE"] {
        let (db, s) = schema();
        s.run("CREATE ASSERTION FEW CHECK ((SELECT COUNT(*) FROM T) <= 12)").unwrap();
        s.commit().unwrap();
        let other = db.session("A");
        exec(&s, &format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
        s.run("INSERT INTO T VALUES (11, 11, 'S')").unwrap();
        other.run("INSERT INTO T VALUES (21, 21, 'S'), (22, 22, 'S')").unwrap();
        other.commit().unwrap();
        let unchanged = exec(&s, "UPDATE T SET S = 'X' WHERE K = 1");
        if level == "READ COMMITTED" {
            assert_eq!(unchanged, Err(Code::Constraint), "{level}: twelve committed, and its own");
        } else {
            assert!(unchanged.is_ok(), "{level}: {unchanged:?}");
            assert!(s.commit().is_err(), "{level}: T changed since its first check");
        }
        s.rollback().unwrap();
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2]; [DESIGN] RULES.md
/// D15, D51. Regression test (R-TEST-7): a query's condition over a view
/// merged into it is evaluated only on the rows the view's own keeps, as
/// the whole check does, so a row the view leaves out raises nothing; it
/// was evaluated first, dividing by zero.
#[test]
fn a_views_rows_are_kept_before_the_query_judges_them_11_34() {
    let (_db, s) = schema();
    s.run("CREATE VIEW V AS SELECT K, N FROM T WHERE K < 100").unwrap();
    s.run("CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM V WHERE 10 / N > 100))").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (200, 0, 'X')").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (50, 0, 'X')"), Err(Code::Data), "a row of the view, divided by zero");
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.12 GR2, §11.34 GR2];
/// [DESIGN] RULES.md D15, D51. Regression test (R-TEST-7): every conjunct
/// of an assertion is evaluated on the change, as AND evaluates both its
/// operands, so an error the second raises is raised though the first is
/// false, as checked whole; the first false ended the check.
#[test]
fn every_conjunct_is_evaluated_on_the_change_11_34() {
    let (_db, s) = schema();
    let condition = "NOT EXISTS (SELECT * FROM T WHERE N > 100) AND NOT EXISTS (SELECT * FROM T WHERE 10 / (N - 101) > 0)";
    s.run(&format!("CREATE ASSERTION PAIRED CHECK ({condition})")).unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (11, 101, 'X')"), Err(Code::Data), "the second divides by zero");
    assert_eq!(exec(&s, "INSERT INTO T VALUES (11, 102, 'X')"), Err(Code::Constraint), "the first false alone");
    let found = rows(&s, &format!("SELECT 1 FROM (VALUES (1)) AS V (X) WHERE NOT ({condition} AND 1 = 1)"));
    assert_eq!(found, Ok(Vec::new()), "neither inserted");
}

/// The assertions of the random test: each name, condition, and whether
/// it is checked from a change.
const ASSERTED: [(&str, &str); 8] = [
    ("NEGATIVE", "NOT EXISTS (SELECT * FROM T WHERE (N IS NULL OR -N > K) AND (K > 30) IS TRUE)"),
    ("BOUNDED", "(SELECT COUNT(*) FROM T WHERE N > 20) <= 12"),
    ("OCCUPIED", "6 <= (SELECT COUNT(*) FROM T)"),
    ("MIXED", "NOT EXISTS (SELECT 1 FROM T WHERE N = K + 7 AND S NOT LIKE 'X%') AND (SELECT COUNT(*) FROM T WHERE N IS NULL) < 5"),
    ("CASED", "NOT EXISTS (SELECT * FROM T WHERE CASE WHEN N > 30 THEN K ELSE 0 END > 25)"),
    ("LINKS", "(SELECT COUNT(*) FROM U WHERE R > 3) <> 2"),
    // Checked whole: a function, a CAST, a SUM.
    ("FUNCTION", "NOT EXISTS (SELECT * FROM T WHERE CHARACTER_LENGTH(S) > 5) AND (SELECT SUM(N) FROM T) < 700"),
    ("CASTED", "NOT EXISTS (SELECT * FROM T WHERE CAST(N AS CHAR(3)) = '33')"),
];

/// A random statement changing T or U, from `rng`.
fn change(rng: &mut Rng) -> String {
    let (k, n) = (rng.below(40), rng.below(50));
    let value = if rng.chance(15) { "NULL".to_owned() } else { n.to_string() };
    match rng.below(7) {
        0 | 1 => format!("INSERT INTO T VALUES ({k}, {value}, '{}{k}')", rng.pick(&["S", "X"])),
        2 => format!("UPDATE T SET N = {value} WHERE K = {k}"),
        3 => format!("UPDATE T SET N = N + {} WHERE K BETWEEN {k} AND {}", rng.below(9), k + rng.below(6)),
        4 => format!("DELETE FROM T WHERE K = {k} OR N = {n}"),
        5 => format!("INSERT INTO U VALUES ({k}, {})", rng.below(12) + 1),
        _ => format!("UPDATE U SET R = {} WHERE K = {k}", rng.below(12) + 1),
    }
}

/// Whether some assertion of `ASSERTED` is false in what `s` sees.
fn broken(s: &Session) -> bool {
    ASSERTED.iter().any(|(_, condition)| {
        let found = rows(s, &format!("SELECT 1 FROM (VALUES (1)) AS V (X) WHERE NOT ({condition})")).unwrap();
        !found.is_empty()
    })
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2];
/// [DESIGN] RULES.md D51, R-TEST-5: random statements under assertions
/// checked from their changes, and whole, succeed exactly when the same
/// statements on a twin of no assertions leave every assertion's condition
/// not false there, at SERIALIZABLE and at READ COMMITTED.
#[test]
fn assertions_checked_from_changes_answer_as_checked_whole() {
    let (asserted, a) = schema();
    let (_twin, b) = schema();
    for (name, condition) in ASSERTED {
        a.run(&format!("CREATE ASSERTION {name} CHECK ({condition})")).unwrap();
    }
    a.commit().unwrap();
    let mut rng = Rng::new(0x5EED_D051_0000_0001);
    let (mut refused, mut done) = (0, 0);
    for step in 0..400 {
        let sql = change(&mut rng);
        if rng.chance(30) {
            exec(&a, "SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
        }
        let (checked, twin) = (exec(&a, &sql), exec(&b, &sql));
        let expected = match twin {
            Err(code) => Some(code),
            Ok(_) if broken(&b) => Some(Code::Constraint),
            Ok(_) => None,
        };
        assert_eq!(checked.err(), expected, "step {step}: {sql}");
        if expected.is_some() {
            refused += 1;
            b.rollback().unwrap();
        } else {
            done += 1;
            b.commit().unwrap();
        }
        a.commit().unwrap();
    }
    assert!(refused > 40 && done > 150, "both outcomes, often: {refused} refused, {done} done");
    drop(asserted);
}
