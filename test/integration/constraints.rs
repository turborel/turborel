//! \[EXTENSION\] The constraints of ISO/IEC 9075:1992 (RULES.md D32): NOT
//! NULL, UNIQUE, PRIMARY KEY [§11.7], FOREIGN KEY with its referential
//! actions [§11.8], CHECK [§11.9], named [§10.6]: their definition, and what
//! each requires of the rows at the end of every statement [§10.6 GR3].

use turborel::db::Database;
use turborel::error::Code;

/// The error code of the `<schema>` `sql`, which must fail.
fn refused(db: &Database, sql: &str) -> Code {
    db.schema(sql).expect_err(sql).code
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §10.6 SR1, SR2; §11.7 SR4,
/// SR5, SR7; §11.8 SR2, SR4, SR7, AR1; §11.9; §11.3 SR5] what a constraint
/// definition refuses: two PRIMARY KEYs, two unique constraints on the same
/// columns, a column named twice or not of the table; a name taken in the
/// schema, or of another schema; a FOREIGN KEY to a table without a PRIMARY
/// KEY, to columns not of a unique constraint, of another type, to a view,
/// or to another's table without REFERENCES; a table with no column; and,
/// a CHECK reading another's table without REFERENCES; a constraint
/// INITIALLY DEFERRED but NOT DEFERRABLE [§10.6 SR4a i].
#[test]
fn what_definitions_refuse_11_6() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O
           CREATE TABLE P (K INTEGER PRIMARY KEY, U INTEGER UNIQUE, V INTEGER)
           CREATE TABLE N (K INTEGER)
           CREATE VIEW W AS SELECT K FROM P",
    )
    .unwrap();
    let cases = [
        ("CREATE TABLE T (A INT PRIMARY KEY, B INT PRIMARY KEY)", Code::SyntaxRule),
        ("CREATE TABLE T (A INT, B INT, UNIQUE (A, B), UNIQUE (B, A))", Code::SyntaxRule),
        ("CREATE TABLE T (A INT, UNIQUE (A, A))", Code::SyntaxRule),
        ("CREATE TABLE T (A INT, PRIMARY KEY (B))", Code::SyntaxRule),
        ("CREATE TABLE T (A INT CONSTRAINT C1 UNIQUE, B INT CONSTRAINT C1 CHECK (B > 0))", Code::SyntaxRule),
        ("CREATE TABLE T (A INT CONSTRAINT O.C1 UNIQUE)", Code::SyntaxRule),
        ("CREATE TABLE T (A INT REFERENCES O.N)", Code::SyntaxRule),
        ("CREATE TABLE T (A INT REFERENCES O.P (V))", Code::SyntaxRule),
        ("CREATE TABLE T (A CHAR(3) REFERENCES O.P)", Code::SyntaxRule),
        ("CREATE TABLE T (A INT REFERENCES O.W)", Code::SyntaxRule),
        ("CREATE TABLE T (A INT REFERENCES O.P)", Code::Privilege),
        ("CREATE TABLE T (A INT, FOREIGN KEY (B) REFERENCES T (A))", Code::SyntaxRule),
        ("CREATE TABLE T (A INT CHECK (A IN (SELECT K FROM O.P)))", Code::Privilege),
        ("CREATE TABLE T (A INT REFERENCES O.P MATCH PARTIAL)", Code::Privilege),
        ("CREATE TABLE T (A INT UNIQUE INITIALLY DEFERRED NOT DEFERRABLE)", Code::SyntaxRule),
        ("CREATE TABLE T (A INT UNIQUE NOT DEFERRABLE INITIALLY DEFERRED)", Code::SyntaxRule),
        ("CREATE TABLE T (A INT UNIQUE DEFERRABLE DEFERRABLE)", Code::Syntax),
        ("CREATE TABLE T (UNIQUE (A))", Code::SyntaxRule),
    ];
    for (table, expected) in cases {
        assert_eq!(refused(&db, &format!("CREATE SCHEMA AUTHORIZATION S {table}")), expected, "{table}");
    }
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (A INT UNIQUE NOT DEFERRABLE INITIALLY IMMEDIATE, CONSTRAINT T_UNIQUE1 CHECK (A > 0))")
        .unwrap();
}

use crate::common::{exec, rows, sorted};

