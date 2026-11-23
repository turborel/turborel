//! Groupings and DISTINCT streamed, their rows in the order of their keys
//! (RULES.md D550, D551, `physical/streamed.rs`, `exec/group/streamed.rs`,
//! `exec/batch/groups/streamed.rs`): each query's rows, in their order,
//! alike with PLANNER.STREAMED_GROUPING on and off, read by batches and
//! row by row, on the row store and the column store; and the plans EXPLAIN
//! shows of them.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// The rows of each table.
const ROWS: i64 = 3000;

/// T of the row store, keyed by K, an index on G, and C of the column store
/// kept in the order of G, alike in their rows, of schema A: G of 40
/// values and nulls, X of 7 and nulls, S of strings alike but for case and
/// trailing spaces, Y approximate.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER PRIMARY KEY, G INTEGER, X INTEGER, S VARCHAR(10), Y DOUBLE PRECISION)
           CREATE TABLE C (K INTEGER, G INTEGER, X INTEGER, S VARCHAR(10), Y DOUBLE PRECISION)
             USING columnstore WITH (COLUMNSTORE.ORDER = 'G', COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("CREATE INDEX T_G ON T (G)").unwrap();
    for first in (0..ROWS).step_by(500) {
        let values: Vec<String> = (first..first + 500).map(row).collect();
        for table in ["T", "C"] {
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
    }
    // A table of the row store keeps zones from the first plan that weighs
    // an index-only read of it (RULES.md D868): asked, then settled.
    assert_ne!(plan(&a, "SELECT K, COUNT(*) FROM T GROUP BY K"), "");
    db.maintain().unwrap();
    (db, a)
}

/// Row `k` of the tables.
fn row(k: i64) -> String {
    let g = if k % 17 == 0 { "NULL".to_owned() } else { (k * 7 % 40).to_string() };
    let x = if k % 11 == 0 { "NULL".to_owned() } else { (k % 7).to_string() };
    let s = ["'a'", "'A'", "'a  '", "'b'", "'B '", "NULL"][usize::try_from(k % 6).unwrap()];
    format!("({k}, {g}, {x}, {s}, {}.25)", k % 13)
}

/// The physical plan EXPLAIN shows of `sql`.
fn plan(a: &Session, sql: &str) -> String {
    let shown = a.explain(sql, &Params::new()).unwrap();
    shown.split("-- physical:").nth(1).unwrap_or(&shown).to_owned()
}

/// The rows of `sql` in each way: streamed groupings allowed and not, each
/// by batches and row by row.
fn each_way(a: &Session, sql: &str) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    for streamed in ["ON", "OFF"] {
        exec(a, &format!("SET PLANNER.STREAMED_GROUPING = '{streamed}'")).unwrap();
        for batches in ["ON", "OFF"] {
            exec(a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
            found.push(rows(a, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}")));
        }
    }
    exec(a, "SET PLANNER.STREAMED_GROUPING TO DEFAULT").unwrap();
    exec(a, "SET EXECUTOR.BATCHES TO DEFAULT").unwrap();
    found
}

/// Whether the rows found each way, as `each_way` finds them, are alike
/// streamed and not, by batches and row by row apart: of strings alike but
/// for trailing spaces, which DISTINCT or a group shows is
/// implementation-dependent, and the two ways of reading may show either
/// [X3.135-1986 §5.25 GR2].
fn alike(ways: &[Vec<String>]) -> bool {
    ways[0] == ways[2] && ways[1] == ways[3]
}

/// Queries whose rows come in an order, compared in it.
const ORDERED: &[&str] = &[
    "SELECT G, COUNT(*), SUM(X), MIN(S), MAX(Y), AVG(X) FROM {t} GROUP BY G ORDER BY G",
    "SELECT G, X, COUNT(*), SUM(Y) FROM {t} GROUP BY G, X ORDER BY G DESC, X",
    "SELECT X, G, COUNT(*) FROM {t} GROUP BY G, X ORDER BY X NULLS FIRST, G DESC",
    "SELECT DISTINCT G FROM {t} ORDER BY G",
    "SELECT DISTINCT S FROM {t} ORDER BY S",
    "SELECT DISTINCT K / 2, X FROM {t} ORDER BY 1, 2",
    "SELECT G, COUNT(*) FROM {t} WHERE K < 0 GROUP BY G ORDER BY G",
    "SELECT S COLLATE CASE_INSENSITIVE, COUNT(*) FROM {t} GROUP BY S COLLATE CASE_INSENSITIVE ORDER BY 1",
    "SELECT G, COUNT(DISTINCT X), SUM(DISTINCT X) FROM {t} GROUP BY G ORDER BY G",
    "SELECT G, COUNT(*) FILTER (WHERE X > 2), SUM(X) FILTER (WHERE S = 'a') FROM {t} GROUP BY G ORDER BY G",
    "SELECT G, COUNT(*) FROM {t} GROUP BY G ORDER BY G FETCH FIRST 3 ROWS ONLY",
    "SELECT G, SUM(X) FROM {t} WHERE G > 30 GROUP BY G ORDER BY G OFFSET 2 ROWS FETCH FIRST 4 ROWS ONLY",
    "SELECT K, COUNT(*) FROM {t} GROUP BY K ORDER BY K FETCH FIRST 5 ROWS ONLY",
];

/// Queries whose rows come in no order asked, compared sorted.
const UNORDERED: &[&str] = &[
    "SELECT G, COUNT(*), SUM(X) FROM {t} GROUP BY G",
    "SELECT K, COUNT(*), MAX(S) FROM {t} GROUP BY K",
    "SELECT DISTINCT G, X FROM {t}",
    "SELECT COUNT(*), SUM(X) FROM {t}",
    "SELECT G, X, COUNT(*) FROM {t} GROUP BY GROUPING SETS ((G), (X), ())",
    "SELECT D.G, D.Z, COUNT(*) FROM (SELECT G, X + G FROM {t}) AS D (G, Z) GROUP BY D.G, D.Z",
];

/// Verifies: [X3.135-1986 §5.22 GR2, §5.25 GR2] [ISO/IEC 9075-2:2023 §7.13
/// GR2a, §10.9 GR4, GR7b i] [DESIGN] RULES.md D550, D551: a grouping or
/// DISTINCT gives the same rows, in the same order where one is asked,
/// streamed or hashed, read by batches or row by row: of nulls, of strings
/// alike but for case or trailing spaces, by a collation, of no rows, of
/// the empty grouping set, of groups made before any row, of DISTINCT set
/// functions and filtered ones, under FETCH FIRST and OFFSET; on the row
/// store and the column store kept in the order of a key.
#[test]
fn streamed_and_hashed_alike() {
    let (_db, a) = tables();
    for table in ["T", "C"] {
        for sql in ORDERED {
            let sql = sql.replace("{t}", table);
            let ways = each_way(&a, &sql);
            assert!(alike(&ways), "{sql}: {ways:?}");
            assert!(!ways[0].is_empty() || sql.contains("K < 0"), "{sql}");
        }
        for sql in UNORDERED {
            let sql = sql.replace("{t}", table);
            let mut ways = each_way(&a, &sql);
            for w in &mut ways {
                w.sort();
            }
            assert!(alike(&ways), "{sql}: {ways:?}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D550: EXPLAIN shows a grouping streamed over
/// a read in its keys' order, a column store's kept in it or an index's
/// covering it, and over a sort of a DISTINCT's rows under an ORDER BY of
/// them, where they are nearly all distinct; and hashed when the setting is
/// OFF, of a grouping by a collation, of groups made before any row, and of
/// a DISTINCT under ORDER BY of few distinct rows, whose groups are sorted.
#[test]
fn explain_shows_streamed_groupings() {
    let (_db, a) = tables();
    let kept = plan(&a, "SELECT G, COUNT(*), SUM(X) FROM C GROUP BY G");
    assert!(kept.contains("Aggregate streamed group by") && kept.contains("in order of 1"), "{kept}");
    let covered = plan(&a, "SELECT K, COUNT(*) FROM T GROUP BY K");
    assert!(covered.contains("Aggregate streamed") && covered.contains("IndexOnlyScan"), "{covered}");
    let sorted = plan(&a, "SELECT DISTINCT K / 2, X FROM T ORDER BY 1, 2");
    let (distinct, sort) = (sorted.find("Distinct streamed").unwrap(), sorted.find("Sort").unwrap());
    assert!(distinct < sort, "a sort below a streamed DISTINCT: {sorted}");
    let few = plan(&a, "SELECT DISTINCT S FROM T ORDER BY S");
    assert!(few.find("Sort").unwrap() < few.find("Distinct").unwrap() && !few.contains("streamed"), "{few}");
    for hashed in [
        "SELECT S COLLATE CASE_INSENSITIVE, COUNT(*) FROM C GROUP BY S COLLATE CASE_INSENSITIVE",
        "SELECT G, X, COUNT(*) FROM C GROUP BY GROUPING SETS ((G), (X), ())",
        "SELECT G, COUNT(DISTINCT X), MAX(S) FROM C GROUP BY G",
    ] {
        let shown = plan(&a, hashed);
        assert!(shown.contains("Aggregate") && !shown.contains("Aggregate streamed"), "{shown}");
    }
    exec(&a, "SET PLANNER.STREAMED_GROUPING = 'OFF'").unwrap();
    assert!(!plan(&a, "SELECT G, COUNT(*), SUM(X) FROM C GROUP BY G").contains("streamed"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR7] [DESIGN] RULES.md D551: a
/// streamed grouping under FETCH FIRST gives its groups as its input is
/// read, so the read stops once they are given, by batches and row by row.
#[test]
fn fetch_first_stops_a_streamed_grouping() {
    let (_db, a) = tables();
    for (batches, table, scan) in [("OFF", "C", "SeqScan"), ("OFF", "T", "IndexOnlyScan"), ("ON", "T", "IndexOnlyScan")] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        let key = if table == "C" { "G" } else { "K" };
        let sql = format!("SELECT {key}, COUNT(*), MIN({key}) FROM {table} GROUP BY {key} FETCH FIRST 3 ROWS ONLY");
        let analyzed = a.analyzed(&sql, &Params::new()).unwrap();
        let read = analyzed.operators.iter().find(|o| o.label.starts_with(scan)).unwrap();
        let grouped = analyzed.operators.iter().find(|o| o.label.starts_with("Aggregate streamed")).unwrap();
        assert!(read.rows * 2 < u64::try_from(ROWS).unwrap(), "{batches}: {}", analyzed.text);
        assert!(grouped.rows <= 4, "{batches}: {}", analyzed.text);
    }
}
