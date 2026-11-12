//! The `<like clause>` and the `<as subquery clause>` of a `<table
//! definition>` [ISO/IEC 9075-2:2023 §11.3, Features T171 to T173]: the
//! columns each stands for, what INCLUDING copies, the rows WITH DATA
//! inserts, their Syntax and Access Rules, what the statement holds of
//! what it read, and recovery.

use std::sync::Arc;

use turborel::db::{Completion, Database, OpenOptions, Session};
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

/// The SQLCODE class of a refusal.
fn refused<T>(result: Result<T, SqlError>) -> Option<Code> {
    result.err().map(|e| e.code)
}

/// The columns of table `t` of schema A, in order, as the Information
/// Schema describes them: name, type, nullable, default, identity with its
/// generation, start and increment, generation expression, collation.
fn columns(s: &Session, t: &str) -> Vec<String> {
    query(
        s,
        &format!(
            "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT, IS_IDENTITY, IDENTITY_GENERATION, IDENTITY_START,
                    IDENTITY_INCREMENT, GENERATION_EXPRESSION, COLLATION_NAME, ORDINAL_POSITION
               FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = 'A' AND TABLE_NAME = '{t}' ORDER BY ORDINAL_POSITION"
        ),
    )
    .into_iter()
    .map(|row| row.rsplit_once(',').map_or(row.clone(), |(r, _)| r.to_owned()))
    .collect()
}

/// Schema A: T, of an identity column K from 5 by 2, a NOT NULL column N
/// of a default and a collation, a generated column G, a PRIMARY KEY P and
/// a CHECK; rows K 5 and 7.
fn source() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER GENERATED ALWAYS AS IDENTITY (START WITH 5 INCREMENT BY 2),
                           N VARCHAR(10) DEFAULT 'x' NOT NULL COLLATE CASE_INSENSITIVE,
                           G INTEGER GENERATED ALWAYS AS (T.K * 2 + A.T.K),
                           P DECIMAL(5,2) PRIMARY KEY, CHECK (N <> 'y'))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (N, P) VALUES ('a', 1), ('b', 2)").unwrap();
    a.commit().unwrap();
    db
}

