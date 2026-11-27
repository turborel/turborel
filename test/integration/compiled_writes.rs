//! \[DESIGN\] A compiled program's point change written straight on its
//! table (RULES.md D841): each body here runs compiled and interpreted,
//! as `compiled_procedures` runs them, and gives the same of each. A
//! point change checks the constraints of the row it writes as its
//! statement's end checks them; one whose end would check more, or whose
//! increments are reserved, runs the general path. Each condition that
//! sends it there has a case, each called several times as the kept plans
//! settle, and then so that its check fails. An INSERT of a query's rows
//! runs straight too, each of its rows checked (RULES.md D901).

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

/// Verifies: [DESIGN] RULES.md D841, [ISO/IEC 9075:1992 §11.5 GR2] an
/// INSERT of a compiled program gives a column whose default is
/// CURRENT_TIMESTAMP the instant of each execution, not that of the
/// execution its target was kept at, compiled as interpreted: of five
/// rows inserted, the last a day after the others, it alone has the
/// latest instant; a column of a constant default has it in each.
#[test]
fn an_insert_takes_its_defaults_at_each_execution() {
    const DAY: i64 = 86_400_000_000;
    for compiled in [false, true] {
        let (db, a) = database(compiled);
        let clock = std::sync::Arc::new(turborel::clock::FixedClock::at(DAY));
        db.set_clock(std::sync::Arc::clone(&clock) as std::sync::Arc<dyn turborel::clock::Clock>).unwrap();
        a.run("CREATE TABLE W (K INTEGER PRIMARY KEY, MADE TIMESTAMP DEFAULT CURRENT_TIMESTAMP, NOTE VARCHAR(5) DEFAULT 'x')").unwrap();
        a.run("CREATE PROCEDURE ADDW (IN KK INTEGER) MODIFIES SQL DATA BEGIN INSERT INTO W (K) VALUES (KK); END").unwrap();
        a.commit().unwrap();
        for k in 1..=5 {
            if k == 5 {
                clock.set(2 * DAY);
            }
            a.run(&format!("CALL ADDW({k})")).unwrap();
            a.commit().unwrap();
        }
        let latest = a.open_cursor("SELECT K, NOTE FROM W WHERE MADE = (SELECT MAX(MADE) FROM W)").unwrap().fetch().unwrap().unwrap();
        let shown: Vec<String> = latest.iter().map(|v| v.to_string().trim_end().to_owned()).collect();
        assert_eq!(shown, ["5", "x"], "compiled: {compiled}");
        let mut all = a.open_cursor("SELECT COUNT(*) FROM W WHERE MADE < (SELECT MAX(MADE) FROM W) AND NOTE = 'x'").unwrap();
        assert_eq!(all.fetch().unwrap().unwrap()[0].to_string(), "4", "compiled: {compiled}");
    }
}

/// The tables and procedures of the INSERT ... SELECT cases. LN has a
/// key, a UNIQUE column, a foreign key to P and one to itself, a NOT NULL
/// column and a CHECK. LINES inserts the rows of arrays UNNEST gives, as
/// an order's lines are, a scalar subquery for each. COPYL inserts
/// rows of LN read from LN. DG holds the ten digits, and MANY inserts the
/// ten thousand rows of four of it joined into BIG.
const LINES: [&str; 7] = [
    "CREATE TABLE LN (K INTEGER PRIMARY KEY, U INTEGER UNIQUE, PK INTEGER REFERENCES P, PV INTEGER,
       NN INTEGER NOT NULL, DK INTEGER CHECK (DK > 0), SELF INTEGER REFERENCES LN)",
    "CREATE PROCEDURE LINES (IN BASE INTEGER, IN US INTEGER ARRAY[5], IN PKS INTEGER ARRAY[5]) MODIFIES SQL DATA
       BEGIN INSERT INTO LN (K, U, PK, PV, NN, DK, SELF)
         SELECT BASE + X.N, X.U, X.PK, (SELECT V FROM P WHERE K = X.PK), NULLIF(X.U, -1), X.U + 5, BASE + 1
           FROM UNNEST(US, PKS) WITH ORDINALITY AS X (U, PK, N); END",
    "CREATE PROCEDURE COPYL (IN D INTEGER, IN ABOVE INTEGER) MODIFIES SQL DATA
       BEGIN INSERT INTO LN (K, U, PK, PV, NN, DK, SELF) SELECT K + D, U + D, PK, PV, NN, DK, SELF FROM LN WHERE K > ABOVE; END",
    "CREATE TABLE DG (N INTEGER PRIMARY KEY)",
    "INSERT INTO DG VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9)",
    "CREATE TABLE BIG (K INTEGER PRIMARY KEY, V INTEGER NOT NULL)",
    "CREATE PROCEDURE MANY (IN BASE INTEGER, IN FLAG INTEGER) MODIFIES SQL DATA
       BEGIN INSERT INTO BIG SELECT BASE + 1000 * A.N + 100 * B.N + 10 * C.N + E.N, NULLIF(A.N + 10 * FLAG, 9)
           FROM DG A, DG B, DG C, DG E; END",
];

