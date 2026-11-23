//! \[DESIGN\] Point operations (RULES.md D752). A kept query that probes a
//! whole unique key, prepared or a kept body's, reads its row straight
//! from the table, and gives the rows, errors and diagnostics it gives
//! with EXECUTOR.POINT_OPERATIONS OFF, through the executor. So does a
//! correlated subquery that probes one (RULES.md D900).

use turborel::db::{Database, Session};
use turborel::value::{Decimal, Value};

/// Schema A: T (K, A, B, C), K its primary key, of 1500 rows, enough that a
/// probe of the key is chosen unpriced; L (K, GOT) the values a procedure
/// reads; and READ_ONE, which reads A of the row its K names into L.
fn defined() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, A INTEGER, B CHARACTER(6), C VARCHAR(20))
           CREATE TABLE L (K INTEGER, GOT INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let rows: Vec<String> = (1..=1500_i64).map(|k| format!("({k}, {}, 'b{k}', 'c{k}')", k * 10)).collect();
    a.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
    a.run("UPDATE T SET C = NULL WHERE K = 9").unwrap();
    a.run(
        "CREATE PROCEDURE READ_ONE (IN WANTED INTEGER) MODIFIES SQL DATA
         BEGIN
           DECLARE GOT INTEGER;
           SET GOT = (SELECT A FROM T WHERE K = WANTED);
           INSERT INTO L VALUES (WANTED, GOT);
         END",
    )
    .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The exact number `n`.
fn number(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// What `query`, prepared in `s`, gives for each of `args`. That is its
/// rows, each its values as text joined by commas, or its error at open
/// or fetch.
fn prepared(s: &Session, query: &str, args: &[Value]) -> Vec<String> {
    let p = s.prepare(query).unwrap();
    let mut out = Vec::new();
    for arg in args {
        match s.open_prepared(&p, std::slice::from_ref(arg)) {
            Ok(mut c) => loop {
                match c.fetch() {
                    Ok(Some(row)) => out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(",")),
                    Ok(None) => break out.push("end".to_owned()),
                    Err(e) => break out.push(format!("error at fetch {e}")),
                }
            },
            Err(e) => out.push(format!("error {e}")),
        }
    }
    out
}

/// What a session of `db` reads with EXECUTOR.POINT_OPERATIONS `how`. It
/// reads the rows of prepared point queries of found, missing and null
/// keys and of a value of the wrong type, the rows it changed itself and
/// not those another session changed and did not commit, and the values
/// READ_ONE reads into L.
fn read(db: &Database, how: &str) -> Vec<String> {
    let a = db.session("A");
    a.run(&format!("SET EXECUTOR.POINT_OPERATIONS = '{how}'")).unwrap();
    let keys = [number(3), number(9), number(2000), Value::Null, number(3), Value::Char("x".into())];
    let mut out = prepared(&a, "SELECT B, K, C FROM T WHERE K = ?", &keys);
    out.extend(prepared(&a, "SELECT * FROM T WHERE K = ?", &keys));
    out.extend(prepared(&a, "SELECT C, A, C FROM T WHERE K = ?", &keys));
    let other = db.session("A");
    other.run("UPDATE T SET A = -1 WHERE K = 4").unwrap();
    a.run("UPDATE T SET A = -2 WHERE K = 5").unwrap();
    out.extend(prepared(&a, "SELECT K, A FROM T WHERE K = ?", &[number(4), number(5)]));
    other.rollback().unwrap();
    for key in ["3", "5", "2000", "CAST(NULL AS INTEGER)"] {
        a.run(&format!("CALL READ_ONE({key})")).unwrap();
    }
    a.rollback().unwrap();
    out.extend(written(&a));
    out
}

/// What prepared point UPDATEs and DELETEs of `s` do, undone after. That
/// is each completion or error, of a key found, a key missing, a value
/// that breaks the primary key and one of the wrong type, and the rows
/// left.
fn written(s: &Session) -> Vec<String> {
    let update = s.prepare("UPDATE T SET A = A + ?, C = 'n' WHERE K = ?").unwrap();
    let moved = s.prepare("UPDATE T SET K = ? WHERE K = ?").unwrap();
    let delete = s.prepare("DELETE FROM T WHERE K = ?").unwrap();
    let mut out = Vec::new();
    let mut done = |r: turborel::error::Result<turborel::db::Completion>| out.push(format!("{r:?}"));
    for k in [11, 11, 2000] {
        done(s.execute_prepared(&update, &[number(1), number(k)]));
    }
    done(s.execute_prepared(&update, &[Value::Char("x".into()), number(12)]));
    done(s.execute_prepared(&moved, &[number(13), number(14)]));
    done(s.execute_prepared(&moved, &[number(1601), number(14)]));
    for k in [15, 15, 2000] {
        done(s.execute_prepared(&delete, &[number(k)]));
    }
    let left = s.prepare("SELECT K, A, C FROM T WHERE K = ?").unwrap();
    for k in [11, 12, 14, 15, 1601] {
        let mut c = s.open_prepared(&left, &[number(k)]).unwrap();
        while let Some(row) = c.fetch().unwrap() {
            out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
        }
    }
    s.rollback().unwrap();
    out
}

/// Verifies: [DESIGN] RULES.md D752: prepared point queries and a kept
/// body's read of a row by its whole key give the same rows and errors
/// with point operations on as with them off. The cases are a key found, one of a row
/// with a null, one missing, a null key, columns in another order and one
/// twice, a value of another type refused, a row the session changed seen
/// changed and one another session changed seen as committed.
#[test]
fn point_operations_read_as_the_executor_does() {
    let (db, a) = defined();
    let on = read(&db, "ON");
    let off = read(&db, "OFF");
    assert_eq!(on, off);
    assert!(on.contains(&"b3,3,c3".to_owned()), "{on:?}");
    assert!(on.contains(&"9,90,b9,NULL".to_owned()), "{on:?}");
    assert!(on.contains(&"c3,30,c3".to_owned()), "{on:?}");
    assert!(on.contains(&"4,40".to_owned()) && on.contains(&"5,-2".to_owned()), "{on:?}");
    assert!(on.iter().any(|r| r.starts_with("error")), "{on:?}");
    assert!(on.contains(&"11,112,n".to_owned()) && on.contains(&"1601,140,c14".to_owned()), "{on:?}");
    assert!(!on.iter().any(|r| r.starts_with("15,")), "{on:?}");
    drop(a);
}

/// Verifies: [DESIGN] RULES.md D752: a kept body's read of a row by its
/// whole key, run as a point read, reads the row each CALL's value names.
#[test]
fn a_kept_body_reads_each_calls_row_by_point_operations() {
    let (_db, a) = defined();
    for how in ["ON", "OFF"] {
        a.run(&format!("SET EXECUTOR.POINT_OPERATIONS = '{how}'")).unwrap();
        a.run("DELETE FROM L").unwrap();
        for key in ["3", "5", "2000", "CAST(NULL AS INTEGER)", "5"] {
            a.run(&format!("CALL READ_ONE({key})")).unwrap();
        }
        let mut c = a.open_cursor("SELECT K, GOT FROM L").unwrap();
        let mut got = Vec::new();
        while let Some(row) = c.fetch().unwrap() {
            got.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
        }
        got.sort();
        assert_eq!(got, ["2000,NULL", "3,30", "5,50", "5,50", "NULL,NULL"], "{how}");
        a.rollback().unwrap();
    }
}

/// What `query`, prepared in `s`, gives for `arg`, read to its end.
fn once(s: &Session, query: &str, arg: i64) -> Vec<String> {
    prepared(s, query, &[number(arg)])
}

/// Verifies: [DESIGN] RULES.md D881, D443: a point read through its
/// index's whole key keeps a row only if the version its transaction sees
/// has the key. A row whose key was changed keeps the entry of its old
/// key while a snapshot before the change is open, and a read of the old
/// key gives no row, of the new key the row.
#[test]
fn a_point_read_passes_over_an_entry_of_a_key_its_row_left() {
    let (db, a) = defined();
    let query = "SELECT K, A FROM T WHERE K = ?";
    for _ in 0..4 {
        assert_eq!(once(&a, query, 7), ["7,70", "end"]);
    }
    a.commit().unwrap();
    // A snapshot from before the change, so the old version is kept.
    let earlier = db.session("A");
    earlier.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    assert_eq!(once(&earlier, query, 7), ["7,70", "end"]);
    a.run("UPDATE T SET K = 2007 WHERE K = 7").unwrap();
    a.commit().unwrap();
    assert_eq!(once(&a, query, 7), ["end"], "the entry of the key it left finds no row");
    assert_eq!(once(&a, query, 2007), ["2007,70", "end"]);
    // A point UPDATE reads its object row whole, by the same entry.
    let update = a.prepare("UPDATE T SET A = A + 1 WHERE K = ?").unwrap();
    let changed = |key: i64| format!("{:?}", a.execute_prepared(&update, &[number(key)]));
    for _ in 0..4 {
        assert_eq!(changed(3), changed(4));
    }
    assert_eq!(changed(7), changed(5_000), "no row of the key it left is changed");
    assert_eq!(once(&a, query, 2007), ["2007,70", "end"]);
    assert_eq!(once(&earlier, query, 7), ["7,70", "end"], "the snapshot before still reads it");
    earlier.rollback().unwrap();
    a.rollback().unwrap();
}

/// Verifies: [DESIGN] RULES.md D881, D29: a point read through its
/// index's whole key is recorded as read where reads are checked. A
/// SERIALIZABLE transaction that read a key with no row, and wrote, is
/// refused at its commit (SQLSTATE 40001) once another committed a row of
/// that key; at READ COMMITTED it commits.
#[test]
fn a_point_read_is_recorded_where_reads_are_checked() {
    let outcome = |level: &str, key: i64| {
        let (db, a) = defined();
        let query = "SELECT K, A FROM T WHERE K = ?";
        for _ in 0..4 {
            assert_eq!(once(&a, query, 3), ["3,30", "end"]);
        }
        a.commit().unwrap();
        a.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
        assert_eq!(once(&a, query, key), ["end"]);
        a.run(&format!("INSERT INTO L VALUES ({key}, 0)")).unwrap();
        let other = db.session("A");
        other.run(&format!("INSERT INTO T VALUES ({key}, 1, 'b', 'c')")).unwrap();
        other.commit().unwrap();
        a.commit().map_err(|e| e.sqlstate().as_str().to_owned())
    };
    assert_eq!(outcome("SERIALIZABLE", 5_000), Err("40001".to_owned()));
    assert_eq!(outcome("READ COMMITTED", 5_001), Ok(()));
}

/// The rows `query` gives `s`, each its values as text joined by commas;
/// or its error.
fn queried(s: &Session, query: &str) -> Vec<String> {
    let mut out = Vec::new();
    match s.open_cursor(query) {
        Ok(mut c) => loop {
            match c.fetch() {
                Ok(Some(row)) => out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(",")),
                Ok(None) => break,
                Err(e) => break out.push(format!("error at fetch {e}")),
            }
        },
        Err(e) => out.push(format!("error {e}")),
    }
    out
}

