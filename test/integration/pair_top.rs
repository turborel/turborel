//! \[DESIGN\] (RULES.md D1281) The first rows of a sort over a pair of
//! groupings run as one part, as a DISTINCT set function makes them
//! (RULES.md D340, D1113), taken from the second grouping's table of each
//! partition. The rows and their order are those the plan run alone
//! gives, and a pair whose second no partition makes whole in a table is
//! read as before.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Rows of each table.
const ROWS: i64 = 12_000;

/// Values of G, each of as many rows.
const GROUPS: i64 = 600;

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike: K, a key; G, one of 600, each of 20 rows scattered among
/// the others', null for every 41st; S, a text of 150 values, four of G
/// each; N, of 1 to 7 values in a group of G by what 7 leaves of G, so
/// many groups tie on each count of them, null for every 13th.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(8), N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(8), N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["C", "R"] {
        let values: Vec<String> = (0..ROWS)
            .map(|k| {
                let g = k * 7 % GROUPS;
                format!("({k}, {}, 's{}', {})", null_or(k % 41 == 0, g), g % 150, null_or(k % 13 == 0, k.div_euclid(GROUPS) % (1 + g % 7)))
            })
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The rows of `sql` run alone, a row at a time on one thread.
fn alone(s: &Session, sql: &str) -> Rows {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    rows(s, sql)
}

/// Sets `s` to run in parallel, on 4 threads in morsels of 256 rows.
fn together(s: &Session) {
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
}

/// The rows of `sql` run alone, its rows run in parallel, and its plan
/// then.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    let alone = alone(s, sql);
    together(s);
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    (alone, rows(s, sql), shown)
}

/// Whether `shown`, a plan, has a part run in parallel whose top is two
/// groupings, one over the other, under a filter or not, projections
/// between them or none.
fn paired_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().map(str::trim_start).filter(|l| !l.starts_with("Filter")).collect();
    lines.windows(2).enumerate().any(|(at, w)| {
        let part = w.first().is_some_and(|l| l.starts_with("Parallel")) && w.get(1).is_some_and(|l| l.starts_with("Aggregate"));
        let below = lines.iter().skip(at.saturating_add(2)).find(|l| !l.starts_with("Project"));
        part && below.is_some_and(|l| l.starts_with("Aggregate"))
    })
}

