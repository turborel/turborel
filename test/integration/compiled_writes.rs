//! \[DESIGN\] A compiled program's point change written straight on its
//! table (RULES.md D841): each body here runs compiled and interpreted,
//! as `compiled_procedures` runs them, and gives the same of each. A
//! point change checks the constraints of the row it writes as its
//! statement's end checks them; one whose end would check more, or whose
//! increments are reserved, runs the general path. Each condition that
//! sends it there has a case, each called several times as the kept plans
//! settle, and then so that its check fails.

use crate::compiled_procedures::{database, same};

/// The tables of these cases: P, referenced by C's PK; C, with a NOT NULL
/// column, a UNIQUE one, a foreign key and a CHECK; E, with a deferrable
/// CHECK.
const SETUP: [&str; 6] = [
    "CREATE TABLE P (K INTEGER PRIMARY KEY, V INTEGER)",
    "CREATE TABLE C (K INTEGER PRIMARY KEY, V INTEGER NOT NULL, U INTEGER UNIQUE, PK INTEGER REFERENCES P, DK INTEGER CHECK (DK > 0))",
    "CREATE TABLE E (K INTEGER PRIMARY KEY, DK INTEGER CONSTRAINT DK_POSITIVE CHECK (DK > 0) DEFERRABLE INITIALLY IMMEDIATE)",
    "INSERT INTO P VALUES (1, 0), (2, 0), (3, 0), (4, 0)",
    "INSERT INTO C VALUES (1, 1, 1, 1, 1), (2, 2, 2, 2, 2), (3, 3, 3, 1, 3)",
    "INSERT INTO E VALUES (1, 1), (2, 2)",
];

/// The procedures of these cases, one point change each.
const PROCEDURES: [&str; 11] = [
    "CREATE PROCEDURE SETV (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE C SET V = NULLIF(X, -1) WHERE K = KK; END",
    "CREATE PROCEDURE SETU (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE C SET U = X WHERE K = KK; END",
    "CREATE PROCEDURE SETPK (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE C SET PK = X WHERE K = KK; END",
    "CREATE PROCEDURE SETDK (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE E SET DK = X WHERE K = KK; END",
    "CREATE PROCEDURE SETCK (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE C SET DK = X WHERE K = KK; END",
    "CREATE PROCEDURE REKEY (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE P SET K = X WHERE K = KK; END",
    "CREATE PROCEDURE DROPP (IN KK INTEGER) MODIFIES SQL DATA BEGIN DELETE FROM P WHERE K = KK; END",
    "CREATE PROCEDURE DROPC (IN KK INTEGER, OUT X INTEGER) MODIFIES SQL DATA
       BEGIN SELECT V INTO X FROM OLD TABLE (DELETE FROM C WHERE K = KK); END",
    "CREATE PROCEDURE BUMP (IN KK INTEGER, IN AMT INTEGER, OUT X INTEGER) MODIFIES SQL DATA
       BEGIN SELECT V INTO X FROM FINAL TABLE (UPDATE C SET V = V + AMT WHERE K = KK); END",
    "CREATE PROCEDURE DROPH (IN KK INTEGER, OUT X INTEGER) MODIFIES SQL DATA
       BEGIN DECLARE CONTINUE HANDLER FOR NOT FOUND SET X = -1; SET X = 0;
         SELECT V INTO X FROM OLD TABLE (DELETE FROM C WHERE K = KK); END",
    "CREATE PROCEDURE DROPPH (IN KK INTEGER, OUT X INTEGER) MODIFIES SQL DATA
       BEGIN DECLARE CONTINUE HANDLER FOR NOT FOUND SET X = -1; SET X = 0;
         SELECT V INTO X FROM OLD TABLE (DELETE FROM P WHERE K = KK); END",
];

/// `setup` and `PROCEDURES`, then `script`, compiled and interpreted.
fn alike(setup: &[&str], script: &[String]) -> Vec<String> {
    let all: Vec<&str> = SETUP.iter().chain(setup).chain(PROCEDURES.iter()).copied().collect();
    let read = ["? SELECT K, V, U, PK, DK FROM C ORDER BY K", "? SELECT K, V FROM P ORDER BY K", "? SELECT K, DK FROM E ORDER BY K"];
    let script: Vec<&str> = script.iter().map(String::as_str).chain(read).collect();
    same(&all, &script)
}

/// `call` with each argument list of `args`, the last one's to fail.
fn calls(call: &str, args: &[&str]) -> Vec<String> {
    args.iter().map(|a| format!("CALL {call}({a})")).collect()
}

