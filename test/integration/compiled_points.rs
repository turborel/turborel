//! \[DESIGN\] A compiled program's values read where the frame holds them
//! and its point changes written with their probe placed once (RULES.md
//! D910 to D914): each body here runs compiled and interpreted, as
//! `compiled_procedures` runs them, and gives the same of each. An array
//! held in a parameter or variable is read an element at a time, by a
//! probe, a SET value and a condition; `SET A = A || ARRAY[x]` appends in
//! place while the array has room and is assigned as bound once it is
//! null or full, or its source is of another shape; a probe whose value
//! raises sends its change to the general path, which raises it.

use crate::compiled_procedures::{called, database, outcome, rows, same};

/// A procedure that grows arrays an element at a time, each shape the
/// in-place append takes and each it leaves to the assignment as bound:
/// a CHAR element padded, an element reading the array itself, an element
/// cast to the element type, the array second, another target, and an
/// element of a wider type.
const GROW: &str = "CREATE PROCEDURE GROW (IN N INTEGER, IN X VARCHAR(5), IN START_NULL INTEGER,
     OUT TEXTS CHAR(3) ARRAY[4], OUT SUMS INTEGER ARRAY[4], OUT PRICES DECIMAL(4,1) ARRAY[4],
     OUT FRONT CHAR(3) ARRAY[4], OUT OTHER CHAR(3) ARRAY[5], OUT WIDE CHAR(3) ARRAY[4])
   CONTAINS SQL
   BEGIN
     DECLARE E CHAR(3);
     DECLARE K INTEGER DEFAULT 0;
     DECLARE T CHAR(3) ARRAY[4] DEFAULT ARRAY[];
     DECLARE SU INTEGER ARRAY[4] DEFAULT ARRAY[];
     DECLARE P DECIMAL(4,1) ARRAY[4] DEFAULT ARRAY[];
     DECLARE F CHAR(3) ARRAY[4] DEFAULT ARRAY[];
     DECLARE WD CHAR(3) ARRAY[4] DEFAULT ARRAY[];
     SET E = X;
     IF START_NULL = 1 THEN SET T = NULL; END IF;
     WHILE K < N DO
       SET K = K + 1;
       SET T = T || ARRAY[E];
       SET SU = SU || ARRAY[CARDINALITY(SU) * 10 + K];
       SET P = P || ARRAY[K * 400];
       SET F = ARRAY[E] || F;
       SET OTHER = T || ARRAY[E];
       SET WD = WD || ARRAY[X];
       SET TEXTS = T; SET SUMS = SU; SET PRICES = P; SET FRONT = F; SET WIDE = WD;
     END WHILE;
   END";

/// Verifies: [DESIGN] RULES.md D914, D913. `SET A = A || ARRAY[x]`
/// appended in place gives the array the assignment as bound gives
/// [ISO/IEC 9075-2:2023 §6.47 GR2, §9.2 GR3b xxii]: each element padded
/// or cast to the element type, an element reading the array read before
/// it grows, a null array left null, a null element past the maximum
/// cardinality dropped, any other raising SQLSTATE 2202F, and an element
/// its type takes not raising as it does as bound.
#[test]
fn an_array_grows_in_place_as_it_does_as_bound() {
    let script = [
        "CALL GROW(0, 'ab', 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(1, 'ab', 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(2, 'ab', 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(2, 'abc', 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(2, 'ab', 1, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(4, CAST(NULL AS VARCHAR(5)), 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(5, CAST(NULL AS VARCHAR(5)), 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(3, 'x', 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(4, 'x', 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(5, 'x', 0, ?, ?, ?, ?, ?, ?)",
        "CALL GROW(1, 'abcd', 0, ?, ?, ?, ?, ?, ?)",
    ];
    let gave = same(&[GROW], &script);
    let of = |call: &str| gave.iter().find(|l| l.starts_with(call)).unwrap_or_else(|| panic!("{call}: {gave:?}")).clone();
    let two = of("CALL GROW(2, 'ab', 0");
    assert!(two.contains("ARRAY['ab ', 'ab ']"), "padded: {two}");
    assert!(two.contains("ARRAY[1, 12]"), "an element reading its array: {two}");
    assert!(two.contains("ARRAY[400.0, 800.0]"), "cast: {two}");
    assert!(of("CALL GROW(2, 'ab', 1").contains("ok"), "a null array: {gave:?}");
    assert!(of("CALL GROW(3, 'x', 0").contains("error 22003"), "an element out of range: {gave:?}");
    assert!(of("CALL GROW(5, CAST").contains("error 22003"), "{gave:?}");
    assert!(of("CALL GROW(1, 'abcd'").contains("error 22001"), "a string too long: {gave:?}");
}

