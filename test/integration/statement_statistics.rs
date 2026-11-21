//! INFORMATION_SCHEMA.STATEMENT_STATISTICS (RULES.md D521): the statements
//! run since the database was opened, by normalized text and user, their
//! calls, rows, rows read and bytes spilled; who sees their texts
//! (RULES.md D520); and STATEMENTS.KEPT, which bounds them and resets them.

use turborel::db::{Database, Session};
use turborel::value::{Decimal, Value};

use crate::common::rows;

/// What a test reads of the tallies of `user`'s statements: each text, its
/// calls and its rows given or changed, in order of text.
fn tallies(reader: &Session, user: &str) -> Vec<String> {
    let sql = format!(
        "SELECT STATEMENT_TEXT, CALLS, ROW_COUNT FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS \
         WHERE USER_NAME = '{user}' ORDER BY STATEMENT_TEXT"
    );
    rows(reader, &sql).unwrap()
}

/// A database whose owner is OWNER, with X's table T of one row.
fn owned() -> Database {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    db
}

/// A count, as a setting takes it.
fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md
/// D521: runs differing in their literals alone are one statement, its
/// calls each run, its rows those changed, or a cursor's those fetched; a
/// prepared statement's executions are one statement of its text; its
/// rows read are those its scans read.
#[test]
fn statements_are_tallied_by_their_normalized_text() {
    let db = owned();
    let x = db.session("X");
    x.run("INSERT INTO T VALUES (1)").unwrap();
    x.run("insert into t values (22)").unwrap();
    let insert = x.prepare("INSERT INTO T VALUES (?)").unwrap();
    for k in [3, 4, 5] {
        x.execute_prepared(&insert, &[number(k)]).unwrap();
    }
    assert_eq!(rows(&x, "SELECT K FROM T WHERE K > 2 ORDER BY K").unwrap(), ["3", "4", "5", "22"]);
    x.commit().unwrap();
    let owner = db.session("OWNER");
    assert_eq!(
        tallies(&owner, "X"),
        ["INSERT INTO T VALUES (?),5,5", "SELECT K FROM T WHERE K > ? ORDER BY K,1,4"],
        "the two texts and the prepared one are one"
    );
    let read = "SELECT ROWS_READ, MAX_MICROS >= MEAN_MICROS, TOTAL_MICROS >= MAX_MICROS FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS \
                WHERE USER_NAME = 'X' AND STATEMENT_TEXT LIKE 'SELECT%'";
    assert_eq!(rows(&owner, read).unwrap(), ["5,TRUE,TRUE"], "every row of T read");
}

/// Verifies: [DESIGN] RULES.md D520, D521: the database's owner sees every
/// statement's text; another user sees each row, the texts of others'
/// statements as the null value, and their own; a user granted the role
/// MONITOR the owner created sees them all, but not one granted a role
/// MONITOR another user created. Pairs: (owner, role) = (yes, -), (no,
/// none), (no, the owner's), (no, another's).
#[test]
fn another_users_statement_text_is_null() {
    let db = owned();
    db.create_user("Y", b"x").unwrap();
    db.create_user("Z", b"x").unwrap();
    let owner = db.session("OWNER");
    owner.run("ALTER USER Z CREATE ROLE").unwrap();
    owner.commit().unwrap();
    db.session("X").run("INSERT INTO T VALUES (1)").unwrap();
    let y = db.session("Y");
    rows(&y, "VALUES 1").unwrap();
    let texts = "SELECT USER_NAME, STATEMENT_TEXT FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS \
                 WHERE STATEMENT_TEXT IS NULL OR STATEMENT_TEXT NOT LIKE 'SELECT%' ORDER BY USER_NAME";
    assert_eq!(rows(&owner, texts).unwrap(), ["OWNER,ALTER USER Z CREATE ROLE", "X,INSERT INTO T VALUES (?)", "Y,VALUES ?"]);
    assert_eq!(rows(&y, texts).unwrap(), ["OWNER,NULL", "OWNER,NULL", "X,NULL", "Y,VALUES ?"], "another's hidden");
    let z = db.session("Z");
    z.run("CREATE ROLE MONITOR").unwrap();
    z.run("GRANT MONITOR TO Y").unwrap();
    z.commit().unwrap();
    assert_eq!(
        rows(&y, texts).unwrap(),
        ["OWNER,NULL", "OWNER,NULL", "X,NULL", "Y,VALUES ?", "Z,NULL", "Z,NULL"],
        "another user's role gives nothing"
    );
    z.run("DROP ROLE MONITOR CASCADE").unwrap();
    z.commit().unwrap();
    owner.run("CREATE ROLE MONITOR").unwrap();
    owner.run("GRANT MONITOR TO Y").unwrap();
    owner.commit().unwrap();
    let seen = rows(&y, texts).unwrap();
    assert!(seen.iter().all(|r| !r.ends_with("NULL")), "the owner's role shows every text: {seen:?}");
}