/// The SQLSTATE of running `sql` in `s`: "00000" if it succeeds.
fn state(s: &turborel::db::Session, sql: &str) -> String {
    match s.run(sql) {
        Ok(done) => done.sqlstate().as_str().to_owned(),
        Err(e) => e.sqlstate().as_str().to_owned(),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.7 SR3a, GR2; §11.9; §11.6
/// GR3; §10.6 GR4] a PRIMARY KEY refuses a duplicate, naming itself, and a null; a CHECK
/// refuses a row it is false for, not one it is unknown for, on INSERT and
/// UPDATE; each with SQLSTATE 23000, the statement undone.
#[test]
fn primary_keys_and_checks_11_7_11_9() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER CONSTRAINT POSITIVE CHECK (A > 0), B INTEGER, CHECK (B < A))").unwrap();
    let s = db.session("C");
    assert_eq!(state(&s, "INSERT INTO T VALUES (1, 5, 2)"), "00000");
    assert_eq!(state(&s, "INSERT INTO T VALUES (1, 6, 2)"), "23000", "a duplicate key");
    let named = s.run("INSERT INTO T VALUES (1, 6, 2)").unwrap_err().msg;
    assert!(named.contains("PRIMARY KEY C.T_PRIMARY1 (K) of C.T"), "the constraint named: {named}");
    assert_eq!(state(&s, "INSERT INTO T VALUES (NULL, 6, 2)"), "23000", "a null key");
    assert_eq!(state(&s, "INSERT INTO T VALUES (2, -1, NULL)"), "23000", "A > 0 is false");
    assert_eq!(state(&s, "INSERT INTO T VALUES (2, NULL, 7)"), "00000", "unknown passes");
    assert_eq!(state(&s, "UPDATE T SET B = 9 WHERE K = 1"), "23000", "B < A is false");
    assert_eq!(state(&s, "UPDATE T SET K = K + 1"), "00000", "keys pass through each other in one statement");
    assert_eq!(sorted(&s, "SELECT K, A, B FROM T"), vec!["2,5,2", "3,NULL,7"]);
}

/// Schema F: P (K PRIMARY KEY), and children of it, one for each action:
/// R restricts (NO ACTION), D cascades deletes and updates, N sets null, F
/// references P (K, L) MATCH FULL; G a grandchild through D.
fn families() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION F
           CREATE TABLE P (K INTEGER NOT NULL, L INTEGER NOT NULL, PRIMARY KEY (K), UNIQUE (K, L))
           CREATE TABLE R (K INTEGER REFERENCES P)
           CREATE TABLE D (I INTEGER PRIMARY KEY, K INTEGER REFERENCES P ON DELETE CASCADE ON UPDATE CASCADE)
           CREATE TABLE G (I INTEGER REFERENCES D ON DELETE CASCADE)
           CREATE TABLE N (K INTEGER REFERENCES P ON UPDATE SET NULL ON DELETE SET DEFAULT)
           CREATE TABLE FF (K INTEGER, L INTEGER, FOREIGN KEY (K, L) REFERENCES P (K, L) MATCH FULL)",
    )
    .unwrap();
    let s = db.session("F");
    for sql in [
        "INSERT INTO P VALUES (1, 10)",
        "INSERT INTO P VALUES (2, 20)",
        "INSERT INTO D VALUES (100, 1)",
        "INSERT INTO D VALUES (200, 2)",
        "INSERT INTO G VALUES (100)",
        "INSERT INTO N VALUES (1)",
    ] {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR2, §8.10 GR2, GR4] a
/// row references a row of the referenced table, or has a null in its key;
/// with MATCH FULL, all or none of its key is null; NO ACTION refuses a
/// delete, or a change of key, that leaves a row referencing nothing.
#[test]
fn references_are_kept_11_8() {
    let db = families();
    let s = db.session("F");
    assert_eq!(state(&s, "INSERT INTO R VALUES (3)"), "23000", "no P 3");
    assert_eq!(state(&s, "INSERT INTO R VALUES (NULL)"), "00000");
    assert_eq!(state(&s, "INSERT INTO R VALUES (1)"), "00000");
    assert_eq!(state(&s, "INSERT INTO FF VALUES (1, 10)"), "00000");
    assert_eq!(state(&s, "INSERT INTO FF VALUES (NULL, NULL)"), "00000");
    assert_eq!(state(&s, "INSERT INTO FF VALUES (1, NULL)"), "23000", "MATCH FULL: some null");
    assert_eq!(state(&s, "INSERT INTO FF VALUES (1, 20)"), "23000", "no P (1, 20)");
    assert_eq!(state(&s, "DELETE FROM P WHERE K = 1"), "23000", "R and FF reference it");
    assert_eq!(state(&s, "UPDATE P SET L = 11 WHERE K = 1"), "23000", "FF references (1, 10)");
    assert_eq!(state(&s, "UPDATE R SET K = 9"), "23000");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM P"), Ok(vec!["2".into()]), "undone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR5, GR6, GR9] the
