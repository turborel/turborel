//! Joins (RULES.md D35): the rows of a product that its conditions keep,
//! however the planner computes them. These are `[DESIGN]` requirements on
//! the plan; the standard governs only the rows [X3.135-1986 §5.20 GR2b,
//! §5.21 GR2]. Each query is checked against the same query written so that
//! the plan is a nested loop: `+ 0` makes an equality one that can raise an
//! error, which the planner leaves to be evaluated on every pair.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{both_ways, code, exec, rows, sorted};

/// Schema J: numbers exact (A), approximate (B) and exact again (E), and
/// strings of two lengths (C, D), with nulls and repeated keys.
fn joins() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION J
           CREATE TABLE A (K DECIMAL(5,2), N INTEGER)
           CREATE TABLE B (K FLOAT, N INTEGER)
           CREATE TABLE E (K INTEGER, N INTEGER)
           CREATE TABLE C (S CHAR(4), N INTEGER)
           CREATE TABLE D (S CHAR(2), N INTEGER)",
    )
    .unwrap();
    let s = db.session("J");
    for sql in [
        "INSERT INTO A VALUES (1.50, 1)",
        "INSERT INTO A VALUES (2, 2)",
        "INSERT INTO A VALUES (NULL, 3)",
        "INSERT INTO A VALUES (3, 4)",
        "INSERT INTO A VALUES (2.00, 5)",
        "INSERT INTO B VALUES (1.5E0, 10)",
        "INSERT INTO B VALUES (2E0, 20)",
        "INSERT INTO B VALUES (NULL, 30)",
        "INSERT INTO B VALUES (4E0, 40)",
        "INSERT INTO B VALUES (2E0, 50)",
        "INSERT INTO E VALUES (2, 7)",
        "INSERT INTO E VALUES (3, 8)",
        "INSERT INTO C VALUES ('AB', 1)",
        "INSERT INTO C VALUES ('AB  ', 2)",
        "INSERT INTO C VALUES ('X', 3)",
        "INSERT INTO D VALUES ('AB', 4)",
        "INSERT INTO D VALUES ('X ', 5)",
        "INSERT INTO D VALUES (NULL, 6)",
    ] {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    db
}

/// The rows of `sql`, sorted, checked against those of `nested`, the same
/// query planned as a nested loop; and the plan of `sql`. No ORDER BY is
/// written, so the rows may come in any order.
fn same_rows(s: &Session, sql: &str, nested: &str) -> (Vec<String>, String) {
    let (found, reference) = (sorted(s, sql), sorted(s, nested));
    assert_eq!(found, reference, "{sql}");
    let plan = s.physical_plan(sql, &Params::new()).unwrap();
    assert!(!s.physical_plan(nested, &Params::new()).unwrap().contains("HashJoin"), "{nested} is a nested loop");
    (found, plan)
}

/// Verifies: [DESIGN] RULES.md D35: an equality of a column of each operand
/// of a product is a hash join, which keeps the pairs whose values are equal
/// [X3.135-1986 §5.11]: an exact and an approximate number by algebraic
/// value (GR4), strings with padding (GR5), never a null (GR2), each
/// repeated key with each.
#[test]
fn equalities_are_hash_joins() {
    let db = joins();
    let s = db.session("J");
    let cases = [
        ("SELECT A.N, B.N FROM A, B WHERE A.K = B.K", "SELECT A.N, B.N FROM A, B WHERE A.K + 0 = B.K", 5),
        ("SELECT A.N, B.N FROM A, B WHERE B.K = A.K", "SELECT A.N, B.N FROM A, B WHERE B.K + 0 = A.K", 5),
        ("SELECT A.N, E.N FROM A, E WHERE A.K = E.K", "SELECT A.N, E.N FROM A, E WHERE A.K + 0 = E.K", 3),
        ("SELECT C.N, D.N FROM C, D WHERE C.S = D.S", "SELECT C.N, D.N FROM C, D WHERE C.S = D.S AND C.N + D.N > 0", 3),
        (
            "SELECT A.N, B.N FROM A, B WHERE A.K = B.K AND (A.N = 1 OR B.N = 50)",
            "SELECT A.N, B.N FROM A, B WHERE A.K + 0 = B.K AND (A.N = 1 OR B.N = 50)",
            3,
        ),
        (
            "SELECT A.N, B.N, E.N FROM A, B, E WHERE A.K = B.K AND B.K = E.K",
            "SELECT A.N, B.N, E.N FROM A, B, E WHERE A.K + 0 = B.K AND B.K = E.K",
            4,
        ),
    ];
    for (sql, nested, n) in cases {
        let (found, plan) = same_rows(&s, sql, nested);
        assert_eq!(found.len(), n, "{sql}: {found:?}");
        assert!(plan.contains("HashJoin"), "{sql}: {plan}");
    }
}