/// T's columns copied: K, N, G and P, with no default, identity or
/// generation; N of its collation; K, N and P NOT NULL.
const COPIED: [&str; 4] = [
    "K,INTEGER,NO,NULL,NO,NULL,NULL,NULL,NULL,NULL",
    "N,CHARACTER VARYING,NO,NULL,NO,NULL,NULL,NULL,NULL,CASE_INSENSITIVE",
    "G,INTEGER,YES,NULL,NO,NULL,NULL,NULL,NULL,NULL",
    "P,DECIMAL,NO,NULL,NO,NULL,NULL,NULL,NULL,NULL",
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.3 SR9b iii, iv 2 to 4,
/// v; NOTE 538] a `<like clause>` stands for a column definition of each
/// column of the table it names, in its place among the others: its name,
/// its data type with its collation, and NOT NULL if known not nullable;
/// with no option, nor with EXCLUDING, no default, identity or generation;
/// no constraint but NOT NULL, so its CHECK and PRIMARY KEY are not copied.
/// Its rows are not copied either.
#[test]
fn like_columns_11_3() {
    let db = source();
    let a = db.session("A");
    a.run("CREATE TABLE L (LIKE T)").unwrap();
    assert_eq!(columns(&a, "L"), COPIED);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM L"), ["0"]);
    a.run("INSERT INTO L VALUES (1, 'y', NULL, 1), (1, 'Y', 3, 1)").unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM L WHERE N = 'y'"), ["2"], "its collation copied, its CHECK and key not");
    assert_eq!(refused(a.run("INSERT INTO L (K, P) VALUES (1, 1)")), Some(Code::Constraint), "N NOT NULL, and of no default");
    a.rollback().unwrap();
    a.run("CREATE TABLE M (X INTEGER, LIKE T EXCLUDING IDENTITY EXCLUDING DEFAULTS EXCLUDING GENERATED, Y INTEGER)").unwrap();
    let mut placed = vec!["X,INTEGER,YES,NULL,NO,NULL,NULL,NULL,NULL,NULL"];
    placed.extend(COPIED);
    placed.push("Y,INTEGER,YES,NULL,NO,NULL,NULL,NULL,NULL,NULL");
    assert_eq!(columns(&a, "M"), placed);
    a.run("CREATE VIEW V AS SELECT N, P * 2 AS D FROM T").unwrap();
    a.run("CREATE TABLE W (LIKE V)").unwrap();
    assert_eq!(
        columns(&a, "W"),
        ["N,CHARACTER VARYING,NO,NULL,NO,NULL,NULL,NULL,NULL,CASE_INSENSITIVE", "D,NUMERIC,YES,NULL,NO,NULL,NULL,NULL,NULL,NULL"],
        "a view's columns"
    );
    a.run("CREATE TABLE S (LIKE INFORMATION_SCHEMA.SCHEMATA)").unwrap();
    assert_eq!(
        query(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'S'"),
        ["6"],
        "a view of the Information Schema's"
    );
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.3 SR9b iv 5 to 7]
/// INCLUDING IDENTITY copies the identity column's generation and
/// options, as a generator of the new table's own, from its start value;
/// INCLUDING DEFAULTS each default; INCLUDING GENERATED each generation
/// expression, the table's name in it replaced by the new table's. MC/DC
/// of each option: with it, the column has what it copies; without it,
/// as `like_columns_11_3` shows, not.
#[test]
fn like_options_11_3() {
    let db = source();
    let a = db.session("A");
    a.run("CREATE TABLE L (LIKE T INCLUDING IDENTITY INCLUDING DEFAULTS INCLUDING GENERATED)").unwrap();
    assert_eq!(
        columns(&a, "L"),
        [
            "K,INTEGER,NO,NULL,YES,ALWAYS,5,2,NULL,NULL",
            "N,CHARACTER VARYING,NO,'x',NO,NULL,NULL,NULL,NULL,CASE_INSENSITIVE",
            "G,INTEGER,YES,NULL,NO,NULL,NULL,NULL,\"A\".\"L\".K * 2 + \"A\".\"L\".K,NULL",
            "P,DECIMAL,NO,NULL,NO,NULL,NULL,NULL,NULL,NULL",
        ]
    );
    a.run("INSERT INTO L (P) VALUES (1), (2)").unwrap();
    assert_eq!(query(&a, "SELECT K, N, G FROM L ORDER BY K"), ["5,x,15", "7,x,21"], "a generator of its own, from its start");
    a.run("INSERT INTO T (N, P) VALUES ('c', 3)").unwrap();
    assert_eq!(query(&a, "SELECT K FROM T WHERE P = 3"), ["9"], "T's generator goes on alone");
    a.run("CREATE TABLE D (LIKE T INCLUDING DEFAULTS)").unwrap();
    assert_eq!(
        columns(&a, "D")[0..2],
        ["K,INTEGER,NO,NULL,NO,NULL,NULL,NULL,NULL,NULL", "N,CHARACTER VARYING,NO,'x',NO,NULL,NULL,NULL,NULL,CASE_INSENSITIVE"],
        "the default alone"
    );
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.3 SR9b ii, iv 1, AR2;
/// X3.135-1986 §5.4 SR5, §6.3 SR1] a `<like clause>` writes each option
/// once, none of a viewed table; names a table there is; copies no column
/// of a name another has; and needs SELECT on what it names (SQLCODE
/// -104), which a grant gives. [DESIGN] No persistent table's copies a
/// session's declared local temporary table, whose definition the log
/// does not keep, but a declared one's may. In a `<schema definition>`, a
/// table defined before in it is named.
#[test]
fn like_rules_11_3() {
    let db = source();
    let a = db.session("A");
    let rule = |sql: &str| refused(a.run(sql));
    assert_eq!(rule("CREATE TABLE L (LIKE T INCLUDING IDENTITY EXCLUDING IDENTITY)"), Some(Code::SyntaxRule));
    assert_eq!(rule("CREATE TABLE L (LIKE T INCLUDING DEFAULTS INCLUDING DEFAULTS)"), Some(Code::SyntaxRule));
    a.run("CREATE VIEW V AS SELECT N FROM T").unwrap();
    assert_eq!(rule("CREATE TABLE L (LIKE V EXCLUDING DEFAULTS)"), Some(Code::SyntaxRule), "a viewed table's");
    assert_eq!(rule("CREATE TABLE L (LIKE NONE)"), Some(Code::SyntaxRule));
    assert_eq!(rule("CREATE TABLE L (N INTEGER, LIKE T)"), Some(Code::SyntaxRule), "N twice");
    assert_eq!(rule("CREATE TABLE L (LIKE L)"), Some(Code::SyntaxRule), "itself, not yet there");
    a.run("DECLARE LOCAL TEMPORARY TABLE MODULE.D (LIKE T INCLUDING DEFAULTS)").unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM MODULE.D"), ["0"]);
    a.run("DECLARE LOCAL TEMPORARY TABLE MODULE.E (LIKE MODULE.D)").unwrap();
    assert_eq!(rule("CREATE TABLE L (LIKE MODULE.D)"), Some(Code::SyntaxRule), "a declared table's");
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let b = db.session("B");
    assert_eq!(refused(b.run("CREATE TABLE L (LIKE A.T)")), Some(Code::Privilege));
    b.rollback().unwrap();
    a.run("GRANT SELECT ON T TO B").unwrap();
    a.commit().unwrap();
    b.run("CREATE TABLE L (LIKE A.T)").unwrap();
    b.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE C1 (X INTEGER NOT NULL) CREATE TABLE C2 (LIKE C1, Y INTEGER)").unwrap();
    let c = db.session("C");
    c.run("INSERT INTO C2 VALUES (1, NULL)").unwrap();
    assert_eq!(refused(c.run("INSERT INTO C2 VALUES (NULL, 1)")), Some(Code::Constraint), "X NOT NULL");
    c.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.3 SR10, GR10; §14.11
/// GR3] an `<as subquery clause>` stands for a column definition of each
/// column of its query, named as the query or its `<column name list>`
/// names it, of its declared type and collation, NOT NULL if known not
/// nullable. MC/DC of WITH DATA: with it, the query's rows are inserted,
/// none being no data; WITH NO DATA, none. The query may draw values of a
/// sequence generator [§6.14 SR1a ii]. A temporary table's rows are its
/// session's.
#[test]
fn as_subquery_11_3() {
    let db = source();
    let a = db.session("A");
    assert_eq!(a.run("CREATE TABLE Q AS (SELECT K, N, P * 2 AS D FROM T) WITH DATA").unwrap(), Completion::Success);
    assert_eq!(
        columns(&a, "Q"),
        [
            "K,INTEGER,NO,NULL,NO,NULL,NULL,NULL,NULL,NULL",
            "N,CHARACTER VARYING,NO,NULL,NO,NULL,NULL,NULL,NULL,CASE_INSENSITIVE",
            "D,NUMERIC,YES,NULL,NO,NULL,NULL,NULL,NULL,NULL",
        ]
    );
    assert_eq!(query(&a, "SELECT K, N, D FROM Q ORDER BY K"), ["5,a,2.00", "7,b,4.00"]);
    a.run("CREATE TABLE R (X, Y) AS (SELECT K, N FROM T) WITH NO DATA").unwrap();
    assert_eq!(columns(&a, "R")[0], "X,INTEGER,NO,NULL,NO,NULL,NULL,NULL,NULL,NULL");
    assert_eq!(query(&a, "SELECT COUNT(*) FROM R"), ["0"], "WITH NO DATA");
    assert_eq!(a.run("CREATE TABLE E AS (SELECT K FROM T WHERE K > 100) WITH DATA").unwrap(), Completion::NoData);
    a.run("CREATE SEQUENCE S").unwrap();
    a.run("CREATE TABLE Z AS (SELECT K, NEXT VALUE FOR S AS V FROM T) WITH DATA").unwrap();
    assert_eq!(query(&a, "SELECT V FROM Z ORDER BY V"), ["1", "2"]);
    a.run("CREATE GLOBAL TEMPORARY TABLE G AS (SELECT K FROM T) WITH DATA ON COMMIT PRESERVE ROWS").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT K FROM G ORDER BY K"), ["5", "7"], "the session's rows");
    assert_eq!(query(&db.session("A"), "SELECT COUNT(*) FROM G"), ["0"], "not another's");
    assert_eq!(query(&a, "SELECT K FROM Q ORDER BY K"), ["5", "7"], "committed");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.3 SR4, SR10b, SR10d,
/// SR10e] the query of an `<as subquery clause>` names each column, or
/// its list does, once each and one for each, and has no parameter; a
/// `<table subquery>` is in parentheses, and WITH DATA or WITH NO DATA is
/// written. [DESIGN] It is a statement of its own, in no `<schema
/// definition>`, and reads no declared local temporary table. A failure
/// inserting its rows defines no table, and the transaction goes on.
#[test]
fn as_rules_11_3() {
    let db = source();
    let a = db.session("A");
    let code = |sql: &str| refused(a.run(sql));
    assert_eq!(code("CREATE TABLE Q AS (SELECT K + 1 FROM T) WITH NO DATA"), Some(Code::SyntaxRule), "unnamed");
    assert_eq!(code("CREATE TABLE Q AS (SELECT K, K FROM T) WITH NO DATA"), Some(Code::SyntaxRule), "K twice");
    assert_eq!(code("CREATE TABLE Q (X) AS (SELECT K, N FROM T) WITH NO DATA"), Some(Code::SyntaxRule), "one name short");
    assert_eq!(code("CREATE TABLE Q (X, X) AS (SELECT K, N FROM T) WITH NO DATA"), Some(Code::SyntaxRule), "X twice");
    assert_eq!(code("CREATE TABLE Q AS SELECT K FROM T WITH DATA"), Some(Code::Syntax), "no parentheses");
    assert_eq!(code("CREATE TABLE Q AS (SELECT K FROM T)"), Some(Code::Syntax), "no WITH DATA");
    a.run("CREATE TABLE Q (X) AS (SELECT K + 1 FROM T) WITH NO DATA").unwrap();
    let parameter = a.prepare("CREATE TABLE P AS (SELECT K, ? AS V FROM T) WITH DATA").unwrap_err();
    assert_eq!(parameter.code, Code::SyntaxRule, "a dynamic parameter");
    assert!(parameter.msg.contains("<as subquery clause> has no dynamic parameter"), "{parameter}");
    a.run("DECLARE LOCAL TEMPORARY TABLE MODULE.D (X INTEGER)").unwrap();
    let declared = a.run("CREATE TABLE F AS (SELECT X FROM MODULE.D) WITH NO DATA").unwrap_err();
    assert_eq!(declared.code, Code::SyntaxRule, "a declared table");
    assert!(declared.msg.contains("declared local temporary"), "{declared}");
    assert_eq!(code("CREATE TABLE F AS (SELECT K / (K - 5) AS X FROM T) WITH DATA"), Some(Code::Data), "divided by zero");
    assert_eq!(refused(a.open_cursor("SELECT X FROM F")), Some(Code::SyntaxRule), "no table F");
    assert_eq!(query(&a, "SELECT COUNT(*) FROM Q"), ["0"], "the transaction goes on");
    a.commit().unwrap();
    let schema = db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE B1 AS (SELECT K FROM A.T) WITH NO DATA");
    assert_eq!(refused(schema), Some(Code::SyntaxRule), "in a <schema definition>");
}

/// Verifies: [DESIGN] RULES.md D65 the table a `<like clause>` names, and
/// the tables and sequence generators its query reads, are held shared to
/// the transaction's end, as its definition is made again from its text
/// at commit: another transaction's ALTER TABLE or DROP SEQUENCE of one is
/// refused meanwhile (SQLCODE -911, no wait), and goes once it commits.
#[test]
fn held_11_3() {
    let db = source();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("CREATE TABLE L (LIKE T)").unwrap();
    assert_eq!(refused(b.run("ALTER TABLE T ADD COLUMN X INTEGER")), Some(Code::Conflict), "the table a <like clause> names");
    b.rollback().unwrap();
    a.commit().unwrap();
    a.run("CREATE TABLE Q AS (SELECT P FROM T) WITH NO DATA").unwrap();
    assert_eq!(refused(b.run("ALTER TABLE T DROP COLUMN G RESTRICT")), Some(Code::Conflict), "a table the query reads");
    b.rollback().unwrap();
    a.commit().unwrap();
    a.run("CREATE SEQUENCE S").unwrap();
    a.commit().unwrap();
    a.run("CREATE TABLE Z AS (SELECT P, NEXT VALUE FOR S AS V FROM T) WITH NO DATA").unwrap();
    assert_eq!(refused(b.run("DROP SEQUENCE S RESTRICT")), Some(Code::Conflict), "a sequence generator the query draws of");
    b.rollback().unwrap();
    a.commit().unwrap();
    b.run("ALTER TABLE T ADD COLUMN X INTEGER").unwrap();
    b.commit().unwrap();
    assert_eq!(columns(&a, "L").len(), 4, "L as it was");
}

fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D25, D33 tables defined with a `<like
/// clause>` and an `<as subquery clause>` WITH DATA are as committed
/// after a close, a crash, and a crash after a checkpoint: their columns
/// found again from what they named, their rows kept, and the identity
/// column copied going on from where it was.
#[test]
fn recovered() {
    for (case, checkpoint, closed) in [("closed", false, true), ("crashed", false, false), ("checkpointed", true, false)] {
        let scratch = Scratch::new(&format!("contents-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        before.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER GENERATED ALWAYS AS IDENTITY, V CHAR(1) NOT NULL)").unwrap();
        let a = before.session("A");
        a.run("INSERT INTO T (V) VALUES ('a'), ('b')").unwrap();
        a.run("CREATE TABLE L (LIKE T INCLUDING IDENTITY, W INTEGER)").unwrap();
        a.run("INSERT INTO L (V) VALUES ('c')").unwrap();
        a.run("CREATE TABLE Q AS (SELECT K, V FROM T) WITH DATA").unwrap();
        a.commit().unwrap();
        if checkpoint {
            before.checkpoint().unwrap();
        }
        a.run("ALTER TABLE T ADD COLUMN X INTEGER").unwrap();
        a.commit().unwrap();
        if closed {
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        let a = db.session("A");
        assert_eq!(query(&a, "SELECT K, V, W FROM L"), ["1,c,NULL"], "{case}");
        assert_eq!(query(&a, "SELECT K, V FROM Q ORDER BY K"), ["1,a", "2,b"], "{case}");
        assert_eq!(columns(&a, "Q").len(), 2, "{case}: not of the column T has since");
        a.run("INSERT INTO L (V) VALUES ('d')").unwrap();
        let given: i64 = query(&a, "SELECT K FROM L WHERE V = 'd'")[0].parse().unwrap();
        assert!(given > 1, "{case}: 1 was given, so {given} is not");
        a.rollback().unwrap();
        db.close().unwrap();
        drop((db, before));
    }
}
