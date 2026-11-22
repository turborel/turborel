//! Set-shaped access on a key planned as index probes (RULES.md D773 to
//! D777): a column compared with a subquery of one row, as an equality or
//! as bounds; an IN of a subquery or of VALUES, a row value's among them,
//! and EXISTS, probing per distinct key; a LEFT JOIN and a MERGE from a few
//! rows; a join's bounds from the outer row; and the UPDATE and DELETE of
//! these. Each query's rows are those of the same query of a table without
//! indexes, and each plan probes the key.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

use crate::common::{rows, sorted};

/// Orders in table I, indexed by its key, and in S, not.
const ORDERS: i32 = 3000;

/// Lines of each order.
const LINES: i32 = 3;

/// Districts of table I's orders.
const DISTRICTS: i32 = 4;

/// Schema P: tables I, keyed (W, D, O), and S, of the same rows: one
/// warehouse, `DISTRICTS` districts, orders 1 to `ORDERS` of each, V the
/// order mod 7; L, keyed (W, D, O, N), and M, of `LINES` lines of each
/// order of district 1, Q its item; and C, keyed (W, D), and E, the
/// district's next order, `ORDERS` plus one.
fn orders() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION P
           CREATE TABLE I (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, V INTEGER, PRIMARY KEY (W, D, O))
           CREATE TABLE S (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, V INTEGER)
           CREATE TABLE L (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, N INTEGER NOT NULL, Q INTEGER,
             PRIMARY KEY (W, D, O, N))
           CREATE TABLE M (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, N INTEGER NOT NULL, Q INTEGER)
           CREATE TABLE C (W INTEGER NOT NULL, D INTEGER NOT NULL, NXT INTEGER, PRIMARY KEY (W, D))
           CREATE TABLE E (W INTEGER NOT NULL, D INTEGER NOT NULL, NXT INTEGER)",
    )
    .unwrap();
    let s = db.session("P");
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    for o in 1..=ORDERS {
        rows.extend((1..=DISTRICTS).map(|d| format!("(1, {d}, {o}, {})", o % 7)));
        for n in 1..=LINES {
            lines.push(format!("(1, 1, {o}, {n}, {})", (o * 7 + n) % 100));
        }
    }
    for (table, values) in [("I", &rows), ("S", &rows), ("L", &lines), ("M", &lines)] {
        for chunk in values.chunks(500) {
            s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
        }
    }
    let next = ORDERS + 1;
    s.run(&format!("INSERT INTO C VALUES (1, 1, {next}), (1, 2, 1)")).unwrap();
    s.run(&format!("INSERT INTO E VALUES (1, 1, {next}), (1, 2, 1)")).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The physical plan of `query`, over the keyed tables, whose rows, sorted,
/// must be those of it over the others, and `count` of them.
fn planned(s: &Session, query: &str, count: usize) -> String {
    let plain = query.replace(" I ", " S ").replace(" I,", " S,").replace(" L ", " M ").replace(" L,", " M,").replace(" C ", " E ");
    let (keyed, unkeyed) = (sorted(s, query), sorted(s, &plain));
    assert_eq!(keyed, unkeyed, "{query}");
    assert_eq!(keyed.len(), count, "{query}: {keyed:?}");
    physical(s, query)
}

/// The physical plan of `query`.
fn physical(s: &Session, query: &str) -> String {
    s.explain(query, &Params::new()).unwrap().split("-- physical:").nth(1).unwrap_or_default().to_string()
}

/// The plan EXPLAIN ANALYZE shows of the data change `change`, run and
/// rolled back.
fn changed(s: &Session, change: &str) -> String {
    let plan = s.explain_analyze(change, &Params::new()).unwrap();
    s.rollback().unwrap();
    plan
}