/// referential actions: a delete of a referenced row cascades to its
/// matching rows, and on to theirs; a change of its key cascades the new
/// key; SET NULL and SET DEFAULT (null, there being no default) null the
/// referencing columns.
#[test]
fn referential_actions_11_8() {
    let db = families();
    let s = db.session("F");
    s.run("UPDATE P SET K = 3 WHERE K = 1").unwrap();
    assert_eq!(sorted(&s, "SELECT I, K FROM D"), vec!["100,3", "200,2"], "ON UPDATE CASCADE");
    assert_eq!(sorted(&s, "SELECT K FROM N"), vec!["NULL"], "ON UPDATE SET NULL");
    s.run("INSERT INTO N VALUES (2)").unwrap();
    s.run("DELETE FROM P WHERE K = 2").unwrap();
    assert_eq!(sorted(&s, "SELECT I FROM D"), vec!["100"], "ON DELETE CASCADE");
    assert_eq!(sorted(&s, "SELECT I FROM G"), vec!["100"], "and on to G, which 200 had none of");
    assert_eq!(sorted(&s, "SELECT K FROM N"), vec!["NULL", "NULL"], "ON DELETE SET DEFAULT");
    s.run("DELETE FROM P").unwrap();
    assert_eq!((sorted(&s, "SELECT I FROM D"), sorted(&s, "SELECT I FROM G")), (vec![], vec![]), "two levels");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR5, GR7; §11.4] a table
/// referencing itself cascades along its rows; a referential action giving
/// a column a value the statement gave it another of is refused (SQLSTATE
/// 27000); SET NULL of a NOT NULL column is refused (23000).
#[test]
fn actions_within_one_table_11_8() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE T (K INTEGER PRIMARY KEY, UP INTEGER REFERENCES T ON DELETE CASCADE ON UPDATE CASCADE)
           CREATE TABLE P (K INTEGER PRIMARY KEY)
           CREATE TABLE C (K INTEGER NOT NULL REFERENCES P ON DELETE SET NULL)",
    )
    .unwrap();
    let s = db.session("H");
    for sql in [
        "INSERT INTO T VALUES (1, NULL)",
        "INSERT INTO T VALUES (2, 1)",
        "INSERT INTO T VALUES (3, 2)",
        "INSERT INTO P VALUES (1)",
        "INSERT INTO C VALUES (1)",
    ] {
        s.run(sql).unwrap();
    }
    assert_eq!(state(&s, "UPDATE T SET K = 20, UP = 5 WHERE K = 2"), "23000", "UP 5 references no row");
    assert_eq!(state(&s, "UPDATE T SET K = K + 10, UP = 1"), "27000", "row 2's UP given 1, then 11");
    s.run("UPDATE T SET K = K + 10").unwrap();
    assert_eq!(sorted(&s, "SELECT K, UP FROM T"), vec!["11,NULL", "12,11", "13,12"]);
    s.run("DELETE FROM T WHERE K = 11").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec!["0".into()]), "the chain");
    assert_eq!(state(&s, "DELETE FROM P"), "23000", "C.K is NOT NULL");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 AR1, §10.3] a foreign key
/// to another's table needs REFERENCES on its referenced columns; granted,
/// it is kept as any other.
#[test]
fn references_to_another_schema_11_8() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION O CREATE TABLE P (K INTEGER PRIMARY KEY) GRANT REFERENCES (K) ON P TO S").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE C (K INTEGER REFERENCES O.P)").unwrap();
    db.session("O").run("INSERT INTO P VALUES (1)").unwrap();
    let s = db.session("S");
    assert_eq!(state(&s, "INSERT INTO C VALUES (1)"), "00000");
    assert_eq!(state(&s, "INSERT INTO C VALUES (2)"), "23000");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8, §4.28]; [DESIGN] RULES.md
