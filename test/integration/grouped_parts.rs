//! \[DESIGN\] A grouping run in parallel under its HAVING condition, and a
//! grouping read by a join (RULES.md D1072, D1073): the groups a part
//! keeps are those the plan run alone keeps, in its order, and a join
//! over a grouping gives the rows it gives row by row.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike: K, a key; G, one of 500, null for every 41st; N, a
/// number, null for every 13th. And D, of the column store, of as many
/// rows: K, a key; A, one of 7; B, one of 60, so 420 pairs of them, more
/// than a morsel's thread merges alone, so the partitions hold them.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, N INTEGER)
           CREATE TABLE D (K INTEGER PRIMARY KEY, A INTEGER, B INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["C", "R"] {
        let values: Vec<String> = (0_i64..7000)
            .map(|k| format!("({k}, {}, {})", null_or(k % 41 == 0, k * 7 % 500), null_or(k % 13 == 0, k * 7919 % 100)))
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    let pairs: Vec<String> = (0_i64..7000).map(|k| format!("({k}, {}, {})", k % 7, k % 60)).collect();
    a.run(&format!("INSERT INTO D VALUES {}", pairs.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The rows of `sql` run alone a row at a time, its rows run in parallel
/// on 4 threads in morsels of 256 rows, and its plan then.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let alone = rows(s, sql);
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    (alone, rows(s, sql), shown)
}

/// Whether `shown`, a plan, has a part run in parallel whose top is a
/// filter: the line after a `Parallel` line begins with `Filter`.
fn filtered_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().map(str::trim_start).collect();
    lines
        .windows(2)
        .any(|pair| pair.first().is_some_and(|l| l.starts_with("Parallel")) && pair.get(1).is_some_and(|l| l.starts_with("Filter")))
}

/// Verifies: [X3.135-1986 §5.23 GR1] (RULES.md D1073, D1135) a HAVING condition
/// that cannot raise over a grouping run in parallel is a part with it,
/// as EXPLAIN shows, and keeps the groups it keeps run alone, in their
/// order: of some groups, of none, of every group, of a null sum, which
/// is not true of it, and of a grouping by no column, of one group even
/// of no rows; and of one set function compared with a constant, judged
/// of its value alone; and of few groups, whose sums and counts are fed
/// by lanes where no value is null (D1120).
#[test]
fn a_having_condition_is_judged_by_its_part() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT G, SUM(N), COUNT(*) FROM {t} GROUP BY G HAVING SUM(N) > 700",
            "SELECT G, COUNT(N) FROM {t} GROUP BY G HAVING SUM(N) > 1000000",
            "SELECT G, MAX(N) FROM {t} GROUP BY G HAVING COUNT(*) > 0",
            "SELECT G, COUNT(*), SUM(N) FROM {t} WHERE N IS NULL OR K < 100 GROUP BY G HAVING NOT (SUM(N) > 100)",
            "SELECT G, MIN(K) FROM {t} WHERE K < 3000 GROUP BY G HAVING COUNT(N) < 5 AND MAX(N) > 90",
            "SELECT COUNT(*), SUM(N) FROM {t} HAVING COUNT(*) > 10",
            "SELECT COUNT(*) FROM {t} WHERE N < 0 HAVING COUNT(*) = 0",
            "SELECT COUNT(*) FROM {t} WHERE N < 0 HAVING COUNT(*) > 0",
            "SELECT G, SUM(N) FROM {t} GROUP BY G HAVING SUM(N) > 700",
            "SELECT G, SUM(N) FROM {t} GROUP BY G HAVING 700 >= SUM(N)",
            "SELECT G, K, SUM(N) FROM {t} WHERE K < 900 GROUP BY G, K HAVING SUM(N) <> 7",
            "SELECT G, MAX(N) FROM {t} GROUP BY G HAVING MAX(N) = 99",
            "SELECT G, AVG(N), SUM(N), COUNT(N), COUNT(*) FROM {t} WHERE G < 20 GROUP BY G HAVING COUNT(*) > 0",
            "SELECT G, AVG(K), SUM(K), COUNT(*) FROM {t} WHERE G < 20 GROUP BY G HAVING COUNT(*) > 0",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            assert!(filtered_part(&shown), "{sql}\n{shown}");
            assert!(alone.is_ok(), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1073, D15) a HAVING condition that can
/// raise stays above its grouping's part, judged in the groups' order,
/// and raises the error it raises run alone.
#[test]
fn a_having_condition_that_can_raise_stays_above_its_part() {
    let (_db, a) = tables();
    for sql in [
        "SELECT G, SUM(N) FROM C GROUP BY G HAVING 1000 / SUM(N) > 1",
        "SELECT G, COUNT(N) FROM C GROUP BY G HAVING 10 / (COUNT(N) - 12) > 1",
    ] {
        let (alone, together, shown) = both(&a, sql);
        assert!(!filtered_part(&shown) && shown.contains("Parallel"), "{sql}\n{shown}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1] (RULES.md D1072) a join over a
/// grouping that takes no runtime filter, of SUM, alone or run in
/// parallel under its HAVING condition, gives the rows it gives row by
/// row on one thread: joined as a derived table, and as the subquery of
/// IN.
#[test]
fn a_join_over_a_grouping_reads_it_by_batch() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT COUNT(*), SUM(X.S), SUM({t}.N) FROM {t} JOIN (SELECT G, SUM(N) AS S FROM {t} GROUP BY G) AS X ON X.G = {t}.K",
            "SELECT COUNT(*), SUM(X.S), MIN({t}.K) FROM {t} JOIN (SELECT G, SUM(N) AS S FROM {t} GROUP BY G HAVING SUM(N) > 700) AS X ON X.G = {t}.K",
            "SELECT COUNT(*), SUM(N) FROM {t} WHERE K IN (SELECT G FROM {t} GROUP BY G HAVING COUNT(*) > 13 AND MAX(N) > 95)",
            "SELECT COUNT(*), SUM(N) FROM {t} WHERE K NOT IN (SELECT G FROM {t} WHERE G IS NOT NULL GROUP BY G HAVING COUNT(*) > 13)",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, _) = both(&a, &sql);
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
            exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
            assert_eq!(rows(&a, &sql), alone, "by batch on one thread: {sql}");
        }
    }
}

/// Verifies: [X3.135-1986 §8.3 GR3b] [ISO/IEC 9075-2:2023 §7.17 GR6]
/// (RULES.md D1124) the first rows of a sort over a grouping run in
/// parallel, taken from its partitions' tables, are the rows the plan run
/// alone gives, in its order: of counts that many groups tie on, where
/// the earlier group comes first, by a set function and then a key, past
/// an offset, by a key with a null, of a column given twice, of more rows
/// than there are groups, of a projection that gives constants beside
/// its columns, and of the row store, whose groups are made of rows.
#[test]
fn the_first_rows_of_a_sort_over_a_grouping_are_the_plans() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT G, COUNT(*) FROM {t} GROUP BY G ORDER BY 2 DESC FETCH FIRST 9 ROWS ONLY",
            "SELECT G, COUNT(N), SUM(K) FROM {t} GROUP BY G ORDER BY 2, 1 DESC FETCH FIRST 7 ROWS ONLY",
            "SELECT COUNT(*), G FROM {t} GROUP BY G ORDER BY 1 OFFSET 5 ROWS FETCH FIRST 6 ROWS ONLY",
            "SELECT G, MAX(K) FROM {t} GROUP BY G ORDER BY G DESC FETCH FIRST 3 ROWS ONLY",
            "SELECT G, G, COUNT(*) FROM {t} GROUP BY G ORDER BY 3 DESC FETCH FIRST 4 ROWS ONLY",
            "SELECT N, COUNT(*) FROM {t} GROUP BY N ORDER BY 2 DESC FETCH FIRST 1000 ROWS ONLY",
            "SELECT 1, G, COUNT(*), 'x' FROM {t} GROUP BY G ORDER BY 3 DESC FETCH FIRST 6 ROWS ONLY",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            assert!(shown.contains("Parallel"), "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR6] (RULES.md D1124) a sort
/// whose first rows are not taken from the tables gives the plan's rows
/// too: one that asks the peers of its last row, which a group's first
/// row's number would part; one over a HAVING condition, which the
/// tables' groups are not yet judged by; and one over a projection that
/// computes a value.
#[test]
fn a_sort_of_first_rows_the_tables_do_not_give_reads_the_groups() {
    let (_db, a) = tables();
    for sql in [
        "SELECT G, COUNT(*) FROM C GROUP BY G ORDER BY 2 DESC FETCH FIRST 3 ROWS WITH TIES",
        "SELECT G, COUNT(*) FROM C GROUP BY G HAVING COUNT(*) < 14 ORDER BY 2 DESC FETCH FIRST 5 ROWS ONLY",
        "SELECT G, COUNT(*) + 1 FROM C GROUP BY G ORDER BY 2 DESC FETCH FIRST 5 ROWS ONLY",
    ] {
        let (alone, together, _) = both(&a, sql);
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [X3.135-1986 §5.22 GR2] (RULES.md D1125) a grouping run in
/// parallel with no HAVING condition, whose partitions give their
/// tables' groups as batches, gives the groups the plan run alone gives,
/// in its order: of counts, sums and extremes, a null key and null
/// values among them, and read by a join.
#[test]
fn a_grouping_run_in_parallel_gives_its_tables_groups() {
    let (_db, a) = tables();
    for sql in [
        "SELECT G, COUNT(*), COUNT(N), SUM(N), MIN(N), MAX(K) FROM C GROUP BY G",
        "SELECT N, COUNT(*), SUM(K) FROM C GROUP BY N",
        "SELECT COUNT(*), SUM(X.S) FROM C JOIN (SELECT G, SUM(N) AS S FROM C GROUP BY G) AS X ON X.G = C.K",
    ] {
        let (alone, together, shown) = both(&a, sql);
        assert!(shown.contains("Parallel"), "{sql}\n{shown}");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Whether `shown`, a plan, has a grouping that is a part run in
/// parallel: the line after a `Parallel` line begins with `Aggregate`.
fn grouped_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().map(str::trim_start).collect();
    lines
        .windows(2)
        .any(|pair| pair.first().is_some_and(|l| l.starts_with("Parallel")) && pair.get(1).is_some_and(|l| l.starts_with("Aggregate")))
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1] (RULES.md D1221) a join that
/// builds of a grouping run in parallel on keys holding every grouping
/// column, which is handed the partitions' groups in any order, gives the
/// rows the plan run alone gives, in its order: an inner join and a LEFT
/// join of a derived table, whose projection lies between, a semi join
/// on the key and a set function, and a grouping of two columns keyed by
/// both, as a step of a pipeline run in parallel and run alone over the
/// first rows of a sort. A join on one of two grouping columns finds
/// several groups of a key and gives them in the order the plan run alone
/// does, either way, and so does a FULL join its unmatched groups.
#[test]
fn a_join_keyed_by_a_groupings_columns_gives_the_plans_rows() {
    let (_db, a) = tables();
    for sql in [
        "SELECT C.K, X.G, X.S FROM C JOIN (SELECT G, SUM(N) AS S FROM C GROUP BY G) AS X ON X.G = C.K",
        "SELECT R.K, X.G, X.S FROM R JOIN (SELECT G, SUM(N) AS S FROM R GROUP BY G) AS X ON X.G = R.K",
        "SELECT C.K, X.S FROM C LEFT JOIN (SELECT G, COUNT(*) AS S FROM C GROUP BY G) AS X ON X.G = C.K",
        "SELECT K, N FROM C WHERE (K, N) IN (SELECT G, MIN(N) FROM C GROUP BY G)",
        "SELECT D.K, X.S FROM D JOIN (SELECT A, B, COUNT(*) AS S FROM D GROUP BY A, B) AS X ON X.A = D.A AND X.B = D.B",
        "SELECT D.K, X.A, X.S FROM D JOIN (SELECT A, B, COUNT(*) AS S FROM D GROUP BY A, B) AS X ON X.B = D.A",
        "SELECT L.K, X.S FROM (SELECT K, A, B FROM D ORDER BY K FETCH FIRST 6000 ROWS ONLY) AS L \
         JOIN (SELECT A, B, COUNT(*) AS S FROM D GROUP BY A, B) AS X ON X.A = L.A AND X.B = L.B",
        "SELECT L.K, X.A, X.S FROM (SELECT K, A, B FROM D ORDER BY K FETCH FIRST 6000 ROWS ONLY) AS L \
         JOIN (SELECT A, B, COUNT(*) AS S FROM D GROUP BY A, B) AS X ON X.B = L.A",
        "SELECT C.K, X.G, X.S FROM C FULL JOIN (SELECT G, SUM(N) AS S FROM C GROUP BY G) AS X ON X.G = C.K + 6700",
    ] {
        let (alone, together, shown) = both(&a, sql);
        assert!(grouped_part(&shown) && !shown.contains("IndexNestedLoop"), "{sql}\n{shown}");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [EXTENSION] (RULES.md D1254, D378) EXPLAIN ANALYZE of a
/// sort's first rows over a grouping run in parallel takes them from the
/// partitions' tables as the statement does, under a projection that is
/// counted and under none. The grouping is counted the groups it made,
/// and the projection the rows the partitions kept for the sort, fewer
/// than the groups. A sort that asks its last row's peers reads every
/// group, and its projection is counted a row a group.
#[test]
fn an_analyzed_sort_takes_its_first_rows_from_the_tables() {
    let (_db, a) = tables();
    // The select list's constant stays below the FETCH FIRST, where its
    // rows are counted (RULES.md D1306).
    exec(&a, "SET OPTIMIZER.COMPUTE_AFTER_FETCH_FIRST = 'OFF'").unwrap();
    let counted = |sql: &str| -> (u64, Option<u64>, Option<u64>) {
        let (alone, together, shown) = both(&a, sql);
        assert!(grouped_part(&shown), "{sql}\n{shown}");
        assert_eq!(alone, together, "{sql}");
        let analyzed = a.analyzed(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        let of = |label: &str| analyzed.operators.iter().find(|o| o.label.starts_with(label)).map(|o| o.rows);
        (analyzed.rows, of("Aggregate"), of("Project"))
    };
    let groups: u64 = rows(&a, "SELECT COUNT(*) FROM (SELECT G FROM C GROUP BY G) AS X").unwrap().concat().parse().unwrap();
    let (given, made, projected) = counted("SELECT 1, G, COUNT(*) FROM C GROUP BY G ORDER BY 3 DESC FETCH FIRST 6 ROWS ONLY");
    assert_eq!((given, made), (6, Some(groups)), "the groups its tables made");
    assert!(projected.is_some_and(|kept| (6..groups).contains(&kept)), "the rows the partitions kept: {projected:?} of {groups}");
    let (given, made, _) = counted("SELECT G, COUNT(*) FROM C GROUP BY G ORDER BY 2 DESC, 1 FETCH FIRST 6 ROWS ONLY");
    assert_eq!((given, made), (6, Some(groups)), "under no projection that computes");
    let (given, made, projected) = counted("SELECT 1, G, COUNT(*) FROM C GROUP BY G ORDER BY 3 DESC FETCH FIRST 6 ROWS WITH TIES");
    assert!(given >= 6, "the peers of the last row too");
    assert_eq!((made, projected), (Some(groups), Some(groups)), "every group read and projected");
}

/// Verifies: [X3.135-1986 §8.3 GR3b] [ISO/IEC 9075:1992 §6.5 GR2b iv]
/// (RULES.md D1256) a sort by set functions alone, whose first rows are
/// picked before their groups' rows are made, gives the rows and the
/// order the plan run alone gives, with several functions between the
/// keys and beside them; and a set function no key names still raises
/// what it raises of a group that is not picked, here a sum past the
/// precision of its type in a group outside the first rows.
#[test]
fn the_first_rows_by_set_functions_are_picked_before_they_are_made() {
    // Two of these are past 38 digits and within the 128 bits a sum is
    // kept in, so their sum is made and raises only as it is finished.
    const HUGE: &str = "60000000000000000000000000000000000000";
    let (db, a) = tables();
    exec(&a, "CREATE TABLE E (K INTEGER PRIMARY KEY, G INTEGER, X NUMERIC(38, 0)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)")
        .unwrap();
    let values: Vec<String> =
        (0_i64..7000).map(|k| format!("({k}, {}, {})", k % 500, if k == 77 || k == 577 { HUGE } else { "1" })).collect();
    a.run(&format!("INSERT INTO E VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    for sql in [
        "SELECT G, COUNT(*), SUM(N), MIN(K), AVG(N) FROM C GROUP BY G ORDER BY 2 DESC FETCH FIRST 7 ROWS ONLY",
        "SELECT SUM(N), G, COUNT(N), 'x' FROM C GROUP BY G ORDER BY 3 DESC, 1 FETCH FIRST 9 ROWS ONLY",
        "SELECT N, MAX(K), COUNT(*) FROM C GROUP BY N ORDER BY 2 OFFSET 3 ROWS FETCH FIRST 5 ROWS ONLY",
        "SELECT G, COUNT(*), MAX(X) FROM E GROUP BY G ORDER BY 2 DESC FETCH FIRST 3 ROWS ONLY",
    ] {
        let (alone, together, shown) = both(&a, sql);
        assert!(grouped_part(&shown), "{sql}\n{shown}");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
    let raising = "SELECT G, COUNT(*), SUM(X) FROM E GROUP BY G ORDER BY 2 DESC FETCH FIRST 3 ROWS ONLY";
    let (alone, together, shown) = both(&a, raising);
    assert!(grouped_part(&shown), "{raising}\n{shown}");
    assert_eq!(alone, Err(turborel::error::Code::Data), "the sum of group 77 is past its type's precision");
    assert_eq!(alone, together, "{raising}");
}