/// Verifies: [X3.135-1986 §5.11 GR1, §5.24 GR2]; [DESIGN] RULES.md D773,
/// D774: a key compared with a subquery of one row, an ungrouped set
/// function's or one fetched first, probes the index from that row, by an
/// equality or by bounds; a subquery of no row finds none, as its null
/// compares unknown; one that may give more rows is left to raise its
/// cardinality error.
#[test]
fn a_key_compared_with_a_subquery_of_one_row_is_probed() {
    let db = orders();
    let s = db.session("P");
    let least = "SELECT O, V FROM I WHERE W = 1 AND D = 1 AND O = (SELECT MIN(O) FROM I WHERE W = 1 AND D = 1)";
    let plan = planned(&s, least, 1);
    assert!(plan.contains("IndexNestedLoop P.I"), "{plan}");
    assert!(!plan.contains("SeqScan P.I"), "{plan}");
    let fetched =
        "SELECT O FROM I WHERE W = 1 AND D = 1 AND O = (SELECT O FROM I WHERE W = 1 AND D = 1 ORDER BY O DESC FETCH FIRST 1 ROW ONLY)";
    assert!(planned(&s, fetched, 1).contains("IndexNestedLoop P.I"));
    let bounded = "SELECT O FROM I WHERE W = 1 AND D = 1 AND O > (SELECT MAX(O) FROM I WHERE W = 1 AND D = 1 AND O < 2996) \
                   AND O <= (SELECT MAX(O) FROM I WHERE W = 1 AND D = 1)";
    let plan = planned(&s, bounded, 5);
    assert!(plan.contains("IndexNestedLoop P.I") && plan.contains('>'), "{plan}");
    let none = "SELECT O FROM I WHERE W = 1 AND D = 1 AND O = (SELECT MIN(O) FROM I WHERE W = 9)";
    assert!(planned(&s, none, 0).contains("IndexNestedLoop"));
    // Of many rows the comparison stays, and raises.
    assert_eq!(rows(&s, "SELECT O FROM I WHERE O = (SELECT O FROM I WHERE O < 3)"), Err(Code::Cardinality));
}