/// S (K, V, D), the outer rows of the subquery cases: V names a row of T
/// by its key, found, found twice, missing and null, and D names it as a
/// decimal, whole or not. P2 has a key of two columns.
const OUTER: [&str; 4] = [
    "CREATE TABLE P2 (A INTEGER NOT NULL, B INTEGER NOT NULL, V INTEGER, PRIMARY KEY (A, B))",
    "INSERT INTO P2 VALUES (1, 2, 12), (2, 1, 21), (1, 1, 11), (2, 2, 22), (3, 5, 35), (5, 3, 53)",
    "CREATE TABLE S (K INTEGER NOT NULL PRIMARY KEY, V INTEGER, D DECIMAL(6, 1))",
    "INSERT INTO S VALUES (1, 3, 3.0), (2, 5, 5.5), (3, 2000, 7.0), (4, NULL, NULL), (5, 3, 9.0), (6, 9, 2000.0), (7, 11, 11.0),
       (8, 12, 12.0), (9, 13, 13.5), (10, 14, 14.0), (11, 15, 15.0), (12, 5, 5.0)",
];

/// What the statements with a scalar subquery that probes T's whole key
/// give a session of `db` with EXECUTOR.POINT_OPERATIONS `how`: queries
/// with the subquery in the select list and in the condition, by an
/// integer and by a decimal, a prepared one run several times, an INSERT
/// and an UPDATE of its rows, and what it reads of a row it changed and
/// of one another session changed and did not commit.
fn subqueried(db: &Database, how: &str) -> Vec<String> {
    let a = db.session("A");
    a.run(&format!("SET EXECUTOR.POINT_OPERATIONS = '{how}'")).unwrap();
    let mut out = queried(&a, "SELECT K, (SELECT A FROM T WHERE T.K = S.V) FROM S ORDER BY K");
    // The elements of an array, as an order's lines are read.
    let lines = "UNNEST(ARRAY[3, 5, 2000, 3, 9, CAST(NULL AS INTEGER), 11, 12, 13, 14, 15, 5], ARRAY[1, 2, 2, 1, 5, 3, 3, 5, 1, 1, 2, 2])
       WITH ORDINALITY AS X (I, J, N)";
    out.extend(queried(
        &a,
        &format!("SELECT X.N, (SELECT A FROM T WHERE T.K = X.I), (SELECT C FROM T WHERE T.K = X.I + 0.5) FROM {lines}"),
    ));
    out.extend(queried(&a, &format!("SELECT X.N, (SELECT V FROM P2 WHERE P2.A = X.J AND P2.B = MOD(X.I, 6)) FROM {lines}")));
    out.extend(queried(&a, &format!("SELECT X.N FROM {lines} WHERE EXISTS (SELECT * FROM T WHERE T.K = X.I)")));
    out.push(format!("{:?}", a.run(&format!("INSERT INTO L SELECT X.N, (SELECT A FROM T WHERE T.K = X.I) FROM {lines}"))));
    out.extend(queried(&a, "SELECT K, (SELECT C FROM T WHERE T.K = S.D), (SELECT B FROM T WHERE T.K = S.V) FROM S ORDER BY K"));
    out.extend(queried(&a, "SELECT K FROM S WHERE (SELECT A FROM T WHERE T.K = S.V) > 40 ORDER BY K"));
    out.extend(queried(&a, "SELECT K FROM S WHERE EXISTS (SELECT * FROM T WHERE T.K = S.V) ORDER BY K"));
    out.extend(queried(&a, "SELECT K FROM S WHERE V IN (SELECT K FROM T WHERE T.K = S.V AND A > 40) ORDER BY K"));
    for least in [0, 3, 6, 9, 0] {
        out.extend(prepared(&a, "SELECT K, (SELECT A FROM T WHERE T.K = S.V) FROM S WHERE K > ? ORDER BY K", &[number(least)]));
        let lines = "UNNEST(ARRAY[3, 5, 2000, 9, 11, 12, 13, 14, 15]) AS X (I)";
        out.extend(prepared(&a, &format!("SELECT X.I, (SELECT A + ? FROM T WHERE T.K = X.I) FROM {lines}"), &[number(least)]));
    }
    let other = db.session("A");
    other.run("UPDATE T SET A = -1 WHERE K = 5").unwrap();
    a.run("UPDATE T SET A = -2 WHERE K = 3").unwrap();
    a.run("DELETE FROM T WHERE K = 9").unwrap();
    out.extend(queried(&a, "SELECT K, (SELECT A FROM T WHERE T.K = S.V) FROM S ORDER BY K"));
    for _ in 0..3 {
        out.push(format!("{:?}", a.run("INSERT INTO L SELECT K, (SELECT A FROM T WHERE T.K = S.V) FROM S")));
        out.push(format!("{:?}", a.run("UPDATE S SET V = (SELECT A FROM T WHERE T.K = S.V) / 10 WHERE K < 12")));
    }
    out.extend(queried(&a, "SELECT K, GOT FROM L ORDER BY K, GOT"));
    out.extend(queried(&a, "SELECT K, V FROM S ORDER BY K"));
    other.rollback().unwrap();
    a.rollback().unwrap();
    out
}

