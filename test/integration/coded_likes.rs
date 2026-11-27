//! LIKE and NOT LIKE of a constant pattern judged by a column store's
//! scan of a VARCHAR column's texts where they lie, those coded by a
//! table of symbols and those of a dictionary, before the filter's other
//! columns are decoded [X3.135-1986 §5.14; RULES.md D1251, D1253]: what
//! the scan gives, and where it gives way to the filter.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value, like};

use crate::common::{exec, rows};

/// The rows written to each table, six groups of the column store's.
const MANY: usize = 12_288;

/// U of row `k`, a web address of its own, null every 11th: one row in
/// 97 holds `find-me`, one in 53 `50%`, the others `other`.
fn u_of(k: usize) -> Option<String> {
    let part = if k % 97 == 5 {
        "find-me"
    } else if k % 53 == 7 {
        "50%"
    } else {
        "other"
    };
    (!k.is_multiple_of(11)).then(|| format!("http://site{}.example/{part}/page{}", (k * 7919) % 300, (k * 104_729) % 50_000))
}

/// W of row `k`: one of five words, or of one row in fifty a sixth, a
/// dictionary's.
fn w_of(k: usize) -> String {
    if k % 50 == 3 { "a rare one" } else { ["red", "green", "blue", "a green one", "grey"][k % 5] }.to_owned()
}