/// Verifies: [DESIGN] RULES.md D15: a condition that can raise an error,
/// of the columns of both operands, is evaluated on every pair of the
/// product, so it is not a hash join, which would evaluate it only on the
/// pairs whose keys are equal.
#[test]
fn a_condition_that_can_raise_is_not_a_hash_join() {
    let db = joins();
    let s = db.session("J");
    let plan = s.physical_plan("SELECT A.N FROM A, B WHERE A.K = B.K AND A.N / B.N > 0", &Params::new()).unwrap();
    assert!(plan.contains("NestedLoop") && !plan.contains("HashJoin"), "{plan}");
}

/// Verifies: [DESIGN] RULES.md D35: the operands of a product are joined in
/// the order estimated cheapest, not the FROM clause's: two large tables
/// each joined to a small one are joined through it, not paired first; the
/// rows, and their columns' order, are the same. A table whose order an
/// ORDER BY asks for stays first, where its order can be kept.
#[test]
fn products_are_joined_in_the_cheapest_order() {
    let db = large_and_small();
    let s = db.session("J");
    let sql = "SELECT T.V, U.W, S.K FROM T, U, S WHERE T.K = S.K AND U.K = S.K";
    let nested = "SELECT T.V, U.W, S.K FROM T, U, S WHERE T.K + 0 = S.K AND U.K = S.K";
    let (found, plan) = same_rows(&s, sql, nested);
    assert_eq!(found, vec!["10,15,5", "14,21,7"]);
    assert!(!plan.contains("NestedLoop"), "T and U are not paired: {plan}");
    let ordered = "SELECT T.K, U.W FROM T, U, S WHERE T.K = S.K AND U.K = S.K ORDER BY 2 DESC";
    let plan = s.physical_plan(ordered, &Params::new()).unwrap();
    // The first join, the innermost, reads U first.
    let lines: Vec<&str> = plan.lines().collect();
    let innermost = lines.iter().rposition(|l| l.contains("HashJoin")).unwrap();
    assert!(lines[innermost + 1].contains("Scan J.U"), "U is read first: {plan}");
    assert_eq!(rows(&s, ordered), Ok(vec!["7,21".into(), "5,15".into()]));
}

/// Verifies: [DESIGN] RULES.md D35, `PREDICATE_TRANSITIVITY`: a comparison
/// of a column equated with another is made of the other too [X3.135-1986
/// §5.11 GR4], so U's index answers `T.K = 7` through `T.K = U.K`, and a
/// range on T restricts U; the rows are those of the nested loop.
#[test]
fn comparisons_carry_across_equalities() {
    let db = large_and_small();
    db.create_index("U_K", "J.U", &["K"], "btree").unwrap();
    let s = db.session("J");
    let (found, plan) =
        same_rows(&s, "SELECT T.V, U.W FROM T, U WHERE T.K = U.K AND T.K = 7", "SELECT T.V, U.W FROM T, U WHERE T.K + 0 = U.K AND T.K = 7");
    assert_eq!(found, vec!["14,21"]);
    assert!(plan.contains("IndexScan J.U using U_K"), "{plan}");
    let (found, plan) =
        same_rows(&s, "SELECT U.W FROM T, U WHERE U.K = T.K AND 298 <= T.K", "SELECT U.W FROM T, U WHERE U.K + 0 = T.K AND 298 <= T.K");
    assert_eq!(found, vec!["894", "897"]);
    assert!(plan.contains("U.K >= 298"), "{plan}");
}

