//! Spilling (RULES.md D38): operators that hold more rows than WORK_MEMORY
//! spill them to areas of the database's spill provider, reserved of the
//! memory budget, and give the rows, in the order, they give in memory.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, Scratch, exec, os, rows};

/// The smallest WORK_MEMORY, 64 KiB.
const SMALL: usize = 64 << 10;
/// Rows of each table: some thousands, many times the smallest work area.
const ROWS: usize = 3000;
/// Rows of `V`, half of them of one key: more than the smallest work area
/// holds.
const SKEWED: usize = 1200;

fn bytes(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0))
}

/// Schema `S` with tables `T` and `U`, each `ROWS` rows: `K` from 0, `G` of
/// 1000 values and `S` of 701 in `T`, of 2500 in `U`; in `U` a null `G` every 100th
/// row; `V` of `SKEWED` rows, `C` 0 in every other one; and `W` of four,
/// 0 to 3.
fn filled(db: &Database) {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE T (K INTEGER, G INTEGER, S CHARACTER(60))
           CREATE TABLE U (K INTEGER, G INTEGER, S CHARACTER(60))
           CREATE TABLE V (K INTEGER, C INTEGER, S CHARACTER(60))
           CREATE TABLE W (K INTEGER)",
    )
    .unwrap();
    let s = db.session("S");
    let chunks: Vec<Vec<usize>> = (0..ROWS).collect::<Vec<_>>().chunks(100).map(<[usize]>::to_vec).collect();
    for chunk in chunks {
        let t: Vec<String> = chunk.iter().map(|k| format!("({k}, {}, 'S{:05}')", k % 1000, (k * 7) % 701)).collect();
        let u: Vec<String> = chunk.iter().map(|&k| u_row(k)).collect();
        s.run(&format!("INSERT INTO T VALUES {}", t.join(", "))).unwrap();
        s.run(&format!("INSERT INTO U VALUES {}", u.join(", "))).unwrap();
    }
    let v: Vec<String> = (0..SKEWED).map(|k| format!("({k}, {}, 'V{k:05}')", if k % 2 == 0 { 0 } else { k })).collect();
    s.run(&format!("INSERT INTO V VALUES {}", v.join(", "))).unwrap();
    s.run("INSERT INTO W VALUES (0), (1), (2), (3)").unwrap();
}

/// Row `k` of `U`, as a row value constructor.
fn u_row(k: usize) -> String {
    if k.is_multiple_of(100) { format!("({k}, NULL, 'S{k:05}')") } else { format!("({k}, {}, 'S{:05}')", (k * 3) % 1000, k % 2500) }
}

/// Queries whose operators hold their rows: a sort with ties, grouping,
/// DISTINCT, a hash join, a product, outer joins with keys and without,
/// the set operations, subqueries kept and decorrelated, and DISTINCT set
/// functions.
const QUERIES: &[&str] = &[
    "SELECT G, K FROM T ORDER BY G DESC",
    "SELECT S, G, K FROM T ORDER BY S, G",
    "SELECT G, COUNT(*), SUM(K), MIN(S) FROM T GROUP BY G",
    "SELECT S, COUNT(DISTINCT G) FROM T GROUP BY S",
    "SELECT S, K, COUNT(*), MAX(G) FROM U GROUP BY S, K",
    "SELECT DISTINCT S FROM U",
    "SELECT DISTINCT G, S FROM T",
    "SELECT T.K, U.K FROM T, U WHERE T.G = U.G",
    "SELECT T.K, V.K FROM T, V WHERE T.K = V.C",
    "SELECT W.K, T.K FROM W, T",
    "SELECT T.K, U.K FROM T LEFT OUTER JOIN U ON T.G = U.G",
    "SELECT T.K, U.K FROM T RIGHT OUTER JOIN U ON T.G = U.G AND T.K < 2000",
    "SELECT T.K, U.K FROM T FULL OUTER JOIN U ON T.G = U.G AND U.K > 100",
    "SELECT T.K, V.K FROM T FULL OUTER JOIN V ON T.K = V.C",
    "SELECT U.K, T.K FROM U LEFT OUTER JOIN T ON U.G = T.G",
    "SELECT W.K, T.K FROM W LEFT OUTER JOIN T ON W.K + 2990 < T.K",
    "SELECT W.K, T.K FROM W FULL OUTER JOIN T ON W.K + 2995 < T.K",
    "SELECT G FROM T EXCEPT SELECT G FROM U",
    "SELECT G FROM T EXCEPT ALL SELECT G FROM U",
    "SELECT G FROM T INTERSECT SELECT G FROM U",
    "SELECT G FROM T INTERSECT ALL SELECT G FROM U",
    "SELECT G FROM T UNION SELECT G FROM U",
    "SELECT S FROM U EXCEPT SELECT S FROM T",
    "SELECT S, G FROM U EXCEPT ALL SELECT S, G FROM T",
    "SELECT S FROM T INTERSECT SELECT S FROM U",
    "SELECT S FROM U INTERSECT ALL SELECT S FROM U WHERE K > 500",
    "SELECT K FROM W WHERE K + 3 IN (SELECT G FROM U)",
    "SELECT K FROM T WHERE S IN (SELECT S FROM U)",
    "SELECT K FROM T WHERE G >= ALL (SELECT G FROM U WHERE U.K < 2900)",
    "SELECT K FROM T WHERE EXISTS (SELECT * FROM U WHERE U.G = T.G AND U.K > 10)",
    "SELECT K FROM W WHERE K < ALL (SELECT G FROM U WHERE U.K > 100)",
    "SELECT K FROM W WHERE UNIQUE (SELECT S FROM U)",
    "SELECT K FROM W WHERE K MATCH UNIQUE (SELECT G FROM T)",
    "SELECT COUNT(DISTINCT S), COUNT(DISTINCT G), COUNT(*) FROM T",
];

