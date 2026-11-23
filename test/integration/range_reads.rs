//! \[DESIGN\] Direct range reads (RULES.md D930). A query of a kept body
//! that reads one range of an index, under a filter, a projection, FETCH
//! FIRST, a sort or set functions, is read where its plan is kept once
//! the plan is learned. Each body here runs with EXECUTOR.POINT_OPERATIONS
//! ON and OFF, compiled and by the interpreter, in databases made alike,
//! and every CALL's outputs, completion or error are the same of all.

use turborel::db::{Database, Session};
use turborel::value::{Decimal, Value};

/// Schema A: T (W, D, K, N, V, S), its primary key (W, D, K), of 2400
/// rows, 120 of each of 20 districts, N null of every seventh row and S
/// one of thirteen names, with an index by name; P (W, K, NAME, Q), its
/// primary key (W, K), of 601 rows a W; and the procedures of `PROCEDURES`.
fn defined() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (W INTEGER NOT NULL, D INTEGER NOT NULL, K INTEGER NOT NULL, N INTEGER, V DECIMAL(8,2), S VARCHAR(12),
             PRIMARY KEY (W, D, K))
           CREATE TABLE L (K INTEGER, GOT INTEGER)
           CREATE TABLE P (W INTEGER NOT NULL, K INTEGER NOT NULL, NAME VARCHAR(12), Q INTEGER, PRIMARY KEY (W, K))",
    )
    .unwrap();
    let a = db.session("A");
    for w in 1..=2_i64 {
        for d in 1..=10_i64 {
            let rows: Vec<String> = (1..=120_i64)
                .map(|k| {
                    let n = if k % 7 == 0 { "NULL".to_owned() } else { (k % 5).to_string() };
                    format!("({w}, {d}, {k}, {n}, {}.50, 'S{}')", k.saturating_mul(d), k % 13)
                })
                .collect();
            a.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
        }
    }
    for w in 1..=2_i64 {
        let rows: Vec<String> = (0..=600_i64).map(|k| format!("({w}, {k}, 'P{k}', {})", k % 9)).collect();
        a.run(&format!("INSERT INTO P VALUES {}", rows.join(", "))).unwrap();
    }
    a.commit().unwrap();
    a.run("CREATE INDEX T_S ON T (W, D, S, K)").unwrap();
    for p in PROCEDURES {
        a.run(p).unwrap();
    }
    a.commit().unwrap();
    db
}