/// Schema J: tables T and U of 300 rows, keys 0 to 299, and S of three
/// keys, two of them in T and U.
fn large_and_small() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION J
           CREATE TABLE T (K INTEGER NOT NULL, V INTEGER)
           CREATE TABLE U (K INTEGER NOT NULL, W INTEGER)
           CREATE TABLE S (K INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("J");
    for k in 0..300 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {})", k * 2)).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({k}, {})", k * 3)).unwrap();
    }
    for k in [5, 7, 400] {
        s.run(&format!("INSERT INTO S VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    db
}

/// Schema J: BIG of 40 rows, keys 0 to 7 repeated and 4 nulls, V its row's
/// number; SMALL of 5 rows, key 1 twice, 3, 9 and a null.
fn built_operands() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION J
           CREATE TABLE BIG (K INTEGER, V INTEGER)
           CREATE TABLE SMALL (K INTEGER, W INTEGER)",
    )
    .unwrap();
    let s = db.session("J");
    for k in 0..36 {
        s.run(&format!("INSERT INTO BIG VALUES ({}, {k})", k % 8)).unwrap();
    }
    for k in 36..40 {
        s.run(&format!("INSERT INTO BIG VALUES (NULL, {k})")).unwrap();
    }
    s.run("INSERT INTO SMALL VALUES (1, 10), (1, 11), (3, 30), (9, 90), (NULL, 0)").unwrap();
    s.commit().unwrap();
    db
}

/// The line of `plan` after its first that contains `operator`: the first
/// operand of that operator.
fn first_operand<'p>(plan: &'p str, operator: &str) -> &'p str {
    plan.lines().skip_while(|l| !l.contains(operator)).nth(1).unwrap_or_default()
}

/// Verifies: [DESIGN] RULES.md D35: a hash join builds the operand of
/// fewer estimated rows, whichever side of the product it is on, and
/// probes it with the other; the rows, with their nulls and repeated keys,
/// and their columns' order are the product's that the condition keeps
/// [X3.135-1986 §5.20 GR2b, §5.11 GR2], by batches and row by row alike.
#[test]
fn the_operand_of_fewer_rows_is_built() {
    let db = built_operands();
    let s = db.session("J");
    let cases = [
        ("SELECT * FROM SMALL, BIG WHERE SMALL.K = BIG.K", "SELECT * FROM SMALL, BIG WHERE SMALL.K + 0 = BIG.K", 15),
        ("SELECT * FROM BIG, SMALL WHERE BIG.K = SMALL.K", "SELECT * FROM BIG, SMALL WHERE BIG.K + 0 = SMALL.K", 15),
        (
            "SELECT * FROM SMALL, BIG WHERE SMALL.K = BIG.K AND SMALL.W < BIG.V",
            "SELECT * FROM SMALL, BIG WHERE SMALL.K + 0 = BIG.K AND SMALL.W < BIG.V",
            7,
        ),
    ];
    for (sql, nested, n) in cases {
        let (found, plan) = same_rows(&s, sql, nested);
        assert_eq!(found.len(), n, "{sql}: {found:?}");
        assert!(first_operand(&plan, "HashJoin").contains("SeqScan J.BIG"), "BIG probes SMALL built: {plan}");
        let (batched, rowwise) = both_ways(&s, sql);
        assert_eq!((batched, rowwise), (found.clone(), found), "{sql}");
    }
    assert_eq!(rows(&s, "SELECT * FROM SMALL, BIG WHERE SMALL.K = BIG.K AND BIG.V = 9"), Ok(vec!["1,10,1,9".into(), "1,11,1,9".into()]));
}

/// Verifies: [DESIGN] RULES.md D35: a hash join whose left operand gives
/// the order an ORDER BY asks for keeps building its right, of more rows,
/// so the left's order is kept and no sort is needed.
#[test]
fn an_ordered_left_operand_is_not_built() {
    let db = large_and_small();
    db.create_index("T_K", "J.T", &["K"], "btree").unwrap();
    let s = db.session("J");
    s.run("CREATE TABLE W (K INTEGER NOT NULL, V INTEGER)").unwrap();
    for _ in 0..3 {
        s.run("INSERT INTO W SELECT K, V FROM T").unwrap();
    }
    s.commit().unwrap();
    let sql = "SELECT T.K, W.V FROM T, W WHERE T.K = W.K AND T.K > 290 ORDER BY T.K";
    let plan = s.physical_plan(sql, &Params::new()).unwrap();
    assert!(!plan.contains("Sort") && first_operand(&plan, "HashJoin").contains("J.T"), "{plan}");
    let found = rows(&s, sql).unwrap();
    assert_eq!((found.len(), found.first().map(String::as_str), found.last().map(String::as_str)), (27, Some("291,582"), Some("299,598")));
}

