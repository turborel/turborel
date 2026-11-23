//! \[DESIGN\] A procedure's body compiled (RULES.md D841): each body here
//! runs both compiled and by the interpreter, EXECUTOR.COMPILED_PROCEDURES
//! ON and OFF, in databases made alike, and each statement's completion
//! or error, each value read back and each table as it ends are the same
//! of both: of the shapes that run straight on their table and of those
//! that fall back to the general path, of the conditions handlers handle,
//! of the warnings, of the constraints, of the triggers, of the isolation
//! levels, and once a definition, a privilege or the schema changes.

use turborel::db::{Database, Session};

/// The tables each test starts with, in schema A: S, stock by warehouse
/// and item; D, districts with their next order number; O, orders; L,
/// order lines; and N, a log with an identity, a default and a generated
/// column.
const TABLES: &str = "CREATE SCHEMA AUTHORIZATION A
   CREATE TABLE S (W INTEGER NOT NULL, I INTEGER NOT NULL, Q INTEGER CHECK (Q >= 0), NAME VARCHAR(10), PRIMARY KEY (W, I))
   CREATE TABLE D (W INTEGER NOT NULL, D INTEGER NOT NULL, NEXT_O INTEGER, TAX DECIMAL(4,2), PRIMARY KEY (W, D))
   CREATE TABLE O (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, LINES INTEGER, PRIMARY KEY (W, D, O))
   CREATE TABLE L (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, N INTEGER NOT NULL, I INTEGER, AMOUNT DECIMAL(8,2),
     PRIMARY KEY (W, D, O, N), FOREIGN KEY (W, D, O) REFERENCES O)
   CREATE TABLE N (ID INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY, NOTE VARCHAR(20) DEFAULT 'none', K INTEGER UNIQUE,
     TWICE INTEGER GENERATED ALWAYS AS (K * 2))";

/// A database with `TABLES` and some rows, and A's session in it, compiled
/// procedures ON or OFF.
pub(crate) fn database(compiled: bool) -> (Database, Session) {
    let db = Database::new();
    db.schema(TABLES).unwrap();
    db.schema("CREATE SCHEMA B AUTHORIZATION A CREATE TABLE S (W INTEGER NOT NULL, I INTEGER NOT NULL, Q INTEGER, PRIMARY KEY (W, I))")
        .unwrap();
    let a = db.session("A");
    a.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    let stock: Vec<String> = (1..=20_i64).flat_map(|i| (1..=2_i64).map(move |w| format!("({w}, {i}, 20, 'item {i}')"))).collect();
    a.run(&format!("INSERT INTO S VALUES {}", stock.join(", "))).unwrap();
    a.run("INSERT INTO D VALUES (1, 1, 1, 0.10), (1, 2, 1, 0.05), (2, 1, 1, 0.20)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// What a statement gave: its completion, or its error whole.
fn outcome<T: std::fmt::Debug>(r: turborel::error::Result<T>) -> String {
    match r {
        Ok(done) => format!("ok {done:?}"),
        Err(e) => format!("error {} {} {e:?}", e.sqlstate().as_str(), e.sqlcode()),
    }
}

/// The rows of `query` in `s`, each its values as text joined by commas;
/// or its error.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = match s.open_cursor(query) {
        Ok(c) => c,
        Err(e) => return vec![outcome::<()>(Err(e))],
    };
    let mut out = Vec::new();
    loop {
        match c.fetch() {
            Ok(Some(row)) => out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(",")),
            Ok(None) => return out,
            Err(e) => {
                out.push(outcome::<()>(Err(e)));
                return out;
            }
        }
    }
}

/// What `sql` gave: a CALL with `?`s for its outputs prepared and
/// executed, its outputs with its completion; any other statement run.
fn called(s: &Session, sql: &str) -> String {
    if !sql.contains('?') {
        return outcome(s.run(sql));
    }
    match s.prepare(sql) {
        Ok(p) => {
            outcome(s.execute_prepared_out(&p, &[]).map(|(done, out)| (done, out.iter().map(ToString::to_string).collect::<Vec<_>>())))
        }
        Err(e) => outcome::<()>(Err(e)),
    }
}