/// The procedures: each reads one range of an index of T into its OUT
/// parameters.
const PROCEDURES: [&str; 10] = [
    "CREATE PROCEDURE CNT (IN PW INTEGER, IN PD INTEGER, OUT C INTEGER) READS SQL DATA
     BEGIN SELECT COUNT(N) INTO C FROM T WHERE W = PW AND D = PD; END",
    "CREATE PROCEDURE AGG (IN PW INTEGER, IN PD INTEGER, IN LO INTEGER, IN HI INTEGER,
       OUT C INTEGER, OUT TOTAL DECIMAL(12,2), OUT LEAST INTEGER, OUT MOST INTEGER, OUT KINDS INTEGER) READS SQL DATA
     BEGIN SELECT COUNT(*), SUM(V), MIN(K), MAX(K), COUNT(DISTINCT N) INTO C, TOTAL, LEAST, MOST, KINDS
       FROM T WHERE W = PW AND D = PD AND K >= LO AND K < HI; END",
    "CREATE PROCEDURE LAST_ONE (IN PW INTEGER, IN PD INTEGER, IN PN INTEGER, OUT GOT_K INTEGER, OUT GOT_V DECIMAL(8,2)) READS SQL DATA
     BEGIN SELECT K, V INTO GOT_K, GOT_V FROM (SELECT K, V FROM T WHERE W = PW AND D = PD AND N = PN
       ORDER BY K DESC FETCH FIRST 1 ROW ONLY) AS LAST_ROW; END",
    "CREATE PROCEDURE LEAST_K (IN PW INTEGER, IN PD INTEGER, IN LO INTEGER, OUT GOT INTEGER) READS SQL DATA
     BEGIN SELECT MIN(K) INTO GOT FROM T WHERE W = PW AND D = PD AND K > LO; END",
    "CREATE PROCEDURE NAMED (IN PW INTEGER, IN PD INTEGER, IN PS VARCHAR(12), OUT C INTEGER, OUT FIRST_K INTEGER) READS SQL DATA
     BEGIN SELECT COUNT(S), MIN(K) INTO C, FIRST_K FROM T WHERE S = PS AND D = PD AND W = PW; END",
    "CREATE PROCEDURE ONE_ROW (IN PW INTEGER, IN PD INTEGER, IN LO INTEGER, OUT GOT INTEGER, OUT NAME VARCHAR(12)) READS SQL DATA
     BEGIN SELECT K, S INTO GOT, NAME FROM T WHERE W = PW AND D = PD AND K >= LO; END",
    "CREATE PROCEDURE SHARED (IN PW INTEGER, IN PD INTEGER, IN HI INTEGER, OUT C INTEGER, OUT PART DECIMAL(14,4)) READS SQL DATA
     BEGIN SELECT COUNT(*), SUM(V / N) INTO C, PART FROM T WHERE W = PW AND D = PD AND K < HI; END",
    "CREATE PROCEDURE SORTED (IN PW INTEGER, IN PD INTEGER, IN PS VARCHAR(12), OUT GOT_K INTEGER, OUT GOT_N INTEGER) READS SQL DATA
     BEGIN SELECT K, N INTO GOT_K, GOT_N FROM (SELECT K, N FROM T WHERE W = PW AND D = PD AND S = PS
       ORDER BY V DESC, K OFFSET 2 ROWS FETCH FIRST 1 ROW ONLY) AS THIRD; END",
    "CREATE PROCEDURE PAIRED (IN PW INTEGER, IN PD INTEGER, IN PK INTEGER, OUT GOT_V DECIMAL(8,2), OUT GOT_NAME VARCHAR(12))
       READS SQL DATA
     BEGIN SELECT V, NAME INTO GOT_V, GOT_NAME FROM T, P
       WHERE P.W = PW AND P.K = PK AND T.W = PW AND T.D = PD AND T.K = PK; END",
    "CREATE PROCEDURE STOCKED (IN PW INTEGER, IN PD INTEGER, IN HI INTEGER, IN LESS INTEGER, OUT KINDS INTEGER) READS SQL DATA
     BEGIN SELECT COUNT(DISTINCT P.K) INTO KINDS FROM T, P
       WHERE T.W = PW AND T.D = PD AND T.K < HI AND T.K >= HI - 20 AND P.W = PW AND P.K = T.N AND P.Q < LESS; END",
];

/// The exact number `n`.
fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// What `call` with `values` gave in `s`: its completion and outputs, or
/// its error.
fn called(s: &Session, call: &str, values: &[Value]) -> String {
    let gave = s.prepare(call).and_then(|p| s.execute_prepared_out(&p, values));
    let shown = match gave {
        Ok((done, out)) => format!("ok {done:?} {:?}", out.iter().map(ToString::to_string).collect::<Vec<_>>()),
        Err(e) => format!("error {} {} {}", e.sqlstate().as_str(), e.sqlcode(), e.msg),
    };
    format!("{call} {values:?}: {shown}")
}

/// The CALLs of every procedure, each many times over, so its plan is
/// learned and then kept: ranges of many rows, of one and of none, a null
/// and a value of another type for a bound, a row too many, and a
/// division by zero in a set function's argument.
fn script() -> Vec<(&'static str, Vec<Value>)> {
    let mut calls = Vec::new();
    let text = |s: &str| Value::Char(s.to_owned());
    for round in 0..9_i64 {
        let (w, d) = (round % 2 + 1, round % 10 + 1);
        calls.push(("CALL CNT(?,?,?)", vec![int(w), int(d)]));
        calls.push(("CALL AGG(?,?,?,?,?,?,?,?,?)", vec![int(w), int(d), int(round * 3), int(round * 9 + 4)]));
        calls.push(("CALL LAST_ONE(?,?,?,?,?)", vec![int(w), int(d), int(round % 6)]));
        calls.push(("CALL LEAST_K(?,?,?,?)", vec![int(w), int(d), int(round * 15)]));
        calls.push(("CALL NAMED(?,?,?,?,?)", vec![int(w), int(d), text(&format!("S{}", round * 2))]));
        calls.push(("CALL ONE_ROW(?,?,?,?,?)", vec![int(w), int(d), int(112 + round)]));
        calls.push(("CALL SHARED(?,?,?,?,?)", vec![int(w), int(d), int(9 - round)]));
        calls.push(("CALL SORTED(?,?,?,?,?)", vec![int(w), int(d), text(&format!("S{round}"))]));
        calls.push(("CALL PAIRED(?,?,?,?,?)", vec![int(w), int(d), int(round * 20)]));
        calls.push(("CALL STOCKED(?,?,?,?,?)", vec![int(w), int(d), int(round * 14 + 8), int(round % 5)]));
    }
    calls.push(("CALL PAIRED(?,?,?,?,?)", vec![int(1), Value::Null, int(20)]));
    calls.push(("CALL STOCKED(?,?,?,?,?)", vec![int(1), int(1), Value::Null, int(3)]));
    calls.push(("CALL CNT(?,?,?)", vec![int(3), int(1)]));
    calls.push(("CALL CNT(?,?,?)", vec![Value::Null, int(1)]));
    calls.push(("CALL CNT(?,?,?)", vec![text("x"), int(1)]));
    calls.push(("CALL AGG(?,?,?,?,?,?,?,?,?)", vec![int(1), int(1), int(50), int(50)]));
    calls.push(("CALL AGG(?,?,?,?,?,?,?,?,?)", vec![int(1), int(1), Value::Null, int(50)]));
    calls.push(("CALL LEAST_K(?,?,?,?)", vec![int(1), int(1), int(500)]));
    calls.push(("CALL ONE_ROW(?,?,?,?,?)", vec![int(1), int(1), int(121)]));
    calls.push(("CALL ONE_ROW(?,?,?,?,?)", vec![int(1), int(1), int(120)]));
    calls
}

