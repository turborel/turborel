//! The first rows FETCH FIRST asks for, read alone [ISO/IEC 9075-2:2023
//! §7.17 GR5, GR7; RULES.md D31, D38]: a sort holding only those gives the
//! rows a whole sort gives first, of ONLY and WITH TIES, by batches and row
//! by row, in the work area and past it; a scan of the column store in
//! COLUMNSTORE.ORDER's order gives those a table of the row store gives,
//! of groups sorted and not and of rows not yet frozen; an input that can
//! raise an exception is read to its end (SQL-92 §3.3.4.4).

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, exec, rows};
use crate::fetch_first::{staff, state};

/// Rows of M.
const ROWS: i64 = 3000;

/// Schema A, table M of `ROWS` rows of no key: V of 50 values, every 13th
/// null, and padding P.
fn made() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (K INTEGER NOT NULL, V INTEGER, P VARCHAR(40))").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (1..=ROWS)
        .map(|k| {
            let v = if k % 13 == 0 { "NULL".to_owned() } else { (k.checked_mul(7).unwrap() % 50).to_string() };
            format!("({k}, {v}, 'padding padding padding {}')", k % 3)
        })
        .collect();
    a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    (db, a)
}

/// What OFFSET `offset` FETCH FIRST `fetch` keeps of `whole`, rows whose
/// sort key is all but their first column, and of `ties` the peers of the
/// last after them.
fn kept(whole: &[String], (offset, fetch): (usize, usize), ties: bool) -> Vec<String> {
    let key = |row: &String| row.split_once(',').map(|(_, key)| key.to_owned());
    let first: Vec<String> = whole.iter().skip(offset).take(fetch).cloned().collect();
    let Some(last) = first.last().filter(|_| ties).map(key) else { return first };
    let peers = whole.iter().skip(offset.saturating_add(fetch)).take_while(|r| key(r) == last).cloned();
    first.into_iter().chain(peers).collect()
}

/// The orders of M checked, each query's columns K then its keys.
const ORDERS: [&str; 3] =
    ["SELECT K, V FROM M ORDER BY V", "SELECT K, V FROM M ORDER BY V DESC NULLS FIRST", "SELECT K, V, P FROM M ORDER BY V, P DESC"];

/// The OFFSET and FETCH FIRST counts checked.
const COUNTS: [(usize, usize); 5] = [(0, 1), (0, 10), (5, 60), (2990, 20), (0, 4000)];