/// Each table of `TABLES` as it is, in order.
fn tables(s: &Session) -> Vec<String> {
    let mut all = Vec::new();
    for q in [
        "SELECT W, I, Q, NAME FROM S ORDER BY W, I",
        "SELECT W, D, NEXT_O, TAX FROM D ORDER BY W, D",
        "SELECT W, D, O, LINES FROM O ORDER BY W, D, O",
        "SELECT W, D, O, N, I, AMOUNT FROM L ORDER BY W, D, O, N",
        "SELECT ID, NOTE, K, TWICE FROM N ORDER BY ID",
    ] {
        all.extend(rows(s, q));
    }
    all
}

/// What running `script` gives in `s`: each statement's outcome, each
/// query's rows, a statement and its query apart by a line `--`.
fn ran(s: &Session, script: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for sql in script {
        if let Some(query) = sql.strip_prefix("? ") {
            out.push(format!("{query}: {:?}", rows(s, query)));
        } else {
            out.push(format!("{sql}: {}", called(s, sql)));
        }
    }
    out
}

/// `setup`, then `script`, run in a database compiled and in one not:
/// what each gave the same, and its tables as they end. What they gave,
/// to be checked further.
pub(crate) fn same(setup: &[&str], script: &[&str]) -> Vec<String> {
    let mut seen = Vec::new();
    for compiled in [false, true] {
        let (_db, a) = database(compiled);
        for sql in setup {
            a.run(sql).unwrap();
        }
        a.commit().unwrap();
        let mut gave = ran(&a, script);
        a.commit().unwrap();
        gave.extend(tables(&a));
        seen.push(gave);
    }
    let compiled = seen.pop().unwrap();
    let interpreted = seen.pop().unwrap();
    for (n, (c, i)) in compiled.iter().zip(&interpreted).enumerate() {
        assert_eq!(c, i, "line {n}: compiled, then interpreted");
    }
    assert_eq!(compiled.len(), interpreted.len());
    compiled
}

/// NEW_ORDER: reads a district's next order number and takes it by an
/// UPDATE's FINAL TABLE, writes the order, and for each item reads its
/// stock by key, takes from it, and writes a line; the count of lines and
/// of no data read back through OUT parameters.
const NEW_ORDER: &str = "CREATE PROCEDURE NEW_ORDER (IN WH INTEGER, IN DI INTEGER, IN ITEMS INTEGER ARRAY[10], IN QS INTEGER ARRAY[10],
     OUT TAKEN INTEGER, OUT MISSING INTEGER, OUT ROWS_CHANGED INTEGER) MODIFIES SQL DATA
   BEGIN
     DECLARE NEXT_NO, LEFT_OVER, NTH INTEGER DEFAULT 0;
     DECLARE T DECIMAL(4,2);
     DECLARE ITEM_NAME VARCHAR(10);
     DECLARE CONTINUE HANDLER FOR NOT FOUND SET MISSING = MISSING + 1;
     SET (TAKEN, MISSING) = (0, 0);
     SELECT NEXT_O + DI - DI, TAX INTO NEXT_NO, T FROM FINAL TABLE (UPDATE D SET NEXT_O = NEXT_O + 1 WHERE W = WH AND D = DI);
     INSERT INTO O VALUES (WH, DI, NEXT_NO - 1, CARDINALITY(ITEMS));
     WHILE NTH < CARDINALITY(ITEMS) DO
       SET NTH = NTH + 1;
       SET ITEM_NAME = NULL;
       SELECT NAME INTO ITEM_NAME FROM S WHERE W = WH AND I = ITEMS[NTH];
       IF ITEM_NAME IS NOT NULL THEN
         UPDATE S SET Q = CASE WHEN Q >= QS[NTH] + 10 THEN Q - QS[NTH] ELSE Q - QS[NTH] + 91 END WHERE W = WH AND I = ITEMS[NTH];
         GET DIAGNOSTICS ROWS_CHANGED = ROW_COUNT;
         INSERT INTO L VALUES (WH, DI, NEXT_NO - 1, NTH, ITEMS[NTH], QS[NTH] * (1 + T));
         SET TAKEN = TAKEN + 1;
       END IF;
     END WHILE;
   END";