/// What the script gave with point operations `how`, compiled or not.
fn ran(how: &str, compiled: bool) -> Vec<String> {
    let db = defined();
    let a = db.session("A");
    a.run(&format!("SET EXECUTOR.POINT_OPERATIONS = '{how}'")).unwrap();
    a.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    script().iter().map(|(call, values)| called(&a, call, values)).collect()
}

/// Verifies: [DESIGN] RULES.md D930: a kept body's query of one range of
/// an index gives the same outputs, completions and errors read direct as
/// through the executor, compiled and interpreted: set functions of a
/// range of many rows, of one and of none, DISTINCT among them, a join by
/// a whole unique key of one row and of a range's (D932), the first
/// row of an order the index gives and the third of one it does not, a
/// filter beside the range, a null or a value of another type for a
/// bound, more than one row (SQLSTATE 21000), and a division by zero in a
/// set function's argument (SQLSTATE 22012).
#[test]
fn ranges_are_read_direct_as_the_executor_reads_them() {
    let through = ran("OFF", true);
    for (how, compiled) in [("ON", true), ("ON", false), ("OFF", false)] {
        let direct = ran(how, compiled);
        for (n, (d, t)) in direct.iter().zip(&through).enumerate() {
            assert_eq!(d, t, "line {n}: point operations {how}, compiled {compiled}; then through the executor");
        }
        assert_eq!(direct.len(), through.len());
    }
    let has = |part: &str| through.iter().any(|line| line.contains(part));
    assert!(has("CALL CNT(?,?,?) [Exact(Decimal { units: 1, scale: 0 }), Exact(Decimal { units: 1, scale: 0 })]: ok Success [\"103\"]"));
    assert!(has("error 21000") && has("error 22012"), "{through:#?}");
}

/// Verifies: [DESIGN] RULES.md D930, D29: a range read direct is recorded
/// as read where reads are checked. A SERIALIZABLE transaction that read
/// a range by a kept body, and wrote, is refused at its commit (SQLSTATE
/// 40001) once another committed a row in the range, by the range's
/// condition it recorded; at READ COMMITTED it commits, and its next read
/// sees the row.
#[test]
fn a_range_read_direct_is_recorded_where_reads_are_checked() {
    let outcome = |level: &str, how: &str| {
        let db = defined();
        let a = db.session("A");
        a.run(&format!("SET EXECUTOR.POINT_OPERATIONS = '{how}'")).unwrap();
        let counted = |s: &Session| called(s, "CALL CNT(?,?,?)", &[int(1), int(3)]);
        a.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
        // Its plan learned and kept by reads of another range, in this
        // transaction, so the one read of this range is the direct one.
        for _ in 0..8 {
            assert!(called(&a, "CALL CNT(?,?,?)", &[int(2), int(5)]).ends_with("[\"103\"]"));
        }
        assert!(counted(&a).ends_with("[\"103\"]"));
        a.run("INSERT INTO L VALUES (1, 0)").unwrap();
        let other = db.session("A");
        other.run("INSERT INTO T VALUES (1, 3, 500, 1, 1.00, 'S1')").unwrap();
        other.commit().unwrap();
        // Read again, a serializable transaction would come on the row's
        // entry and refuse for that, so it is not, and its first read's
        // condition alone refuses it.
        let seen = level != "SERIALIZABLE" && counted(&a).ends_with("[\"104\"]");
        (a.commit().map_err(|e| e.sqlstate().as_str().to_owned()), seen)
    };
    for how in ["ON", "OFF"] {
        assert_eq!(outcome("SERIALIZABLE", how), (Err("40001".to_owned()), false), "{how}");
        assert_eq!(outcome("READ COMMITTED", how), (Ok(()), true), "{how}");
    }
}