/// The codes `sql` fails with by each plan given here: with the raising
/// conjuncts moved onto their operands or not
/// (`PUSH_RAISING_FILTER_INTO_PRODUCT`), by batches and row by row.
fn codes(s: &Session, sql: &str) -> Vec<Code> {
    let mut found = Vec::new();
    for rule in ["ON", "OFF"] {
        exec(s, &format!("SET OPTIMIZER.PUSH_RAISING_FILTER_INTO_PRODUCT = '{rule}'")).unwrap();
        for batches in ["ON", "OFF"] {
            exec(s, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
            found.push(code(s, sql));
        }
    }
    exec(s, "SET OPTIMIZER.PUSH_RAISING_FILTER_INTO_PRODUCT = 'ON'").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    found
}

/// Verifies: [DESIGN] RULES.md D15, D35: a hash join builds the operand of
/// fewer rows though both operands can raise an error. Whatever the plan
/// (hash join or nested loop, by batches or row by row, the raising
/// conjuncts moved onto their operands or not) the statement raises an
/// error, one its conjuncts can raise: BIG's division by zero or SMALL's
/// subquery of more than one row; with one of them alone, that one.
/// Regression test (R-TEST-7): the hash join kept building BIG, of more
/// rows, to raise its error first.
#[test]
fn operands_that_can_raise_raise_one_of_their_errors() {
    let db = built_operands();
    let s = db.session("J");
    let both = "SELECT * FROM SMALL, BIG WHERE SMALL.K = BIG.K AND SMALL.W = (SELECT K FROM BIG) AND BIG.V / BIG.K > 0";
    let plan = s.physical_plan(both, &Params::new()).unwrap();
    assert!(first_operand(&plan, "HashJoin").contains("BIG"), "BIG probes SMALL built: {plan}");
    let nested = "SELECT * FROM SMALL, BIG WHERE SMALL.K + 0 = BIG.K AND SMALL.W = (SELECT K FROM BIG) AND BIG.V / BIG.K > 0";
    let product = "SELECT * FROM SMALL, BIG WHERE SMALL.W = (SELECT K FROM BIG) AND BIG.V / BIG.K > 0";
    for sql in [both, nested, product] {
        for c in codes(&s, sql) {
            assert!(matches!(c, Code::Data | Code::Cardinality), "{sql}: {c:?}");
        }
    }
    let alone = [
        ("SELECT * FROM SMALL, BIG WHERE SMALL.K = BIG.K AND BIG.V / BIG.K > 0", Code::Data),
        ("SELECT * FROM SMALL, BIG WHERE SMALL.K = BIG.K AND SMALL.W = (SELECT K FROM BIG)", Code::Cardinality),
    ];
    for (sql, expected) in alone {
        assert_eq!(codes(&s, sql), vec![expected; 4], "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D15, D35: operands of a chain of products
/// that can each raise an error raise one of their errors in any join
/// order, the FROM clause's or the cheapest (PLANNER.REORDER_GAIN), each
/// operand read whole: T's division by zero or U's subquery of more than
/// one row.
#[test]
fn any_join_order_raises_one_of_the_errors() {
    let db = large_and_small();
    let s = db.session("J");
    let sql = "SELECT T.V, U.W, S.K FROM T, U, S WHERE T.K = S.K AND U.K = S.K AND T.V / (T.K - 5) >= 0 AND U.W = (SELECT K FROM S)";
    let mut plans = Vec::new();
    for gain in [0.0_f64, 0.999_f64] {
        db.set_setting("PLANNER.REORDER_GAIN", &Value::Approx(gain)).unwrap();
        plans.push(s.physical_plan(sql, &Params::new()).unwrap());
        for c in codes(&s, sql) {
            assert!(matches!(c, Code::Data | Code::Cardinality), "{c:?}");
        }
    }
    assert_ne!(plans.first(), plans.last(), "the orders differ");
}

/// Verifies: [DESIGN] RULES.md D35: a RIGHT outer join is planned as the
/// LEFT one of its operands swapped, its left built, unless its left has
/// more rows, though an operand can raise an error or the join keeps a
/// conjunct that can (one over the preserved operand's columns alone, or
/// one over each operand's); the rows are the RIGHT join's [ISO/IEC
/// 9075:1992 §7.5 GR5], each row of the right no row of the left matches,
/// a null key's too, with nulls, by batches and row by row alike.
#[test]
fn a_right_join_is_the_left_join_swapped() {
    let db = built_operands();
    let s = db.session("J");
    let cases = [
        (
            "SELECT * FROM SMALL RIGHT JOIN BIG ON SMALL.K = BIG.K",
            "SELECT SMALL.*, BIG.* FROM BIG LEFT JOIN SMALL ON SMALL.K = BIG.K",
            45,
            true,
        ),
        (
            "SELECT * FROM SMALL RIGHT JOIN BIG ON SMALL.K = BIG.K AND SMALL.W < BIG.V",
            "SELECT SMALL.*, BIG.* FROM BIG LEFT JOIN SMALL ON SMALL.K = BIG.K AND SMALL.W < BIG.V",
            43,
            true,
        ),
        (
            "SELECT * FROM SMALL RIGHT JOIN BIG ON SMALL.K = BIG.K AND SMALL.W / 2 > 0",
            "SELECT SMALL.*, BIG.* FROM BIG LEFT JOIN SMALL ON SMALL.K = BIG.K AND SMALL.W / 2 > 0",
            45,
            true,
        ),
        (
            "SELECT * FROM SMALL RIGHT JOIN BIG ON SMALL.K = BIG.K AND BIG.V / 2 > 0",
            "SELECT SMALL.*, BIG.* FROM BIG LEFT JOIN SMALL ON SMALL.K = BIG.K AND BIG.V / 2 > 0",
            45,
            true,
        ),
        (
            "SELECT * FROM SMALL RIGHT JOIN BIG ON SMALL.K = BIG.K AND BIG.V / 2 > 0 AND SMALL.W / 2 > 5",
            "SELECT SMALL.*, BIG.* FROM BIG LEFT JOIN SMALL ON SMALL.K = BIG.K AND BIG.V / 2 > 0 AND SMALL.W / 2 > 5",
            40,
            true,
        ),
        (
            "SELECT * FROM BIG RIGHT JOIN SMALL ON SMALL.K = BIG.K",
            "SELECT BIG.*, SMALL.* FROM SMALL LEFT JOIN BIG ON SMALL.K = BIG.K",
            17,
            false,
        ),
        (
            "SELECT * FROM SMALL RIGHT JOIN (SELECT K, V / 2 FROM BIG) AS H (K, V) ON SMALL.K = H.K",
            "SELECT SMALL.*, H.* FROM (SELECT K, V / 2 FROM BIG) AS H (K, V) LEFT JOIN SMALL ON SMALL.K = H.K",
            45,
            true,
        ),
    ];
    for (sql, left, n, swapped) in cases {
        let found = sorted(&s, sql);
        assert_eq!((found.len(), &found), (n, &sorted(&s, left)), "{sql}");
        let plan = s.physical_plan(sql, &Params::new()).unwrap();
        assert_eq!((plan.contains("LeftOuterJoin"), plan.contains("RightOuterJoin")), (swapped, !swapped), "{sql}: {plan}");
        let (batched, rowwise) = both_ways(&s, sql);
        assert_eq!((batched, rowwise), (found.clone(), found), "{sql}");
    }
    assert!(sorted(&s, "SELECT * FROM SMALL RIGHT JOIN BIG ON SMALL.K = BIG.K").contains(&"NULL,NULL,NULL,36".to_owned()));
}

/// Verifies: [DESIGN] [ISO/IEC 9075:1992 §7.5 GR1c]; RULES.md D15, D35: a
/// RIGHT outer join keeping an ON conjunct that can raise an error, of
/// its preserved operand's columns alone, planned as the LEFT join of its
/// operands swapped, raises its error though the row raising it matches
/// nothing by its keys, by batches and row by row, and none when the
/// other operand is empty, as the RIGHT join does.
#[test]
fn a_swapped_right_join_raises_as_the_right_join() {
    let db = built_operands();
    let s = db.session("J");
    // BIG.V = 38 has a null key, so matches no row of SMALL.
    let sql = "SELECT BIG.V FROM SMALL RIGHT JOIN BIG ON SMALL.K = BIG.K AND BIG.V / (BIG.V - 38) > -1";
    let plan = s.physical_plan(sql, &Params::new()).unwrap();
    assert!(plan.contains("LeftOuterJoin") && !plan.contains("RightOuterJoin"), "{plan}");
    for batches in ["ON", "OFF"] {
        exec(&s, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(rows(&s, sql).err(), Some(Code::Data), "{batches}");
    }
    s.run("DELETE FROM SMALL").unwrap();
    for batches in ["ON", "OFF"] {
        exec(&s, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(rows(&s, sql).map(|r| r.len()), Ok(40), "{batches}");
    }
}
