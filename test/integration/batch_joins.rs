//! \[DESIGN\] Joins read a batch at a time (RULES.md D31,
//! `exec/batch/joined.rs`): an aggregation or a projection over inner hash
//! joins, products and LEFT, RIGHT and FULL outer joins of scans, filtered
//! or not, gives what it gives row by row, in the order asked of it.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{both_ways, exec, rows};

/// The rows of F, of the column store, and R, of the row store, alike: K,
/// a key; G, one of 7, null for every 13th; B; S, a CHAR of 'a' or 'b'.
/// D, of the row store, two names for G = 1 and none for G = 6; DC, of the
/// column store, alike; E, a tag of some of the names; P, of CHAR keys
/// longer.
fn tables(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE F (K INTEGER PRIMARY KEY, G INTEGER, B INTEGER, S CHAR(2)) USING columnstore
             WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, B INTEGER, S CHAR(2))
           CREATE TABLE D (G INTEGER, NAME VARCHAR(10))
           CREATE TABLE DC (G INTEGER, NAME VARCHAR(10)) USING columnstore
           CREATE TABLE E (NAME VARCHAR(10), TAG INTEGER)
           CREATE TABLE P (S CHAR(5), N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["F", "R"] {
        for first in (0..6000).step_by(1000) {
            let values: Vec<String> = (first..first + 1000)
                .map(|k: i64| {
                    let g = if k % 13 == 0 { "NULL".to_owned() } else { (k % 7).to_string() };
                    format!("({k}, {g}, {}, '{}')", k * 3 % 1000, if k % 2 == 0 { "a" } else { "b" })
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    for table in ["D", "DC"] {
        a.run(&format!("INSERT INTO {table} VALUES (0, 'zero'), (1, 'one'), (1, 'uno'), (2, 'two'), (3, 'three'), (4, 'four'), (5, 'five'), (NULL, 'none')")).unwrap();
    }
    a.run("INSERT INTO E VALUES ('one', 1), ('uno', 1), ('two', 2), ('three', 3), ('none', 9)").unwrap();
    a.run("INSERT INTO P VALUES ('a', 10), ('b    ', 20)").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    a
}

/// Queries of a join: `{t}` the table, F or R.
const JOINED: [&str; 12] = [
    "SELECT COUNT(*), SUM({t}.B), MAX(D.NAME) FROM D, {t} WHERE D.G = {t}.G",
    "SELECT COUNT(*), SUM({t}.B), MIN(D.NAME) FROM {t}, D WHERE {t}.K < 300",
    "SELECT D.NAME, COUNT(*), SUM({t}.B) FROM D, {t} WHERE {t}.S = 'a' AND D.G > {t}.G GROUP BY D.NAME",
    "SELECT COUNT(*), SUM({t}.B) FROM {t}, D WHERE {t}.G = D.G",
    "SELECT D.NAME, COUNT(*), SUM({t}.B), MAX({t}.K) FROM {t}, D WHERE {t}.G = D.G GROUP BY D.NAME",
    "SELECT COUNT(*), SUM({t}.B * 2 - D.G) FROM {t}, D WHERE {t}.G = D.G AND {t}.B > D.G * 100",
    "SELECT D.NAME, SUM({t}.B) FROM {t}, D WHERE {t}.G = D.G AND {t}.K < 2500 AND D.NAME LIKE 't%' GROUP BY D.NAME",
    "SELECT E.TAG, COUNT(*), SUM({t}.B) FROM {t}, D, E WHERE {t}.G = D.G AND D.NAME = E.NAME GROUP BY E.TAG",
    "SELECT P.N, COUNT(*) FROM {t}, P WHERE {t}.S = P.S GROUP BY P.N",
    "SELECT {t}.K, DC.NAME FROM {t}, DC WHERE {t}.G = DC.G AND {t}.K < 60",
    "SELECT D.NAME, COUNT(*), SUM({t}.B), MAX({t}.S) FROM D, {t} GROUP BY D.NAME",
    "SELECT {t}.K, {t}.B, D.NAME FROM {t}, D WHERE {t}.K < 25",
];

/// Queries of an outer join: `{t}` the table, F or R.
const OUTER: [&str; 20] = [
    "SELECT COUNT(*), COUNT(D.NAME), SUM({t}.B) FROM {t} LEFT JOIN D ON {t}.G = D.G AND {t}.B > D.G * 100",
    "SELECT D.NAME, COUNT(*), SUM({t}.B) FROM {t} LEFT JOIN D ON {t}.G = D.G AND {t}.B > D.G GROUP BY D.NAME",
    "SELECT D.NAME, COUNT({t}.K), SUM({t}.B) FROM {t} RIGHT JOIN D ON {t}.G = D.G GROUP BY D.NAME",
    "SELECT COUNT(*), COUNT(D.NAME), SUM({t}.B) FROM {t} FULL JOIN D ON {t}.G = D.G AND D.NAME <> 'one'",
    "SELECT {t}.K, D.NAME FROM D FULL JOIN {t} ON {t}.G = D.G AND {t}.S = 'a' WHERE {t}.K < 30 OR {t}.K IS NULL",
    "SELECT COUNT(*), SUM({t}.B) FROM {t} LEFT JOIN D ON {t}.G = D.G AND {t}.B / (D.G + 1) > 100",
    "SELECT COUNT(*), COUNT(D.NAME) FROM {t} LEFT JOIN D ON {t}.G = D.G AND {t}.B / ({t}.G + 1) > 100",
    "SELECT COUNT(*), COUNT(D.NAME), SUM({t}.B) FROM {t} LEFT OUTER JOIN D ON {t}.G = D.G",
    "SELECT D.NAME, COUNT(*), SUM({t}.B) FROM {t} LEFT JOIN D ON {t}.G = D.G GROUP BY D.NAME",
    "SELECT {t}.K, D.NAME FROM {t} LEFT JOIN D ON {t}.G = D.G WHERE {t}.K < 40",
    "SELECT COUNT(*), MIN({t}.K) FROM {t} LEFT JOIN D ON {t}.G = D.G WHERE D.NAME IS NULL",
    "SELECT COUNT(*), SUM({t}.B) FROM {t} LEFT JOIN D ON {t}.G = D.G AND D.NAME <> 'uno'",
    "SELECT D.NAME, COUNT({t}.K), SUM({t}.B) FROM D LEFT JOIN {t} ON D.G = {t}.G GROUP BY D.NAME",
    "SELECT COUNT(*), COUNT(D.NAME), SUM({t}.B) FROM {t} FULL JOIN D ON {t}.G = D.G AND 10 / (D.G + 1) > 1 AND {t}.B / ({t}.G + 1) > 100",
    "SELECT D.NAME, COUNT({t}.K), SUM({t}.B) FROM {t} RIGHT JOIN D ON {t}.G = D.G AND 10 / (D.G + 1) > 1 GROUP BY D.NAME",
    "SELECT COUNT(*), COUNT(D.NAME), SUM({t}.B) FROM {t} FULL JOIN D ON {t}.G > D.G + 4",
    "SELECT COUNT(*), COUNT(D.NAME), COUNT({t}.K) FROM {t} FULL JOIN D ON {t}.B / (D.G + 1) > 400",
    "SELECT D.NAME, COUNT({t}.K) FROM {t} RIGHT JOIN D ON {t}.G < D.G - 4 GROUP BY D.NAME",
    "SELECT {t}.K, {t}.B, {t}.S, D.NAME FROM {t} FULL JOIN D ON {t}.G = D.G AND {t}.K < 20",
    "SELECT {t}.K, D.NAME FROM {t} LEFT JOIN D ON {t}.G = D.G AND {t}.B / ({t}.G + 1) > 100 WHERE {t}.K < 50",
];

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.20 GR2b, §5.11 GR2]:
/// an aggregation or a projection over inner hash joins and products read
/// by batches gives what one read row by row gives (EXECUTOR.BATCHES
/// OFF), of a table of either store: a null key joining nothing, a key of
/// two rows joining each, CHAR keys equal as their padding makes them, a
/// filter over the join, one told a scan, three tables, grouped or not, a
/// set function of an expression, the large table the right operand.
#[test]
fn joins_by_batch_answer_as_row_by_row() {
    let db = Database::new();
    let a = tables(&db);
    for query in JOINED {
        for table in ["F", "R"] {
            let sql = query.replace("{t}", table);
            let (batched, row) = both_ways(&a, &sql);
            assert_eq!(batched, row, "{sql}");
        }
    }
    assert_eq!(rows(&a, "SELECT P.N, COUNT(*) FROM F, P WHERE F.S = P.S GROUP BY P.N").map(|r| r.len()), Ok(2), "'b' is 'b    '");
}

/// Verifies: [DESIGN] RULES.md D31 [ISO/IEC 9075:1992 §7.5 GR1c, GR4,
/// GR5]: an aggregation or a projection over an outer join read by
/// batches gives what one read row by row gives, of a table of either
/// store: a left row no right row matches, its key null or not, with
/// nulls; one matched by two, twice; the join filtered; a condition
/// beyond its keys, of both operands or of one, that can raise an error
/// or not, an operand's own condition judged as the other has a row; a
/// join on no keys, every pair judged; a small left operand of a large
/// right; RIGHT and FULL joins, a right row no left row matches after
/// nulls, its key null or not. An error of a row of an operand is raised
/// both ways; EXISTS of a join that raises past its first row stops at it
/// both ways (RULES.md D15).
#[test]
fn outer_joins_by_batch_answer_as_row_by_row() {
    let db = Database::new();
    let a = tables(&db);
    for query in OUTER {
        for table in ["F", "R"] {
            let sql = query.replace("{t}", table);
            let (batched, row) = both_ways(&a, &sql);
            assert_eq!(batched, row, "{sql}");
        }
    }
    // Of 6000 rows, 1/7 have G = 1, joined twice; G = 6 and null, alone.
    let counted = rows(&a, "SELECT COUNT(*), COUNT(D.NAME) FROM F LEFT JOIN D ON F.G = D.G");
    assert_eq!(counted.map(|r| r.len()), Ok(1));
    for raising in [
        "SELECT COUNT(*) FROM F FULL JOIN D ON F.G = D.G AND F.B / (F.K - 7) > 0",
        "SELECT COUNT(*) FROM D RIGHT JOIN F ON F.G = D.G AND F.B / (F.K - 7) > 0",
        "SELECT COUNT(*) FROM DC FULL JOIN (SELECT * FROM F WHERE B / (K - 7) > 0) AS V ON V.G = DC.G",
        "SELECT COUNT(*) FROM (SELECT * FROM F WHERE B / (K - 7) > 0) AS V RIGHT JOIN DC ON V.G = DC.G",
        "SELECT COUNT(*) FROM F, (SELECT * FROM DC WHERE 1 / (G - 3) > 0) AS V",
        "SELECT COUNT(*) FROM (SELECT * FROM F WHERE B / (K - 7) > 0) AS V, D",
        "SELECT COUNT(*) FROM (SELECT * FROM F WHERE B / (K - 7) > 0) AS V, D WHERE V.G = D.G",
        "SELECT COUNT(*) FROM (SELECT * FROM F WHERE B / (K - 7) > 0) AS V LEFT JOIN D ON V.G = D.G",
        "SELECT COUNT(*) FROM (SELECT * FROM F WHERE B / (K - 7) > 0) AS V FULL JOIN D ON V.G = D.G",
        "SELECT COUNT(*) FROM F FULL JOIN D ON F.G = D.G AND 10 / (D.G - 3) > 0",
        "SELECT COUNT(*) FROM F FULL JOIN D ON F.B / (D.G - 3) > 0",
    ] {
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        assert_eq!(rows(&a, raising).err(), Some(Code::Data), "{raising}");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        assert_eq!(rows(&a, raising).err(), Some(Code::Data), "{raising}");
    }
    // F's rows of K < 7 are joined before that of K = 7 raises.
    let first = "SELECT COUNT(*) FROM D WHERE EXISTS (SELECT F.K FROM F FULL JOIN DC ON F.G = DC.G AND F.B / (F.K - 7) > 0)";
    let (batched, row) = both_ways(&a, first);
    assert_eq!((batched, row), (vec!["8".to_owned()], vec!["8".to_owned()]));
}

/// Verifies: [DESIGN] RULES.md D31, D38: a join whose right operand
/// outgrows the work area, partitioned, gives its rows to an aggregation
/// read by batches as row by row, a LEFT outer join's too; one that
/// raises an error raises it.
#[test]
fn a_partitioned_join_by_batch_answers_as_row_by_row() {
    let db = Database::new();
    let a = tables(&db);
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    let (batched, row) = both_ways(&a, "SELECT F.G, COUNT(*), SUM(R.B) FROM F, R WHERE F.K = R.K GROUP BY F.G");
    assert_eq!(batched, row);
    // A product's right operand, outgrowing it, read again from a run.
    let (batched, row) = both_ways(&a, "SELECT D.NAME, COUNT(*), MAX(F.B) FROM D, F WHERE F.S = 'b' GROUP BY D.NAME");
    assert_eq!(batched, row);
    // A LEFT join's right operand, outgrowing it, moved to a table; a
    // RIGHT or FULL join's, partitioned as the row path partitions it.
    for left in [
        "SELECT F.G, COUNT(R.K), SUM(R.B) FROM F LEFT JOIN R ON F.K = R.B GROUP BY F.G",
        "SELECT D.NAME, COUNT(F.K) FROM D LEFT JOIN F ON D.G = F.G GROUP BY D.NAME",
        "SELECT F.G, COUNT(R.K), SUM(R.B) FROM F LEFT JOIN R ON F.K = R.B AND R.G <> F.G GROUP BY F.G",
        "SELECT R.G, COUNT(F.K), SUM(R.B) FROM F RIGHT JOIN R ON F.K = R.B GROUP BY R.G",
        "SELECT D.NAME, COUNT(F.K), COUNT(*) FROM D FULL JOIN F ON D.G = F.G GROUP BY D.NAME",
    ] {
        let (batched, row) = both_ways(&a, left);
        assert_eq!(batched, row, "{left}");
    }
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM F, R WHERE F.K = R.K"), Ok(vec!["6000".to_owned()]));
    let raising = "SELECT COUNT(*) FROM F, D WHERE F.G = D.G AND F.B / (D.G - D.G) > 0";
    assert_eq!(rows(&a, raising).err(), Some(Code::Data), "division by zero");
}

/// Verifies: [DESIGN] RULES.md D31, D38 [X3.135-1986 §8.3 GR3b]: a join of
/// streams whose left operand is read in its groups' order, the planner
/// asking it for an ORDER BY, gives its rows in that order by batches as
/// row by row, its right operand past the work area or not: an inner hash
/// join partitioned, its joined rows merged into the left's order; a
/// product whose right operand is in a run, read again for each left row;
/// an outer join.
#[test]
fn joins_by_batch_keep_their_left_operands_order() {
    let db = Database::new();
    let a = tables(&db);
    a.run(
        "CREATE TABLE O (K INTEGER, G INTEGER, B INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'G')",
    )
    .unwrap();
    a.run("INSERT INTO O SELECT K, G, B FROM R WHERE K < 3000").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for query in [
        "SELECT O.G, R.G, COUNT(*), SUM(R.B) FROM O, R WHERE O.K = R.B GROUP BY O.G, R.G ORDER BY O.G",
        "SELECT O.G, R.G, COUNT(*), SUM(R.B) FROM O, R WHERE R.K < 2000 AND O.K < 40 GROUP BY O.G, R.G ORDER BY O.G",
        "SELECT O.G, D.NAME, COUNT(*) FROM O LEFT JOIN D ON O.G = D.G AND O.B / (O.K + 1) > 0 GROUP BY O.G, D.NAME ORDER BY O.G",
    ] {
        let explained = a.explain(query, &turborel::params::Params::new()).unwrap();
        let plan = explained.split("-- physical:").nth(1).unwrap_or_default();
        assert!(plan.contains("in order of") && !plan.contains("Sort"), "{plan}");
        let batched = rows(&a, query);
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        assert_eq!(batched, rows(&a, query), "{query}");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D31, D44: a product read by batches whose
/// operands are held in memory, reading no table as it pairs them, looks
/// for a cancel as it makes each batch: past its STATEMENT.TIMEOUT_MS it is
/// cancelled (SQLCODE -920). Regression test (R-TEST-7): a product of
/// held operands made its batches without looking, so a count of a
/// billion pairs ran on to its end.
#[test]
fn a_product_by_batches_is_cancelled_as_it_pairs() {
    let db = Database::new();
    let a = tables(&db);
    a.run("CREATE TABLE T (K INTEGER)").unwrap();
    a.run("INSERT INTO T VALUES (1)").unwrap();
    for _ in 0..10 {
        a.run("INSERT INTO T SELECT K FROM T").unwrap();
    }
    a.commit().unwrap();
    exec(&a, "SET STATEMENT.TIMEOUT_MS = 50").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T A, T B, T C").err(), Some(Code::Cancelled));
}