/// Verifies: [DESIGN] RULES.md D930, D443: a range read direct keeps a row
/// only if the version its transaction sees has its entry's key. A row
/// whose key was changed keeps the entry of its old key while a snapshot
/// before the change is open, and a read of the range counts the row
/// once, where it is now; the snapshot before still counts it where it
/// was.
#[test]
fn a_range_read_direct_passes_over_an_entry_of_a_key_its_row_left() {
    let db = defined();
    let a = db.session("A");
    let counted = |s: &Session, d: i64| called(s, "CALL CNT(?,?,?)", &[int(1), int(d)]);
    for _ in 0..8 {
        assert!(counted(&a, 3).ends_with("[\"103\"]"));
    }
    a.commit().unwrap();
    // A snapshot from before the change, so the old version is kept.
    let earlier = db.session("A");
    earlier.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    assert!(counted(&earlier, 3).ends_with("[\"103\"]"));
    a.run("UPDATE T SET D = 4, K = 500 WHERE W = 1 AND D = 3 AND K = 8").unwrap();
    a.commit().unwrap();
    for how in ["ON", "OFF"] {
        a.run(&format!("SET EXECUTOR.POINT_OPERATIONS = '{how}'")).unwrap();
        // A setting changed has the body bound again: its plan learned
        // and kept again by reads of another range.
        for _ in 0..8 {
            assert!(called(&a, "CALL CNT(?,?,?)", &[int(2), int(5)]).ends_with("[\"103\"]"), "{how}");
        }
        assert!(counted(&a, 3).ends_with("[\"102\"]"), "{how}: the entry of the key it left finds no row");
        assert!(counted(&a, 4).ends_with("[\"104\"]"), "{how}");
    }
    assert!(counted(&earlier, 3).ends_with("[\"103\"]"), "the snapshot before still reads it");
    earlier.rollback().unwrap();
    a.rollback().unwrap();
}

/// What `query`, prepared in `s`, gives for each of `args`, each read to
/// its end: its rows, each its values as text joined by commas, or its
/// error at open or at fetch.
fn prepared(s: &Session, query: &str, args: &[Vec<Value>]) -> Vec<String> {
    let p = s.prepare(query).unwrap();
    let mut out = Vec::new();
    for arg in args {
        match s.open_prepared(&p, arg) {
            Ok(mut c) => loop {
                match c.fetch() {
                    Ok(Some(row)) => out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(",")),
                    Ok(None) => break out.push("end".to_owned()),
                    Err(e) => break out.push(format!("error at fetch {}", e.sqlstate().as_str())),
                }
            },
            Err(e) => out.push(format!("error at open {}", e.sqlstate().as_str())),
        }
    }
    out
}