/// A procedure that fills arrays past their maximum cardinality with
/// nulls and with values.
const FILL: &str = "CREATE PROCEDURE FILL (IN N INTEGER, IN X INTEGER, OUT GOT INTEGER ARRAY[3])
   CONTAINS SQL
   BEGIN
     DECLARE K INTEGER DEFAULT 0;
     DECLARE G INTEGER ARRAY[3] DEFAULT ARRAY[];
     WHILE K < N DO
       SET K = K + 1;
       SET G = G || ARRAY[X];
       SET GOT = G;
     END WHILE;
   END";

/// Verifies: [DESIGN] RULES.md D914. An array at its maximum cardinality
/// is assigned as bound: a null element appended is dropped [ISO/IEC
/// 9075-2:2023 §9.2 GR3b xxii 2A] and any other raises SQLSTATE 2202F
/// \[2B\], compiled as interpreted.
#[test]
fn a_full_array_is_assigned_as_bound() {
    let script = [
        "CALL FILL(3, 7, ?)",
        "CALL FILL(3, 7, ?)",
        "CALL FILL(4, 7, ?)",
        "CALL FILL(5, CAST(NULL AS INTEGER), ?)",
        "CALL FILL(2, CAST(NULL AS INTEGER), ?)",
    ];
    let gave = same(&[FILL], &script);
    assert!(gave.iter().any(|l| l.starts_with("CALL FILL(4, 7, ?): error 2202F")), "{gave:?}");
    assert!(
        gave.iter().any(|l| l.starts_with("CALL FILL(5, CAST(NULL AS INTEGER), ?): ok") && l.contains("ARRAY[NULL, NULL, NULL]")),
        "{gave:?}"
    );
}

/// A procedure of point changes probed and set by elements of its array
/// parameters, as NEW ORDER takes stock.
const TAKE: &str =
    "CREATE PROCEDURE TAKE (IN ITEMS INTEGER ARRAY[5], IN QS INTEGER ARRAY[5], IN N INTEGER, OUT LABELS VARCHAR(10) ARRAY[5])
   MODIFIES SQL DATA
   BEGIN
     DECLARE K INTEGER DEFAULT 1;
     DECLARE NM VARCHAR(10);
     DECLARE LS VARCHAR(10) ARRAY[5] DEFAULT ARRAY[];
     WHILE K <= N DO
       SET NM = NULL;
       SELECT NAME INTO NM FROM FINAL TABLE (UPDATE S SET Q = CASE WHEN Q < QS[K] + 10 THEN Q + 91 ELSE Q END - QS[K]
         WHERE W = 1 AND I = ITEMS[K]);
       SET LS = LS || ARRAY[NM];
       SET LABELS = LS;
       SET K = K + 1;
     END WHILE;
   END";

