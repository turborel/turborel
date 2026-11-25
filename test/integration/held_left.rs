//! \[DESIGN\] A semi join or an antijoin that builds its left, of a left
//! operand run in parallel, holds the left by the threads that read it
//! (RULES.md D1115): the rows it gives, their order and the errors it
//! raises are those it gives holding its left on one thread, within the
//! work area and past it.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables O, orders, of 5000 rows, and L, their lines, of 9000, of the
/// column store in groups of 2048, and OW and LW, of the row store,
/// alike. O has K, a key; G, one of 9, null for every 17th; T, a text of
/// G; D, a number below 1000; W, a text of 180 letters. L has K, an order's key or one no order
/// has, null for every 19th; S, one of 5; T, a text of S; A and B,
/// numbers compared. N names each G but the last, its V a text.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE O (K INTEGER PRIMARY KEY, G INTEGER, T VARCHAR(8), D INTEGER, W VARCHAR(200)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE L (K INTEGER, S INTEGER, T VARCHAR(8), A INTEGER, B INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE OW (K INTEGER PRIMARY KEY, G INTEGER, T VARCHAR(8), D INTEGER, W VARCHAR(200))
           CREATE TABLE LW (K INTEGER, S INTEGER, T VARCHAR(8), A INTEGER, B INTEGER)
           CREATE TABLE N (G INTEGER PRIMARY KEY, V VARCHAR(8))",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["O", "OW"] {
        let wide = |k: i64| format!("w{k}{}", "x".repeat(170));
        let values: Vec<String> = (0_i64..5000)
            .map(|k| format!("({k}, {}, 't{}', {}, '{}')", null_or(k % 17 == 0, k % 9), k % 5, k * 7919 % 1000, wide(k)))
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    for table in ["L", "LW"] {
        let values: Vec<String> = (0_i64..9000)
            .map(|n| format!("({}, {}, 't{}', {}, {})", null_or(n % 19 == 0, n * 31 % 6000), n % 5, n % 3, n * 13 % 50, n * 29 % 50))
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.run("INSERT INTO N VALUES (0, 'zero'), (1, 'one'), (2, 'two'), (3, 'three'), (4, 'four'), (5, 'five'), (6, 'six'), (7, 'seven')")
        .unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The shapes joined, `{o}` and `{l}` the tables, each of orders joined
/// to their G's name, so that a scan of the column store is no stream
/// that stays on one thread (RULES.md D291), and each giving the held
/// rows themselves: EXISTS and NOT EXISTS of an order's lines under a
/// condition beyond the key; NOT EXISTS on two keys, a line's K null
/// for every 19th; EXISTS on a key of texts and one of numbers.
const SHAPES: [&str; 4] = [
    "SELECT {o}.K, N.V, {o}.D FROM {o} JOIN N ON N.G = {o}.G WHERE {o}.D < 600 AND EXISTS (SELECT * FROM {l} WHERE {l}.K = {o}.K AND {l}.A < {l}.B)",
    "SELECT {o}.K, {o}.T, N.V FROM {o} JOIN N ON N.G = {o}.G WHERE {o}.D < 600 AND NOT EXISTS (SELECT * FROM {l} WHERE {l}.K = {o}.K AND {l}.A > {l}.B)",
    "SELECT {o}.K, N.V FROM {o} JOIN N ON N.G = {o}.G WHERE {o}.D < 500 AND NOT EXISTS (SELECT * FROM {l} WHERE {l}.K = {o}.K AND {l}.S = {o}.G AND {l}.A < 45)",
    "SELECT {o}.K, {o}.D FROM {o} JOIN N ON N.G = {o}.G WHERE {o}.D < 400 AND EXISTS (SELECT * FROM {l} WHERE {l}.T = {o}.T AND {l}.K = {o}.K)",
];

/// Applies each of `settings` to `s`.
fn set(s: &Session, settings: &[&str]) {
    for setting in settings {
        exec(s, setting).unwrap();
    }
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The rows of `sql` holding its left on one thread, then its rows on 4
/// threads in morsels of 256 rows, and its plan then.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    let alone = rows(s, sql);
    set(s, &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]);
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    (alone, rows(s, sql), shown)
}

/// Whether `shown`, a plan, has a semi join or an antijoin that builds
/// its left, its left a part run in parallel.
fn held_by_parts(shown: &str) -> bool {
    let mut lines = shown.lines().skip_while(|l| !l.contains("HashRightSemiJoin") && !l.contains("HashRightAntiJoin"));
    lines.next().is_some() && lines.next().is_some_and(|left| left.contains("Parallel"))
}

