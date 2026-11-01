//! Assertions judged of the totals they keep [ISO/IEC 9075:1992 §6.5,
//! §11.34 GR2] (RULES.md D51): SUM and AVG of exact numbers, and COUNT,
//! compared with a constant, and the nulls of a set function's argument
//! that may be null, immediate and deferred. Each answers as the same assertion
//! checked whole, which `AND 1 = 1` makes it; two transactions conflict
//! only if they move a value the same way; the totals are made with the
//! assertion, kept across a restart, dropped with it, and named by no
//! statement.

use std::path::Path;

use turborel::db::{Completion, Database, OpenOptions, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::threads::JobPolicy;

use crate::common::{Rng, Scratch, exec, os, rows};

/// The schema of schema A: table T (K INTEGER NOT NULL PRIMARY KEY, G
/// INTEGER, N INTEGER, D DECIMAL(18,2)).
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION A
   CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, G INTEGER, N INTEGER, D DECIMAL(18,2))";

/// Schema A of `db`, T of rows K 1 to 10, G K mod 3, N K, D K / 4.
fn filled(db: &Database) -> Session {
    db.schema(SCHEMA).unwrap();
    let s = db.session("A");
    for k in 1..=10 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {}, {k}, {k} / 4.0)", k % 3)).unwrap();
    }
    s.commit().unwrap();
    s
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

/// `sql`'s outcome in `a` and in `b`, which must be the same, warnings
/// among it: the outcome.
fn same(a: &Session, b: &Session, sql: &str, step: usize) -> Result<Completion, Code> {
    let (checked, whole) = (exec(a, sql), exec(b, sql));
    assert_eq!(checked, whole, "step {step}: {sql}");
    assert_eq!(a.warning(), b.warning(), "step {step}: {sql}");
    checked
}

/// The assertions of the random test: each name, condition and attributes.
const ASSERTED: [(&str, &str, &str); 12] = [
    ("CAPPED", "(SELECT SUM(N) FROM T) <= 400", ""),
    ("FLOORED", "(SELECT SUM(N) FROM T WHERE G = 1) >= 4", ""),
    ("SPREAD", "(SELECT AVG(N) FROM T WHERE K > 2) <= 24", ""),
    ("RAISED", "(SELECT AVG(D) FROM T) > 0.75", ""),
    ("APART", "(SELECT SUM(N) FROM T WHERE G = 2) <> 50", ""),
    ("COUNTED", "(SELECT COUNT(N) FROM T WHERE G = 0) <= 12", ""),
    ("CEILING", "(SELECT MAX(N) FROM T) < 90", ""),
    ("LATESUM", "(SELECT SUM(D) FROM T) < 60", "DEFERRABLE INITIALLY DEFERRED"),
    ("LATEAVG", "(SELECT AVG(N) FROM T WHERE G <> 1) >= 3", "DEFERRABLE INITIALLY DEFERRED"),
    ("FEW", "(SELECT COUNT(*) FROM T WHERE G = 1) <= 9", ""),
    ("ENOUGH", "(SELECT COUNT(N) FROM T) >= 6", ""),
    ("LATEFEW", "(SELECT COUNT(*) FROM T WHERE N > 30) < 5", "DEFERRABLE INITIALLY DEFERRED"),
];

