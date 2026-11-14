//! Sequence generators [ISO/IEC 9075-2:2023 §4.29, §6.14, §9.35 to §9.37,
//! §11.72 to §11.74; RULES.md D62]: their definition, alteration and drop,
//! the values NEXT VALUE FOR draws and where it may, USAGE on them, and a
//! value never given twice, by a rollback, a crash or a checkpoint.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::{Code, SqlError};

use crate::common::{Scratch, after_crash};

fn query(s: &Session, sql: &str) -> Vec<String> {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE class and SQLSTATE of a refusal.
fn refused<T>(result: Result<T, SqlError>) -> (Option<Code>, String) {
    let e = result.err();
    (e.as_ref().map(|e| e.code), e.map(|e| e.sqlstate().as_str().to_owned()).unwrap_or_default())
}

/// The class of a refusal, a Syntax Rule's.
fn rule<T>(result: Result<T, SqlError>) -> Option<Code> {
    result.err().map(|e| e.code)
}

/// A database with schema A, table T of three rows.
fn three_rows() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, V INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 0), (2, 0), (3, 0)").unwrap();
    a.commit().unwrap();
    db
}

/// The next `n` values of A's sequence generator `name`.
fn drawn(s: &Session, name: &str, n: usize) -> Vec<String> {
    (0..n).flat_map(|_| query(s, &format!("SELECT NEXT VALUE FOR {name} FROM (VALUES (0)) AS X (Z)"))).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.36 SR7 to SR9, SR13, §11.72 SR6,
/// §9.35 GR3] a sequence generator of no options ascends from 1 by 1, of
/// NUMERIC of the greatest precision (ID157); a descending one of no start
/// starts at -1, its maximum (ID112), and one given a start, an increment
/// and bounds steps within them.
#[test]
fn defaults_and_options_9_36() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S").unwrap();
    a.run("CREATE SEQUENCE D INCREMENT BY -2").unwrap();
    a.run("CREATE SEQUENCE B AS SMALLINT START WITH 10 INCREMENT BY 5 MINVALUE 0 MAXVALUE 20").unwrap();
    assert_eq!(drawn(&a, "S", 3), ["1", "2", "3"]);
    assert_eq!(drawn(&a, "D", 2), ["-1", "-3"]);
    assert_eq!(drawn(&a, "B", 3), ["10", "15", "20"]);
    let described = "SELECT DATA_TYPE, NUMERIC_PRECISION, START_VALUE, MINIMUM_VALUE, MAXIMUM_VALUE, INCREMENT, CYCLE_OPTION
                     FROM INFORMATION_SCHEMA.SEQUENCES WHERE SEQUENCE_NAME = 'S'";
    assert_eq!(query(&a, described), ["NUMERIC,18,1,1,999999999999999999,1,NO"]);
    let descending = "SELECT MINIMUM_VALUE, MAXIMUM_VALUE FROM INFORMATION_SCHEMA.SEQUENCES WHERE SEQUENCE_NAME = 'D'";
    assert_eq!(query(&a, descending), ["-999999999999999999,-1"], "ID113 and ID112");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.35 GR3a, GR3b] past its maximum a