/// The procedures that read a range by a cursor: WALK fetches the first
/// rows of a sorted range by OPEN, FETCH and CLOSE; TOTAL adds a range up
/// by FOR; and BUMP changes, in a FOR over a range, the row after each it
/// reads, which its cursor is not to see.
const CURSORS: [&str; 3] = [
    "CREATE PROCEDURE WALK (IN PW INTEGER, IN PD INTEGER, IN PS VARCHAR(12), IN STEPS INTEGER, OUT GOT INTEGER, OUT PART DECIMAL(8,2))
       READS SQL DATA
     BEGIN
       DECLARE I INTEGER DEFAULT 0;
       DECLARE C CURSOR FOR SELECT K, V FROM T WHERE W = PW AND D = PD AND S = PS ORDER BY V DESC;
       OPEN C;
       WHILE I < STEPS DO
         FETCH C INTO GOT, PART;
         SET I = I + 1;
       END WHILE;
       CLOSE C;
     END",
    "CREATE PROCEDURE TOTAL (IN PW INTEGER, IN PD INTEGER, IN LO INTEGER, OUT LINES INTEGER, OUT AMOUNT DECIMAL(12,2)) READS SQL DATA
     BEGIN
       SET LINES = 0;
       SET AMOUNT = 0;
       FOR R AS SELECT K, V FROM T WHERE W = PW AND D = PD AND K >= LO DO
         SET LINES = LINES + 1;
         SET AMOUNT = AMOUNT + R.V;
       END FOR;
     END",
    "CREATE PROCEDURE BUMP (IN PW INTEGER, IN PD INTEGER, OUT SEEN INTEGER) MODIFIES SQL DATA
     BEGIN
       SET SEEN = 0;
       FOR R AS SELECT K, N FROM T WHERE W = PW AND D = PD AND K < 7 DO
         UPDATE T SET N = COALESCE(N, 0) + 10 WHERE W = PW AND D = PD AND K = R.K + 1;
         SET SEEN = SEEN + COALESCE(R.N, 0);
       END FOR;
     END",
];

/// What prepared cursors and the cursors of `CURSORS` gave with point
/// operations `how`, compiled or not.
fn cursors(how: &str, compiled: bool) -> Vec<String> {
    let db = defined();
    let a = db.session("A");
    for p in CURSORS {
        a.run(p).unwrap();
    }
    a.commit().unwrap();
    a.run(&format!("SET EXECUTOR.POINT_OPERATIONS = '{how}'")).unwrap();
    a.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    let pairs = |more: &[Vec<Value>]| (1..=9_i64).map(|d| vec![int(d % 2 + 1), int(d)]).chain(more.iter().cloned()).collect::<Vec<_>>();
    let odd = [vec![int(3), int(1)], vec![Value::Null, int(1)], vec![Value::Char("x".to_owned()), int(1)]];
    let mut out = prepared(&a, "SELECT K, V FROM T WHERE W = ? AND D = ? AND K < 4", &pairs(&odd));
    out.extend(prepared(&a, "SELECT K, N FROM T WHERE W = ? AND D = ? AND K > 110 ORDER BY N, K DESC", &pairs(&odd)));
    out.extend(prepared(&a, "SELECT 60 / (K - 3) FROM T WHERE W = ? AND D = ? AND K < 6", &pairs(&odd)));
    out.extend(prepared(&a, "SELECT COUNT(*), MAX(K) FROM T WHERE W = ? AND D < ?", &pairs(&odd)));
    // A range of more rows than a cursor reads ahead as it opens.
    let long = prepared(&a, "SELECT K FROM T WHERE W = ? AND D < ?", &pairs(&[]));
    out.push(format!("{} rows of long ranges, the last {:?}", long.len(), long.iter().rev().nth(1)));
    for round in 0..9_i64 {
        let (w, d) = (round % 2 + 1, round % 10 + 1);
        out.push(called(&a, "CALL WALK(?,?,?,?,?,?)", &[int(w), int(d), Value::Char(format!("S{round}")), int(round % 4)]));
        out.push(called(&a, "CALL TOTAL(?,?,?,?,?)", &[int(w), int(d), int(120 - round * 2)]));
        out.push(called(&a, "CALL BUMP(?,?,?)", &[int(w), int(round % 3 + 1)]));
    }
    out.push(called(&a, "CALL WALK(?,?,?,?,?,?)", &[int(1), int(1), Value::Char("S1".to_owned()), int(40)]));
    out.push(called(&a, "CALL TOTAL(?,?,?,?,?)", &[int(1), int(1), int(500)]));
    out.extend(prepared(&a, "SELECT D, K, N FROM T WHERE W = ? AND D = ? AND K < 9", &[vec![int(1), int(1)], vec![int(2), int(3)]]));
    out
}

/// Verifies: [DESIGN] RULES.md D931: a cursor of a kept plan that is a
/// direct range read gives the rows, in the order, and the errors, at
/// OPEN or at FETCH, that it gives through the executor, prepared and of
/// a kept body, compiled and interpreted: a short range, a sorted one,
/// one that raises (SQLSTATE 22012, at OPEN), one of more rows than are
/// read ahead, a null and a value of another type for a bound, FETCH past
/// the last row, and a FOR whose body changes rows its cursor has yet to
/// give, which it gives as they were.
#[test]
fn cursors_of_ranges_read_direct_give_what_the_executor_gives() {
    let through = cursors("OFF", true);
    for (how, compiled) in [("ON", true), ("ON", false), ("OFF", false)] {
        let direct = cursors(how, compiled);
        for (n, (d, t)) in direct.iter().zip(&through).enumerate() {
            assert_eq!(d, t, "line {n}: point operations {how}, compiled {compiled}; then through the executor");
        }
        assert_eq!(direct.len(), through.len());
    }
    let has = |part: &str| through.iter().any(|line| line.contains(part));
    assert!(has("error at open 22012") && has("error at") && has("4329 rows of long ranges"), "{through:#?}");
}