/// Verifies: [DESIGN] RULES.md D521: STATEMENTS.KEPT bounds a session's
/// tallies, a new one past them taking the place of the one called least;
/// at 0 none is kept, those kept are forgotten, and none is tallied
/// until it is set again.
#[test]
fn statements_kept_bounds_and_resets_them() {
    let db = owned();
    db.set_setting("STATEMENTS.KEPT", &number(2)).unwrap();
    let x = db.session("X");
    for _ in 0..3 {
        rows(&x, "VALUES 1").unwrap();
    }
    rows(&x, "VALUES 'a'").unwrap();
    rows(&x, "VALUES (1, 2)").unwrap();
    let owner = db.session("OWNER");
    assert_eq!(tallies(&owner, "X"), ["VALUES (?, ?),1,1", "VALUES ?,4,4"], "the one called least gave way");
    db.set_setting("STATEMENTS.KEPT", &number(0)).unwrap();
    rows(&x, "VALUES 1").unwrap();
    assert_eq!(tallies(&owner, "X"), Vec::<String>::new(), "forgotten");
    db.set_setting("STATEMENTS.KEPT", &number(10)).unwrap();
    rows(&x, "VALUES 1").unwrap();
    assert_eq!(tallies(&owner, "X"), ["VALUES ?,1,1"], "tallied afresh");
}

/// Verifies: [DESIGN] RULES.md D521: a session's tallies outlive it,
/// merged with those of sessions ended.
#[test]
fn an_ended_sessions_statements_are_kept() {
    let db = owned();
    for _ in 0..2 {
        let x = db.session("X");
        rows(&x, "VALUES 1").unwrap();
        x.commit().unwrap();
    }
    assert_eq!(tallies(&db.session("OWNER"), "X"), ["VALUES ?,2,2"]);
}

/// Verifies: [DESIGN] RULES.md D521, D38: a sort past MEMORY.WORK_BYTES
/// counts the bytes it spilled; one held in memory none.
#[test]
fn a_spilled_sort_counts_its_bytes() {
    let db = owned();
    let x = db.session("X");
    x.run("INSERT INTO T VALUES (1)").unwrap();
    for _ in 0..14 {
        x.run("INSERT INTO T SELECT K + 1 FROM T").unwrap();
    }
    x.commit().unwrap();
    x.run("SET MEMORY.WORK_BYTES = 65536").unwrap();
    rows(&x, "SELECT K FROM T ORDER BY K DESC").unwrap();
    rows(&x, "SELECT K FROM T WHERE K = 1").unwrap();
    let spilled = "SELECT STATEMENT_TEXT, BYTES_SPILLED > 0 FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS \
                   WHERE STATEMENT_TEXT LIKE 'SELECT K FROM T%' ORDER BY STATEMENT_TEXT";
    assert_eq!(rows(&db.session("OWNER"), spilled).unwrap(), ["SELECT K FROM T ORDER BY K DESC,TRUE", "SELECT K FROM T WHERE K = ?,FALSE"]);
}

/// Verifies: [DESIGN] RULES.md D521: a session that runs as another user
/// tallies its statements under that user, by name, and under the first
/// again once it runs as them again; the user it runs as is kept with its
/// tallies, so a statement by the same user as the last takes no lock to
/// find them. The owner's session sets the identifier [ISO/IEC 9075:1992
/// §16.4 GR5].
#[test]
fn a_session_tallies_each_user_it_runs_as() {
    let db = owned();
    db.create_user("Y", b"x").unwrap();
    let s = db.session("OWNER");
    let users = "SELECT 1 FROM INFORMATION_SCHEMA.USERS";
    for run_as in ["'Y'", "'OWNER'"] {
        rows(&s, users).unwrap();
        s.commit().unwrap();
        s.run(&format!("SET SESSION AUTHORIZATION {run_as}")).unwrap();
    }
    rows(&s, users).unwrap();
    s.commit().unwrap();
    let reader = db.session("OWNER");
    // Each SELECT's text and calls.
    let selects = |user: &str| -> Vec<String> {
        let all = tallies(&reader, user).into_iter().filter(|t| t.starts_with("SELECT ? FROM"));
        all.filter_map(|t| t.rsplit_once(',').map(|(called, _)| called.to_owned())).collect()
    };
    assert_eq!(selects("OWNER"), ["SELECT ? FROM INFORMATION_SCHEMA.USERS,2"], "the owner's, before and after");
    assert_eq!(selects("Y"), ["SELECT ? FROM INFORMATION_SCHEMA.USERS,1"], "Y's");
}