/// Of `sql` analyzed as it runs in parallel, a sort's first rows over a
/// pair under a projection: the rows given, those counted of the second
/// grouping, and those counted of the projection over the part. Its rows
/// run in parallel are those it gives alone.
fn counted(s: &Session, sql: &str) -> (u64, Option<u64>, Option<u64>) {
    let (alone, together, shown) = both(s, sql);
    assert!(paired_part(&shown), "{sql}\n{shown}");
    assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
    assert_eq!(alone, together, "{sql}");
    let analyzed = s.analyzed(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    let of = |label: &str| analyzed.operators.iter().find(|o| o.label.starts_with(label)).map(|o| o.rows);
    (analyzed.rows, of("Aggregate"), of("Project"))
}

/// The one number `sql` gives.
fn number(s: &Session, sql: &str) -> u64 {
    rows(s, sql).unwrap().concat().parse().unwrap()
}

/// Verifies: [X3.135-1986 §8.3 GR3b] [ISO/IEC 9075-2:2023 §7.17 GR6]
/// (RULES.md D1281) the first rows of a sort over a pair of groupings run
/// as one part, taken from the second grouping's tables, are the rows the
/// plan run alone gives, in its order: by the count of distinct values,
/// which many groups tie on at the last row asked, where the group whose
/// first row came earlier is given; past an offset that ends among peers;
/// of a text key under a WHERE clause; by the count and then the key; by
/// a set function computed in parts, under a projection that moves the
/// columns; of more rows than there are groups; and of the row store,
/// read by batch as the column store is.
#[test]
fn the_first_rows_of_a_sort_over_a_pair_are_the_plans() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT G, COUNT(DISTINCT N) FROM {t} GROUP BY G ORDER BY 2 DESC FETCH FIRST 5 ROWS ONLY",
            "SELECT G, COUNT(DISTINCT N) FROM {t} GROUP BY G ORDER BY 2 DESC OFFSET 80 ROWS FETCH FIRST 10 ROWS ONLY",
            "SELECT S, COUNT(DISTINCT N) FROM {t} WHERE S <> '' GROUP BY S ORDER BY 2 DESC FETCH FIRST 10 ROWS ONLY",
            "SELECT G, COUNT(DISTINCT N) FROM {t} GROUP BY G ORDER BY 2, 1 DESC FETCH FIRST 7 ROWS ONLY",
            "SELECT COUNT(DISTINCT N), 'x', G, COUNT(*), SUM(N) FROM {t} GROUP BY G ORDER BY 4 DESC, 1 FETCH FIRST 9 ROWS ONLY",
            "SELECT G, COUNT(DISTINCT N) FROM {t} GROUP BY G ORDER BY 2 DESC FETCH FIRST 1000 ROWS ONLY",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            assert!(paired_part(&shown), "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
    // The fifth row and the sixth tie on their counts, so the fifth is
    // given for its first row alone.
    let six = alone(&a, "SELECT G, COUNT(DISTINCT N) FROM C GROUP BY G ORDER BY 2 DESC FETCH FIRST 6 ROWS ONLY").unwrap();
    let count_of = |row: &String| row.rsplit(',').next().map(str::to_owned);
    assert_eq!(six.get(4).map(count_of), six.get(5).map(count_of), "{six:?}");
}

/// Verifies: [EXTENSION] (RULES.md D1281, D1254, D378) EXPLAIN ANALYZE of
/// a sort's first rows over a pair made by batch counts the second
/// grouping the groups its tables made, and the projection the rows the
/// partitions kept for the sort, fewer than the groups: by a set function
/// alone, whose groups are picked before their rows are made, and by a
/// grouping column.
#[test]
fn an_analyzed_sort_over_a_pair_takes_its_first_rows_from_the_seconds_tables() {
    let (_db, a) = tables();
    // The select list's constant stays below the FETCH FIRST, where its
    // rows are counted (RULES.md D1306).
    exec(&a, "SET OPTIMIZER.COMPUTE_AFTER_FETCH_FIRST = 'OFF'").unwrap();
    let groups = number(&a, "SELECT COUNT(*) FROM (SELECT G FROM C GROUP BY G) AS X");
    for sql in [
        "SELECT 1, G, COUNT(DISTINCT N) FROM C GROUP BY G ORDER BY 3 DESC FETCH FIRST 5 ROWS ONLY",
        "SELECT 1, G, COUNT(DISTINCT N) FROM C GROUP BY G ORDER BY 2 DESC FETCH FIRST 5 ROWS ONLY",
    ] {
        let (given, made, projected) = counted(&a, sql);
        assert_eq!((given, made), (5, Some(groups)), "{sql}: the groups the second's tables made");
        assert!(
            projected.is_some_and(|kept| (5..groups).contains(&kept)),
            "{sql}: the rows the partitions kept: {projected:?} of {groups}"
        );
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR6] (RULES.md D1281, D1124) a
/// sort's first rows over a pair whose second no partition makes whole in
/// a table are not taken from tables. Every group is read and projected,
/// as an analyzed plan counts, and the rows are the plan's: of a second
/// with a filtered set function, made of rows, as one DISTINCT function
/// under a filter and two of different arguments have; of one that
/// crosses the first's partitions, by a count of the first, and by no
/// column, whose one group every partition holds a part of; of a sort
/// that asks its last row's peers; under a HAVING condition, whose groups
/// kept are read; and of morsels read a row at a time, whose groups of
/// the first are made of rows.
#[test]
fn a_sort_over_a_pair_no_partition_makes_in_a_table_reads_every_group() {
    let (_db, a) = tables();
    // The select list's constant stays below the FETCH FIRST, where its
    // rows are counted (RULES.md D1306).
    exec(&a, "SET OPTIMIZER.COMPUTE_AFTER_FETCH_FIRST = 'OFF'").unwrap();
    let groups = number(&a, "SELECT COUNT(*) FROM (SELECT G FROM C GROUP BY G) AS X");
    let kept = number(&a, "SELECT COUNT(*) FROM (SELECT G FROM C GROUP BY G HAVING COUNT(DISTINCT N) > 2) AS X");
    let counts = number(&a, "SELECT COUNT(*) FROM (SELECT X.M FROM (SELECT G, COUNT(N) AS M FROM C GROUP BY G) AS X GROUP BY X.M) AS Y");
    for (sql, read) in [
        ("SELECT 1, G, COUNT(DISTINCT N) FILTER (WHERE N > 0) FROM C GROUP BY G ORDER BY 3 DESC FETCH FIRST 5 ROWS ONLY", groups),
        ("SELECT 1, G, COUNT(DISTINCT N), COUNT(DISTINCT S) FROM C GROUP BY G ORDER BY 3 DESC FETCH FIRST 5 ROWS ONLY", groups),
        (
            "SELECT 1, X.M, COUNT(*) FROM (SELECT G, COUNT(N) AS M FROM C GROUP BY G) AS X GROUP BY X.M ORDER BY 3 DESC FETCH FIRST 2 ROWS ONLY",
            counts,
        ),
        ("SELECT 1, COUNT(DISTINCT N) FROM C ORDER BY 2 FETCH FIRST 1 ROW ONLY", 1),
        ("SELECT 1, G, COUNT(DISTINCT N) FROM C GROUP BY G ORDER BY 3 DESC FETCH FIRST 5 ROWS WITH TIES", groups),
        ("SELECT 1, G, COUNT(DISTINCT N) FROM C GROUP BY G HAVING COUNT(DISTINCT N) > 2 ORDER BY 3 DESC FETCH FIRST 5 ROWS ONLY", kept),
    ] {
        let (_, _, projected) = counted(&a, sql);
        assert_eq!(projected, Some(read), "{sql}: every group read and projected");
    }
    let sql = "SELECT 1, G, COUNT(DISTINCT N) FROM C GROUP BY G ORDER BY 3 DESC FETCH FIRST 5 ROWS ONLY";
    let alone = alone(&a, sql);
    together(&a);
    exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    assert_eq!(rows(&a, sql), alone, "{sql}, its morsels read by rows");
    let analyzed = a.analyzed(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    let projected = analyzed.operators.iter().find(|o| o.label.starts_with("Project")).map(|o| o.rows);
    assert_eq!(projected, Some(groups), "{sql}, its morsels read by rows: every group read and projected");
}