/// D29: a row inserted referencing a row another open transaction deletes
/// cannot both commit: the second to commit is refused (SQLSTATE 40001).
#[test]
fn references_are_kept_between_transactions_11_8() {
    let db = families();
    let (a, b) = (db.session("F"), db.session("F"));
    a.run("DELETE FROM D WHERE K = 2").unwrap();
    a.run("DELETE FROM P WHERE K = 2").unwrap();
    b.run("INSERT INTO R VALUES (2)").unwrap();
    a.commit().unwrap();
    assert_eq!(b.commit().map_err(|e| e.sqlstate().as_str().to_owned()), Err("40001".to_owned()));
    assert_eq!(sorted(&b, "SELECT K FROM R"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.11 GR4] a column added with
/// constraints is added only if the rows there are satisfy them: UNIQUE,
/// REFERENCES, CHECK.
#[test]
fn columns_added_with_constraints_11_11() {
    let db = families();
    let s = db.session("F");
    assert_eq!(state(&s, "ALTER TABLE D ADD COLUMN X INTEGER CHECK (X IS NOT NULL)"), "23000", "X is null in D's rows");
    assert_eq!(state(&s, "ALTER TABLE D ADD COLUMN X INTEGER REFERENCES P"), "00000", "nulls reference nothing");
    assert_eq!(state(&s, "UPDATE D SET X = 9"), "23000");
    assert_eq!(state(&s, "UPDATE D SET X = 1"), "00000");
    assert_eq!(exec(&s, "ALTER TABLE D ADD COLUMN Y INTEGER UNIQUE").err(), None);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.16 GR2, §11.17 SR2, SR3,
/// SR4, GR1, §11.18 SR4, GR1] constraints added by ALTER TABLE hold of the
/// rows there, or are refused (23000); then of every row written. A
/// constraint is dropped by its name; a unique one that a FOREIGN KEY
/// depends on only with CASCADE, which drops the key too; a table another
/// references only with CASCADE, which drops those keys.
#[test]
fn constraints_added_and_dropped_11_16_11_17() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (K INTEGER, V INTEGER) CREATE TABLE C (K INTEGER)").unwrap();
    let s = db.session("A");
    for sql in ["INSERT INTO P VALUES (1, 1)", "INSERT INTO P VALUES (1, 2)", "INSERT INTO P VALUES (NULL, 3)", "INSERT INTO C VALUES (2)"]
    {
        s.run(sql).unwrap();
    }
    assert_eq!(state(&s, "ALTER TABLE P ADD UNIQUE (K)"), "23000", "two rows of K 1");
    assert_eq!(state(&s, "ALTER TABLE P ADD CONSTRAINT V_UP CHECK (V < 3)"), "23000", "V 3");
    s.run("DELETE FROM P WHERE V = 2").unwrap();
    assert_eq!(state(&s, "ALTER TABLE P ADD PRIMARY KEY (K)"), "23000", "a null K");
    s.run("UPDATE P SET K = 2 WHERE K IS NULL").unwrap();
    assert_eq!(state(&s, "ALTER TABLE P ADD CONSTRAINT P_KEY PRIMARY KEY (K)"), "00000");
    assert_eq!(state(&s, "ALTER TABLE P ADD PRIMARY KEY (V)"), "2A000", "a second PRIMARY KEY");
    assert_eq!(state(&s, "INSERT INTO P VALUES (2, 9)"), "23000", "enforced");
    assert_eq!(state(&s, "ALTER TABLE C ADD CONSTRAINT C_P FOREIGN KEY (K) REFERENCES P"), "00000");
    assert_eq!(state(&s, "INSERT INTO C VALUES (7)"), "23000", "enforced");
    assert_eq!(state(&s, "ALTER TABLE P DROP CONSTRAINT NOPE CASCADE"), "2A000");
    assert_eq!(state(&s, "ALTER TABLE P DROP CONSTRAINT P_KEY RESTRICT"), "2A000", "C_P depends on it");
    assert_eq!(state(&s, "DROP TABLE P RESTRICT"), "2A000", "C references it");
    s.run("ALTER TABLE P DROP CONSTRAINT P_KEY CASCADE").unwrap();
    assert_eq!(state(&s, "INSERT INTO C VALUES (7)"), "00000", "C_P went with it");
    assert_eq!(state(&s, "INSERT INTO P VALUES (2, 9)"), "00000", "and the key");
    s.run("ALTER TABLE P ADD PRIMARY KEY (V)").unwrap();
    s.run("DELETE FROM C WHERE K = 7").unwrap();
    s.run("ALTER TABLE C ADD FOREIGN KEY (K) REFERENCES P").unwrap_err();
    s.run("INSERT INTO P VALUES (5, 2)").unwrap();
    s.run("ALTER TABLE C ADD FOREIGN KEY (K) REFERENCES P").unwrap();
    s.run("DROP TABLE P CASCADE").unwrap();
    assert_eq!(state(&s, "INSERT INTO C VALUES (99)"), "00000", "its key went with P");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.15 SR4, GR1, GR6] a column
/// dropped takes its own constraints with it — NOT NULL, a CHECK of it
/// alone — and with CASCADE those of it and other columns too, and the
/// foreign keys to it; RESTRICT refuses those. The columns after it move,
/// and a foreign key to one of them goes on referencing it.
#[test]
fn columns_dropped_with_their_constraints_11_15() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION D
           CREATE TABLE P (X INTEGER NOT NULL CHECK (X > 0), K INTEGER PRIMARY KEY, A INTEGER, B INTEGER, CHECK (A < B), UNIQUE (A, B))
           CREATE TABLE C (K INTEGER REFERENCES P)",
    )
    .unwrap();
    let s = db.session("D");
    s.run("INSERT INTO P VALUES (1, 1, 1, 2)").unwrap();
    s.run("INSERT INTO C VALUES (1)").unwrap();
    s.run("ALTER TABLE P DROP COLUMN X RESTRICT").unwrap();
    assert_eq!(state(&s, "INSERT INTO C VALUES (3)"), "23000", "C references K, now first");
    assert_eq!(state(&s, "INSERT INTO P VALUES (2, 5, 5)"), "23000", "A < B");
    assert_eq!(state(&s, "ALTER TABLE P DROP COLUMN A RESTRICT"), "2A000", "the CHECK and UNIQUE of A and B");
    assert_eq!(state(&s, "ALTER TABLE P DROP COLUMN K RESTRICT"), "2A000", "C's key references it");
    s.run("ALTER TABLE P DROP COLUMN A CASCADE").unwrap();
    assert_eq!(state(&s, "INSERT INTO P VALUES (2, 1)"), "00000", "neither left");
    s.run("ALTER TABLE P DROP COLUMN K CASCADE").unwrap();
    assert_eq!(state(&s, "INSERT INTO C VALUES (77)"), "00000", "C's key went with K");
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR2b, GR3b, GR4, GR5b;
/// §8.10 GR3]: with MATCH PARTIAL a row's values not null must equal a
/// referenced row's, all null passing; a delete cascades only to its unique
/// matching rows, a row that matches another being kept, until the last
/// row it matches goes, in one statement too; NO ACTION refuses a delete
/// only once a row matches no row left.
#[test]
fn partial_matches_11_8() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION M
           CREATE TABLE P (A INTEGER, B INTEGER, UNIQUE (A, B))
           CREATE TABLE C (A INTEGER, B INTEGER, FOREIGN KEY (A, B) REFERENCES P (A, B) MATCH PARTIAL ON DELETE CASCADE)
           CREATE TABLE N (A INTEGER, B INTEGER, FOREIGN KEY (A, B) REFERENCES P (A, B) MATCH PARTIAL)",
    )
    .unwrap();
    let s = db.session("M");
    for sql in ["INSERT INTO P VALUES (1, 1)", "INSERT INTO P VALUES (1, 2)", "INSERT INTO P VALUES (2, 1)", "INSERT INTO P VALUES (3, 3)"]
    {
        s.run(sql).unwrap();
    }
    for (row, expected) in [("(1, NULL)", None), ("(NULL, 2)", None), ("(NULL, NULL)", None), ("(1, 1)", None), ("(3, NULL)", None)] {
        assert_eq!(exec(&s, &format!("INSERT INTO C VALUES {row}")).err(), expected, "{row}");
    }
    assert_eq!(exec(&s, "INSERT INTO C VALUES (4, NULL)"), Err(Code::Constraint), "no A is 4");
    assert_eq!(exec(&s, "INSERT INTO C VALUES (2, 2)"), Err(Code::Constraint), "no row is (2, 2)");
    s.run("DELETE FROM P WHERE A = 1 AND B = 1").unwrap();
    assert_eq!(sorted(&s, "SELECT A, B FROM C"), vec!["1,NULL", "3,NULL", "NULL,2", "NULL,NULL"], "(1, NULL) matches (1, 2) too");
    s.run("DELETE FROM P WHERE A = 1 OR A = 3").unwrap();
    assert_eq!(sorted(&s, "SELECT A, B FROM C"), vec!["NULL,NULL"], "the last each matched gone");
    s.rollback().unwrap();

    for sql in
        ["INSERT INTO P VALUES (1, 1)", "INSERT INTO P VALUES (1, 2)", "INSERT INTO C VALUES (1, NULL)", "INSERT INTO N VALUES (1, NULL)"]
    {
        s.run(sql).unwrap();
    }
    s.run("DELETE FROM P WHERE B = 1").unwrap();
    assert_eq!(exec(&s, "DELETE FROM P WHERE B = 2"), Err(Code::Constraint), "N's (1, NULL) would match no row");
    s.run("DELETE FROM N").unwrap();
    s.run("DELETE FROM P WHERE A = 1").unwrap();
    assert_eq!(sorted(&s, "SELECT A, B FROM C"), Vec::<String>::new(), "both rows (1, NULL) matched in one statement");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR6b]: with MATCH
