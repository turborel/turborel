//! ORDER BY a column under a collation of NO PAD that orders strings as
//! their octets do [ISO/IEC 9075-2:2023 §4.3.2, §10.10; RULES.md D1185]:
//! the sort reads the column in place, by octets, through each way a sort
//! runs, and gives the order the strings' octets give; where it cannot,
//! it sorts by the key made, to the same order.

use std::cmp::Ordering;

use turborel::db::{Completion, Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{exec, rows};

/// The rows written to each table.
const ROWS: usize = 6000;

/// The strings of column S. Unpadded they are in this order; padded the
/// first two are equal, the third and fifth are, and the fourth is below
/// the third.
const STRINGS: [&str; 7] = ["", "  ", "a", "a\t", "a ", "ab", "b"];

/// What follows the long start of column U's strings, past the octets a
/// sort's key holds of a string.
const ENDS: [&str; 4] = ["", " ", "\t", "x"];

/// S of row `k`: null every 11th.
fn s_of(k: usize) -> Option<String> {
    (!k.is_multiple_of(11)).then(|| STRINGS[k % 7].to_owned())
}

/// U of row `k`: alike in its first 40 octets, then one of 50 numbers and
/// one of `ENDS`.
fn u_of(k: usize) -> String {
    format!("a start that forty octets of it are alike {:02}{}", (k * 7) % 50, ENDS[k % 4])
}

/// Tables R, of the row store, and C, of the column store, alike: S and U
/// under BYTES, of NO PAD from UCS_BASIC, G one of five numbers, F a
/// DECFLOAT, P padded. Written, then frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let columns = "(K INTEGER PRIMARY KEY, S VARCHAR(20) COLLATE A.BYTES, U VARCHAR(80) COLLATE A.BYTES, G INTEGER, F DECFLOAT(16), \
                   P VARCHAR(20))";
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
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k| format!("({k}, {}, {}, {}, {}, {})", text(s_of(k)), text(Some(u_of(k))), k % 5, k % 3, text(s_of(k))))
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
    }
    db.maintain().unwrap();
    (db, a)
}

/// The order of two strings by their octets, a null last, or first if
/// `nulls_first`; reversed, but for the nulls' place, if `descending`.
fn by_octets(a: Option<&String>, b: Option<&String>, (descending, nulls_first): (bool, bool)) -> Ordering {
    let null = if nulls_first { Ordering::Less } else { Ordering::Greater };
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => null,
        (Some(_), None) => null.reverse(),
        (Some(x), Some(y)) => {
            let up = x.as_bytes().cmp(y.as_bytes());
            if descending { up.reverse() } else { up }
        }
    }
}

/// The keys K of every row, in the order `order` gives two rows' numbers.
fn modeled(order: impl Fn(usize, usize) -> Ordering) -> Vec<String> {
    let mut ks: Vec<usize> = (0..ROWS).collect();
    ks.sort_by(|&a, &b| order(a, b).then(a.cmp(&b)));
    ks.iter().map(ToString::to_string).collect()
}

/// The plan `s` makes of `sql`, as optimized.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &Params::new()).unwrap();
    plan.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The ways a sort runs: by batches, a row at a time, in parallel, and
/// past its work area; each the settings of a session of its own.
const WAYS: [&[&str]; 4] = [
    &[],
    &["SET EXECUTOR.BATCHES = 'OFF'"],
    &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"],
    &["SET MEMORY.WORK_BYTES = 65536"],
];

/// The rows of `sql` of each table, each way a sort runs, asserted to be
/// `wanted`.
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