/// Tables R, of the row store, and C, of the column store in groups of
/// 2,048 rows, alike: U a web address, B the same under a collation of
/// NO PAD by octets, I the same under one that folds case, F the word W
/// in a CHARACTER column, N one of a hundred numbers and P a text of
/// the row's own. Written, then frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let columns = "(K INTEGER PRIMARY KEY, U VARCHAR(80), B VARCHAR(80) COLLATE A.BYTES, I VARCHAR(80) COLLATE CASE_INSENSITIVE, \
                   W VARCHAR(20), F CHARACTER(12), N INTEGER, P VARCHAR(40))";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE R {columns}
           CREATE TABLE C {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let a = db.session("A");
    let text = |s: Option<String>| s.map_or("NULL".to_owned(), |s| format!("'{s}'"));
    for table in ["R", "C"] {
        for first in (0..MANY).step_by(512) {
            let values: Vec<String> = (first..first + 512)
                .map(|k| {
                    let (u, w) = (text(u_of(k)), w_of(k));
                    format!("({k}, {u}, {u}, {u}, '{w}', '{w}', {}, 'payload of row {k}')", k % 100)
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
    }
    db.maintain().unwrap();
    (db, a)
}

/// The keys K of the rows whose U the pattern matches, or, if `not`,
/// does not, as the predicate is true of them: none of a null.
fn modeled(pattern: &str, escape: Option<char>, not: bool, also: impl Fn(usize) -> bool) -> Vec<String> {
    (0..MANY).filter(|&k| also(k) && u_of(k).is_some_and(|u| like(&u, pattern, escape).unwrap() != not)).map(|k| k.to_string()).collect()
}

/// The keys K of the rows `keeps` is true of.
fn keys(keeps: impl Fn(usize) -> bool) -> Vec<String> {
    (0..MANY).filter(|&k| keeps(k)).map(|k| k.to_string()).collect()
}

/// The rows the scan of `sql` gave, as EXPLAIN ANALYZE counts them.
fn scanned(s: &Session, sql: &str) -> usize {
    let analyzed = s.analyzed(sql, &Params::new()).unwrap();
    let scan =
        analyzed.operators.iter().find(|o| o.label.starts_with("SeqScan")).unwrap_or_else(|| panic!("no scan in\n{}", analyzed.text));
    usize::try_from(scan.rows).unwrap()
}

/// The ways a filter over a scan runs: by batches, a row at a time, and
/// in parallel; each the settings of a session of its own.
const WAYS: [&[&str]; 3] = [
    &[],
    &["SET EXECUTOR.BATCHES = 'OFF'"],
    &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"],
];

/// Asserts `sql`, `{t}` each table in turn, gives the rows `wanted`
/// each way a filter runs.
fn every_way(db: &Database, sql: &str, wanted: &[String]) {
    for table in ["R", "C"] {
        for set in WAYS {
            let a = db.session("A");
            for setting in set {
                exec(&a, setting).unwrap();
            }
            assert_eq!(rows(&a, &sql.replace("{t}", table)).unwrap(), wanted, "{table} {set:?}: {sql}");
        }
    }
}

/// Verifies: [X3.135-1986 §5.14 GR2a, GR4, GR5] (RULES.md D1251, D1253,
/// D360) a LIKE or NOT LIKE of a constant pattern of no `_`, of a column
/// store's VARCHAR column, is judged by the scan before the
/// filter's other columns are decoded: EXPLAIN ANALYZE shows the scan
/// giving fewer than one row in four of the table, where a scan that
/// does not judge it gives every row. So of a piece between, a first
/// piece, a last, several pieces, an ESCAPE that makes `%` a character,
/// NOT LIKE, a LIKE beside another conjunct, a column under a
/// collation of NO PAD by octets, and a column of few strings, kept as a
/// dictionary; each gives the rows the predicate is
/// true of, none of a null, as the row store gives them and each way a
/// filter runs, and rows written after the freeze, which the scan gives
/// unjudged, are judged by the filter.
#[test]
fn a_like_is_judged_by_the_scan_before_other_columns_are_decoded_5_14() {
    let (db, a) = tables();
    let cases: [(&str, Vec<String>); 10] = [
        ("U LIKE '%find-me%'", modeled("%find-me%", None, false, |_| true)),
        ("U LIKE 'http://site12.%'", modeled("http://site12.%", None, false, |_| true)),
        ("U LIKE '%/page777'", modeled("%/page777", None, false, |_| true)),
        ("U LIKE '%site1%find%7'", modeled("%site1%find%7", None, false, |_| true)),
        ("U LIKE '%/50!%/%' ESCAPE '!'", modeled("%/50!%/%", Some('!'), false, |_| true)),
        ("U NOT LIKE '%other%'", modeled("%other%", None, true, |_| true)),
        ("U LIKE '%find-me%' AND N < 50", modeled("%find-me%", None, false, |k| k % 100 < 50)),
        ("B LIKE '%find-me%'", modeled("%find-me%", None, false, |_| true)),
        ("W LIKE '%rare%'", keys(|k| w_of(k).contains("rare"))),
        ("W NOT LIKE '%e%'", Vec::new()),
    ];
    for (predicate, wanted) in &cases {
        assert!(wanted.len() * 8 < MANY, "few rows match {predicate}: {}", wanted.len());
        let sql = format!("SELECT K FROM {{t}} WHERE {predicate} ORDER BY K");
        every_way(&db, &sql, wanted);
        let given = scanned(&a, &format!("SELECT K, P FROM C WHERE {predicate}"));
        // A group not yet frozen would be given whole, so a quarter of the
        // table is the most a scan that judges gives.
        assert!(given * 4 < MANY && given >= wanted.len(), "the scan judged {predicate}: {given} rows given of {MANY}");
    }
    for table in ["R", "C"] {
        a.run(&format!(
            "INSERT INTO {table} (K, U, P) VALUES (20000, 'a find-me not frozen', 'p'), (20001, 'nor this', 'p'), (20002, NULL, 'p')"
        ))
        .unwrap();
    }
    a.commit().unwrap();
    let mut with_new = modeled("%find-me%", None, false, |_| true);
    with_new.push("20000".to_owned());
    every_way(&db, "SELECT K FROM {t} WHERE U LIKE '%find-me%' ORDER BY K", &with_new);
    let mut without = modeled("%other%", None, true, |_| true);
    without.extend(["20000".to_owned(), "20001".to_owned()]);
    every_way(&db, "SELECT K FROM {t} WHERE U NOT LIKE '%other%' ORDER BY K", &without);
}

/// Verifies: [X3.135-1986 §5.14 GR2b] [ISO/IEC 9075-2:2023 §8.5 GR3]
/// (RULES.md D1251) a LIKE the scan does not judge in place gives way to
/// the filter, which gives the same rows as the row store's: a pattern
/// with `_`, a LIKE under a collation that folds case, and a CHARACTER
/// column, whose strings are padded as they are read. So does a LIKE
/// that keeps most rows, which the scan stops judging as it sees each
/// run is given whole. EXPLAIN ANALYZE shows the scan giving every row
/// of the table for each.
#[test]
fn a_like_the_scan_cannot_judge_gives_way_to_the_filter_8_5() {
    let (db, a) = tables();
    let folded = |k: usize| u_of(k).is_some_and(|u| u.to_uppercase().contains("FIND-ME"));
    let cases: [(&str, Vec<String>); 5] = [
        ("U LIKE 'http://%'", modeled("http://%", None, false, |_| true)),
        ("U LIKE '%find_me%'", modeled("%find_me%", None, false, |_| true)),
        ("I LIKE '%FIND-ME%'", keys(folded)),
        ("F LIKE '%green%'", keys(|k| w_of(k).contains("green"))),
        ("F LIKE '%green'", Vec::new()),
    ];
    for (predicate, wanted) in &cases {
        every_way(&db, &format!("SELECT K FROM {{t}} WHERE {predicate} ORDER BY K"), wanted);
        let given = scanned(&a, &format!("SELECT K, P FROM C WHERE {predicate}"));
        assert_eq!(given, MANY, "the scan gave every row for the filter to judge {predicate}");
    }
}