/// PARTIAL an update acts on a unique matching row's columns that are not
/// null and correspond to one changed: SET NULL nulls them; CASCADE gives
/// them the new value, once every referenced row the row matched is
/// updated to it, else a triggered data change violation (27000).
#[test]
fn partial_updates_11_8() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION M
           CREATE TABLE P (A INTEGER, B INTEGER, UNIQUE (A, B))
           CREATE TABLE X (A INTEGER, B INTEGER, FOREIGN KEY (A, B) REFERENCES P (A, B) MATCH PARTIAL ON UPDATE CASCADE)
           CREATE TABLE S (A INTEGER, B INTEGER, FOREIGN KEY (A, B) REFERENCES P (A, B) MATCH PARTIAL ON UPDATE SET NULL)",
    )
    .unwrap();
    let s = db.session("M");
    for sql in
        ["INSERT INTO P VALUES (1, 1)", "INSERT INTO P VALUES (2, 1)", "INSERT INTO X VALUES (NULL, 1)", "INSERT INTO X VALUES (1, 1)"]
    {
        s.run(sql).unwrap();
    }
    s.run("INSERT INTO S VALUES (2, 1)").unwrap();
    s.run("UPDATE P SET B = B + 10").unwrap();
    assert_eq!(sorted(&s, "SELECT A, B FROM X"), vec!["1,11", "NULL,11"], "(NULL, 1) took 11 once both were 11");
    assert_eq!(sorted(&s, "SELECT A, B FROM S"), vec!["2,NULL"], "B, the column changed, nulled");
    assert_eq!(state(&s, "UPDATE P SET B = A + 20"), "27000", "(NULL, 11) matched rows updated to 21 and 22");
    s.run("UPDATE P SET A = 5 WHERE A = 1").unwrap();
    assert_eq!(sorted(&s, "SELECT A, B FROM X"), vec!["5,11", "NULL,11"], "A of (NULL, 11) is null, so stays");
}