/// Each of `COUNTS` of OFFSET and FETCH FIRST, of ONLY and WITH TIES,
/// keeps of `q` what it keeps of `q`'s whole rows.
fn each_count_alike(a: &Session, q: &str, when: &str) {
    let whole = rows(a, q).unwrap();
    for counts @ (offset, fetch) in COUNTS {
        for (ties, words) in [(false, "ONLY"), (true, "WITH TIES")] {
            let sql = format!("{q} OFFSET {offset} ROWS FETCH FIRST {fetch} ROWS {words}");
            assert_eq!(rows(a, &sql).unwrap(), kept(&whole, counts, ties), "{when}: {sql}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR5, GR6, GR7, GR7d
/// i; RULES.md D38] a sort of which FETCH FIRST asks the first rows gives
/// those a whole sort gives first, equal rows in the order the whole sort
/// gives them, of ONLY and WITH TIES, ascending and descending, nulls first
/// and last, of more asked for than there are; holding only those, it takes
/// no spill area where the whole sort does; past the work area, it gives
/// the same; by batches and row by row; of a count given as a parameter.
#[test]
fn a_sort_of_the_first_rows_gives_a_whole_sorts_7_17() {
    let (db, a) = made();
    let spilled = Counted::default();
    db.set_spill(Box::new(spilled.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    let alike = |when: &str| {
        for batches in ["ON", "OFF"] {
            exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
            for q in ORDERS {
                each_count_alike(&a, q, &format!("{when}, batches {batches}"));
            }
        }
    };
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        rows(&a, "SELECT K, V, P FROM M ORDER BY V, P DESC FETCH FIRST 10 ROWS WITH TIES").unwrap();
    }
    assert_eq!(spilled.taken(), 0, "only the first rows held");
    alike("past the work area");
    assert!(spilled.taken() > 0, "the whole sorts spilled");
    exec(&a, "SET MEMORY.WORK_BYTES = 67108864").unwrap();
    alike("in the work area");
    let q = a.prepare("SELECT K, V FROM M ORDER BY V OFFSET ? ROWS FETCH FIRST ? ROWS ONLY").unwrap();
    let whole = rows(&a, "SELECT K, V FROM M ORDER BY V").unwrap();
    let int = |n: usize| Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0));
    for counts @ (offset, fetch) in COUNTS {
        let mut opened = a.open_prepared(&q, &[int(offset), int(fetch)]).unwrap();
        let got: Vec<String> = std::iter::from_fn(|| opened.fetch().unwrap()).map(|r| format!("{},{}", r[0], r[1])).collect();
        assert_eq!(got, kept(&whole, counts, false), "given {counts:?}");
    }
}

/// Tables R, of the row store, and O, of the column store, alike: K a key,
/// V of 40 values. O's first rows are frozen into groups sorted otherwise,
/// then O is sorted by V and K and more rows are frozen, sorted; the last
/// are left in its tree.
fn ordered(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER PRIMARY KEY, V INTEGER, P VARCHAR(20))
           CREATE TABLE O (K INTEGER PRIMARY KEY, V INTEGER, P VARCHAR(20))
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let insert = |(first, last): (i64, i64)| {
        for table in ["R", "O"] {
            let values: Vec<String> = (first..last)
                .map(|k| {
                    let v = if k % 17 == 0 { "NULL".to_owned() } else { (k.checked_mul(13).unwrap() % 40).to_string() };
                    format!("({k}, {v}, 'p{}')", k % 5)
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
    };
    insert((0, 4096));
    db.maintain().unwrap();
    a.run("ALTER TABLE O SET (COLUMNSTORE.ORDER = 'V, K')").unwrap();
    a.commit().unwrap();
    insert((4096, 8192));
    db.maintain().unwrap();
    insert((8192, 8700));
    a
}

/// Queries R and O answer alike, `{t}` the table.
const IN_ORDER: [&str; 7] = [
    "SELECT K, V FROM {t} ORDER BY V, K FETCH FIRST 7 ROWS ONLY",
    "SELECT K, V FROM {t} ORDER BY V, K OFFSET 300 ROWS FETCH FIRST 40 ROWS ONLY",
    "SELECT K, V FROM {t} ORDER BY V DESC, K DESC FETCH FIRST 25 ROWS ONLY",
    "SELECT K, P FROM {t} WHERE V > 30 ORDER BY V, K FETCH FIRST 9 ROWS ONLY",
    "SELECT K FROM {t} ORDER BY V, K OFFSET 8690 ROWS FETCH FIRST 50 ROWS ONLY",
    "SELECT V, COUNT(*) FROM (SELECT V FROM {t} ORDER BY V FETCH FIRST 1 ROW WITH TIES) X GROUP BY V",
    "SELECT V, COUNT(*) FROM (SELECT V FROM {t} ORDER BY V DESC OFFSET 200 ROWS FETCH FIRST 300 ROWS WITH TIES) X GROUP BY V ORDER BY V",
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR5, GR6, GR7] RULES.md
/// D31, D41, COLUMNSTORE.ORDER: a scan of the column store in the order
/// ORDER BY asks, told how many of its first rows are asked for, gives
/// those a table of the row store gives: of groups sorted by its order, of
/// groups sorted otherwise and of rows not yet frozen, forward and
/// backward, filtered, past all its rows, and of WITH TIES, which it is
/// not told of; by batches and row by row; no sort planned.
#[test]
fn an_ordered_scan_of_the_column_store_reads_the_first_rows_7_17() {
    let db = Database::new();
    let a = ordered(&db);
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        for q in IN_ORDER {
            assert_eq!(rows(&a, &q.replace("{t}", "O")), rows(&a, &q.replace("{t}", "R")), "batches {batches}: {q}");
        }
    }
    for q in ["SELECT K, V FROM O ORDER BY V, K FETCH FIRST 7 ROWS ONLY", "SELECT V FROM O ORDER BY V FETCH FIRST 1 ROW WITH TIES"] {
        let plan = a.explain(q, &Params::new()).unwrap();
        let physical = plan.split("-- physical:").nth(1).unwrap_or_default();
        assert!(physical.contains("Limit fetch") && !physical.contains("Sort"), "{plan}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR6, GR7; SQL-92
/// §3.3.4.4] OFFSET and FETCH FIRST over rows whose values or condition can
/// raise an exception read them to their end, so a row past those kept
/// raises it, as reading them all would; of a sort too, and in the order of
/// a column store's scan.
#[test]
fn rows_that_can_raise_are_read_to_their_end_7_17() {
    let (_db, a) = staff();
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(state(&a, "SELECT 10 / (ID - 3) FROM STAFF ORDER BY ID FETCH FIRST 1 ROW ONLY"), "22012", "batches {batches}");
        assert_eq!(state(&a, "SELECT ID FROM STAFF WHERE 10 / (ID - 4) <> 0 ORDER BY ID FETCH FIRST 1 ROW ONLY"), "22012");
        assert_eq!(state(&a, "SELECT ID, 10 / (ID - 5) FROM STAFF ORDER BY NAME OFFSET 1 ROW FETCH FIRST 1 ROW ONLY"), "22012");
        assert_eq!(rows(&a, "SELECT 10 / (ID - 3) FROM STAFF WHERE ID < 3 ORDER BY ID FETCH FIRST 1 ROW ONLY"), Ok(vec!["-5".to_owned()]));
    }
    let db = Database::new();
    let o = ordered(&db);
    let q = "SELECT 1 / (K - 8600) FROM O ORDER BY V, K FETCH FIRST 1 ROW ONLY";
    assert_eq!(state(&o, q), "22012", "a column store's scan in order");
}