/// Verifies: [DESIGN] RULES.md D900: a correlated scalar subquery that
/// probes a whole unique key, run as a point read from its second run,
/// gives the rows and errors it gives through the executor, with
/// EXECUTOR.POINT_OPERATIONS OFF: in a select list, a condition, an
/// EXISTS, an INSERT and an UPDATE, of a prepared query's later
/// executions, by a key found, found twice, missing and null, and by a
/// decimal of the key or between two; a row its transaction changed or
/// deleted seen so, and one another changed seen as committed.
#[test]
fn a_subquery_of_a_whole_key_reads_as_the_executor_does() {
    let (db, a) = defined();
    for sql in OUTER {
        a.run(sql).unwrap();
    }
    a.commit().unwrap();
    let on = subqueried(&db, "ON");
    let off = subqueried(&db, "OFF");
    assert_eq!(on, off);
    assert!(on.starts_with(&["1,30", "2,50", "3,NULL", "4,NULL", "5,30", "6,90"].map(str::to_owned)), "{on:?}");
    assert!(on.contains(&"1,c3,b3".to_owned()) && on.contains(&"2,NULL,b5".to_owned()), "by a decimal: {on:?}");
    assert!(on.contains(&"1,-2".to_owned()) && on.contains(&"6,NULL".to_owned()), "its own changes: {on:?}");
    assert!(on.contains(&"2,50,NULL".to_owned()) && on.contains(&"7,110,NULL".to_owned()), "an array's elements: {on:?}");
    assert!(
        on.contains(&"1,NULL".to_owned()) && on.contains(&"2,NULL".to_owned()) && on.contains(&"4,NULL".to_owned()),
        "two columns: {on:?}"
    );
    assert!(!on.iter().any(|r| r.starts_with("error")), "{on:?}");
}