/// Verifies: [DESIGN] RULES.md D901, an INSERT of the rows of a query, a
/// scalar subquery for each, runs straight and checks each row's
/// constraints at its end as the general path does. A key twice among its
/// rows, a key a row already has, NOT NULL, a CHECK and a foreign key of a
/// row other than its first are each SQLSTATE 23000 with none of its rows
/// kept. A row referencing one the same statement inserts, rows read from
/// its own table, one row and a query of no row are as interpreted.
#[test]
fn an_insert_of_a_querys_rows_checks_each() {
    let mut script = calls("LINES", &["10, ARRAY[1, 2, 3], ARRAY[1, 2, 3]", "20, ARRAY[4, 5], ARRAY[1, 1]"]);
    script.extend(calls("LINES", &["30, ARRAY[6, 7], ARRAY[2, 2]", "35, ARRAY[30], ARRAY[4]"]));
    script.extend(calls("LINES", &["40, ARRAY[8, 8], ARRAY[1, 1]", "50, ARRAY[9, 1], ARRAY[1, 1]", "60, ARRAY[10, -1], ARRAY[1, 1]"]));
    script.extend(calls("LINES", &["70, ARRAY[11, -9], ARRAY[1, 1]", "80, ARRAY[12, 13], ARRAY[1, 9]", "90, ARRAY[14], ARRAY[4]"]));
    script.extend(calls("LINES", &["100, ARRAY[-1, 15, 15], ARRAY[9, 1, 1]", "110, CAST(NULL AS INTEGER ARRAY[5]), ARRAY[1]"]));
    script.extend(calls("LINES", &["120, ARRAY[16, 17], ARRAY[1]", "10, ARRAY[18, 19], ARRAY[1, 2]"]));
    script.extend(calls("COPYL", &["1000, 5000", "1000, 0", "1000, 0", "2000, 25"]));
    script.push("? SELECT K, U, PK, PV, NN, DK, SELF FROM LN ORDER BY K".to_owned());
    let gave = alike(&LINES, &script);
    for broken in ["40, ARRAY[8, 8]", "50, ARRAY[9, 1]", "60, ARRAY[10, -1]", "70, ARRAY[11, -9]", "80, ARRAY[12, 13]", "100, ARRAY[-1"] {
        assert!(gave.iter().any(|l| l.starts_with(&format!("CALL LINES({broken}")) && l.contains("error 23000")), "{broken}: {gave:?}");
    }
    assert!(gave.iter().any(|l| l.starts_with("CALL LINES(10, ARRAY[18, 19]") && l.contains("error 23000")), "a key LN has: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL LINES(120, ") && l.contains(": ok")), "arrays of unequal length: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL COPYL(1000, 0): error 23000")), "copied twice: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL COPYL(2000, 25): ok")), "copied: {gave:?}");
    let left = gave.iter().find(|l| l.starts_with("SELECT K, U, PK, PV, NN, DK, SELF FROM LN")).unwrap();
    assert!(left.contains("\"11,1,1,0,1,6,11\"") && left.contains("\"36,30,4,0,30,35,36\""), "{left}");
    assert!(!left.contains("\"41,") && !left.contains("\"101,"), "no row of a statement refused: {left}");
}

