//! A polymorphic table function's table arguments of set semantics: each
//! partitioned, co-partitioned, pruned and ordered, and a virtual
//! processor run for each partition [ISO/IEC 9075-2:2023 §9.18 SR12b to
//! SR12d, §9.25 GR6 to GR16] (Features B201, B202, B203, B204, B207).

use turborel::db::{Database, Session};

/// Schema A: S (G, V), rows (1, 10), (1, 20), (2, 30); U (G, W), rows
/// (1, 100), (3, 300); E (G, V), no row.
fn defined() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE S (G INTEGER, V INTEGER) CREATE TABLE U (G INTEGER, W INTEGER) \
         CREATE TABLE E (G INTEGER, V INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO S VALUES (1, 10), (1, 20), (2, 30)").unwrap();
    a.run("INSERT INTO U VALUES (1, 100), (3, 300)").unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas,
/// a null value as `-`.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        let text: Vec<String> =
            row.iter().map(|v| if v.is_null() { "-".to_owned() } else { v.to_string().trim_end().to_owned() }).collect();
        out.push(text.join(","));
    }
    out
}

/// SUMMED (T TABLE WITH SET SEMANTICS), of columns TOTAL and FIRST_V: of
/// each partition, the sum of its rows' V and the V of its first row.
fn summed(a: &Session) {
    a.run(
        "CREATE PROCEDURE SUM_F (IN CR VARCHAR(128), IN PT VARCHAR(128), IN OD VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), \
         INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE G, V, FIRSTV INTEGER; DECLARE TOTAL INTEGER DEFAULT 0; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO G, V; IF DONE THEN LEAVE L; END IF; \
         IF FIRSTV IS NULL THEN SET FIRSTV = V; END IF; SET TOTAL = TOTAL + V; END LOOP L; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = TOTAL; SET DESCRIPTOR PTF RR VALUE 2 DATA = FIRSTV; PIPE ROW PTF RR; END",
    )
    .unwrap();
    a.run(
        "CREATE FUNCTION SUMMED (T TABLE WITH SET SEMANTICS) RETURNS TABLE (TOTAL INTEGER, FIRST_V INTEGER) CONTAINS SQL \
         FULFILL WITH PROCEDURE SUM_F",
    )
    .unwrap();
}