/// Verifies: [DESIGN] RULES.md D38: with WORK_MEMORY at its least, a sort,
/// grouping, DISTINCT, a hash join, a product, LEFT, RIGHT and FULL outer
/// joins on keys and on no keys [ISO/IEC 9075:1992 §7.5], EXCEPT,
/// INTERSECT and UNION, the rows of subqueries, kept and decorrelated, for
/// IN, EXISTS, ALL, UNIQUE and MATCH, and the values of DISTINCT set
/// functions spill to
/// areas of WORK_MEMORY's bytes, of the provider given (`set_spill`), and
/// give the rows, in the order, they give in memory, whatever SPILL_BATCH,
/// SPILL.PARTITIONS and SPILL.PARTITION_DEPTH;
/// the areas' memory goes back to the budget once the query is read.
#[test]
fn operators_spill_and_give_the_same_rows() {
    let db = Database::new();
    filled(&db);
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let s = db.session("S");
    let in_memory: Vec<Vec<String>> = QUERIES.iter().map(|q| rows(&s, q).unwrap()).collect();
    assert_eq!(counted.taken(), 0, "the default WORK_MEMORY holds them all");
    let reserved = db.memory().unwrap().1;

    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    // The default batch, then the least, with which a join partitions most;
    // then two partitions, never partitioned again, built in tables.
    for (batch, partitions, depth) in [(16 << 10, 64, 8), (4 << 10, 64, 8), (16 << 10, 2, 0)] {
        db.set_setting("SPILL_BATCH", &bytes(batch)).unwrap();
        db.set_setting("SPILL.PARTITIONS", &bytes(partitions)).unwrap();
        db.set_setting("SPILL.PARTITION_DEPTH", &bytes(depth)).unwrap();
        for (q, expected) in QUERIES.iter().zip(&in_memory) {
            assert_eq!(&rows(&s, q).unwrap(), expected, "{q}, batch {batch}");
            assert!(counted.taken() > 0, "{q} spilled");
            assert_eq!(db.memory().unwrap().1, reserved, "{q}: its areas given back");
        }
    }
    assert!(counted.0.lock().unwrap().iter().all(|&b| b == SMALL));
    assert_eq!(db.set_setting("WORK_MEMORY", &bytes(SMALL - 1)).err().map(|e| e.code), Some(Code::Api), "below its least");
}

/// Verifies: [DESIGN] RULES.md D38: a spill area is reserved of the memory
/// budget: one the budget can grant is made, one it cannot is refused
/// (SQLCODE -918), and the query with it; under the default WORK_MEMORY,
/// which holds its rows, the same query needs no area.
#[test]
fn a_spill_area_past_the_budget_is_refused() {
    const SORT: &str = "SELECT S, K FROM T ORDER BY S";
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    let in_memory = rows(&s, SORT).unwrap();
    let reserved = db.memory().unwrap().1;
    assert!(reserved >= 1 << 20, "the engine's pool");
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    db.set_setting("MEMORY_BUDGET", &bytes(reserved + (1 << 20))).unwrap();
    assert_eq!(rows(&s, SORT).unwrap(), in_memory, "an area the budget grants");
    db.set_setting("MEMORY_BUDGET", &bytes(1 << 20)).unwrap();
    assert_eq!(rows(&s, SORT).err(), Some(Code::Resources), "one it cannot");
    db.reset_setting("WORK_MEMORY").unwrap();
    assert_eq!(rows(&s, SORT).unwrap(), in_memory, "no area needed");
}

/// Verifies: [DESIGN] RULES.md D38: a database kept in a directory keeps
/// its spill areas' files in `tmp/` there, each removed once its operator
/// is done, and removes any left there when it opens.
#[test]
fn a_kept_database_spills_to_its_directory() {
    let scratch = Scratch::new("spilling");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    filled(&db);
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    let s = db.session("S");
    let mut c = s.open_cursor("SELECT S, K FROM T ORDER BY S").unwrap();
    assert!(c.fetch().unwrap().is_some());
    assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 1, "a sort's area in tmp/ while it is read");
    drop(c);
    assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0, "and removed after");
    drop(s);
    db.close().unwrap();
    drop(db);
    std::fs::write(dir.join("tmp").join("left.spill"), b"stale").unwrap();
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0, "a file left is removed");
}

/// Verifies: [DESIGN] RULES.md D36, D38: WORK_MEMORY is a session's to set:
/// its statements spill in work areas of its size, another session's in
/// the database's, which hold them; SET WORK_MEMORY TO DEFAULT takes the
/// database's again. The rows are the same either way.
#[test]
fn a_session_sets_its_own_work_memory() {
    let db = Database::new();
    filled(&db);
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let (small, other) = (db.session("S"), db.session("S"));
    let sort = "SELECT S, K FROM T ORDER BY S";
    exec(&small, &format!("SET WORK_MEMORY = {SMALL}")).unwrap();
    let spilled = rows(&small, sort).unwrap();
    let sizes: Vec<usize> = counted.0.lock().unwrap().drain(..).collect();
    assert!(!sizes.is_empty() && sizes.iter().all(|&b| b == SMALL), "{sizes:?}");
    assert_eq!(rows(&other, sort).unwrap(), spilled, "the same rows");
    assert_eq!(counted.taken(), 0, "another session's in the database's work area");
    exec(&small, "SET WORK_MEMORY TO DEFAULT").unwrap();
    assert_eq!(rows(&small, sort).unwrap(), spilled);
    assert_eq!(counted.taken(), 0, "the database's again");
}
