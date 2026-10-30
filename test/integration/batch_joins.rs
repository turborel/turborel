//! \[DESIGN\] Joins read a batch at a time (RULES.md D31,
//! `exec/batch/stream.rs`): an aggregation or a projection over inner hash
//! joins and LEFT outer joins of scans, filtered or not, gives what it
//! gives row by row.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::ThreadPolicy;

use crate::common::{both_ways, exec, rows};

/// The rows of F, of the column store, and R, of the row store, alike: K,
/// a key; G, one of 7, null for every 13th; B; S, a CHAR of 'a' or 'b'.
/// D, of the row store, two names for G = 1 and none for G = 6; DC, of the
/// column store, alike; E, a tag of some of the names; P, of CHAR keys
/// longer.
fn tables(db: &Database) -> Session {
    db.set_thread_policy(ThreadPolicy::Manual).unwrap();
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
const JOINED: [&str; 7] = [
    "SELECT COUNT(*), SUM({t}.B) FROM {t}, D WHERE {t}.G = D.G",
    "SELECT D.NAME, COUNT(*), SUM({t}.B), MAX({t}.K) FROM {t}, D WHERE {t}.G = D.G GROUP BY D.NAME",
    "SELECT COUNT(*), SUM({t}.B * 2 - D.G) FROM {t}, D WHERE {t}.G = D.G AND {t}.B > D.G * 100",
    "SELECT D.NAME, SUM({t}.B) FROM {t}, D WHERE {t}.G = D.G AND {t}.K < 2500 AND D.NAME LIKE 't%' GROUP BY D.NAME",
    "SELECT E.TAG, COUNT(*), SUM({t}.B) FROM {t}, D, E WHERE {t}.G = D.G AND D.NAME = E.NAME GROUP BY E.TAG",
    "SELECT P.N, COUNT(*) FROM {t}, P WHERE {t}.S = P.S GROUP BY P.N",
    "SELECT {t}.K, DC.NAME FROM {t}, DC WHERE {t}.G = DC.G AND {t}.K < 60",
];

/// Queries of a LEFT outer join: `{t}` the table, F or R.
const OUTER: [&str; 6] = [
    "SELECT COUNT(*), COUNT(D.NAME), SUM({t}.B) FROM {t} LEFT OUTER JOIN D ON {t}.G = D.G",
    "SELECT D.NAME, COUNT(*), SUM({t}.B) FROM {t} LEFT JOIN D ON {t}.G = D.G GROUP BY D.NAME",
    "SELECT {t}.K, D.NAME FROM {t} LEFT JOIN D ON {t}.G = D.G WHERE {t}.K < 40",
    "SELECT COUNT(*), MIN({t}.K) FROM {t} LEFT JOIN D ON {t}.G = D.G WHERE D.NAME IS NULL",
    "SELECT COUNT(*), SUM({t}.B) FROM {t} LEFT JOIN D ON {t}.G = D.G AND D.NAME <> 'uno'",
    "SELECT D.NAME, COUNT({t}.K), SUM({t}.B) FROM D LEFT JOIN {t} ON D.G = {t}.G GROUP BY D.NAME",
];

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.20 GR2b, §5.11 GR2]:
/// an aggregation or a projection over inner hash joins read by batches
/// gives what one read row by row gives (EXECUTOR.BATCHES OFF), of a
/// table of either store: a null key joining nothing, a key of two rows
/// joining each, CHAR keys equal as their padding makes them, a filter
/// over the join, one told a scan, three tables, grouped or not, a set
/// function of an expression.
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

/// Verifies: [DESIGN] RULES.md D31 [ISO/IEC 9075:1992 §7.5 GR4, GR5b]: an
/// aggregation or a projection over a LEFT outer join read by batches
/// gives what one read row by row gives, of a table of either store: a
/// left row no right row matches, its key null or not, with nulls; one
/// matched by two, twice; the join filtered; one with a condition beyond
/// its keys; a small left operand of a large right.
#[test]
fn left_joins_by_batch_answer_as_row_by_row() {
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
}

/// Verifies: [DESIGN] RULES.md D31, D38: a join whose right operand
/// outgrows the work area, partitioned, gives its rows to an aggregation
/// read by batches as row by row, a LEFT outer join's too; one that
/// raises an error raises it.
#[test]
fn a_partitioned_join_by_batch_answers_as_row_by_row() {
    let db = Database::new();
    let a = tables(&db);
    exec(&a, "SET WORK_MEMORY = 65536").unwrap();
    let (batched, row) = both_ways(&a, "SELECT F.G, COUNT(*), SUM(R.B) FROM F, R WHERE F.K = R.K GROUP BY F.G");
    assert_eq!(batched, row);
    // A LEFT join's right operand, outgrowing it, moved to a table.
    for left in [
        "SELECT F.G, COUNT(R.K), SUM(R.B) FROM F LEFT JOIN R ON F.K = R.B GROUP BY F.G",
        "SELECT D.NAME, COUNT(F.K) FROM D LEFT JOIN F ON D.G = F.G GROUP BY D.NAME",
    ] {
        let (batched, row) = both_ways(&a, left);
        assert_eq!(batched, row, "{left}");
    }
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM F, R WHERE F.K = R.K"), Ok(vec!["6000".to_owned()]));
    let raising = "SELECT COUNT(*) FROM F, D WHERE F.G = D.G AND F.B / (D.G - D.G) > 0";
    assert_eq!(rows(&a, raising).err(), Some(Code::Data), "division by zero");
}