/// generator of NO CYCLE raises SQLSTATE 2200H, each time, and one of
/// CYCLE starts again at its minimum ascending, its maximum descending.
#[test]
fn exhausted_or_cycled_9_35() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE N START WITH 1 MAXVALUE 2").unwrap();
    a.run("CREATE SEQUENCE C START WITH 2 INCREMENT BY 2 MINVALUE 1 MAXVALUE 5 CYCLE").unwrap();
    a.run("CREATE SEQUENCE D START WITH 3 INCREMENT BY -2 MINVALUE 1 MAXVALUE 4 CYCLE").unwrap();
    assert_eq!(drawn(&a, "N", 2), ["1", "2"]);
    let next = "SELECT NEXT VALUE FOR N FROM (VALUES (0)) AS X (Z)";
    for _ in 0..2 {
        let fetched = a.open_cursor(next).and_then(|mut cursor| cursor.fetch());
        assert_eq!(refused(fetched), (Some(Code::Data), "2200H".to_owned()));
    }
    assert_eq!(drawn(&a, "C", 4), ["2", "4", "1", "3"]);
    assert_eq!(drawn(&a, "D", 4), ["3", "1", "4", "2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.14 GR1, §4.29.1] each row of a query
/// draws one value of a generator however many times it names it, in its
/// select list, an INSERT's VALUES or query, and an UPDATE's SET; each row
/// a value of its own.
#[test]
fn one_value_a_row_6_14() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S").unwrap();
    let pairs = query(&a, "SELECT NEXT VALUE FOR S, NEXT VALUE FOR S + 100 FROM T ORDER BY 1");
    assert_eq!(pairs, ["1,101", "2,102", "3,103"]);
    a.run("CREATE TABLE U (K NUMERIC(18), L NUMERIC(18))").unwrap();
    a.run("INSERT INTO U VALUES (NEXT VALUE FOR S, NEXT VALUE FOR S), (NEXT VALUE FOR S, 0)").unwrap();
    a.run("INSERT INTO U SELECT NEXT VALUE FOR S, NEXT VALUE FOR S FROM T WHERE K = 1").unwrap();
    assert_eq!(query(&a, "SELECT K, L FROM U ORDER BY K"), ["4,4", "5,0", "6,6"]);
    a.run("UPDATE T SET K = NEXT VALUE FOR S, V = NEXT VALUE FOR S").unwrap();
    assert_eq!(query(&a, "SELECT K - V FROM T"), ["0", "0", "0"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT K) FROM T"), ["3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.14 SR1, SR2] NEXT VALUE FOR is
/// refused where it may not be (SQLCODE -103): a search condition, a CASE,
/// a set function, a grouped query or one of DISTINCT, a subquery, a view,
/// a constraint, an UPDATE's or DELETE's search condition; ORDER BY, whose
/// sort keys are columns, and a default do not parse it (SQLCODE -102).
#[test]
fn where_it_may_not_be_6_14() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S").unwrap();
    for sql in [
        "SELECT K FROM T WHERE K < NEXT VALUE FOR S",
        "SELECT CASE WHEN K = 1 THEN NEXT VALUE FOR S END FROM T",
        "SELECT SUM(NEXT VALUE FOR S) FROM T",
        "SELECT K, NEXT VALUE FOR S FROM T GROUP BY K",
        "SELECT DISTINCT NEXT VALUE FOR S FROM T",
        "SELECT K FROM T WHERE K IN (SELECT NEXT VALUE FOR S FROM T)",
        "SELECT X FROM (SELECT NEXT VALUE FOR S FROM T) AS D (X)",
        "SELECT K FROM T UNION SELECT NEXT VALUE FOR S FROM T",
    ] {
        let e = a.open_cursor(sql).err();
        assert_eq!(e.as_ref().map(|e| e.code), Some(Code::SyntaxRule), "{sql}: {e:?}");
    }
    let ordered = a.open_cursor("SELECT K FROM T ORDER BY NEXT VALUE FOR S").err();
    assert_eq!(ordered.map(|e| e.code), Some(Code::SyntaxRule), "[§6.14 SR1] none in a sort key");
    let defaulted = a.run("CREATE TABLE F (K NUMERIC(18) DEFAULT NEXT VALUE FOR S)");
    assert_eq!(rule(defaulted), Some(Code::Syntax), "a default is a literal or a function of no arguments");
    for sql in [
        "CREATE VIEW W AS SELECT NEXT VALUE FOR S AS N FROM T",
        "CREATE TABLE C (K INTEGER CHECK (K < NEXT VALUE FOR S))",
        "UPDATE T SET K = 1 WHERE K = NEXT VALUE FOR S",
        "DELETE FROM T WHERE K = NEXT VALUE FOR S",
    ] {
        assert_eq!(rule(a.run(sql)), Some(Code::SyntaxRule), "{sql}");
    }
    assert_eq!(drawn(&a, "S", 1), ["1"], "none drawn by those refused");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.36 SR5, SR10 to SR12, §11.72 SR3,
/// SR5, SR6] a definition is refused (SQLCODE -103) of an increment of 0,
/// a maximum not above its minimum, a start beyond them, a value its type
/// cannot hold, a type not exact of scale 0, an option given twice, or a
/// name its schema has already; and NEXT VALUE FOR of none.
#[test]
fn definitions_refused_11_72() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S").unwrap();
    a.commit().unwrap();
    for sql in [
        "CREATE SEQUENCE X INCREMENT BY 0",
        "CREATE SEQUENCE X MINVALUE 5 MAXVALUE 5",
        "CREATE SEQUENCE X START WITH 9 MAXVALUE 8",
        "CREATE SEQUENCE X START WITH 0",
        "CREATE SEQUENCE X AS SMALLINT MAXVALUE 100000",
        "CREATE SEQUENCE X START WITH 1.5",
        "CREATE SEQUENCE X AS DECIMAL(5, 2)",
        "CREATE SEQUENCE X AS REAL",
        "CREATE SEQUENCE X START WITH 1 START WITH 2",
        "CREATE SEQUENCE X CYCLE NO CYCLE",
        "CREATE SEQUENCE X NO MINVALUE MINVALUE 3",
        "CREATE SEQUENCE S",
    ] {
        assert_eq!(rule(a.run(sql)), Some(Code::SyntaxRule), "{sql}");
        a.rollback().unwrap();
    }
    assert_eq!(rule(a.run("CREATE SEQUENCE X MAXVALUE NO MAXVALUE")), Some(Code::Syntax));
    assert_eq!(a.open_cursor("SELECT NEXT VALUE FOR NONE FROM T").err().map(|e| e.code), Some(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.73, §9.37 GR1, SR12; T177] ALTER
/// SEQUENCE RESTART goes on from the start, RESTART WITH from the value
/// given; one not restarting keeps the current base value, its increment
/// changed, and is refused if that is beyond its new bounds, but not if
/// the generator has none left.
#[test]
fn alter_sequence_11_73() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S START WITH 5 MAXVALUE 100").unwrap();
    a.commit().unwrap();
    assert_eq!(drawn(&a, "S", 2), ["5", "6"]);
    a.run("ALTER SEQUENCE S INCREMENT BY 10").unwrap();
    assert_eq!(drawn(&a, "S", 2), ["7", "17"]);
    a.run("ALTER SEQUENCE S RESTART").unwrap();
    assert_eq!(drawn(&a, "S", 1), ["5"]);
    a.run("ALTER SEQUENCE S RESTART WITH 50").unwrap();
    assert_eq!(drawn(&a, "S", 1), ["50"]);
    a.commit().unwrap();
    assert_eq!(rule(a.run("ALTER SEQUENCE S MAXVALUE 40")), Some(Code::SyntaxRule), "60 beyond it");
    assert_eq!(rule(a.run("ALTER SEQUENCE S RESTART WITH 101")), Some(Code::SyntaxRule));
    assert_eq!(rule(a.run("ALTER SEQUENCE S RESTART WITH 1 RESTART")), Some(Code::SyntaxRule), "twice");
    a.rollback().unwrap();
    a.run("ALTER SEQUENCE S RESTART WITH 100").unwrap();
    assert_eq!(drawn(&a, "S", 1), ["100"]);
    a.run("ALTER SEQUENCE S MAXVALUE 100 NO CYCLE").unwrap();
    assert_eq!(rule(a.run("ALTER SEQUENCE NONE RESTART")), Some(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.74, §11.2 GR3] DROP SEQUENCE, of
/// RESTRICT or CASCADE, drops it, so it is named no more; DROP SCHEMA of
/// RESTRICT is refused while the schema holds one, and of CASCADE drops
/// it; a generator made again of the name starts anew.
#[test]
fn drop_sequence_11_74() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S").unwrap();
    a.run("CREATE SEQUENCE R").unwrap();
    assert_eq!(drawn(&a, "S", 2), ["1", "2"]);
    a.run("DROP SEQUENCE S RESTRICT").unwrap();
    a.run("DROP SEQUENCE R CASCADE").unwrap();
    assert_eq!(a.open_cursor("SELECT NEXT VALUE FOR S FROM T").err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(rule(a.run("DROP SEQUENCE S RESTRICT")), Some(Code::SyntaxRule));
    assert_eq!(rule(a.run("DROP SEQUENCE S")), Some(Code::Syntax), "a drop behavior is required");
    a.run("CREATE SEQUENCE S").unwrap();
    assert_eq!(drawn(&a, "S", 1), ["1"]);
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE SEQUENCE Q").unwrap();
    let b = db.session("B");
    assert_eq!(rule(b.run("DROP SCHEMA B RESTRICT")), Some(Code::SyntaxRule), "it holds Q");
    b.rollback().unwrap();
    b.run("DROP SCHEMA B CASCADE").unwrap();
    b.commit().unwrap();
    assert_eq!(rule(a.open_cursor("SELECT NEXT VALUE FOR B.Q FROM T")), Some(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.29.1, §9.35] a value drawn is never
/// drawn again: not after the transaction that drew it rolls back, nor by
/// a CREATE SEQUENCE rolled back and made again; nor by concurrent
/// sessions, each value drawn once.
#[test]
fn never_drawn_twice() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S").unwrap();
    a.commit().unwrap();
    assert_eq!(drawn(&a, "S", 2), ["1", "2"]);
    a.rollback().unwrap();
    assert_eq!(drawn(&a, "S", 1), ["3"]);
    a.commit().unwrap();
    let mut threads = Vec::new();
    for _ in 0..4 {
        let db = db.clone();
        threads.push(std::thread::spawn(move || {
            let s = db.session("A");
            let values = drawn(&s, "S", 200);
            s.commit().unwrap();
            values
        }));
    }
    let mut all: Vec<i64> = threads.into_iter().flat_map(|t| t.join().unwrap()).map(|v| v.parse().unwrap()).collect();
    all.sort_unstable();
    assert_eq!(all, (4..804).collect::<Vec<i64>>());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.14 AR1b, §11.72 GR2, §11.73 AR1,
/// §11.74 AR1, §12.2, §12.7] NEXT VALUE FOR without USAGE is refused
/// (SQLCODE -104); GRANT USAGE ON SEQUENCE lets the grantee draw, and
/// REVOKE takes it away, refused with RESTRICT (SQLSTATE 2B000) while a
/// grant it gave depends on it; only the owner alters or drops one.
#[test]
fn usage_on_a_sequence_12_2() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S").unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let b = db.session("B");
    let next = "SELECT NEXT VALUE FOR A.S FROM (VALUES (0)) AS X (Z)";
    assert_eq!(b.open_cursor(next).err().map(|e| e.code), Some(Code::Privilege));
    let visible = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.SEQUENCES";
    assert_eq!(query(&b, visible), ["0"]);
    a.run("GRANT USAGE ON SEQUENCE S TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&b, next), ["1"]);
    assert_eq!(query(&b, visible), ["1"]);
    let usage = "SELECT GRANTOR, GRANTEE, IS_GRANTABLE FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES WHERE OBJECT_TYPE = 'SEQUENCE'";
    assert_eq!(query(&b, usage), ["A,B,YES"]);
    b.run("GRANT USAGE ON SEQUENCE A.S TO C").unwrap();
    b.commit().unwrap();
    assert_eq!(refused(a.run("REVOKE USAGE ON SEQUENCE S FROM B RESTRICT")), (Some(Code::SyntaxRule), "2B000".to_owned()));
    a.rollback().unwrap();
    assert_eq!(rule(b.run("ALTER SEQUENCE A.S RESTART")), Some(Code::Privilege));
    assert_eq!(rule(b.run("DROP SEQUENCE A.S CASCADE")), Some(Code::Privilege));
    b.rollback().unwrap();
    a.run("REVOKE USAGE ON SEQUENCE S FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(b.open_cursor(next).err().map(|e| e.code), Some(Code::Privilege));
    assert_eq!(db.session("C").open_cursor(next).err().map(|e| e.code), Some(Code::Privilege), "abandoned");
    assert_eq!(rule(a.run("GRANT SELECT ON SEQUENCE S TO B")), Some(Code::SyntaxRule), "USAGE only");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.72 SR2, §11.1, §12.2] a `<schema
/// definition>` defines sequence generators with its tables, and grants
/// USAGE on them, and refuses one named in another schema (SQLCODE -103).
#[test]
fn in_a_schema_definition_11_72() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE SEQUENCE S START WITH 7 CREATE TABLE T (K INTEGER) GRANT USAGE ON SEQUENCE S TO B")
        .unwrap();
    assert_eq!(drawn(&db.session("A"), "S", 1), ["7"]);
    assert_eq!(drawn(&db.session("B"), "A.S", 1), ["8"], "granted by the schema");
    let other = db.schema("CREATE SCHEMA AUTHORIZATION B CREATE SEQUENCE A.Q");
    assert_eq!(rule(other), Some(Code::SyntaxRule));
}

/// The database in `dir`, opened from its files by `fs`.
fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D62, D25, [ISO/IEC 9075-2:2023 §4.29.1] a
/// database reopened, after a close, a crash, or a crash after a
/// checkpoint, never draws a value drawn before, though the transactions
/// that drew them rolled back or never ended; a generator restarted and
/// committed goes on from its restart, and one made by a transaction the
/// crash ended is gone.
#[test]
fn never_drawn_again_after_reopening() {
    for (case, checkpoint, closed) in [("closed", false, true), ("crashed", false, false), ("checkpointed", true, false)] {
        let scratch = Scratch::new(&format!("sequences-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        before.schema("CREATE SCHEMA AUTHORIZATION A CREATE SEQUENCE S CREATE SEQUENCE R START WITH 10 CREATE SEQUENCE P").unwrap();
        let a = before.session("A");
        assert_eq!(drawn(&a, "S", 40), (1..=40).map(|v| v.to_string()).collect::<Vec<_>>());
        // Drawn before the checkpoint alone: its record is before the
        // checkpoint's start, so it goes on from the image.
        assert_eq!(drawn(&a, "P", 5), ["1", "2", "3", "4", "5"]);
        // An epoch given and never logged: one replayed is the logged one.
        a.run("CREATE SEQUENCE ROLLED").unwrap();
        a.rollback().unwrap();
        a.run("ALTER SEQUENCE R RESTART WITH 30").unwrap();
        a.commit().unwrap();
        if checkpoint {
            before.checkpoint().unwrap();
        }
        assert_eq!(drawn(&a, "S", 1), ["41"]);
        assert_eq!(drawn(&a, "R", 1), ["30"]);
        a.run("CREATE SEQUENCE GONE").unwrap();
        assert_eq!(drawn(&a, "GONE", 1), ["1"], "never committed");
        if closed {
            a.rollback().unwrap();
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        let a = db.session("A");
        let s: i64 = drawn(&a, "S", 1)[0].parse().unwrap();
        let r: i64 = drawn(&a, "R", 1)[0].parse().unwrap();
        let p: i64 = drawn(&a, "P", 1)[0].parse().unwrap();
        assert!(s > 41 && r > 30 && p > 5, "{case}: S at {s}, R at {r}, P at {p}");
        assert_eq!(
            a.open_cursor("SELECT NEXT VALUE FOR GONE FROM (VALUES (0)) AS X (Z)").err().map(|e| e.code),
            Some(Code::SyntaxRule),
            "{case}"
        );
        a.rollback().unwrap();
        db.close().unwrap();
        drop((db, before));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.14 GR1, §9.37] a statement run again,
/// prepared or kept, draws new values each time; a prepared query draws from
/// the generator as an ALTER that restarted it left it.
#[test]
fn statements_run_again_draw_anew() {
    let db = three_rows();
    let a = db.session("A");
    a.run("CREATE SEQUENCE S").unwrap();
    a.run("CREATE TABLE U (K NUMERIC(18))").unwrap();
    a.commit().unwrap();
    for _ in 0..3 {
        a.run("INSERT INTO U VALUES (NEXT VALUE FOR S)").unwrap();
        a.run("INSERT INTO U SELECT NEXT VALUE FOR S FROM T WHERE K = 1").unwrap();
    }
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT K), MIN(K), MAX(K) FROM U"), ["6,1,6"]);
    let p = a.prepare("SELECT NEXT VALUE FOR S FROM T WHERE K = 1").unwrap();
    let opened = |a: &Session| {
        let mut c = a.open_prepared(&p, &[]).unwrap();
        c.fetch().unwrap().unwrap()[0].to_string()
    };
    assert_eq!([opened(&a), opened(&a)], ["7", "8"]);
    a.commit().unwrap();
    a.run("ALTER SEQUENCE S RESTART WITH 100").unwrap();
    a.commit().unwrap();
    assert_eq!(opened(&a), "100");
}