/// The schema of each constraint of the tables of schema `schema`, its
/// name, and its table's schema, as INFORMATION_SCHEMA.TABLE_CONSTRAINTS
/// shows them to `s`.
fn placed(s: &turborel::db::Session, schema: &str) -> Vec<String> {
    let sql = format!(
        "SELECT CONSTRAINT_SCHEMA, CONSTRAINT_NAME, TABLE_SCHEMA FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA = '{schema}'"
    );
    sorted(s, &sql)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4 SR8, GR18; §10.6 SR1,
/// SR2; §11.6 SR2; §11.17 SR2] outside a `<schema definition>`, a
/// constraint's name written without a schema is of the session's default
/// schema [SR8b], which may be another than its table's: no Syntax Rule
/// makes them one. It is a constraint of that schema, whose others' names
/// it may not take, nor they its; ALTER TABLE DROP CONSTRAINT finds it
/// there. A schema written is taken as written, and must exist, as must
/// the default schema (3F000); MODULE is no schema of one. A name the
/// implementation gives is of the table's schema. In a `<schema
/// definition>`, a name is of its schema [SR8a] and no other [§10.6 SR1].
#[test]
fn constraint_names_outside_a_schema_definition_5_4() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    db.schema("CREATE SCHEMA S2 AUTHORIZATION U").unwrap();
    db.schema("CREATE SCHEMA S3 AUTHORIZATION V").unwrap();
    let u = db.session("U");
    u.run("CREATE TABLE S2.T (K INTEGER CONSTRAINT KEYED PRIMARY KEY, N INTEGER UNIQUE, CONSTRAINT S2.OTHER CHECK (N > 0))").unwrap();
    assert_eq!(placed(&u, "S2"), vec!["S2,OTHER,S2", "S2,T_UNIQUE1,S2", "U,KEYED,S2"]);
    u.run("INSERT INTO S2.T VALUES (1, 1)").unwrap();
    let refused = u.run("INSERT INTO S2.T VALUES (1, 2)").unwrap_err();
    assert_eq!(refused.subject.and_then(|s| s.constraint), Some(("U".to_owned(), "KEYED".to_owned())), "named so [§18.1 GR3f i]");
    let taken = [
        ("CREATE TABLE U.T2 (K INTEGER CONSTRAINT KEYED CHECK (K > 0))", "§10.6 SR2"),
        ("CREATE ASSERTION KEYED CHECK (1 = 1)", "§11.34 SR2, SR7"),
        ("ALTER TABLE S2.T ADD CONSTRAINT KEYED CHECK (K > 0)", "§10.6 SR2"),
        ("ALTER TABLE S2.T DROP CONSTRAINT S2.KEYED CASCADE", "§11.17 SR2"),
        ("CREATE TABLE S2.T3 (K INTEGER CONSTRAINT MODULE.C UNIQUE)", "MODULE names"),
        ("CREATE SCHEMA S4 AUTHORIZATION U CREATE TABLE T (K INTEGER CONSTRAINT U.C UNIQUE)", "§10.6 SR1"),
        ("DECLARE LOCAL TEMPORARY TABLE MODULE.D (K INTEGER CONSTRAINT U.C UNIQUE)", "is in MODULE"),
    ];
    for (sql, why) in taken {
        let refused = if sql.starts_with("CREATE SCHEMA") { u.direct(sql).map(|_| ()) } else { u.run(sql).map(|_| ()) };
        let e = refused.expect_err(sql);
        assert!(e.code == Code::SyntaxRule && e.described().contains(why), "{sql}: {e}");
    }
    u.direct("CREATE SCHEMA S4 AUTHORIZATION U CREATE TABLE T (K INTEGER CONSTRAINT C UNIQUE)").unwrap();
    assert_eq!(placed(&u, "S4"), vec!["S4,C,S4"], "of the <schema definition>'s schema");
    u.run("ALTER TABLE S2.T DROP CONSTRAINT KEYED CASCADE").unwrap();
    u.run("CREATE ASSERTION KEYED CHECK (1 = 1)").unwrap();
    u.commit().unwrap();
    let v = db.session("V");
    let unplaced = v.run("CREATE TABLE S3.T (K INTEGER CONSTRAINT C UNIQUE)").unwrap_err();
    assert_eq!((unplaced.code, unplaced.sqlstate().as_str()), (Code::SyntaxRule, "3F000"), "no schema V");
    v.run("CREATE TABLE S3.T (K INTEGER CONSTRAINT S3.C UNIQUE, N INTEGER UNIQUE)").unwrap();
    v.run("ALTER TABLE S3.T ADD CONSTRAINT S3.D CHECK (K > 0)").unwrap();
    assert_eq!(placed(&v, "S3"), vec!["S3,C,S3", "S3,D,S3", "S3,T_UNIQUE1,S3"]);
}