/// Verifies: [DESIGN] RULES.md D841, a body of the shapes that run
/// straight on their table (a SELECT ... INTO of a whole key, an UPDATE's
/// FINAL TABLE, an UPDATE by key, an INSERT of one row) runs compiled as
/// it does interpreted, CALLed again and again, with an item missing
/// counted by its NOT FOUND handler.
#[test]
fn a_body_of_point_shapes_runs_as_interpreted() {
    let mut script = Vec::new();
    for (w, d, items) in [(1, 1, "1, 2, 3"), (1, 2, "4, 99, 5"), (2, 1, "7, 7"), (1, 1, "1, 8, 9, 10"), (1, 1, "2")] {
        script.push(format!("CALL NEW_ORDER({w}, {d}, ARRAY[{items}], ARRAY[5, 6, 7, 8], ?, ?, ?)"));
    }
    let script: Vec<&str> = script.iter().map(String::as_str).collect();
    let gave = same(&[NEW_ORDER], &script);
    assert!(gave.contains(&"1,1,1,3".to_owned()), "the first order, of three lines: {gave:?}");
    assert!(gave.contains(&"1,1,4,0.10".to_owned()), "district 1 of warehouse 1 took three orders: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, the values OUT parameters take, the
/// error store assignment raises of a value too long for its target
/// [ISO/IEC 9075-2:2023 §9.2 GR3b ii, 22001], a duplicate key, ROW_COUNT
/// after each change and no data of a change that changes no row are the
/// same compiled.
#[test]
fn out_values_errors_and_row_counts_are_kept() {
    let body = "CREATE PROCEDURE P (IN WH INTEGER, IN ITEM INTEGER, OUT NAMED CHAR(6), OUT C1 INTEGER, OUT C2 INTEGER, OUT C3 INTEGER)
       MODIFIES SQL DATA
       BEGIN
         SELECT NAME INTO NAMED FROM S WHERE W = WH AND I = ITEM;
         UPDATE S SET Q = Q - 1 WHERE W = WH AND I = ITEM;
         GET DIAGNOSTICS C1 = ROW_COUNT;
         DELETE FROM L WHERE W = WH AND D = 1 AND O = 1 AND N = ITEM;
         GET DIAGNOSTICS C2 = ROW_COUNT;
         INSERT INTO N (K) VALUES (ITEM);
         GET DIAGNOSTICS C3 = ROW_COUNT;
       END";
    let mut script = Vec::new();
    for (w, i) in [(1, 1), (1, 12), (2, 30), (1, 4), (2, 4), (1, 9)] {
        script.push(format!("CALL P({w}, {i}, ?, ?, ?, ?)"));
    }
    let script: Vec<&str> = script.iter().map(String::as_str).collect();
    let gave = same(&[body], &script);
    assert!(gave.iter().any(|l| l.contains("error 22001")), "a name too long for CHAR(6): {gave:?}");
    assert!(gave.iter().any(|l| l.contains("error 23000")), "the second K = 4 is a duplicate: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, an error a direct change raises is
/// the interpreter's, SQLSTATE and SQLCODE and message alike, and undoes
/// that statement alone: a CHECK constraint broken by an UPDATE by key,
/// a primary key duplicated by an INSERT of one row, a foreign key with
/// no row it references, a null value in a NOT NULL column, and a unique
/// key broken as the statement ends.
#[test]
fn constraint_errors_are_the_interpreters() {
    let body = "CREATE PROCEDURE P (IN WH INTEGER, IN ITEM INTEGER, IN TAKE INTEGER, IN ORD INTEGER, OUT FAILED VARCHAR(5))
       MODIFIES SQL DATA
       BEGIN
         DECLARE CONTINUE HANDLER FOR SQLEXCEPTION GET DIAGNOSTICS CONDITION 1 FAILED = RETURNED_SQLSTATE;
         UPDATE S SET Q = Q - TAKE WHERE W = WH AND I = ITEM;
         INSERT INTO O VALUES (WH, 1, ORD, TAKE);
         INSERT INTO L VALUES (WH, 1, ORD + 100, 1, ITEM, 1);
         INSERT INTO S VALUES (WH, ITEM + ORD, NULL, 'x');
         UPDATE S SET I = ITEM + 1 WHERE W = WH AND I = ITEM;
       END";
    let mut script = Vec::new();
    for (w, i, t, o) in [(1, 1, 5, 1), (1, 1, 50, 1), (1, 3, 1, 2), (2, 20, 1, 2), (1, 4, 1, 40)] {
        script.push(format!("CALL P({w}, {i}, {t}, {o}, ?)"));
    }
    let bare = "CREATE PROCEDURE Q (IN WH INTEGER, IN ITEM INTEGER) MODIFIES SQL DATA UPDATE S SET Q = -1 WHERE W = WH AND I = ITEM";
    let unhandled = "CREATE PROCEDURE R (IN WH INTEGER, IN ITEM INTEGER) MODIFIES SQL DATA BEGIN UPDATE S SET Q = Q - 1 WHERE W = WH AND I = ITEM; INSERT INTO O VALUES (WH, 9, 9, NULL); INSERT INTO O VALUES (WH, 9, 9, NULL); END";
    script.extend(["CALL Q(1, 5)".to_owned(), "CALL Q(1, 5)".to_owned(), "CALL R(1, 6)".to_owned(), "CALL R(1, 6)".to_owned()]);
    let script: Vec<&str> = script.iter().map(String::as_str).collect();
    let gave = same(&[body, bare, unhandled], &script);
    assert!(gave.iter().any(|l| l.contains("CHECK A.S_CHECK1")), "a CHECK broken: {gave:?}");
    assert!(gave.iter().any(|l| l.contains("O_PRIMARY1")), "a primary key duplicated: {gave:?}");
    assert_eq!(gave.iter().filter(|l| l.starts_with("CALL R(1, 6): error 23000")).count(), 2, "R fails both times: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("1,6,") && l.ends_with(",item 6")), "{gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, control flow compiled as the
/// interpreter runs it: CASE with no WHEN true and no ELSE (SQLSTATE
/// 20000), LEAVE and ITERATE, REPEAT, a labelled loop, SIGNAL and RESIGNAL
/// with an EXIT handler, an UNDO handler of an ATOMIC compound statement,
/// a SELECT ... INTO of more than one row (21000), and RETURN-less ends.
#[test]
fn control_flow_and_conditions_are_the_interpreters() {
    let body = "CREATE PROCEDURE P (IN K INTEGER, OUT V INTEGER, OUT M VARCHAR(40)) MODIFIES SQL DATA
       BEGIN
         DECLARE N INTEGER DEFAULT 0;
         DECLARE EXIT HANDLER FOR SQLSTATE '75001' BEGIN GET DIAGNOSTICS CONDITION 1 M = MESSAGE_TEXT; SET V = -V; END;
         SET V = 0;
         L1: LOOP
           SET N = N + 1;
           IF N > 6 THEN LEAVE L1; END IF;
           IF MOD(N, 2) = 0 THEN ITERATE L1; END IF;
           SET V = V + N;
         END LOOP L1;
         REPEAT SET V = V * 2; UNTIL V > K * 10 END REPEAT;
         B: BEGIN ATOMIC
           DECLARE UNDO HANDLER FOR SQLEXCEPTION SET M = 'undone';
           UPDATE S SET Q = Q + 1 WHERE W = 1 AND I = K;
           UPDATE S SET Q = Q / (K - 3) WHERE W = 1 AND I = K;
         END B;
         CASE K WHEN 1 THEN SET M = COALESCE(M, 'one'); WHEN 2 THEN SIGNAL SQLSTATE '75001' SET MESSAGE_TEXT = 'two'; WHEN 4 THEN
           SELECT Q INTO N FROM S WHERE I = K;
         END CASE;
       END";
    let resignal = "CREATE PROCEDURE R (IN K INTEGER) MODIFIES SQL DATA
       BEGIN
         DECLARE EXIT HANDLER FOR SQLEXCEPTION RESIGNAL SET MESSAGE_TEXT = 'again';
         UPDATE S SET Q = Q - 100 WHERE W = 1 AND I = K;
       END";
    let script = ["CALL P(1, ?, ?)", "CALL P(2, ?, ?)", "CALL P(3, ?, ?)", "CALL P(4, ?, ?)", "CALL P(5, ?, ?)", "CALL R(1)", "CALL R(1)"];
    let gave = same(&[body, resignal], &script);
    assert!(gave.iter().any(|l| l.contains("20000")), "CASE not found: {gave:?}");
    assert!(gave.iter().any(|l| l.contains("21000")), "more than one row: {gave:?}");
    assert!(gave.iter().any(|l| l.contains("again")), "resignalled: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, a statement of a table some trigger
/// is on runs the general path, its triggers fired as interpreted; a
/// cursor and FOR, which fall back too, read the same rows.
#[test]
fn triggers_cursors_and_for_fall_back_alike() {
    let trigger = "CREATE TRIGGER LOGGED AFTER UPDATE ON S REFERENCING NEW ROW AS NR FOR EACH ROW INSERT INTO N (NOTE, K) VALUES ('stock', NR.Q * 1000 + NR.I)";
    let before =
        "CREATE TRIGGER NAMED BEFORE INSERT ON O REFERENCING NEW ROW AS NR FOR EACH ROW SET NR.LINES = COALESCE(NR.LINES, 0) + 100";
    let body = "CREATE PROCEDURE P (IN WH INTEGER, IN ITEM INTEGER, OUT TOTAL INTEGER) MODIFIES SQL DATA
       BEGIN
         DECLARE QTY INTEGER;
         DECLARE DONE BOOLEAN DEFAULT FALSE;
         DECLARE C CURSOR FOR SELECT Q FROM S WHERE W = WH AND I <= ITEM ORDER BY I;
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE;
         UPDATE S SET Q = Q - 1 WHERE W = WH AND I = ITEM;
         INSERT INTO O VALUES (WH, 3, ITEM, NULL);
         SET TOTAL = 0;
         OPEN C;
         FETCH C INTO QTY;
         WHILE NOT DONE DO SET TOTAL = TOTAL + QTY; FETCH C INTO QTY; END WHILE;
         CLOSE C;
         FOR R AS SELECT I FROM S WHERE W = WH AND I < 3 DO SET TOTAL = TOTAL + R.I; END FOR;
       END";
    let script = ["CALL P(1, 2, ?)", "CALL P(1, 3, ?)", "CALL P(2, 5, ?)", "CALL P(1, 2, ?)"];
    let gave = same(&[trigger, before, body], &script);
    assert!(gave.iter().any(|l| l.contains(",stock,")), "the trigger fired: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, a DELETE's OLD TABLE read by a
/// SELECT ... INTO, a DELETE by key, an INSERT of defaults, identity and a
/// generated column, and an INSERT ... SELECT, the same compiled.
#[test]
fn old_tables_deletes_and_generated_values_alike() {
    let body = "CREATE PROCEDURE P (IN WH INTEGER, IN ITEM INTEGER, OUT GONE INTEGER, OUT LAST_ID INTEGER) MODIFIES SQL DATA
       BEGIN
         SELECT Q INTO GONE FROM OLD TABLE (DELETE FROM S WHERE W = WH AND I = ITEM);
         DELETE FROM S WHERE W = WH AND I = ITEM + 1;
         INSERT INTO N (K) VALUES (ITEM * 10 + WH);
         INSERT INTO N (NOTE, K) VALUES ('given', ITEM * 10 + WH + 5);
         INSERT INTO N (NOTE, K) SELECT 'copied', I + 500 FROM S WHERE W = WH AND I BETWEEN ITEM + 2 AND ITEM + 3;
         SELECT MAX(ID) INTO LAST_ID FROM N;
       END";
    let script = ["CALL P(1, 1, ?, ?)", "CALL P(1, 1, ?, ?)", "CALL P(2, 4, ?, ?)", "CALL P(1, 7, ?, ?)", "? SELECT COUNT(*) FROM N"];
    let gave = same(&[body], &script);
    assert!(gave.iter().any(|l| l.ends_with(",given,16,32")), "{gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, a procedure CALLed, then its tables
/// changed (a column added, an index made, a row trigger made) and its
/// definer's privilege revoked, runs as interpreted after each: its
/// compiled body is made again with its program.
#[test]
fn definitions_and_privileges_changed_compile_again() {
    let mut seen = Vec::new();
    for compiled in [false, true] {
        let db = Database::new();
        db.schema(
            "CREATE SCHEMA AUTHORIZATION C CREATE TABLE T (K INTEGER PRIMARY KEY, V INTEGER)
             CREATE TABLE G (K INTEGER, V INTEGER)",
        )
        .unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let (a, c) = (db.session("A"), db.session("C"));
        a.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
        c.run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
        c.run("GRANT SELECT, UPDATE, INSERT ON T TO A").unwrap();
        c.commit().unwrap();
        a.run(
            "CREATE PROCEDURE P (IN KEY_OF INTEGER, OUT GOT INTEGER) MODIFIES SQL DATA
             BEGIN
               UPDATE C.T SET V = V + 1 WHERE K = KEY_OF;
               SELECT V INTO GOT FROM C.T WHERE K = KEY_OF;
             END",
        )
        .unwrap();
        a.commit().unwrap();
        let mut gave = Vec::new();
        let call = |gave: &mut Vec<String>, k: i64| {
            gave.push(called(&a, &format!("CALL P({k}, ?)")));
            gave.push(outcome(a.commit()));
        };
        for k in [1, 2, 1] {
            call(&mut gave, k);
        }
        for change in [
            "ALTER TABLE T ADD COLUMN W INTEGER DEFAULT 7",
            "CREATE INDEX T_V ON T (V)",
            "CREATE TRIGGER COPY AFTER UPDATE ON T REFERENCING NEW ROW AS NR FOR EACH ROW INSERT INTO G VALUES (NR.K, NR.V)",
            "REVOKE UPDATE ON T FROM A CASCADE",
        ] {
            gave.push(outcome(c.run(change)));
            gave.push(outcome(c.commit()));
            for k in [1, 3] {
                call(&mut gave, k);
            }
        }
        c.run("GRANT UPDATE ON T TO A").unwrap();
        c.commit().unwrap();
        call(&mut gave, 2);
        gave.extend(rows(&c, "SELECT K, V FROM T ORDER BY K"));
        gave.extend(rows(&c, "SELECT K, V FROM G ORDER BY K, V"));
        seen.push(gave);
    }
    assert_eq!(seen[1], seen[0], "compiled, then interpreted");
    assert!(seen[1].iter().any(|l| l.starts_with("error")), "the revoked privilege refuses a CALL: {:?}", seen[1]);
}

/// Verifies: [DESIGN] RULES.md D841, a procedure whose statements name
/// tables unqualified reads its own schema's, the session's schema set
/// elsewhere, compiled as interpreted.
#[test]
fn a_schema_set_changes_nothing_a_body_names() {
    let setup = [
        "INSERT INTO B.S VALUES (1, 1, 500)",
        "CREATE PROCEDURE P (IN ITEM INTEGER, OUT GOT INTEGER) MODIFIES SQL DATA BEGIN UPDATE S SET Q = Q + 1 WHERE W = 1 AND I = ITEM; SELECT Q INTO GOT FROM S WHERE W = 1 AND I = ITEM; END",
    ];
    let script = ["CALL P(1, ?)", "SET SCHEMA 'B'", "CALL P(1, ?)", "? SELECT Q FROM S WHERE I = 1", "SET SCHEMA 'A'", "CALL P(1, ?)"];
    let gave = same(&setup, &script);
    assert!(gave.iter().any(|l| l.contains("[\"500\"]")), "B.S is not changed: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D841, RULES.md D454: at READ COMMITTED a
/// compiled body's point read reads at a snapshot taken as its statement
/// begins, seeing what another committed since; at SERIALIZABLE its reads
/// are recorded, so of two transactions each reading what the other
/// writes, one fails as it commits, as interpreted.
#[test]
fn isolation_levels_are_kept() {
    let mut seen = Vec::new();
    for compiled in [false, true] {
        let mut gave = Vec::new();
        for level in ["READ COMMITTED", "SERIALIZABLE"] {
            let (db, a) = database(compiled);
            a.run(
                "CREATE PROCEDURE MOVE (IN FROM_I INTEGER, IN TO_I INTEGER, OUT GOT INTEGER) MODIFIES SQL DATA
                 BEGIN
                   SELECT Q INTO GOT FROM S WHERE W = 1 AND I = FROM_I;
                   UPDATE S SET Q = Q + GOT WHERE W = 1 AND I = TO_I;
                 END",
            )
            .unwrap();
            a.commit().unwrap();
            let b = db.session("A");
            b.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
            for s in [&a, &b] {
                s.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
            }
            gave.push(called(&a, "CALL MOVE(1, 2, ?)"));
            gave.push(outcome(b.run("UPDATE S SET Q = 77 WHERE W = 1 AND I = 3")));
            gave.push(outcome(b.commit()));
            gave.push(outcome(b.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}"))));
            gave.push(called(&a, "CALL MOVE(3, 4, ?)"));
            gave.push(called(&b, "CALL MOVE(4, 3, ?)"));
            gave.push(outcome(a.commit()));
            gave.push(outcome(b.commit()));
            gave.extend(rows(&a, "SELECT I, Q FROM S WHERE W = 1 AND I <= 4 ORDER BY I"));
        }
        seen.push(gave);
    }
    assert_eq!(seen[1], seen[0], "compiled, then interpreted");
}

/// Verifies: [DESIGN] RULES.md D841, INFORMATION_SCHEMA.STATEMENT_STATISTICS
/// counts each CALL of a compiled body once, with the rows it read, as of
/// one interpreted.
#[test]
fn statement_statistics_count_the_same() {
    let script = [
        "CALL NEW_ORDER(1, 1, ARRAY[1, 2], ARRAY[1, 1], ?, ?, ?)",
        "CALL NEW_ORDER(1, 1, ARRAY[3, 4], ARRAY[1, 1], ?, ?, ?)",
        "CALL NEW_ORDER(1, 2, ARRAY[5], ARRAY[1], ?, ?, ?)",
        "? SELECT CALLS, ROW_COUNT, ROWS_READ FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS WHERE STATEMENT_TEXT LIKE 'CALL NEW_ORDER%'",
    ];
    let gave = same(&[NEW_ORDER], &script);
    assert!(gave.iter().any(|l| l.contains("[\"2,0,")), "two calls of one text: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D943: a compiled body nothing of which
/// reads the diagnostics area places no statement's condition there, and
/// gives what the interpreter gives; one with a GET DIAGNOSTICS and no
/// handler reads the statement before it, its command and its row count,
/// and a CALL after either reads its own area. MC/DC of what places the
/// conditions: a GET DIAGNOSTICS in the body or none.
#[test]
fn a_body_that_reads_no_diagnostics_gives_the_same() {
    let plain = "CREATE PROCEDURE PLAIN (IN WH INTEGER, OUT LEFT_Q INTEGER) MODIFIES SQL DATA
       BEGIN
         DECLARE N INTEGER DEFAULT 0;
         WHILE N < 3 DO
           SET N = N + 1;
           UPDATE S SET Q = Q - 1 WHERE W = WH AND I = N;
           IF N = 2 THEN INSERT INTO O VALUES (WH, 1, N, 0); END IF;
         END WHILE;
         SELECT Q INTO LEFT_Q FROM S WHERE W = WH AND I = 1;
         SELECT Q INTO N FROM S WHERE W = WH AND I = 99;
       END";
    let reading = "CREATE PROCEDURE READING (IN WH INTEGER, OUT WHAT VARCHAR(30), OUT CHANGED INTEGER) MODIFIES SQL DATA
       BEGIN
         UPDATE S SET Q = Q - 1 WHERE W = WH AND I < 4;
         GET DIAGNOSTICS WHAT = COMMAND_FUNCTION, CHANGED = ROW_COUNT;
       END";
    let script = [
        "CALL PLAIN(1, ?)",
        "CALL READING(1, ?, ?)",
        "CALL PLAIN(2, ?)",
        "CALL READING(2, ?, ?)",
        "CALL PLAIN(1, ?)",
        "CALL READING(1, ?, ?)",
    ];
    let gave = same(&[plain, reading], &script);
    assert!(gave[0].contains("\"19\""), "{gave:?}");
    assert!(gave[5].contains("UPDATE WHERE") && gave[5].contains("\"3\""), "the statement before it: {gave:?}");
}