/// The procedures whose SELECT ... INTO folds set functions over a delta
/// table: of the rows an UPDATE of a range wrote, of the one row an
/// UPDATE by the whole key wrote, and of the rows a DELETE took.
const FOLDED: [&str; 3] = [
    "CREATE PROCEDURE DELIVER (IN PW INTEGER, IN PD INTEGER, IN HI INTEGER,
       OUT TOTAL DECIMAL(12,2), OUT LINES INTEGER, OUT PART DECIMAL(14,4))
       MODIFIES SQL DATA
     BEGIN SELECT SUM(V), COUNT(*), SUM(V / N) INTO TOTAL, LINES, PART FROM FINAL TABLE
       (UPDATE T SET N = N - 2, S = 'done' WHERE W = PW AND D = PD AND K < HI); END",
    "CREATE PROCEDURE DELIVER_ONE (IN PW INTEGER, IN PD INTEGER, IN PK INTEGER, OUT MOST DECIMAL(8,2), OUT LINES INTEGER)
       MODIFIES SQL DATA
     BEGIN SELECT MAX(V) + 1, COUNT(N) INTO MOST, LINES FROM FINAL TABLE
       (UPDATE T SET V = V + 1 WHERE W = PW AND D = PD AND K = PK); END",
    "CREATE PROCEDURE TAKEN (IN PW INTEGER, IN PD INTEGER, IN LO INTEGER, OUT LEAST INTEGER, OUT KINDS INTEGER) MODIFIES SQL DATA
     BEGIN SELECT MIN(K), COUNT(DISTINCT N) INTO LEAST, KINDS FROM OLD TABLE
       (DELETE FROM T WHERE W = PW AND D = PD AND K > LO); END",
];

/// What the procedures of `FOLDED` gave, compiled or not, and the rows
/// of T they left.
fn folded(compiled: bool) -> Vec<String> {
    let db = defined();
    let a = db.session("A");
    for p in FOLDED {
        a.run(p).unwrap();
    }
    a.commit().unwrap();
    a.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    let mut out = Vec::new();
    for round in 0..9_i64 {
        let (w, d) = (round % 2 + 1, round % 10 + 1);
        // Past the first row N is 0 for the second, a division by zero.
        out.push(called(&a, "CALL DELIVER(?,?,?,?,?,?)", &[int(w), int(d), int(round % 3 * 2 + 2)]));
        out.push(called(&a, "CALL DELIVER_ONE(?,?,?,?,?)", &[int(w), int(d), int(round * 20)]));
        out.push(called(&a, "CALL TAKEN(?,?,?,?,?)", &[int(w), int(d), int(116 + round)]));
    }
    let mut c = a.open_cursor("SELECT W, D, K, N, V, S FROM T WHERE K < 8 OR K > 110 ORDER BY W, D, K").unwrap();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [DESIGN] RULES.md D933: a compiled SELECT ... INTO of set
/// functions over the delta table of a change it runs straight gives the
/// values, the errors and the rows written that the interpreter gives:
/// over the rows of a range, over the one row of a whole key, over no
/// row, DISTINCT among them, a select list computed of their values, and
/// a division by zero in an argument (SQLSTATE 22012), which undoes the
/// change.
#[test]
fn set_functions_over_a_delta_table_are_folded_as_the_general_path_makes_them() {
    let (interpreted, compiled) = (folded(false), folded(true));
    for (n, (c, i)) in compiled.iter().zip(&interpreted).enumerate() {
        assert_eq!(c, i, "line {n}: compiled, then interpreted");
    }
    assert_eq!(compiled.len(), interpreted.len());
    let has = |part: &str| compiled.iter().any(|line| line.contains(part));
    assert!(has("error 22012") && has("CALL DELIVER(?,?,?,?,?,?)") && has("ok Success"), "{compiled:#?}");
}