/// Verifies: [DESIGN] RULES.md D901, an INSERT of rows enough to be a load
/// of their own (D532) is handed to the general path once its rows are
/// found, and gives what interpreted gives: ten thousand rows each call,
/// then a NOT NULL broken in the last thousand of them, and a key BIG
/// has, each SQLSTATE 23000 with no row of that call kept.
#[test]
fn an_insert_of_a_loads_rows_runs_the_general_path() {
    let mut script = calls("MANY", &["0, 1", "100000, 1", "200000, 1", "300000, 1", "400000, 0", "500000, 1", "300000, 2"]);
    script.push("? SELECT COUNT(*), MIN(K), MAX(K), SUM(V) FROM BIG".to_owned());
    let gave = alike(&LINES, &script);
    assert!(gave.iter().any(|l| l.starts_with("CALL MANY(400000, 0): error 23000")), "NOT NULL: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL MANY(300000, 2): error 23000")), "a key BIG has: {gave:?}");
    assert!(gave.iter().any(|l| l == "SELECT COUNT(*), MIN(K), MAX(K), SUM(V) FROM BIG: [\"50000,0,509999,725000\"]"), "{gave:?}");
}

/// Verifies: [DESIGN] RULES.md D901, an INSERT of a query's rows whose
/// end would check more than its rows, or whose rows held are not the
/// rows stored, gives what interpreted gives. The cases are a table with
/// a deferrable constraint, immediate and deferred, a view WITH CHECK
/// OPTION, a table with a trigger, a catalog with an assertion, a table
/// with a system-time period and a row holding a large object, each
/// called as its plans settle and then so that a constraint is broken.
/// A regression test (R-TEST-7): an INSERT of one row of values into a
/// table with a system-time period was written straight with its period
/// unstamped once its plans were kept, and refused for its null start.
#[test]
fn an_insert_of_a_querys_rows_falls_back_where_its_end_checks_more() {
    let setup = [
        "CREATE VIEW EV AS SELECT K, DK FROM E WHERE DK < 100 WITH CHECK OPTION",
        "CREATE PROCEDURE ADDE (IN BASE INTEGER, IN DKS INTEGER ARRAY[5]) MODIFIES SQL DATA
           BEGIN INSERT INTO E SELECT BASE + X.N, X.DK FROM UNNEST(DKS) WITH ORDINALITY AS X (DK, N); END",
        "CREATE PROCEDURE ADDEV (IN BASE INTEGER, IN DKS INTEGER ARRAY[5]) MODIFIES SQL DATA
           BEGIN INSERT INTO EV SELECT BASE + X.N, X.DK FROM UNNEST(DKS) WITH ORDINALITY AS X (DK, N); END",
        "CREATE TABLE TG (K INTEGER PRIMARY KEY, V INTEGER NOT NULL)",
        "CREATE TABLE SEEN (K INTEGER)",
        "CREATE TRIGGER ON_TG AFTER INSERT ON TG REFERENCING NEW ROW AS R FOR EACH ROW INSERT INTO SEEN VALUES (R.K)",
        "CREATE PROCEDURE ADDTG (IN BASE INTEGER, IN VS INTEGER ARRAY[5]) MODIFIES SQL DATA
           BEGIN INSERT INTO TG SELECT BASE + X.N, NULLIF(X.V, -1) FROM UNNEST(VS) WITH ORDINALITY AS X (V, N); END",
        "CREATE TABLE ST (K INTEGER PRIMARY KEY, V INTEGER NOT NULL, B TIMESTAMP(6) GENERATED ALWAYS AS ROW START,
           F TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (B, F)) WITH SYSTEM VERSIONING",
        "CREATE PROCEDURE ADDST (IN BASE INTEGER, IN VS INTEGER ARRAY[5]) MODIFIES SQL DATA
           BEGIN INSERT INTO ST (K, V) SELECT BASE + X.N, NULLIF(X.V, -1) FROM UNNEST(VS) WITH ORDINALITY AS X (V, N); END",
        "CREATE PROCEDURE ADDST1 (IN KK INTEGER, IN VV INTEGER) MODIFIES SQL DATA BEGIN INSERT INTO ST (K, V) VALUES (KK, NULLIF(VV, -1)); END",
        "CREATE TABLE LB (K INTEGER PRIMARY KEY, V INTEGER NOT NULL, C CLOB(1M))",
        "CREATE PROCEDURE ADDLB (IN BASE INTEGER, IN VS INTEGER ARRAY[5]) MODIFIES SQL DATA
           BEGIN INSERT INTO LB SELECT BASE + X.N, NULLIF(X.V, -1), CAST('text ' || CAST(X.V AS VARCHAR(10)) AS CLOB(1M))
             FROM UNNEST(VS) WITH ORDINALITY AS X (V, N); END",
    ];
    let good = ["10, ARRAY[1, 2]", "20, ARRAY[3, 4, 5]", "30, ARRAY[6]", "40, ARRAY[7, 8]", "50, ARRAY[9, 10]"];
    let mut script = Vec::new();
    script.extend(calls("ADDE", &good));
    script.extend(calls("ADDE", &["60, ARRAY[11, 0]"]));
    script.extend(calls("ADDEV", &["110, ARRAY[1, 2]", "120, ARRAY[3, 4, 5]", "130, ARRAY[6]", "140, ARRAY[7, 8]", "160, ARRAY[11, 200]"]));
    script.extend(
        ["SET CONSTRAINTS ALL DEFERRED", "CALL ADDE(70, ARRAY[12, 0])", "UPDATE E SET DK = 5 WHERE DK = 0", "COMMIT"].map(str::to_owned),
    );
    for procedure in ["ADDTG", "ADDST", "ADDLB"] {
        script.extend(calls(procedure, &good));
        script.extend(calls(procedure, &["60, ARRAY[11, -1]", "49, ARRAY[12, 13]"]));
    }
    script.extend(calls("ADDST1", &["201, 1", "202, 2", "203, 3", "204, 4", "205, 5", "206, -1", "205, 6"]));
    script.extend(
        ["? SELECT K FROM SEEN ORDER BY K", "? SELECT K, V FROM TG ORDER BY K", "? SELECT K, V FROM ST ORDER BY K"].map(str::to_owned),
    );
    script.push("? SELECT COUNT(*) FROM ST WHERE B IS NOT NULL AND F > B".to_owned());
    script.push("? SELECT K, V, CAST(C AS VARCHAR(20)) FROM LB ORDER BY K".to_owned());
    let gave = alike(&setup, &script);
    for broken in ["ADDE(60", "ADDEV(160", "ADDTG(60", "ADDTG(49", "ADDST(60", "ADDST(49", "ADDLB(60", "ADDLB(49"] {
        assert!(gave.iter().any(|l| l.starts_with(&format!("CALL {broken}")) && l.contains("error")), "{broken}: {gave:?}");
    }
    assert!(gave.iter().any(|l| l.starts_with("CALL ADDE(70, ARRAY[12, 0]): ok")), "deferred: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL ADDST1(204, 4): ok")), "one row, its period stamped: {gave:?}");
    assert!(gave.iter().any(|l| l == "SELECT COUNT(*) FROM ST WHERE B IS NOT NULL AND F > B: [\"15\"]"), "{gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("SELECT K, V, CAST(C AS VARCHAR(20)) FROM LB") && l.contains("\"51,9,text 9\"")), "{gave:?}");
    let with_assertion = ["CREATE ASSERTION FEW CHECK (NOT EXISTS (SELECT * FROM E WHERE DK > 1000))", setup[1]];
    let mut asserted = calls("ADDE", &good);
    asserted.extend(calls("ADDE", &["60, ARRAY[11, 2000]"]));
    let gave = alike(&with_assertion, &asserted);
    assert!(gave.iter().any(|l| l.starts_with("CALL ADDE(60, ARRAY[11, 2000]): error 23000")), "assertion: {gave:?}");
}