/// Verifies: [DESIGN] RULES.md D910, D911, D912. A point change whose
/// probe and SET values read elements of array parameters gives what
/// interpreted gives: the rows written, their FINAL TABLE, a key no row
/// has, a null key and a null quantity, a CHECK its row breaks, and a
/// position past an array's cardinality, whose error [ISO/IEC 9075-2:2023
/// §6.25 GR2b] the general path raises, each after calls that settle its
/// kept plans.
#[test]
fn point_changes_by_elements_of_arrays() {
    let script = [
        "CALL TAKE(ARRAY[1, 2], ARRAY[5, 6], 2, ?)",
        "CALL TAKE(ARRAY[3, 4, 5], ARRAY[1, 1, 1], 3, ?)",
        "CALL TAKE(ARRAY[1, 1, 1, 1, 1], ARRAY[9, 9, 9, 9, 9], 5, ?)",
        "CALL TAKE(ARRAY[2, 99, 3], ARRAY[1, 1, 1], 3, ?)",
        "CALL TAKE(ARRAY[2, NULL, 3], ARRAY[1, 1, 1], 3, ?)",
        "CALL TAKE(ARRAY[6, 7], ARRAY[1, NULL], 2, ?)",
        "CALL TAKE(ARRAY[8, 9], ARRAY[1], 2, ?)",
        "CALL TAKE(ARRAY[10], ARRAY[1, 1], 2, ?)",
        "CALL TAKE(ARRAY[11, 12], ARRAY[2, -500], 2, ?)",
        "CALL TAKE(ARRAY[13, 14], ARRAY[3, 3], 2, ?)",
        "CALL TAKE(ARRAY[1, 2, 3, 4, 5, 6], ARRAY[1, 1, 1, 1, 1, 1], 6, ?)",
        "? SELECT I, Q FROM S WHERE W = 1 ORDER BY I",
    ];
    let gave = same(&[TAKE], &script);
    assert!(
        gave.iter().any(|l| l.starts_with("CALL TAKE(ARRAY[1, 2], ARRAY[5, 6], 2, ?): ok") && l.contains("'item 1', 'item 2'")),
        "{gave:?}"
    );
    assert!(gave.iter().any(|l| l.starts_with("CALL TAKE(ARRAY[8, 9], ARRAY[1], 2, ?): error 2202E")), "a SET value past: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL TAKE(ARRAY[10], ARRAY[1, 1], 2, ?): error 2202E")), "a probe past: {gave:?}");
    assert!(gave.iter().any(|l| l.starts_with("CALL TAKE(ARRAY[2, 99, 3]") && l.contains("'item 2', NULL, 'item 3'")), "no row: {gave:?}");
}

/// What two sessions at `level` give of point changes probed by elements
/// of arrays, each reading what the other writes, compiled or not.
fn crossing(compiled: bool, level: &str) -> Vec<String> {
    let (db, a) = database(compiled);
    a.run(
        "CREATE PROCEDURE MOVE (IN PAIR INTEGER ARRAY[2], OUT GOT INTEGER) MODIFIES SQL DATA
         BEGIN
           SELECT Q INTO GOT FROM S WHERE W = 1 AND I = PAIR[1];
           SELECT Q INTO GOT FROM FINAL TABLE (UPDATE S SET Q = Q + GOT WHERE W = 1 AND I = PAIR[2]);
         END",
    )
    .unwrap();
    a.commit().unwrap();
    let b = db.session("A");
    b.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    let mut gave = Vec::new();
    // Each session's kept plans settle before the levels are set.
    for s in [&a, &b] {
        for pair in ["10, 11", "11, 12", "12, 13", "13, 14"] {
            gave.push(called(s, &format!("CALL MOVE(ARRAY[{pair}], ?)")));
        }
        gave.push(outcome(s.commit()));
        s.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
    }
    gave.push(called(&a, "CALL MOVE(ARRAY[1, 2], ?)"));
    gave.push(outcome(b.run("UPDATE S SET Q = 77 WHERE W = 1 AND I = 3")));
    gave.push(outcome(b.commit()));
    gave.push(outcome(b.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}"))));
    gave.push(called(&a, "CALL MOVE(ARRAY[3, 4], ?)"));
    gave.push(called(&b, "CALL MOVE(ARRAY[4, 3], ?)"));
    gave.push(outcome(a.commit()));
    gave.push(outcome(b.commit()));
    gave.extend(rows(&a, "SELECT I, Q FROM S WHERE W = 1 AND I <= 4 ORDER BY I"));
    gave
}

/// Verifies: [DESIGN] RULES.md D911, RULES.md D454. A point change whose
/// probe is placed once reads at READ COMMITTED what another committed
/// since its transaction began, and at SERIALIZABLE records its read, so
/// of two transactions each reading what the other writes one fails as
/// it commits, compiled as interpreted.
#[test]
fn point_changes_by_elements_keep_the_isolation_levels() {
    for level in ["READ COMMITTED", "SERIALIZABLE"] {
        let (interpreted, compiled) = (crossing(false, level), crossing(true, level));
        assert_eq!(compiled, interpreted, "{level}: compiled, then interpreted");
        let failed = compiled.iter().filter(|l| l.starts_with("error 40001")).count();
        assert_eq!(failed, usize::from(level == "SERIALIZABLE"), "{level}: {compiled:?}");
    }
}