/// Verifies: [DESIGN] RULES.md D900, D29: a subquery's row read by its
/// point read is recorded as read where reads are checked, as its plan's
/// read is. A SERIALIZABLE transaction that inserted the rows of a query
/// whose subquery read a row of T by its key, or found none of a key, is
/// refused at its commit (SQLSTATE 40001) once another committed a change
/// of that row, or a row of that key; at READ COMMITTED it commits. So
/// with EXECUTOR.POINT_OPERATIONS OFF, through the executor.
#[test]
fn a_subquerys_point_read_is_recorded_where_reads_are_checked() {
    let outcome = |how: &str, level: &str, change: &str| {
        let (db, a) = defined();
        for sql in OUTER {
            a.run(sql).unwrap();
        }
        a.commit().unwrap();
        a.run(&format!("SET EXECUTOR.POINT_OPERATIONS = '{how}'")).unwrap();
        a.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
        a.run(
            "INSERT INTO L SELECT X.I, (SELECT A FROM T WHERE T.K = X.I)
               FROM UNNEST(ARRAY[3, 5, 2000, 3, 9, 11, 12, 13, 14, 15]) AS X (I)",
        )
        .unwrap();
        let other = db.session("A");
        other.run(change).unwrap();
        other.commit().unwrap();
        a.commit().map_err(|e| e.sqlstate().as_str().to_owned())
    };
    for how in ["ON", "OFF"] {
        // Row 15 and key 2000 are read by runs after the subquery's first.
        for change in ["UPDATE T SET A = 0 WHERE K = 15", "INSERT INTO T VALUES (2000, 1, 'b', 'c')"] {
            assert_eq!(outcome(how, "SERIALIZABLE", change), Err("40001".to_owned()), "{how}: {change}");
            assert_eq!(outcome(how, "READ COMMITTED", change), Ok(()), "{how}: {change}");
        }
        assert_eq!(outcome(how, "SERIALIZABLE", "UPDATE T SET A = 0 WHERE K = 700"), Ok(()), "{how}: a row it did not read");
    }
}