/// Verifies: [EXTENSION] RULES.md D47; [ISO/IEC 9075:1992 §5.4 SR8b,
/// §10.6 SR1, §11.21, §11.23]: a constraint is named in a schema only by
/// that schema's owner, a table's or a domain's, written or the default
/// schema (SQLCODE -104), stricter than SQL-92; a domain constraint's name
/// written without a schema is of the session's default schema, CREATE
/// DOMAIN's and ALTER DOMAIN's, ADD and DROP, and in a `<schema definition>` of its
/// schema and no other.
#[test]
fn constraints_are_named_by_their_schemas_owner_5_4() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    db.schema("CREATE SCHEMA S2 AUTHORIZATION U").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION W").unwrap();
    let u = db.session("U");
    let refused = [
        "CREATE TABLE S2.T (K INTEGER CONSTRAINT W.C UNIQUE)",
        "CREATE DOMAIN S2.D AS INTEGER CONSTRAINT W.C CHECK (VALUE > 0)",
        "SET SCHEMA 'W'",
    ];
    for sql in &refused[..2] {
        assert_eq!(exec(&u, sql), Err(Code::Privilege), "{sql}");
    }
    u.run("CREATE TABLE S2.T (K INTEGER)").unwrap();
    u.run(refused[2]).unwrap();
    assert_eq!(exec(&u, "ALTER TABLE S2.T ADD CONSTRAINT C CHECK (K > 0)"), Err(Code::Privilege), "W's, the default");
    assert_eq!(exec(&u, "ALTER TABLE S2.T ADD COLUMN N INTEGER CONSTRAINT C UNIQUE"), Err(Code::Privilege), "W's, of a column");
    u.run("SET SCHEMA 'U'").unwrap();
    u.run("CREATE DOMAIN S2.D AS INTEGER CONSTRAINT POSITIVE CHECK (VALUE > 0)").unwrap();
    u.run("ALTER DOMAIN S2.D ADD CONSTRAINT SMALL CHECK (VALUE < 9)").unwrap();
    let sql = "SELECT CONSTRAINT_SCHEMA, CONSTRAINT_NAME, DOMAIN_SCHEMA FROM INFORMATION_SCHEMA.DOMAIN_CONSTRAINTS";
    assert_eq!(sorted(&u, sql), vec!["U,POSITIVE,S2", "U,SMALL,S2"]);
    assert_eq!(exec(&u, "CREATE DOMAIN S2.E AS INTEGER CONSTRAINT POSITIVE CHECK (VALUE > 0)"), Err(Code::SyntaxRule), "taken in U");
    u.run("ALTER DOMAIN S2.D DROP CONSTRAINT SMALL").unwrap();
    assert_eq!(sorted(&u, sql), vec!["U,POSITIVE,S2"], "SMALL, of the default schema, dropped");
    u.commit().unwrap();
    let schema = "CREATE SCHEMA S4 AUTHORIZATION U CREATE DOMAIN D AS INTEGER CONSTRAINT U.C CHECK (VALUE > 0)";
    assert_eq!(u.direct(schema).err().map(|e| e.code), Some(Code::SyntaxRule), "§10.6 SR1");
}