/// Verifies: [X3.135-1986 §5.17 GR2, GR3] (RULES.md D1115, D1070) a semi
/// join or an antijoin that builds its left, a part run in parallel, as
/// EXPLAIN shows, gives the rows it gives holding the left on one thread,
/// in the same order, of both engines: with a condition beyond the keys,
/// keys of nulls, which match nothing, and keys of texts.
#[test]
fn a_left_held_by_its_parts_gives_its_rows_in_order() {
    let (_db, a) = tables();
    for (o, l) in [("O", "L"), ("OW", "LW")] {
        for shape in SHAPES {
            let sql = shape.replace("{o}", o).replace("{l}", l);
            let (alone, together, shown) = both(&a, &sql);
            assert!(held_by_parts(&shown), "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1115, D1150, D38) a left held by its
/// parts past the work area gives way to the join that builds its right,
/// the batches read of it given first, then the rest as its morsels give
/// them, of morsels of 256 rows, and of 4096, each past the grant as it
/// is gathered and read again: its rows are those it gives within a work
/// area that holds it.
#[test]
fn a_left_held_by_its_parts_past_the_work_area_is_read_once() {
    let (db, a) = tables();
    let queries: Vec<String> = SHAPES.iter().map(|shape| shape.replace("{o}", "O").replace("{l}", "L")).collect();
    set(&a, &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]);
    let held: Vec<_> = queries.iter().map(|sql| rows(&a, sql)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
    for morsel in ["SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.MORSEL_ROWS = 4096"] {
        exec(&a, morsel).unwrap();
        for (sql, held) in queries.iter().zip(&held) {
            assert!(held.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {held:?}");
            let mut past = rows(&a, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
            let mut held = held.clone().unwrap_or_default();
            // The join that builds its right gives the rows in no order.
            past.sort();
            held.sort();
            assert_eq!(past, held, "{morsel}: {sql}");
        }
    }
}

/// Verifies: [X3.135-1986 §6.3 GR4] (RULES.md D1115, D15) a left held by
/// its parts that raises for a row raises what it raises held on one
/// thread, of both engines, within the work area and past it: of the row
/// store, of a morsel taken after the rows before it are on course past
/// the grant; of the column store, of a morsel past the grant read
/// again. Of no row held, a semi join gives none.
#[test]
fn a_left_held_by_its_parts_raises_as_alone() {
    let (db, a) = tables();
    // Of the column store, a morsel past a grant of 64 KiB, read again;
    // of the row store, of 192 KiB, the morsels' rows on course past it
    // and those ahead of the reader within it, so the morsel that fails
    // is taken as it failed.
    for (o, l, past) in [("O", "L", 65_536), ("OW", "LW", 196_608)] {
        let sql = format!(
            "SELECT {o}.K, N.V FROM {o} JOIN N ON N.G = {o}.G WHERE {o}.D < 600 AND 10 / ({o}.K - 2048) < 100
               AND EXISTS (SELECT * FROM {l} WHERE {l}.K = {o}.K AND {l}.A < {l}.B)"
        );
        db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(1 << 30, 0))).unwrap();
        let (alone, together, shown) = both(&a, &sql);
        assert!(held_by_parts(&shown), "{shown}");
        assert!(alone.is_err(), "{alone:?}");
        assert_eq!(alone, together, "{shown}");
        db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(past, 0))).unwrap();
        assert_eq!(rows(&a, &sql), alone, "past the work area: {sql}");
    }
    let none = "SELECT O.K FROM O JOIN N ON N.G = O.G WHERE O.D < 0 AND EXISTS (SELECT * FROM L WHERE L.K = O.K)";
    assert_eq!(rows(&a, none), Ok(Vec::new()));
}

/// Verifies: [DESIGN] (RULES.md D1115, D1150, D297) a left held by its
/// parts that reads a range of an index, cut into morsels at its tree's
/// inner keys, as EXPLAIN shows, gives the rows it gives holding the left
/// on one thread, within the work area and past it: a morsel past the
/// grant is read on from where its thread left it, as an index's part is
/// read once.
#[test]
fn a_left_held_by_its_parts_over_an_index_range_past_the_work_area() {
    let (db, a) = tables();
    // Probes priced low, as of a table too large to scan for a range.
    db.set_setting("PLANNER.INDEX_DESCENT_COST", &Value::Approx(0.01_f64)).unwrap();
    let sql = "SELECT OW.K, OW.W, N.V, OW.D FROM OW JOIN N ON N.G = OW.G WHERE OW.K BETWEEN 1000 AND 1400
                 AND EXISTS (SELECT * FROM LW WHERE LW.K = OW.K AND LW.A < LW.B)";
    let (alone, together, shown) = both(&a, sql);
    assert!(held_by_parts(&shown) && shown.contains("IndexScan"), "{shown}");
    assert!(alone.as_ref().is_ok_and(|r| r.len() > 100), "{alone:?}");
    assert_eq!(alone, together, "{shown}");
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
    for morsel in ["SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.MORSEL_ROWS = 4096"] {
        exec(&a, morsel).unwrap();
        let mut past = rows(&a, sql).unwrap_or_else(|e| panic!("{morsel}: {e:?}"));
        let mut held = alone.clone().unwrap_or_default();
        // The join that builds its right gives the rows in no order.
        past.sort();
        held.sort();
        assert_eq!(past, held, "{morsel}");
    }
}