/// What a session at `level` gets at its commit after CALL LINES inserted
/// rows whose subquery read P's row 3 and whose foreign key references
/// it, once another session committed `change`: compiled or interpreted.
fn lines_then(compiled: bool, level: &str, change: &str) -> String {
    let (db, a) = database(compiled);
    for sql in SETUP.iter().chain(&LINES[..2]) {
        a.run(sql).unwrap();
    }
    a.commit().unwrap();
    for base in [10, 20, 30, 40] {
        a.run(&format!("CALL LINES({base}, ARRAY[{base}, {base} + 1], ARRAY[1, 1])")).unwrap();
        a.commit().unwrap();
    }
    a.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
    a.run("CALL LINES(50, ARRAY[50, 51, 52], ARRAY[1, 3, 3])").unwrap();
    let b = db.session("A");
    b.run(change).unwrap();
    b.commit().unwrap();
    match a.commit() {
        Ok(()) => "committed".to_owned(),
        Err(e) => format!("error {}", e.sqlstate().as_str()),
    }
}

/// Verifies: [DESIGN] RULES.md D901, [ISO/IEC 9075:1992 §4.28] an INSERT
/// of a query's rows written straight reads as the general path does at
/// each isolation level: what its commit gives after another transaction
/// committed a change of what it read is the same compiled and
/// interpreted, at READ COMMITTED and at SERIALIZABLE, of a row its
/// foreign key references deleted, of a value its subquery read changed,
/// and of a row of the table it read left as it was. Its foreign key
/// check reads serializably at every level, so the row a line references
/// deleted before it commits has its commit refused (SQLSTATE 40001) at
/// both. What its subquery read is recorded where reads are checked, so a
/// change of that value is refused at SERIALIZABLE.
#[test]
fn an_insert_of_a_querys_rows_reads_as_its_level_says() {
    let changes =
        ["DELETE FROM P WHERE K = 3", "UPDATE P SET V = 7 WHERE K = 3", "UPDATE P SET V = 7 WHERE K = 4", "DELETE FROM C WHERE K = 3"];
    let outcomes = |compiled: bool| {
        let mut all = Vec::new();
        for level in ["READ COMMITTED", "SERIALIZABLE"] {
            all.extend(changes.iter().map(|change| format!("{level}, {change}: {}", lines_then(compiled, level, change))));
        }
        all
    };
    let (interpreted, compiled) = (outcomes(false), outcomes(true));
    assert_eq!(compiled, interpreted);
    assert!(compiled.contains(&"READ COMMITTED, DELETE FROM P WHERE K = 3: error 40001".to_owned()), "{compiled:?}");
    assert!(compiled.contains(&"SERIALIZABLE, DELETE FROM P WHERE K = 3: error 40001".to_owned()), "{compiled:?}");
    assert!(compiled.contains(&"SERIALIZABLE, UPDATE P SET V = 7 WHERE K = 3: error 40001".to_owned()), "{compiled:?}");
    assert!(compiled.contains(&"READ COMMITTED, DELETE FROM C WHERE K = 3: committed".to_owned()), "{compiled:?}");
    assert!(compiled.contains(&"SERIALIZABLE, DELETE FROM C WHERE K = 3: committed".to_owned()), "{compiled:?}");
}