/// Verifies: [DESIGN] RULES.md D841, a point UPDATE checks the NOT NULL,
/// foreign key and UNIQUE constraints of the columns it writes, and a
/// CHECK of its row, at its end as the general path checks them: SQLSTATE
/// 23000 of each broken, the row left as it was; and its OLD TABLE of no
/// row read as no data, which a NOT FOUND handler handles, straight and on
/// the general path alike.
#[test]
fn a_point_change_checks_its_rows_constraints() {
    let mut script = calls("SETV", &["1, 10", "1, 11", "2, 12", "1, 13", "2, -1", "9, 1"]);
    script.extend(calls("SETPK", &["1, 2", "1, 3", "2, 4", "1, 1", "2, 9"]));
    script.extend(calls("SETU", &["1, 10", "1, 11", "2, 12", "1, 13", "2, 13"]));
    script.extend(calls("SETCK", &["1, 10", "1, 11", "2, 12", "1, 13", "2, 0"]));
    script.extend(calls("BUMP", &["1, 1, ?", "1, 1, ?", "2, 1, ?", "9, 1, ?", "1, 1, ?"]));
    script.extend(calls("DROPC", &["9, ?", "3, ?", "3, ?"]));
    script.extend(calls("DROPH", &["9, ?", "2, ?", "2, ?"]));
    script.extend(calls("DROPPH", &["9, ?", "8, ?", "4, ?"]));
    let gave = alike(&[], &script);
    assert!(gave.iter().any(|l| l.starts_with("CALL SETU(2, 13): error 23000")), "unique: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL SETCK(2, 0): error 23000")), "CHECK: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL SETV(2, -1): error 23000")), "NOT NULL: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL SETPK(2, 9): error 23000")), "foreign key: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL DROPC(3, ?): ok")), "OLD TABLE: {gave:?}");
    let handled = |l: &String| l.starts_with("CALL DROPH(9, ?): ok") && l.contains("\"-1\"");
    assert!(gave.iter().any(handled), "no data handled: {gave:?}");
    let fell_back = |l: &String| l.starts_with("CALL DROPPH(9, ?): ok") && l.contains("\"-1\"");
    assert!(gave.iter().any(fell_back), "no data handled on the general path: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, a point change whose statement's end
/// checks more than its row runs the general path, as interpreted: of a
/// deferrable constraint, immediate or deferred, a key a foreign key references, updated or
/// deleted.
#[test]
fn a_point_change_falls_back_where_its_end_checks_more() {
    let mut script = calls("SETDK", &["1, 10", "1, 11", "2, 12", "1, 13", "2, 0"]);
    script.extend(["SET CONSTRAINTS ALL DEFERRED", "CALL SETDK(1, 0)", "CALL SETDK(1, 7)", "COMMIT"].map(str::to_owned));
    script.extend(calls("REKEY", &["4, 5", "5, 6", "6, 7", "7, 8", "1, 9"]));
    script.extend(calls("DROPP", &["8", "3", "9", "2"]));
    let gave = alike(&[], &script);
    assert!(gave.iter().any(|l| l.starts_with("CALL SETDK(2, 0): error 23000")), "deferrable: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL SETDK(1, 0): ok")), "deferred: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL REKEY(1, 9): error 23000")), "referenced: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL DROPP(2): error 23000")), "referenced: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, a point change of a catalog with an
/// assertion runs the general path, the assertion broken as interpreted.
#[test]
fn a_point_change_alongside_an_assertion() {
    let setup = ["CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM C WHERE V > 100))"];
    let script = calls("BUMP", &["1, 1, ?", "1, 1, ?", "9, 1, ?", "1, 1, ?", "1, 200, ?"]);
    let gave = alike(&setup, &script);
    assert!(gave.iter().any(|l| l.starts_with("CALL BUMP(1, 200, ?): error 23000")), "assertion: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, a point change of a catalog with a
/// trigger on another table, finding no row or one, and one whose
/// increments TRANSACTION.ESCROW may reserve, give what interpreted gives.
#[test]
fn a_point_change_alongside_triggers_and_escrow() {
    let setup = [
        "CREATE TRIGGER ON_S AFTER UPDATE ON S FOR EACH STATEMENT INSERT INTO P VALUES (99, 99)",
        "SET TRANSACTION.ESCROW = 'ON'",
        "CREATE PROCEDURE GROW (IN KK INTEGER, IN AMT INTEGER) MODIFIES SQL DATA BEGIN UPDATE P SET V = V + AMT WHERE K = KK; END",
    ];
    let mut script = calls("BUMP", &["1, 1, ?", "1, 1, ?", "9, 1, ?", "1, 1, ?"]);
    script.extend(calls("GROW", &["2, 1", "2, 1", "9, 1", "2, 1", "2, 300"]));
    script.push("UPDATE S SET Q = Q WHERE W = 1 AND I = 1".to_owned());
    let gave = alike(&setup, &script);
    assert!(gave.iter().any(|l| l == "SELECT K, V FROM P ORDER BY K: [\"1,0\", \"2,303\", \"3,0\", \"4,0\", \"99,99\"]"), "{gave:?}");
}

/// What a point UPDATE of C's foreign key gives once another session
/// deletes the row of P it then references, before it commits: its
/// commit's outcome, compiled or interpreted.
fn reference_deleted_under(compiled: bool) -> String {
    let (db, a) = database(compiled);
    for sql in SETUP.iter().chain(&PROCEDURES[2..3]) {
        a.run(sql).unwrap();
    }
    a.commit().unwrap();
    for to in [2, 3, 2, 3, 1] {
        a.run(&format!("CALL SETPK(1, {to})")).unwrap();
        a.commit().unwrap();
    }
    a.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    a.run("CALL SETPK(1, 4)").unwrap();
    let b = db.session("A");
    b.run("DELETE FROM P WHERE K = 4").unwrap();
    b.commit().unwrap();
    match a.commit() {
        Ok(()) => "committed".to_owned(),
        Err(e) => format!("error {}", e.sqlstate().as_str()),
    }
}

/// Verifies: [DESIGN] RULES.md D841, D880, [ISO/IEC 9075:1992 §4.28] a
/// point change's foreign key check reads serializably below
/// SERIALIZABLE, straight as on the general path: the row it referenced
/// deleted by another transaction before it commits, its commit is
/// refused (SQLSTATE 40001), so no row is left referencing none.
#[test]
fn a_point_change_checks_its_reference_serializably() {
    assert_eq!(reference_deleted_under(false), "error 40001");
    assert_eq!(reference_deleted_under(true), "error 40001");
}
