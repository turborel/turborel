//! \[DESIGN\] A window's input pruned and narrowed (RULES.md D1245): a
//! window reads its partitioning's, its ordering's and its calls'
//! columns alone, its rows carry those alone, and it gives the rows the
//! same window gives when every column is read above it, so nothing is
//! pruned or narrowed; of every kind of call and frame, its select list
//! reading a column the window does not, run alone, in parallel a row at
//! a time and by batch. A call nothing above reads is still computed, so
//! the error it raises is not lost.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Every column of N, as a select list reads them.
const ALL: &str = "P, O, X, Y, T, Z";

/// Table N of 3000 rows: K, a key; P, one of 7; O, one of 37, so a
/// partition's rows tie on it; X, one of 11, 0 among them; Y, a number
/// no window reads; T, a text no window reads; and Z, a text of one of 5.
fn table() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE N (K INTEGER, P INTEGER, O INTEGER, X INTEGER, Y INTEGER, T VARCHAR(40), Z VARCHAR(20))",
    )
    .unwrap();
    let a = db.session("A");
    for from in (0_i64..3_000).step_by(500) {
        let values: Vec<String> = (from..from + 500)
            .map(|k| format!("({k}, {}, {}, {}, {}, 'text {}', 'zone {}')", k % 7, k * 13 % 37, k % 11, k * 3, k % 101, k % 5))
            .collect();
        a.run(&format!("INSERT INTO N VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    (db, a)
}

/// The windows: each a call and its window specification, its ordering
/// unique where its value depends on the order of peers.
fn windows() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |call: &str, spec: &str| out.push((call.to_owned(), spec.to_owned()));
    for call in ["ROW_NUMBER()", "NTILE(4)", "LAG(X, 2, -1)", "LEAD(X)"] {
        add(call, "PARTITION BY P ORDER BY O, K");
    }
    for call in ["RANK()", "DENSE_RANK()", "PERCENT_RANK()", "CUME_DIST()"] {
        add(call, "PARTITION BY Z ORDER BY O");
    }
    let rows = [
        "ROWS BETWEEN 2 PRECEDING AND 1 FOLLOWING",
        "ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW",
        "ROWS BETWEEN 1 FOLLOWING AND 3 FOLLOWING",
        "ROWS BETWEEN 3 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW",
    ];
    for frame in rows {
        for call in ["SUM(X)", "COUNT(*)", "MIN(X)", "FIRST_VALUE(X)", "LAST_VALUE(X)", "NTH_VALUE(X, 2)", "COUNT(*) FILTER (WHERE X > 3)"]
        {
            add(call, &format!("PARTITION BY P ORDER BY O, K {frame}"));
        }
    }
    let peers = [
        "",
        "RANGE BETWEEN 3 PRECEDING AND 2 FOLLOWING",
        "RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE CURRENT ROW",
        "RANGE BETWEEN 2 PRECEDING AND 2 FOLLOWING EXCLUDE TIES",
        "GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING",
        "GROUPS BETWEEN CURRENT ROW AND 2 FOLLOWING EXCLUDE GROUP",
    ];
    for frame in peers {
        for call in ["SUM(X)", "COUNT(*)", "MAX(X)", "COUNT(*) FILTER (WHERE X > 3)"] {
            add(call, &format!("PARTITION BY P ORDER BY O {frame}"));
        }
    }
    for spec in ["PARTITION BY P", "PARTITION BY Z", ""] {
        add("COUNT(*) FILTER (WHERE X = 1)", spec);
        add("SUM(X)", spec);
    }
    out
}

/// The rows of `sql`, sorted, each cut to its first `n` columns.
fn first_columns(s: &Session, sql: &str, n: usize) -> Vec<String> {
    let mut out: Vec<String> = rows(s, sql)
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
        .iter()
        .map(|r| r.split(',').take(n).collect::<Vec<_>>().join(","))
        .collect();
    out.sort();
    out
}

/// Sets `s` to run a row at a time alone, in parallel on 4 threads, or by
/// batch in parallel, as `mode` says.
fn run_as(s: &Session, mode: &str) {
    let (degree, batches) = match mode {
        "alone" => ("1", "OFF"),
        "rows" => ("4", "OFF"),
        _ => ("4", "ON"),
    };
    for set in [
        format!("SET EXECUTOR.DEGREE = {degree}"),
        format!("SET EXECUTOR.BATCHES = '{batches}'"),
        "SET EXECUTOR.MORSEL_ROWS = 256".to_owned(),
        "SET EXECUTOR.PARALLEL_MIN_ROWS = 0".to_owned(),
    ] {
        exec(s, &set).unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D1245: a window of each kind of call and
/// frame, its select list reading a column it does not read, gives the
/// rows it gives with every column read above it, alone, in parallel a
/// row at a time and by batch; its plan reads the columns it and the
/// select list read alone, no null carried.
#[test]
fn a_narrowed_window_gives_the_rows_of_the_whole_one() {
    let (_db, a) = table();
    for (call, spec) in windows() {
        let narrow = format!("SELECT K, {call} OVER ({spec}) AS V, T FROM N");
        let whole = format!("SELECT K, {call} OVER ({spec}) AS V, T, {ALL} FROM N");
        let shown = a.physical_plan(&narrow, &Params::new()).unwrap();
        let below: Vec<&str> = shown.lines().skip_while(|l| !l.contains("Window")).skip(1).collect();
        assert!(
            below.iter().any(|l| l.contains("Project N.K")) && !below.iter().any(|l| l.contains("NULL")),
            "{narrow}: no null carried\n{shown}"
        );
        for mode in ["alone", "rows", "batches"] {
            run_as(&a, mode);
            assert_eq!(first_columns(&a, &narrow, 3), first_columns(&a, &whole, 3), "{narrow}, {mode}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1245: a window's call no one above reads
/// is computed all the same, so dividing by a column's zero raises, alone
/// and in parallel; and a window read above in its order gives its rows
/// in that order.
#[test]
fn a_call_read_by_none_still_raises_and_an_order_asked_is_kept() {
    let (_db, a) = table();
    let unread = "SELECT K FROM (SELECT K, T, SUM(100 / X) OVER (PARTITION BY P) AS V FROM N) AS S";
    let ordered = "SELECT K, ROW_NUMBER() OVER (PARTITION BY P ORDER BY O, K) AS R FROM N ORDER BY P, O, K";
    let expected = rows(&a, ordered).unwrap();
    for mode in ["alone", "rows", "batches"] {
        run_as(&a, mode);
        assert_eq!(rows(&a, unread), Err(Code::Data), "{mode}: the division by zero raised");
        assert_eq!(rows(&a, ordered).unwrap(), expected, "{mode}: in the order asked");
    }
}