/// The records a memory log holds after `script` ran in a database
/// compiled or not, `SETUP` and `LINES` made in it first: each record's
/// bytes, in order.
fn logged(compiled: bool, script: &[String]) -> Vec<Vec<u8>> {
    use turborel::log::{Log, Lsn, MemoryLog};
    let (db, a) = database(compiled);
    // Each record's instants are the fixed clock's, the same of both.
    db.set_clock(std::sync::Arc::new(turborel::clock::FixedClock::at(86_400_000_000))).unwrap();
    let log = std::sync::Arc::new(MemoryLog::new());
    db.set_log(std::sync::Arc::clone(&log) as std::sync::Arc<dyn Log>).unwrap();
    for sql in SETUP.iter().chain(&LINES) {
        a.run(sql).unwrap();
    }
    a.commit().unwrap();
    for sql in script {
        // A statement refused leaves its transaction open, its rows undone.
        let _ = a.run(sql);
        a.commit().unwrap();
    }
    log.read_from(Lsn(0)).map(|r| r.unwrap().1).collect()
}

/// Verifies: [DESIGN] RULES.md D901, D530, an INSERT of a query's rows
/// written straight logs what the general path logs, record for record:
/// its rows together in one record where there are two or more, one row
/// alone in its own, a statement refused undone, and rows enough to be a
/// load as the general path loads them.
#[test]
fn an_insert_of_a_querys_rows_logs_what_the_general_path_logs() {
    let mut script = calls("LINES", &["10, ARRAY[1, 2, 3], ARRAY[1, 2, 3]", "20, ARRAY[4, 5], ARRAY[1, 1]", "30, ARRAY[6], ARRAY[2]"]);
    script.extend(calls(
        "LINES",
        &["40, ARRAY[7, 8, 9, 10], ARRAY[1, 2, 3, 4]", "50, ARRAY[11], ARRAY[4]", "60, ARRAY[12, 12], ARRAY[1, 1]"],
    ));
    script.extend(calls("COPYL", &["1000, 5000", "1000, 0", "1000, 0", "2000, 25"]));
    script.extend(calls("MANY", &["0, 1", "100000, 1", "200000, 1", "300000, 1", "400000, 0", "500000, 1"]));
    let (interpreted, compiled) = (logged(false, &script), logged(true, &script));
    assert_eq!(compiled.len(), interpreted.len(), "as many records");
    for (n, (c, i)) in compiled.iter().zip(&interpreted).enumerate() {
        assert_eq!(c, i, "record {n}");
    }
    assert!(compiled.len() > 30, "{} records", compiled.len());
}