/// PAIRED (A, B, both TABLE WITH SET SEMANTICS, B's `pruning`), of
/// columns NA and NB: of each partition, how many rows of each it has.
fn paired(a: &Session, pruning: &str) {
    a.run(
        "CREATE PROCEDURE COUNT_F (IN CA VARCHAR(128), IN PA VARCHAR(128), IN OA VARCHAR(128), IN UA VARCHAR(128), \
         IN CB VARCHAR(128), IN PB VARCHAR(128), IN OB VARCHAR(128), IN UB VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) \
         CONTAINS SQL \
         BEGIN DECLARE G, V, NA, NB INTEGER DEFAULT 0; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         LA: LOOP FETCH NEXT FROM PTF UA INTO G, V; IF DONE THEN LEAVE LA; END IF; SET NA = NA + 1; END LOOP LA; \
         SET DONE = FALSE; \
         LB: LOOP FETCH NEXT FROM PTF UB INTO G, V; IF DONE THEN LEAVE LB; END IF; SET NB = NB + 1; END LOOP LB; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = NA; SET DESCRIPTOR PTF RR VALUE 2 DATA = NB; PIPE ROW PTF RR; END",
    )
    .unwrap();
    a.run(&format!(
        "CREATE FUNCTION PAIRED (A TABLE WITH SET SEMANTICS, B TABLE WITH SET SEMANTICS {pruning}) \
         RETURNS TABLE (NA INTEGER, NB INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE COUNT_F"
    ))
    .unwrap();
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §7.6 SR12a iv 2, §9.18 SR12b iii, vi; §9.25 GR13, GR14, GR20]
/// A table argument partitioned and ordered: a virtual processor for each
/// partition reads its rows in order, and its partitioning column is a
/// column of the result, of its range variable; unpartitioned, the table
/// is one partition.
#[test]
fn each_partition_is_run_in_order_9_25() {
    let db = defined();
    let a = db.session("A");
    summed(&a);
    let q = "SELECT X.G, P.TOTAL, P.FIRST_V FROM TABLE (SUMMED (TABLE (S) AS X PARTITION BY G ORDER BY V DESC)) AS P ORDER BY X.G";
    assert_eq!(rows(&a, q), ["1,30,20", "2,30,30"]);
    let q = "SELECT P.TOTAL, P.FIRST_V FROM TABLE (SUMMED (TABLE (SELECT * FROM S WHERE G = 1) ORDER BY V)) AS P";
    assert_eq!(rows(&a, q), ["30,10"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE (SUMMED (TABLE (S) AS X PARTITION BY G)) AS P ORDER BY 3"), ["30,10,1", "30,30,2"]);
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.25 GR13b vi 1, GR16]
/// An empty table of set semantics and KEEP WHEN EMPTY is one partition of
/// no row: the processor runs once.
#[test]
fn an_empty_table_kept_is_one_partition_9_25() {
    let db = defined();
    let a = db.session("A");
    summed(&a);
    assert_eq!(rows(&a, "SELECT P.TOTAL, P.FIRST_V FROM TABLE (SUMMED (TABLE (E))) AS P"), ["0,-"]);
    assert_eq!(rows(&a, "SELECT P.TOTAL, X.G FROM TABLE (SUMMED (TABLE (E) AS X PARTITION BY G)) AS P"), ["0,-"]);
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.18 SR12d; §9.25 GR13, GR16] (Features B201, B202)
/// Two table arguments co-partitioned: a partition for each value either
/// has, each table's rows of it, of none if it has none.
#[test]
fn copartitioned_tables_share_partitions_9_25() {
    let db = defined();
    let a = db.session("A");
    paired(&a, "");
    let q = "SELECT X.G, Y.G, P.NA, P.NB FROM TABLE (PAIRED (TABLE (S) AS X PARTITION BY G, TABLE (U) AS Y PARTITION BY G \
             COPARTITION (X, Y))) AS P ORDER BY P.NB, P.NA";
    assert_eq!(rows(&a, q), ["2,2,1,0", "3,3,0,1", "1,1,2,1"]);
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.18 SR12b iv; §9.25 GR13b vii] (Feature B204)
/// Of a table of PRUNE ON EMPTY, a partition it has no row of is none;
/// of a table of KEEP ON EMPTY, an invocation's PRUNE WHEN EMPTY prunes
/// it too.
#[test]
fn a_pruned_table_drops_its_empty_partitions_9_25() {
    let db = defined();
    let a = db.session("A");
    paired(&a, "PRUNE ON EMPTY");
    let q = "SELECT X.G, P.NA, P.NB FROM TABLE (PAIRED (TABLE (S) AS X PARTITION BY G, TABLE (U) AS Y PARTITION BY G \
             COPARTITION (X, Y))) AS P ORDER BY X.G";
    assert_eq!(rows(&a, q), ["1,2,1", "3,0,1"]);
    a.run("DROP FUNCTION PAIRED RESTRICT").unwrap();
    a.run("CREATE FUNCTION PAIRED (A TABLE WITH SET SEMANTICS, B TABLE) RETURNS TABLE (NA INTEGER, NB INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE COUNT_F").unwrap();
    let q = "SELECT X.G, P.NA, P.NB FROM TABLE (PAIRED (TABLE (S) AS X PARTITION BY G, TABLE (U) AS Y PARTITION BY G PRUNE WHEN EMPTY \
             COPARTITION (X, Y))) AS P ORDER BY X.G";
    assert_eq!(rows(&a, q), ["1,2,1", "3,0,1"]);
    assert_eq!(rows(&a, "SELECT P.NA, P.NB FROM TABLE (PAIRED (TABLE (S), TABLE (E) PRUNE WHEN EMPTY)) AS P"), Vec::<String>::new());
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.25 GR16] (Feature B207)
/// Two tables partitioned and not co-partitioned: a partition for each
/// pair of one partition of each.
#[test]
fn partitionings_not_copartitioned_cross_9_25() {
    let db = defined();
    let a = db.session("A");
    paired(&a, "");
    let q = "SELECT X.G, Y.G, P.NA, P.NB FROM TABLE (PAIRED (TABLE (S) AS X PARTITION BY G, TABLE (U) AS Y PARTITION BY G)) AS P \
             ORDER BY X.G, Y.G";
    assert_eq!(rows(&a, q), ["1,1,2,1", "1,3,2,1", "2,1,1,1", "2,3,1,1"]);
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.18 SR12d; §9.25 GR13, GR16] (Feature B203)
/// Two `<co-partition specification>`s: each pair shares its partitions,
/// and the partitions of the two pairs cross.
#[test]
fn two_copartition_specifications_cross_9_25() {
    let db = defined();
    let a = db.session("A");
    let parameters: Vec<String> = ["A", "B", "C", "D"]
        .iter()
        .map(|t| format!("IN C{t} VARCHAR(128), IN P{t} VARCHAR(128), IN O{t} VARCHAR(128), IN U{t} VARCHAR(128)"))
        .collect();
    a.run(&format!(
        "CREATE PROCEDURE QUAD_F ({}, IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE G, V, N INTEGER DEFAULT 0; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         LA: LOOP FETCH NEXT FROM PTF UA INTO G, V; IF DONE THEN LEAVE LA; END IF; SET N = N + 1; END LOOP LA; SET DONE = FALSE; \
         LC: LOOP FETCH NEXT FROM PTF UC INTO G, V; IF DONE THEN LEAVE LC; END IF; SET N = N + 10; END LOOP LC; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = N; PIPE ROW PTF RR; END",
        parameters.join(", ")
    ))
    .unwrap();
    a.run(
        "CREATE FUNCTION QUAD (A TABLE WITH SET SEMANTICS, B TABLE WITH SET SEMANTICS, C TABLE WITH SET SEMANTICS, \
         D TABLE WITH SET SEMANTICS) RETURNS TABLE (N INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE QUAD_F",
    )
    .unwrap();
    let q = "SELECT W.G, Y.G, P.N FROM TABLE (QUAD (TABLE (S) AS W PARTITION BY G, TABLE (U) AS X PARTITION BY G, \
             TABLE (S) AS Y PARTITION BY G, TABLE (U) AS Z PARTITION BY G COPARTITION (W, X), (Y, Z))) AS P ORDER BY W.G, Y.G";
    let expected = ["1,1,22", "1,2,12", "1,3,2", "2,1,21", "2,2,11", "2,3,1", "3,1,20", "3,2,10", "3,3,0"];
    assert_eq!(rows(&a, q), expected);
}