/// Verifies: [EXTENSION] RULES.md D47; [ISO/IEC 9075:1992 §11.2]: DROP
/// SCHEMA RESTRICT is refused while a constraint named in the schema is of
/// a table or domain of another; CASCADE drops those constraints, a domain
/// constraint's copies on its columns with it, and leaves their tables and
/// domains.
#[test]
fn a_schema_dropped_takes_the_constraints_named_in_it_11_2() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION O").unwrap();
    db.schema("CREATE SCHEMA O2 AUTHORIZATION O").unwrap();
    let o = db.session("O");
    o.run("CREATE DOMAIN O2.D AS INTEGER CONSTRAINT POSITIVE CHECK (VALUE > 0)").unwrap();
    o.run("CREATE TABLE O2.T (K INTEGER CONSTRAINT KEYED UNIQUE, V O2.D)").unwrap();
    o.run("INSERT INTO O2.T VALUES (1, 1)").unwrap();
    assert_eq!(exec(&o, "DROP SCHEMA O RESTRICT"), Err(Code::SyntaxRule));
    o.run("DROP SCHEMA O CASCADE").unwrap();
    o.run("INSERT INTO O2.T VALUES (1, -1)").unwrap();
    assert_eq!(sorted(&o, "SELECT K, V FROM O2.T"), vec!["1,-1", "1,1"], "KEYED and POSITIVE gone");
    assert_eq!(sorted(&o, "SELECT CAST(-2 AS O2.D) FROM O2.T WHERE V = 1"), vec!["-2"], "the domain stays, unconstrained");
    o.commit().unwrap();
}
