//! The optimizer's rewrites of RULES.md D373-D375: parts of conditions
//! hoisted onto their operands, filters and limits moved into set
//! operations and below windows, and joins removed. Each query's rows, its
//! warning and its error alike with its rule on and off, of rows read by
//! batches and row by row, of the row store and the column store, and
//! EXPLAIN telling whether the rule rewrote it.

mod computed_keys;
mod computed_late;
mod counted_rows;
mod deferred_keys;
mod dependent_keys;
mod elimination;
mod extremes;
mod hoisting;
mod late_fetch;
mod pregrouped;
mod pushdown;
mod shifted;

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// The tables of schema Q, each name after `{P}`: A and B of 600 rows
/// keyed by indexes on K with repeated and null keys, C of 40 and D of 8;
/// Z of a zero divisor in Y; T and U of duplicates and nulls for the set
/// operations; E empty.
const TABLES: [&str; 8] = [
    "CREATE TABLE {P}A (K INTEGER, V INTEGER, S VARCHAR(10))",
    "CREATE TABLE {P}B (K INTEGER, W INTEGER)",
    "CREATE TABLE {P}C (K INTEGER, X INTEGER, S VARCHAR(10))",
    "CREATE TABLE {P}D (K INTEGER, Y INTEGER)",
    "CREATE TABLE {P}Z (K INTEGER, X INTEGER, Y INTEGER)",
    "CREATE TABLE {P}T (K INTEGER, V INTEGER, S VARCHAR(10))",
    "CREATE TABLE {P}U (K INTEGER, V INTEGER, S VARCHAR(10))",
    "CREATE TABLE {P}E (K INTEGER, V INTEGER, S VARCHAR(10))",
];

/// The prefix of the column store's tables.
pub const COLUMNS: &str = "C_";

/// What makes a table of the column store.
const COLUMN_STORE: &str = " USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)";

/// The values of `n` rows, each `of` its number.
fn values(n: i32, of: &dyn Fn(i32) -> String) -> String {
    (0..n).map(of).collect::<Vec<_>>().join(", ")
}

/// Fills the tables of `prefix` as `TABLES` says.
fn fill(s: &Session, prefix: &str) {
    let key = |k: i32, m: i32| if k % 97 == 13 { "NULL".to_owned() } else { (k % m).to_string() };
    let insert = |table: &str, rows: &str| s.run(&format!("INSERT INTO {prefix}{table} VALUES {rows}")).unwrap();
    insert("A", &values(600, &|k| format!("({}, {}, 's{}')", key(k, 300), k % 7, k % 30)));
    insert("B", &values(600, &|k| format!("({}, {})", key(k * 7, 400), k % 40)));
    insert("C", &values(40, &|k| format!("({}, {}, 'S{}')", key(k, 40), k % 5, k % 30)));
    insert("D", &values(8, &|k| format!("({}, {})", k, k % 3)));
    insert("Z", "(1, 10, 2), (2, 7, 0), (3, NULL, 1), (500, 4, 4)");
    insert("T", "(1, 10, 'a'), (1, 10, 'a'), (2, 20, 'b'), (NULL, 30, NULL), (NULL, 30, NULL), (3, NULL, 'a '), (0, 5, 'c')");
    insert("U", "(1, 10, 'a'), (2, 21, 'b'), (NULL, 30, NULL), (4, 40, 'd'), (3, NULL, 'a')");
}

/// Schema Q: the tables of `TABLES` in the row store, and with the
/// prefix `COLUMNS` in the column store, filled alike, and indexes on
/// A's and B's K.
pub fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let rows_of: Vec<String> = TABLES.iter().map(|t| t.replace("{P}", "")).collect();
    let columns_of: Vec<String> = TABLES.iter().map(|t| format!("{}{COLUMN_STORE}", t.replace("{P}", COLUMNS))).collect();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION Q {} {}", rows_of.join(" "), columns_of.join(" "))).unwrap();
    let s = db.session("Q");
    fill(&s, "");
    fill(&s, COLUMNS);
    s.commit().unwrap();
    for prefix in ["", COLUMNS] {
        db.create_index(&format!("{prefix}A_K"), &format!("Q.{prefix}A"), &["K"], "btree").unwrap();
        db.create_index(&format!("{prefix}B_K"), &format!("Q.{prefix}B"), &["K"], "btree").unwrap();
    }
    db.maintain().unwrap();
    (db, s)
}

/// `sql` with each table name written in braces, `{A}`, given `prefix`.
pub fn of_store(sql: &str, prefix: &str) -> String {
    let mut out = String::new();
    let mut rest = sql;
    while let Some((before, after)) = rest.split_once('{') {
        let (name, tail) = after.split_once('}').unwrap_or((after, ""));
        out.push_str(before);
        out.push_str(prefix);
        out.push_str(name);
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// The rows of `sql`, sorted unless it orders them, and its warning; or
/// its error.
fn outcome(s: &Session, sql: &str) -> String {
    match rows(s, sql) {
        Ok(mut found) => {
            if !sql.contains("ORDER BY") {
                found.sort();
            }
            format!("{found:?} {:?}", s.warning())
        }
        Err(code) => format!("{code:?}"),
    }
}

/// Turns the rule `name` on or off for `s`.
pub fn rule(s: &Session, name: &str, on: bool) {
    exec(s, &format!("SET OPTIMIZER.{} = '{}'", name.to_uppercase(), if on { "ON" } else { "OFF" })).unwrap();
}

/// The plan EXPLAIN shows of `sql`.
pub fn plan(s: &Session, sql: &str) -> String {
    s.explain(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Whether the rule `name` rewrote `sql`'s plan.
pub fn rewritten(s: &Session, name: &str, sql: &str) -> bool {
    plan(s, sql).lines().any(|l| l.starts_with("-- rules:") && l.contains(name))
}

/// The outcome of `sql`, alike with the rule `name` on and off, by
/// batches and row by row.
pub fn compared(s: &Session, name: &str, sql: &str) -> String {
    let on = outcome(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let rows_on = outcome(s, sql);
    rule(s, name, false);
    let rows_off = outcome(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    let off = outcome(s, sql);
    rule(s, name, true);
    assert_eq!(on, off, "{sql}");
    assert_eq!(rows_on, rows_off, "{sql}, row by row");
    assert_eq!(on, rows_on, "{sql}, by batches and not");
    on
}

/// Of each store, the outcome of `sql`, alike with the rule `name` on and
/// off, the rule rewriting it if `rewrites`, else leaving it.
fn of_each(s: &Session, (name, rewrites): (&str, bool), sql: &str) -> Vec<String> {
    ["", COLUMNS]
        .iter()
        .map(|prefix| {
            let query = of_store(sql, prefix);
            assert_eq!(rewritten(s, name, &query), rewrites, "{query}\n{}", plan(s, &query));
            compared(s, name, &query)
        })
        .collect()
}

/// Of each store, the outcome of `sql`, which the rule `name` rewrites,
/// alike with it on and off.
pub fn alike(s: &Session, name: &str, sql: &str) -> Vec<String> {
    of_each(s, (name, true), sql)
}

/// Of each store, the outcome of `sql`, which the rule `name` leaves,
/// alike with it on and off.
pub fn left(s: &Session, name: &str, sql: &str) -> Vec<String> {
    of_each(s, (name, false), sql)
}