/// Verifies: [X3.135-1986 §5.16 GR2, §5.17 GR2]; [ISO/IEC 9075-2:2023
/// §8.4 GR3]; [DESIGN] RULES.md D774, D777: an IN of a subquery of UNNEST
/// over arrays, of a row value, an IN of VALUES, and EXISTS of a subquery
/// equating the key with an array's elements probe the index once for each
/// distinct key, the UNNEST estimated by its arrays' elements; a key named
/// twice finds its row once; an antijoin is hashed as before.
#[test]
fn a_few_keys_in_a_set_probe_the_index() {
    let db = orders();
    let s = db.session("P");
    let unnest = "UNNEST(ARRAY[1, 1, 1, 1], ARRAY[1, 1, 1, 1], ARRAY[5, 7, 7, 4000]) AS U(W, D, O)";
    let of_unnest = format!("SELECT O FROM I WHERE (W, D, O) IN (SELECT W, D, O FROM {unnest})");
    let plan = planned(&s, &of_unnest, 2);
    assert!(plan.contains("IndexNestedLoop P.I") && plan.contains("Distinct"), "{plan}");
    let of_values = "SELECT O FROM I WHERE (W, D, O) IN (VALUES (1, 1, 10), (1, 1, 11), (1, 2, 12))";
    assert!(planned(&s, of_values, 3).contains("IndexNestedLoop P.I"));
    let exists = format!("SELECT O FROM I AS T WHERE EXISTS (SELECT 1 FROM {unnest} WHERE U.W = T.W AND U.D = T.D AND U.O = T.O)");
    assert!(planned(&s, &exists, 2).contains("IndexNestedLoop P.I"));
    let single = "SELECT O FROM I WHERE O IN (SELECT O FROM UNNEST(ARRAY[3, 4]) AS U(O)) AND W = 1 AND D = 1";
    assert!(planned(&s, single, 2).contains("IndexNestedLoop P.I"));
    let anti = format!("SELECT O FROM I WHERE (W, D, O) NOT IN (SELECT W, D, O FROM {unnest})");
    assert!(!planned(&s, &anti, 11_998).contains("IndexNestedLoop"));
    // A condition of the subquery's that is no equality with the key, and
    // many keys, read the table as before.
    let ordered = format!("SELECT O FROM I AS T WHERE EXISTS (SELECT 1 FROM {unnest} WHERE U.O < T.O AND U.O > T.O - 2)");
    assert!(!planned(&s, &ordered, 8).contains("IndexNestedLoop"));
    let many = "SELECT O FROM I WHERE (W, D, O) IN (SELECT W, D, O FROM S)";
    let plan = planned(&s, many, 12_000);
    assert!(!plan.contains("IndexNestedLoop") && plan.contains("Semi"), "{plan}");
}

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.21 GR2]; [DESIGN] RULES.md D776:
/// a join whose condition bounds the key's column after its equalities by
/// the outer row's values, a column or a difference of one and a constant,
/// probes the keys between the bounds alone; a null bound finds nothing.
#[test]
fn a_joins_bounds_from_the_outer_row_probe_a_range() {
    let db = orders();
    let s = db.session("P");
    let lines = "SELECT Y.O, Y.N FROM C AS X, L AS Y WHERE X.W = 1 AND X.D = 1 AND Y.W = X.W AND Y.D = X.D AND Y.O < X.NXT \
                 AND Y.O >= X.NXT - 20";
    let plan = planned(&s, lines, 60);
    assert!(plan.contains("IndexNestedLoop P.L") && plan.contains("Y.O < X.NXT") && plan.contains("Y.O >= (X.NXT - 20)"), "{plan}");
    let items = "SELECT COUNT(DISTINCT Y.Q) FROM C AS X, L AS Y, I AS Z WHERE X.W = 1 AND X.D = 1 AND Y.W = X.W AND Y.D = X.D \
                 AND Y.O < X.NXT AND Y.O >= X.NXT - 20 AND Z.W = Y.W AND Z.D = Y.D AND Z.O = Y.Q AND Z.V < 4";
    assert!(planned(&s, items, 1).contains("IndexNestedLoop P.L"));
    s.run("UPDATE C SET NXT = NULL WHERE D = 1").unwrap();
    s.run("UPDATE E SET NXT = NULL WHERE D = 1").unwrap();
    assert!(planned(&s, lines, 0).contains("IndexNestedLoop P.L"));
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1]; [ISO/IEC 9075-2:2023 §14.12
/// GR5]; [DESIGN] RULES.md D777: a LEFT JOIN of an UNNEST of a few
/// elements to a table on its key, and a MERGE from one, probe the key
/// from each element, a row that finds none kept with nulls.
#[test]
fn a_few_rows_joined_left_or_merged_probe_the_key() {
    let db = orders();
    let s = db.session("P");
    let left = "SELECT U.O, T.V FROM UNNEST(ARRAY[1, 1, 1], ARRAY[1, 1, 1], ARRAY[2, 9, 4001]) AS U(W, D, O) \
                LEFT JOIN I AS T ON T.W = U.W AND T.D = U.D AND T.O = U.O";
    let plan = planned(&s, left, 3);
    assert!(plan.contains("LeftOuterIndexNestedLoop P.I"), "{plan}");
    assert!(sorted(&s, left).contains(&"4001,NULL".to_owned()));
    let merge = "MERGE INTO L USING (SELECT * FROM UNNEST(ARRAY[1, 1, 1], ARRAY[1, 1, 1], ARRAY[7, 8, 4001], ARRAY[1, 2, 1]) \
                 AS U(W, D, O, N)) AS U ON L.W = U.W AND L.D = U.D AND L.O = U.O AND L.N = U.N WHEN MATCHED THEN UPDATE SET Q = -1";
    let plan = changed(&s, merge);
    assert!(plan.contains("LeftOuterIndexNestedLoop P.L") && plan.contains("rows changed=2"), "{plan}");
    s.run(merge).unwrap();
    s.run(&merge.replace("INTO L", "INTO M").replace("L.", "M.")).unwrap();
    assert_eq!(sorted(&s, "SELECT O, N FROM L WHERE Q = -1"), sorted(&s, "SELECT O, N FROM M WHERE Q = -1"));
}

/// Verifies: [X3.135-1986 §8.5 GR1, §8.12 GR1]; [DESIGN] RULES.md D775:
/// a searched DELETE or UPDATE whose condition compares the key with a
/// subquery of one row, or puts it in a few keys' set, finds its rows by
/// probes of the key from them, and changes the rows the table without an
/// index changes.
#[test]
fn deletes_and_updates_of_a_few_keys_probe_them() {
    let db = orders();
    let s = db.session("P");
    let unnest = "UNNEST(ARRAY[1, 1, 1], ARRAY[1, 1, 1], ARRAY[50, 51, 4001]) AS U(W, D, O)";
    let changes = [
        "DELETE FROM I WHERE W = 1 AND D = 1 AND O = (SELECT MIN(O) FROM I WHERE W = 1 AND D = 1)".to_owned(),
        format!("DELETE FROM I WHERE (W, D, O) IN (SELECT W, D, O FROM {unnest})"),
        format!("DELETE FROM I WHERE EXISTS (SELECT 1 FROM {unnest} WHERE U.W = I.W AND U.D = I.D AND U.O = I.O)"),
        "UPDATE I SET V = -1 WHERE (W, D, O) IN (VALUES (1, 1, 60), (1, 1, 61))".to_owned(),
        "UPDATE L SET Q = -1 WHERE W = 1 AND D = 1 AND O = (SELECT MAX(O) FROM I WHERE W = 1 AND D = 1)".to_owned(),
    ];
    for change in &changes {
        let plan = changed(&s, change);
        assert!(plan.contains("IndexNestedLoop") && !plan.contains("SeqScan P.I") && !plan.contains("SeqScan P.L"), "{change}: {plan}");
    }
    for change in &changes {
        s.run(change).unwrap();
        s.run(&change.replace(" I ", " S ").replace(" L ", " M ").replace("= I.", "= S.")).unwrap();
    }
    assert_eq!(sorted(&s, "SELECT O, V FROM I"), sorted(&s, "SELECT O, V FROM S"));
    assert_eq!(sorted(&s, "SELECT O, N, Q FROM L WHERE Q = -1"), sorted(&s, "SELECT O, N, Q FROM M WHERE Q = -1"));
}
