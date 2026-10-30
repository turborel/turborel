//! Spilling (RULES.md D38): operators that hold more rows than WORK_MEMORY
//! spill them to areas of the database's spill provider, reserved of the
//! memory budget, and give the rows, in the order, they give in memory.

use turborel::db::{Completion, Cursor, Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

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
/// 1000 values and `S` of 701 in `T`, of 2505 in `U`; in `U` a null `G` every 100th
/// row, its `S` from its `K`; `V` of `SKEWED` rows, `C` 0 in every other one; and `W` of four,
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

/// The rows of `T`, in the order a scan reads them.
const SCANNED: &str = "SELECT K, S FROM T";

/// `row`, as `rows` shows it.
fn shown(row: &[Value]) -> String {
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

/// Every row `c` has still to give, as `rows` shows them.
fn fetch_all(c: &mut Cursor) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(r) = c.fetch().unwrap() {
        out.push(shown(&r));
    }
    out
}

/// A database, filled, spilling to a counted provider, and a session of
/// it.
fn counted_database() -> (Database, Counted, Session) {
    let db = Database::new();
    filled(&db);
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let s = db.session("S");
    (db, counted, s)
}

/// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.8 GR3b]. Regression
/// test (R-TEST-7): a cursor held every row it read ahead of its fetches
/// in memory. With WORK_MEMORY at its least, a cursor whose plan can raise
/// an error, its rows read at OPEN, holds them in a spill area, and gives
/// the rows it gives in memory; one read as it is fetched from holds none,
/// until its transaction changes the table it reads: it then reads the
/// rest so, spilling, and gives them as they were before the change.
#[test]
fn a_cursor_holds_its_rows_by_spilling() {
    let (db, counted, s) = counted_database();
    let raising = "SELECT K + 1, S FROM T";
    let (all_raising, all_scanned) = (rows(&s, raising).unwrap(), rows(&s, SCANNED).unwrap());
    assert_eq!(counted.taken(), 0, "the default WORK_MEMORY holds them");
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    let mut c = s.open_cursor(raising).unwrap();
    assert!(counted.taken() > 0, "read at OPEN, and spilled");
    assert_eq!(fetch_all(&mut c), all_raising);
    drop(c);
    let mut c = s.open_cursor(SCANNED).unwrap();
    let mut read: Vec<String> = (0..10).map(|_| shown(&c.fetch().unwrap().unwrap())).collect();
    assert_eq!(counted.taken(), 0, "read as it is fetched from");
    s.run("INSERT INTO T VALUES (9999, 0, 'new')").unwrap();
    assert!(counted.taken() > 0, "the rest read before T changed, and spilled");
    read.extend(fetch_all(&mut c));
    assert_eq!(read, all_scanned, "the row inserted not among them");
}

/// Verifies: [DESIGN] RULES.md D38: a cursor whose rows cannot be held,
/// its spill area past the memory budget, fails: at OPEN if it reads them
/// there (SQLCODE -918); else, reading the rest before its transaction
/// changes the table, it gives the rows it held, then the error, then no
/// more.
#[test]
fn a_cursor_past_the_budget_fails() {
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    db.set_setting("MEMORY_BUDGET", &bytes(1 << 20)).unwrap();
    assert_eq!(s.open_cursor("SELECT K + 1, S FROM T").err().map(|e| e.code), Some(Code::Resources), "at OPEN");
    let mut c = s.open_cursor(SCANNED).unwrap();
    assert!(c.fetch().unwrap().is_some());
    s.run("INSERT INTO T VALUES (9999, 0, 'new')").unwrap();
    let mut held = 0_usize;
    let failed = loop {
        match c.fetch() {
            Ok(Some(_)) => held += 1,
            Ok(None) => break None,
            Err(e) => break Some(e.code),
        }
    };
    assert_eq!(failed, Some(Code::Resources));
    assert!(held > 0 && held < ROWS, "{held} rows held before it");
    assert!(c.fetch().unwrap().is_none(), "no more after it");
}

/// K_OUT, a FETCH's target.
fn target() -> Params {
    Params::new().declare("K_OUT", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null)
}

/// The K that `FETCH how FROM C` gives, none for no data.
fn fetched(s: &Session, p: &mut Params, how: &str) -> Option<String> {
    match s.execute(&format!("FETCH {how} FROM C INTO K_OUT"), p).unwrap() {
        Completion::NoData => None,
        _ => Some(p.value("K_OUT").to_string()),
    }
}

/// Verifies: [DESIGN] RULES.md D38 [ISO/IEC 9075:1992 §13.3 GR2–GR5].
/// Regression test (R-TEST-7): a SCROLL cursor kept every row it read in
/// memory. With WORK_MEMORY at its least, it keeps them in a spill area,
/// and moves to the row each orientation names, as it does in memory:
/// NEXT, PRIOR, FIRST, LAST, ABSOLUTE and RELATIVE, back and forth across
/// the rows it spilled.
#[test]
fn a_scroll_cursor_keeps_its_rows_by_spilling() {
    let (db, counted, s) = counted_database();
    let keys = rows(&s, "SELECT K FROM T").unwrap();
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    s.declare_cursor("DECLARE C SCROLL CURSOR FOR SELECT K FROM T").unwrap();
    let mut p = target();
    s.execute("OPEN C", &mut p).unwrap();
    assert_eq!(counted.taken(), 0, "nothing read yet");
    let at = |i: usize| Some(keys[i].clone());
    assert_eq!(fetched(&s, &mut p, "NEXT"), at(0));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE 2000"), at(1999));
    assert!(counted.taken() > 0, "the rows read kept in a spill area");
    assert_eq!(fetched(&s, &mut p, "PRIOR"), at(1998));
    assert_eq!(fetched(&s, &mut p, "RELATIVE -1500"), at(498));
    assert_eq!(fetched(&s, &mut p, "FIRST"), at(0));
    assert_eq!(fetched(&s, &mut p, "LAST"), at(ROWS - 1));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE -3000"), at(0));
    assert_eq!(fetched(&s, &mut p, "RELATIVE 2999"), at(ROWS - 1));
    assert_eq!(fetched(&s, &mut p, "NEXT"), None, "after the last");
    assert_eq!(fetched(&s, &mut p, "PRIOR"), at(ROWS - 1));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE 1500"), at(1499));
    assert_eq!(fetched(&s, &mut p, "RELATIVE 0"), at(1499));
    assert_eq!(fetched(&s, &mut p, "NEXT"), at(1500));
    for i in (0..ROWS).rev().step_by(97) {
        assert_eq!(fetched(&s, &mut p, &format!("ABSOLUTE {}", i + 1)), at(i), "row {i}");
    }
}

/// Verifies: [DESIGN] RULES.md D38 [ISO/IEC 9075:1992 §13.1 GR3, §13.9].
/// Regression test (R-TEST-7): an updatable cursor sorted its rows in
/// memory. With WORK_MEMORY at its least, its rows, in its order, are
/// sorted at OPEN in a work area that spills; it gives them in the order it
/// gives in memory, and a positioned UPDATE changes the row it is on.
#[test]
fn an_updatable_cursor_sorts_its_rows_by_spilling() {
    let (db, counted, s) = counted_database();
    // Ordered by G, of ties, a key of the row each FETCH gives with K.
    s.declare_cursor("DECLARE C CURSOR FOR SELECT G, K FROM T ORDER BY G DESC FOR UPDATE OF S").unwrap();
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let next = |p: &mut Params| match s.execute("FETCH NEXT FROM C INTO G_OUT, K_OUT", p).unwrap() {
        Completion::NoData => None,
        _ => Some((p.value("G_OUT").to_string(), p.value("K_OUT").to_string())),
    };
    let mut orders = Vec::new();
    for small in [false, true] {
        if small {
            db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
        }
        let mut p = target().declare("G_OUT", integer, Value::Null);
        s.execute("OPEN C", &mut p).unwrap();
        assert_eq!(counted.taken() > 0, small, "sorted by spilling");
        let order: Vec<Option<(String, String)>> = (0..ROWS).map(|_| next(&mut p)).collect();
        assert!(order.iter().all(Option::is_some));
        s.execute("UPDATE T SET S = 'last' WHERE CURRENT OF C", &mut p).unwrap();
        assert_eq!(next(&mut p), None);
        s.execute("CLOSE C", &mut p).unwrap();
        let (_, last) = order.last().cloned().flatten().unwrap();
        assert_eq!(rows(&s, "SELECT K FROM T WHERE S = 'last'").unwrap(), [last]);
        s.rollback().unwrap();
        orders.push(order);
    }
    assert_eq!(orders[0], orders[1]);
}

/// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.5 GR1, §8.7 GR3, §8.12
/// GR1; ISO/IEC 9075:1992 §13.10 GR6]. Regression test (R-TEST-7): a
/// searched UPDATE or DELETE, and an INSERT of a query, held every row it
/// chose in memory. With WORK_MEMORY at its least, each holds them in a
/// spill area, every one chosen before any is changed, and changes the
/// rows it changes in memory: an INSERT of the table's own rows adds each
/// once, an UPDATE of the key it chooses rows by changes each once.
#[test]
fn writes_hold_their_rows_by_spilling() {
    let mut tables = Vec::new();
    for small in [false, true] {
        let (db, counted, s) = counted_database();
        // Its changes held in memory, so the statements' rows alone spill.
        s.run("ALTER TABLE T SET (TRANSACTIONS.HELD_BYTES = 67108864)").unwrap();
        if small {
            db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
        }
        for write in [
            "INSERT INTO T SELECT K + 3000, G, S FROM T",
            "UPDATE T SET K = K + 10000, S = 'u' WHERE G < 500 AND K < 10000",
            "DELETE FROM T WHERE K >= 10000 AND G < 250",
        ] {
            s.run(write).unwrap();
            assert_eq!(counted.taken() > 0, small, "{write}");
        }
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), ["4500"]);
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K >= 20000").unwrap(), ["0"], "each row updated once");
        tables.push(crate::common::sorted(&s, "SELECT K, G, S FROM T"));
    }
    assert_eq!(tables[0], tables[1]);
}