/// Verifies: [DESIGN] RULES.md D1185: a sort by a column under a collation
/// of NO PAD by octets reads the column, its projection giving null where
/// the key was made, of all the rows and of the first; a sort by a padded
/// column is as it was; the key stays made under FETCH FIRST WITH TIES,
/// which reads it; and an expression computed under the sort is a column
/// to it, read in place too.
#[test]
fn a_sort_by_octets_reads_its_column_in_place() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for tail in ["", " FETCH FIRST 5 ROWS ONLY", " OFFSET 2 ROWS FETCH FIRST 5 ROWS ONLY"] {
            let plan = physical(&a, &format!("SELECT K FROM {table} ORDER BY S{tail}"));
            assert!(plan.contains(&format!("Sort {table}.S COLLATE UCS_BASIC")), "{plan}");
            assert!(plan.contains(&format!("Project {table}.K, {table}.S, NULL")), "{plan}");
        }
        let padded = physical(&a, &format!("SELECT K FROM {table} ORDER BY P FETCH FIRST 5 ROWS ONLY"));
        assert!(!padded.contains("COLLATE") && !padded.contains("NULL"), "{padded}");
        let tied = physical(&a, &format!("SELECT K FROM {table} ORDER BY S FETCH FIRST 5 ROWS WITH TIES"));
        assert!(tied.contains(&format!("Project {table}.K, {table}.S, {table}.S COLLATE UCS_BASIC")), "{tied}");
        let computed = physical(&a, &format!("SELECT K FROM {table} ORDER BY S || 'x' FETCH FIRST 5 ROWS ONLY"));
        assert!(computed.contains("COLLATE UCS_BASIC") && computed.contains(", NULL"), "{computed}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §10.10 GR1] RULES.md
/// D1185: a sort by octets read in place gives the order of the strings'
/// octets, a prefix first, a tab after `'a'` below a space after it, of
/// every row and of the first few, with an offset, ascending and
/// descending under each null ordering; as the first key and as the
/// second; of strings alike in the octets a key holds, told apart whole;
/// beside a key of no normal form; by batches, a row at a time, in
/// parallel and past its work area; of the row store and the column
/// store.
#[test]
fn a_sort_by_octets_orders_as_the_octets_do_10_10() {
    let (db, _a) = tables();
    let s = |k: usize| s_of(k);
    let u = |k: usize| Some(u_of(k));
    let up = modeled(|x, y| by_octets(s(x).as_ref(), s(y).as_ref(), (false, false)));
    every_way(&db, "SELECT K FROM {t} ORDER BY S, K", &up);
    every_way(&db, "SELECT K FROM {t} ORDER BY S, K FETCH FIRST 700 ROWS ONLY", &up[..700]);
    every_way(&db, "SELECT K FROM {t} ORDER BY S, K OFFSET 420 ROWS FETCH FIRST 30 ROWS ONLY", &up[420..450]);
    let down = modeled(|x, y| by_octets(s(x).as_ref(), s(y).as_ref(), (true, false)));
    every_way(&db, "SELECT K FROM {t} ORDER BY S DESC NULLS LAST, K", &down);
    every_way(&db, "SELECT K FROM {t} ORDER BY S DESC NULLS LAST, K FETCH FIRST 40 ROWS ONLY", &down[..40]);
    let first = modeled(|x, y| by_octets(s(x).as_ref(), s(y).as_ref(), (false, true)));
    every_way(&db, "SELECT K FROM {t} ORDER BY S NULLS FIRST, K FETCH FIRST 600 ROWS ONLY", &first[..600]);
    let long = modeled(|x, y| by_octets(u(x).as_ref(), u(y).as_ref(), (false, false)));
    every_way(&db, "SELECT K FROM {t} ORDER BY U, K", &long);
    every_way(&db, "SELECT K FROM {t} ORDER BY U, K FETCH FIRST 45 ROWS ONLY", &long[..45]);
    let long_down = modeled(|x, y| by_octets(u(x).as_ref(), u(y).as_ref(), (true, false)));
    every_way(&db, "SELECT K FROM {t} ORDER BY U DESC, K FETCH FIRST 45 ROWS ONLY", &long_down[..45]);
    let second = modeled(|x, y| (x % 5).cmp(&(y % 5)).then(by_octets(s(x).as_ref(), s(y).as_ref(), (true, true))));
    every_way(&db, "SELECT K FROM {t} ORDER BY G, S DESC, K", &second);
    every_way(&db, "SELECT K FROM {t} ORDER BY G, S DESC, K FETCH FIRST 50 ROWS ONLY", &second[..50]);
    let beside = modeled(|x, y| by_octets(s(x).as_ref(), s(y).as_ref(), (false, false)).then((x % 3).cmp(&(y % 3))));
    every_way(&db, "SELECT K FROM {t} ORDER BY S, F, K", &beside);
    every_way(&db, "SELECT K FROM {t} ORDER BY S, F, K FETCH FIRST 50 ROWS ONLY", &beside[..50]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR7d i, §4.3.2]
/// RULES.md D1185: FETCH FIRST WITH TIES under a collation of NO PAD by
/// octets, whose sort keeps the key made, keeps the peers by octets alone:
/// the rows of the empty string and not those of two spaces, its equal
/// padded; and a sort by an expression under the collation orders by the
/// octets too.
#[test]
fn ties_and_expressions_sort_by_octets_7_17() {
    let (db, a) = tables();
    let s = |k: usize| s_of(k);
    let empty: Vec<String> = (0..ROWS).filter(|&k| s(k).as_deref() == Some("")).map(|k| k.to_string()).collect();
    for table in ["R", "C"] {
        let mut tied = rows(&a, &format!("SELECT K FROM {table} ORDER BY S FETCH FIRST 1 ROW WITH TIES")).unwrap();
        tied.sort_by_key(|k| k.parse::<usize>().unwrap());
        assert_eq!(tied, empty, "{table}");
    }
    let with = |k: usize| s(k).map(|s| format!("{s}x"));
    let computed = modeled(|x, y| by_octets(with(x).as_ref(), with(y).as_ref(), (false, false)));
    every_way(&db, "SELECT K FROM {t} ORDER BY S || 'x', K FETCH FIRST 900 ROWS ONLY", &computed[..900]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §10.10; ISO/IEC
/// 9075:1992 §13.3] RULES.md D1185: a SCROLL cursor FOR UPDATE, whose rows
/// are found by their sort keys, moves among them in the order of the
/// strings' octets.
#[test]
fn a_scroll_cursor_orders_by_octets_13_3() {
    let (_db, a) = tables();
    let s = |k: usize| s_of(k);
    let wanted = modeled(|x, y| by_octets(s(x).as_ref(), s(y).as_ref(), (false, false)));
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    let mut p = Params::new().declare("K_OUT", integer, Value::Null);
    a.declare_cursor("DECLARE X SCROLL CURSOR FOR SELECT K FROM R ORDER BY S, K FOR UPDATE OF G").unwrap();
    a.execute("OPEN X", &mut p).unwrap();
    let mut fetch = |how: &str| match a.execute(&format!("FETCH {how} FROM X INTO K_OUT"), &mut p) {
        Ok(Completion::NoData) => None,
        Ok(_) => Some(p.value("K_OUT").to_string()),
        Err(e) => panic!("{e:?}"),
    };
    // The empty strings end, and those of two spaces begin, where the padded order has them mixed.
    let spaces = (0..ROWS).filter(|&k| s(k).as_deref() == Some("")).count();
    for at in [1, spaces, spaces + 1, spaces + 2, 2 * spaces + 1, ROWS - 7, ROWS] {
        assert_eq!(fetch(&format!("ABSOLUTE {at}")), Some(wanted[at - 1].clone()), "row {at}");
    }
}

/// The rows of table B, several groups of the column store.
const MANY: usize = 12_000;

/// W of row `k` of table B, null every 11th. Each is a string of its
/// own, the rows' strings in an order of their own, and each four are
/// alike but for one of `ENDS` after them, which octets and padding
/// order differently.
fn w_of(k: usize) -> Option<String> {
    let c = (k * 7919) % MANY;
    (!k.is_multiple_of(11)).then(|| format!("w{:04}{}", c.div_euclid(4), ENDS[c % 4]))
}

/// D of row `k` of table B: one of `STRINGS` every fourth row, so few
/// rows of a group are at or before any of them, and one string past
/// them all of the rest.
fn d_of(k: usize) -> String {
    if k.is_multiple_of(4) { STRINGS[k.div_euclid(4) % 7] } else { "zz" }.to_owned()
}

/// Verifies: [DESIGN] RULES.md D1187, D1241 [ISO/IEC 9075-2:2023 §7.17
/// GR5]: a FETCH FIRST by a column under a collation of NO PAD by
/// octets, of a column store table of several groups, tells its scan the
/// bound of its first key, a character string, and the scan judges each
/// row by it. So the scan gives fewer rows than the table holds, of a
/// column of a string a row and of one of a few strings, with a filter
/// and with none, and the rows given are the first by octets, ascending,
/// descending and past an offset of hundreds of rows, no null among them,
/// on one thread and with the bound shared by the morsels of a sort run
/// in parallel.
#[test]
fn a_top_n_by_octets_tells_its_scan_a_bound_of_strings_7_17() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE B (K INTEGER PRIMARY KEY, W VARCHAR(40) COLLATE A.BYTES, D VARCHAR(20) COLLATE A.BYTES)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let text = |s: Option<String>| s.map_or("NULL".to_owned(), |s| format!("'{s}'"));
    for first in (0..MANY).step_by(1000) {
        let values: Vec<String> = (first..first + 1000).map(|k| format!("({k}, {}, {})", text(w_of(k)), text(Some(d_of(k))))).collect();
        a.run(&format!("INSERT INTO B VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    let ordered = |of: &dyn Fn(usize) -> Option<String>, descending: bool| -> Vec<String> {
        let mut ks: Vec<usize> = (0..MANY).collect();
        ks.sort_by(|&x, &y| by_octets(of(x).as_ref(), of(y).as_ref(), (descending, false)).then(x.cmp(&y)));
        ks.iter().map(ToString::to_string).collect()
    };
    let (up, down, few) = (ordered(&w_of, false), ordered(&w_of, true), ordered(&|k| Some(d_of(k)), false));
    let cases: [(&str, &[String], usize); 6] = [
        ("SELECT K FROM B ORDER BY W FETCH FIRST 10 ROWS ONLY", &up[..10], 2),
        ("SELECT K FROM B WHERE W <> '' ORDER BY W OFFSET 5 ROWS FETCH FIRST 20 ROWS ONLY", &up[5..25], 2),
        ("SELECT K FROM B ORDER BY W DESC NULLS LAST FETCH FIRST 10 ROWS ONLY", &down[..10], 2),
        ("SELECT K FROM B WHERE K >= 0 ORDER BY W DESC NULLS LAST OFFSET 40 ROWS FETCH FIRST 9 ROWS ONLY", &down[40..49], 2),
        ("SELECT K FROM B ORDER BY W OFFSET 290 ROWS FETCH FIRST 20 ROWS ONLY", &up[290..310], 1),
        ("SELECT K FROM B ORDER BY D, K FETCH FIRST 10 ROWS ONLY", &few[..10], 2),
    ];
    let scanned = |sql: &str| {
        let analyzed = a.analyzed(sql, &Params::new()).unwrap();
        analyzed.operators.iter().find(|o| o.label.starts_with("SeqScan")).unwrap_or_else(|| panic!("no scan in\n{}", analyzed.text)).rows
    };
    for (sql, wanted, share) in cases {
        assert_eq!(rows(&a, sql).unwrap(), wanted, "{sql}");
        let given = usize::try_from(scanned(sql)).unwrap();
        assert!(given * share < MANY, "rows past the bound are left out, {given} given: {sql}");
        let shared = db.session("A");
        for setting in WAYS[2] {
            exec(&shared, setting).unwrap();
        }
        assert_eq!(rows(&shared, sql).unwrap(), wanted, "in parallel: {sql}");
    }
}