/// A random statement changing T, from `rng`.
fn change(rng: &mut Rng) -> String {
    let (k, g) = (rng.below(40), rng.below(3));
    let n = if rng.chance(12) { "NULL".to_owned() } else { rng.below(60).to_string() };
    let d = if rng.chance(8) { "NULL".to_owned() } else { format!("{}.{:02}", rng.below(9), rng.below(100)) };
    match rng.below(10) {
        0..=3 => format!("INSERT INTO T VALUES ({k}, {g}, {n}, {d})"),
        4 => format!("UPDATE T SET N = {n} WHERE K = {k}"),
        5 => format!("UPDATE T SET G = {g} WHERE K BETWEEN {k} AND {}", k + rng.below(4)),
        6 => format!("UPDATE T SET N = N + {}, D = {d} WHERE G = {g}", rng.below(15)),
        7 => format!("DELETE FROM T WHERE K = {k}"),
        8 => format!("DELETE FROM T WHERE G = {g} AND K > {k}"),
        _ => format!("UPDATE T SET D = D * 2 WHERE K < {k}"),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §6.5 GR1b, GR2b ii,
/// iv, §11.34 GR2, §14.3 GR4]; [DESIGN] RULES.md D51, R-TEST-5: random
/// transactions of random statements under assertions of SUM and AVG of
/// exact numbers, bounded, unequal, filtered, deferred, and of set
/// functions of values that may be null, judged of the totals they keep,
/// answer as the same assertions checked whole: each statement and each
/// commit the same outcome, its SQLCODE and warning, at SERIALIZABLE and
/// at READ COMMITTED.
#[test]
fn totals_kept_answer_as_checked_whole() {
    let (x, y) = (Database::new(), Database::new());
    let (a, b) = (filled(&x), filled(&y));
    assert_all(&a, &ASSERTED, false);
    assert_all(&b, &ASSERTED, true);
    let mut rng = Rng::new(0x5EED_D051_0000_0004);
    let (mut refused, mut done, mut rolled_back) = (0, 0, 0);
    let mut step = 0;
    for _ in 0..300 {
        if rng.chance(30) {
            same(&a, &b, "SET TRANSACTION ISOLATION LEVEL READ COMMITTED", step).unwrap();
        }
        for _ in 0..=rng.below(3) {
            step += 1;
            match same(&a, &b, &change(&mut rng), step) {
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
    assert!(refused > 40 && done > 200 && rolled_back > 3, "{refused} refused, {done} done, {rolled_back} rolled back");
    let sql = "SELECT K, G, N, D FROM T ORDER BY K";
    assert_eq!(rows(&a, sql), rows(&b, sql));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR2b iv, §11.34 GR2];
/// [DESIGN] RULES.md D15, D51: a SUM kept past the range of its result
/// raises 22003 as the whole check does, judged of the total after the
/// statement, not of a sum on the way; a SUM of no value is null, so the
/// assertion holds.
#[test]
fn a_sum_kept_out_of_range_raises_as_whole_6_5() {
    for whole in ["", " AND 1 = 1"] {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE B (K INTEGER NOT NULL, V DECIMAL(18,0))").unwrap();
        let s = db.session("A");
        s.run(&format!("CREATE ASSERTION BIG CHECK ((SELECT SUM(V) FROM B) < 0{whole})")).unwrap();
        s.commit().unwrap();
        s.run("INSERT INTO B VALUES (1, -999999999999999999)").unwrap();
        let past = s.execute("INSERT INTO B VALUES (2, -999999999999999999)", &mut Params::new()).unwrap_err();
        assert_eq!((past.code, past.state.map(SqlState::as_str)), (Code::Data, Some("22003")), "{whole}: out of range");
        s.run("INSERT INTO B VALUES (2, -999999999999999999), (3, 999999999999999999)").unwrap();
        s.run("DELETE FROM B").unwrap();
        s.commit().unwrap();
        assert!(exec(&s, "INSERT INTO B VALUES (4, NULL)").is_ok(), "{whole}: a sum of no value");
        assert_eq!(s.warning().map(SqlState::as_str), Some("01003"), "{whole}: a null eliminated");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28, §11.34 GR2]; [DESIGN]
/// RULES.md D29, D51: of two transactions changing a SUM bounded above,
/// the second to commit fails only if both raise it, or both lower it,
/// whose sum must stay in range [§6.5 GR2b iv]; one raising and one
/// lowering both commit. Of AVG bounded above, only two raising it meet;
/// of a SUM unequal to a constant, any two that change it.
#[test]
fn two_transactions_meet_only_moving_a_value_one_way_4_28() {
    // Inserts alone, which read none of T's other rows.
    let insert = |k: i32, n: i32| format!("INSERT INTO T VALUES ({k}, 0, {n}, 0)");
    let cases = [
        ("(SELECT SUM(N) FROM T) <= 400", [insert(20, 5), insert(21, 7)], false),
        ("(SELECT SUM(N) FROM T) <= 400", [insert(20, 5), insert(21, -3)], true),
        ("(SELECT SUM(N) FROM T) <= 400", [insert(20, -4), insert(21, -3)], false),
        ("(SELECT AVG(N) FROM T) <= 24", [insert(20, 1), insert(21, 2)], true),
        ("(SELECT AVG(N) FROM T) <= 24", [insert(20, 30), insert(21, 40)], false),
        ("(SELECT SUM(N) FROM T) <> 7", [insert(20, 1), insert(21, 2)], false),
        ("(SELECT COUNT(*) FROM T) <= 400", [insert(20, 1), insert(21, 2)], false),
        ("(SELECT COUNT(*) FROM T) <= 400", [insert(20, 1), "DELETE FROM T WHERE K = 10".to_owned()], true),
        ("(SELECT COUNT(*) FROM T) >= 2", [insert(20, 1), insert(21, 2)], true),
    ];
    for (n, (condition, [first, second], both)) in cases.into_iter().enumerate() {
        for level in ["SERIALIZABLE", "READ COMMITTED"] {
            let db = Database::new();
            let s = filled(&db);
            s.run(&format!("CREATE ASSERTION BOUND CHECK ({condition})")).unwrap();
            s.commit().unwrap();
            let other = db.session("A");
            for session in [&s, &other] {
                exec(session, &format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
            }
            s.run(&first).unwrap();
            other.run(&second).unwrap();
            other.commit().unwrap();
            assert_eq!(s.commit().is_ok(), both, "case {n}, {level}");
        }
    }
}

/// The database in `dir`.
fn open(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2]; [DESIGN]
/// RULES.md D25, D51: the totals an assertion keeps are recovered with
/// the rows they count, whether the database stopped without closing or
/// closed, made with an assertion of a session's or of a `<schema
/// definition>`'s.
#[test]
fn totals_kept_are_recovered() {
    let scratch = Scratch::new("assertion-totals");
    let dir = scratch.path("db");
    let db = open(&dir);
    let s = filled(&db);
    s.run("CREATE ASSERTION CAPPED CHECK ((SELECT SUM(N) FROM T) <= 70)").unwrap();
    s.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (V INTEGER) CREATE ASSERTION SMALL CHECK ((SELECT SUM(V) FROM U) < 10)")
        .unwrap();
    s.run("INSERT INTO T VALUES (11, 0, 10, 0)").unwrap();
    s.commit().unwrap();
    db.session("B").run("INSERT INTO U VALUES (8)").unwrap();
    db.session("B").commit().unwrap();
    drop((s, db));
    for round in 0..2 {
        let db = open(&dir);
        let (s, b) = (db.session("A"), db.session("B"));
        assert_eq!(exec(&s, "INSERT INTO T VALUES (12, 0, 6, 0)"), Err(Code::Constraint), "round {round}: 65 and 6");
        s.run("INSERT INTO T VALUES (12, 0, 5, 0)").unwrap();
        assert_eq!(exec(&b, "INSERT INTO U VALUES (2)"), Err(Code::Constraint), "round {round}: 8 and 2");
        s.rollback().unwrap();
        b.rollback().unwrap();
        if round == 0 {
            db.close().unwrap();
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.35, §11.18 GR4, §21.1];
/// [DESIGN] RULES.md D51: the totals of an assertion are of no statement:
/// not read, written, dropped nor listed in the Information Schema; they
/// go with the assertion, so one made again of its name keeps its own.
#[test]
fn totals_kept_are_the_assertions_own_21_1() {
    let db = Database::new();
    let s = filled(&db);
    s.run("CREATE ASSERTION CAPPED CHECK ((SELECT SUM(N) FROM T) <= 60)").unwrap();
    s.commit().unwrap();
    let hidden = "DEFINITION_SCHEMA.\"ASSERTION TOTALS 1 ACAPPED\"";
    assert_eq!(rows(&s, &format!("SELECT * FROM {hidden}")).err(), Some(Code::SyntaxRule), "read");
    assert_eq!(exec(&s, &format!("DELETE FROM {hidden}")), Err(Code::Privilege), "written");
    let keeper = db.session("DEFINITION_SCHEMA");
    assert!(exec(&keeper, &format!("DROP TABLE {hidden} CASCADE")).is_err(), "dropped, even by its schema's name");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'DEFINITION_SCHEMA'").unwrap(), ["0"]);
    s.run("DROP ASSERTION CAPPED").unwrap();
    s.run("DELETE FROM T WHERE K > 5").unwrap();
    s.run("CREATE ASSERTION CAPPED CHECK ((SELECT SUM(N) FROM T) <= 20)").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (11, 0, 6, 0)"), Err(Code::Constraint), "15 and 6");
    s.run("INSERT INTO T VALUES (11, 0, 5, 0)").unwrap();
    s.run("DROP TABLE T CASCADE").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ASSERTIONS").unwrap(), ["0"], "gone with T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2]; [DESIGN]
/// RULES.md D28, D51: an assertion keeping totals is made only once no
/// other transaction holds changes of what it reads, so none goes
/// uncounted: it waits for them, here past LOCKS.TIMEOUT_MS, refused
/// (SQLCODE -911); once they commit it is made, counting them.
#[test]
fn totals_are_made_of_no_change_uncommitted_11_34() {
    let db = Database::new();
    let s = filled(&db);
    let writer = db.session("A");
    writer.run("INSERT INTO T VALUES (11, 0, 10, 0)").unwrap();
    s.run("SET LOCKS.TIMEOUT_MS = 30").unwrap();
    let refused = exec(&s, "CREATE ASSERTION CAPPED CHECK ((SELECT SUM(N) FROM T) <= 70)");
    assert_eq!(refused, Err(Code::Conflict), "T has a change uncommitted");
    s.rollback().unwrap();
    writer.commit().unwrap();
    s.run("CREATE ASSERTION CAPPED CHECK ((SELECT SUM(N) FROM T) <= 70)").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (12, 0, 6, 0)"), Err(Code::Constraint), "65 and 6");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md
/// D29, D51, D55: INFORMATION_SCHEMA.ASSERTION_TOTALS shows the totals of
/// an assertion, as committed, to its schema's owner alone; the rows each
/// statement appends are folded by the job turborel-assertion-totals into
/// one of each form, totals kept, and a transaction that read them
/// meanwhile still commits.
#[test]
fn totals_are_shown_and_folded_21_1() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let s = filled(&db);
    s.run("CREATE ASSERTION KEPT CHECK ((SELECT SUM(N) FROM T) <= 100000 AND (SELECT COUNT(N) FROM T WHERE G = 0) < 1000)").unwrap();
    s.commit().unwrap();
    let shown =
        "SELECT FORM_NUMBER, KIND, VALUE_COUNT, NULL_COUNT, TOTAL, ROWS_KEPT FROM INFORMATION_SCHEMA.ASSERTION_TOTALS ORDER BY FORM_NUMBER";
    assert_eq!(rows(&s, shown).unwrap(), ["1,SUM,10,0,55,1", "2,COUNT,3,0,NULL,1"], "made");
    for k in 11..=90 {
        let n = if k % 10 == 0 { "NULL".to_owned() } else { "1".to_owned() };
        s.run(&format!("INSERT INTO T VALUES ({k}, 0, {n}, 0)")).unwrap();
        s.commit().unwrap();
    }
    assert_eq!(rows(&s, shown).unwrap(), ["1,SUM,82,8,127,81", "2,COUNT,75,8,NULL,81"], "a row of each change");
    let reader = db.session("A");
    reader.run("INSERT INTO T VALUES (100, 1, 1, 0)").unwrap();
    db.run_due().unwrap();
    assert_eq!(rows(&s, shown).unwrap(), ["1,SUM,82,8,127,1", "2,COUNT,75,8,NULL,1"], "folded");
    reader.commit().unwrap();
    assert_eq!(rows(&s, shown).unwrap(), ["1,SUM,83,8,128,2", "2,COUNT,75,8,NULL,1"], "its change kept");
    assert!(rows(&db.session("B"), shown).unwrap().is_empty(), "not its schema's owner");
}

/// The sum of N, and the TOTAL its assertion keeps, as `s` sees them.
fn sum_and_kept(s: &Session) -> (Vec<String>, Vec<String>) {
    let kept = "SELECT TOTAL FROM INFORMATION_SCHEMA.ASSERTION_TOTALS WHERE KIND = 'SUM'";
    (rows(s, "SELECT SUM(N) FROM T").unwrap(), rows(s, kept).unwrap())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.1, §14.3 GR4];
/// [DESIGN] RULES.md D38, D51: a deferred assertion keeping totals whose
/// rows written outgrow MEMORY.WORK_BYTES is checked whole at commit, its
/// totals put right of the table whole; they are then the table's.
#[test]
fn totals_are_put_right_past_the_work_area_14_3() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let s = filled(&db);
    s.run("CREATE ASSERTION LATESUM CHECK ((SELECT SUM(N) FROM T) < 100000) DEFERRABLE INITIALLY DEFERRED").unwrap();
    s.commit().unwrap();
    s.run("SET MEMORY.WORK_BYTES = 65536").unwrap();
    let many = (100..3100).map(|k| format!("({k}, 0, 2, 0)")).collect::<Vec<_>>().join(", ");
    s.run(&format!("INSERT INTO T VALUES {many}")).unwrap();
    s.run("UPDATE T SET N = 3 WHERE K < 2000").unwrap();
    s.commit().unwrap();
    let (sum, kept) = sum_and_kept(&s);
    assert_eq!((sum.clone(), kept), (vec!["7930".to_owned()], vec!["7930".to_owned()]), "put right");
    s.run("SET MEMORY.WORK_BYTES = 4194304").unwrap();
    s.run(&format!("INSERT INTO T VALUES (5000, 0, {}, 0)", 100_000 - 7930)).unwrap();
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::CommitFailed), "100000 reached");
    db.run_due().unwrap();
    db.run_due().unwrap();
    assert_eq!(sum_and_kept(&s).1, sum, "folded, or nothing to fold");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.15 SR4, §11.37 GR5];
/// [DESIGN] RULES.md D51: an assertion keeping totals dropped with a
/// column it refers to, or with the privilege it was made with, takes its
/// totals with it; one made again of its name keeps its own.
#[test]
fn totals_go_with_a_column_or_a_privilege_11_37() {
    let db = Database::new();
    let s = filled(&db);
    s.run("CREATE ASSERTION CAPPED CHECK ((SELECT SUM(N) FROM T) <= 1000)").unwrap();
    s.commit().unwrap();
    s.run("ALTER TABLE T DROP COLUMN N CASCADE").unwrap();
    s.commit().unwrap();
    let kept = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ASSERTION_TOTALS";
    assert_eq!(rows(&s, kept).unwrap(), ["0"], "gone with N");
    s.run("ALTER TABLE T ADD COLUMN N INTEGER").unwrap();
    s.run("UPDATE T SET N = K").unwrap();
    s.run("GRANT REFERENCES ON T TO B").unwrap();
    s.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let b = db.session("B");
    b.run("CREATE ASSERTION CAPPED CHECK ((SELECT SUM(N) FROM A.T) <= 60)").unwrap();
    b.commit().unwrap();
    assert_eq!(rows(&b, kept).unwrap(), ["1"]);
    assert_eq!(exec(&s, "INSERT INTO T VALUES (11, 0, 0, 6)"), Err(Code::Constraint), "55 and 6, kept");
    s.run("REVOKE REFERENCES ON T FROM B CASCADE").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&b, kept).unwrap(), ["0"], "gone with the privilege");
    s.run("INSERT INTO T VALUES (11, 0, 0, 6)").unwrap();
    s.commit().unwrap();
}
