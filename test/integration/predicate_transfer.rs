//! \[DESIGN\] Predicate transfer (RULES.md D665-D668): a chain of inner hash
//! joins whose tables are read first, each filtered by Bloom filters of the
//! join keys of the others, gives the rows, in the order, it gives without
//! the phase; over random rows of fixed seeds, with nulls, empty tables
//! and keys of mixed scales, alone and in parallel, held and past its work
//! area; a chain that could raise an error is planned as it was.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Rng, exec, failed};
use crate::join_order_workload::imdb;

/// Rows of F, of DA and of DB.
const FACTS: usize = 30_000;
const AS: usize = 3_000;
const BS: usize = 1_000;
/// Rows each INSERT writes.
const PER_INSERT: usize = 1_000;

/// Schema P: F, facts of the column store, A a key of DA's, B of DB's or
/// null, V a value of 10,000; DA, one row of each A, D its key as a decimal
/// of another scale; DB, one row of each B; E, empty. Of random rows of
/// `seed`, their statistics gathered.
fn tables(seed: u64) -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION P
           CREATE TABLE F (K INTEGER, A INTEGER, B INTEGER, V INTEGER, S VARCHAR(4)) USING columnstore
           CREATE TABLE DA (A INTEGER, D DECIMAL(9, 2), X INTEGER, S VARCHAR(4))
           CREATE TABLE DB (B INTEGER, Y INTEGER)
           CREATE TABLE E (B INTEGER, Z INTEGER)",
    )
    .unwrap();
    let p = db.session("P");
    let mut rng = Rng::new(seed);
    let words = ["ab", "cd", "ef", "gh"];
    let facts: Vec<String> = (0..FACTS)
        .map(|k| {
            let b = if rng.chance(3) { "NULL".to_owned() } else { rng.below(BS).to_string() };
            format!("({k}, {}, {b}, {}, '{}')", rng.below(AS), rng.below(10_000), rng.pick(&words))
        })
        .collect();
    insert(&p, "F", &facts);
    let dims: Vec<String> = (0..AS).map(|a| format!("({a}, {a}.00, {}, '{}')", rng.below(1_000), rng.pick(&words))).collect();
    insert(&p, "DA", &dims);
    let dims: Vec<String> = (0..BS).map(|b| format!("({b}, {})", rng.below(1_000))).collect();
    insert(&p, "DB", &dims);
    p.commit().unwrap();
    db.maintain().unwrap();
    (db, p)
}

/// Inserts `rows` into `table`, `PER_INSERT` at a time.
fn insert(s: &Session, table: &str, rows: &[String]) {
    for part in rows.chunks(PER_INSERT) {
        s.run(&format!("INSERT INTO {table} VALUES {}", part.join(", "))).unwrap();
    }
}

/// Chains of joins whose probe side's condition is selective: of three
/// tables and of four, a key of another scale, a text key, a key with
/// nulls, an empty table, a selective dimension, and LEFT joins, the
/// selective table preserved and supplying nulls.
const CHAINS: [&str; 9] = [
    "SELECT F.K, DA.X, DB.Y FROM F JOIN DA ON F.A = DA.A JOIN DB ON F.B = DB.B WHERE F.V < 300 AND DB.Y < 20",
    "SELECT F.K, DA.X FROM F JOIN DA ON F.A = DA.D JOIN DB ON F.B = DB.B WHERE F.V < 200",
    "SELECT F.K, DA.X, DB.Y FROM F, DA, DB WHERE F.A = DA.A AND F.B = DB.B AND F.S = DA.S AND F.V < 500",
    "SELECT COUNT(*), MIN(DA.X) FROM F JOIN DA ON F.A = DA.A JOIN DB ON F.B = DB.B WHERE F.V < 100",
    "SELECT F.K FROM F JOIN DA ON F.A = DA.A JOIN E ON F.B = E.B WHERE F.V < 300",
    "SELECT F.K, D2.X FROM F JOIN DA ON F.A = DA.A JOIN DA AS D2 ON DA.X = D2.A JOIN DB ON F.B = DB.B WHERE F.V < 400 AND D2.X < 50",
    "SELECT MIN(F.K), MIN(DB.Y) FROM F JOIN DB ON F.B = DB.B JOIN DA ON F.A = DA.A WHERE DA.X < 5",
    "SELECT F.K, DA.X, DB.Y FROM F JOIN DA ON F.A = DA.A LEFT JOIN DB ON F.B = DB.B AND DB.Y < 500 WHERE F.V < 300",
    "SELECT DB.B, F.K FROM DB LEFT JOIN F ON F.B = DB.B AND F.V < 30 JOIN DA ON DB.Y = DA.A WHERE DA.X < 300",
];

/// The rows of `sql`, in the order given, as text.
fn rows(s: &Session, sql: &str) -> Vec<String> {
    crate::common::rows(s, sql).unwrap()
}

/// Of each of `CHAINS`, its rows with OPTIMIZER.PREDICATE_TRANSFER ON and
/// OFF are the same, in the same order, but `sorted`, past the work area,
/// where a spilled join gives its rows in its partitions' order.
fn chains_agree(p: &Session, (how, sorted): (&str, bool)) {
    let read = |sql: &str| if sorted { crate::common::sorted(p, sql) } else { rows(p, sql) };
    for sql in CHAINS {
        let on = read(sql);
        exec(p, "SET OPTIMIZER.PREDICATE_TRANSFER = 'OFF'").unwrap();
        let off = read(sql);
        exec(p, "SET OPTIMIZER.PREDICATE_TRANSFER = 'ON'").unwrap();
        assert_eq!(on, off, "{sql}, {how}");
    }
}

/// Verifies: [DESIGN] RULES.md D665, D668 (R-TEST-5): over random rows of
/// fixed seeds, every chain gives with the phase the rows, in the order,
/// it gives without: nulls match none, 7 and 7.00 match, an empty table
/// empties the chain; alone, at degree 4, row by row, its leaves held and,
/// the same rows, past a work area of 64 KiB.
#[test]
fn transfer_changes_no_answer() {
    for seed in [3_u64, 4_242] {
        let (_db, p) = tables(seed);
        chains_agree(&p, ("alone", false));
        for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
            exec(&p, set).unwrap();
        }
        chains_agree(&p, ("degree 4", false));
        exec(&p, "SET EXECUTOR.DEGREE = 1").unwrap();
        exec(&p, "SET MEMORY.WORK_BYTES = 65536").unwrap();
        chains_agree(&p, ("past the work area", true));
        exec(&p, "SET MEMORY.WORK_BYTES TO DEFAULT").unwrap();
        exec(&p, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        chains_agree(&p, ("row by row", false));
    }
}

/// Whether EXPLAIN of `sql` shows a phase of predicate transfer.
fn transferred(s: &Session, sql: &str) -> bool {
    s.explain(sql, &Params::new()).unwrap().contains("PredicateTransfer")
}

/// Verifies: [DESIGN] RULES.md D665, D667 (MC/DC): a chain whose probe
/// side's condition is selective is planned with a phase; with the
/// setting OFF, under a limit of no sort, of a chain that could raise an
/// error, and of two tables of no selective condition, none.
#[test]
fn a_phase_is_planned_where_it_pays() {
    let (_db, p) = tables(11);
    let [selective, ..] = CHAINS;
    assert!(transferred(&p, selective), "{}", p.explain(selective, &Params::new()).unwrap());
    exec(&p, "SET OPTIMIZER.PREDICATE_TRANSFER = 'OFF'").unwrap();
    assert!(!transferred(&p, selective), "the setting OFF");
    exec(&p, "SET OPTIMIZER.PREDICATE_TRANSFER = 'ON'").unwrap();
    assert!(!transferred(&p, &format!("{selective} FETCH FIRST 3 ROWS ONLY")), "under a limit");
    let raising = "SELECT F.K FROM F JOIN DA ON F.A = DA.A JOIN DB ON F.B = DB.B WHERE F.V < 300 AND 10 / (DB.Y - 7) > 0";
    assert!(!transferred(&p, raising), "a condition that can raise");
    assert!(!transferred(&p, "SELECT F.K FROM F JOIN DB ON F.B = DB.B"), "nothing selective");
}

/// Verifies: [DESIGN] RULES.md D665, D15: a chain one of whose conditions
/// can raise is joined as planned, and raises as it does without the
/// phase, of a row a filter would drop: `10 / (DB.Y - 7)` of a DB row of
/// Y = 7.
#[test]
fn a_chain_that_can_raise_raises() {
    let (_db, p) = tables(19);
    exec(&p, "UPDATE DB SET Y = 7 WHERE B = 3").unwrap();
    let raising = "SELECT F.K FROM F JOIN DA ON F.A = DA.A JOIN DB ON F.B = DB.B WHERE F.V < 300 AND 10 / (DB.Y - 7) > 0";
    let on = failed(&p, raising).code;
    exec(&p, "SET OPTIMIZER.PREDICATE_TRANSFER = 'OFF'").unwrap();
    assert_eq!(failed(&p, raising).code, on);
    assert_eq!(on, Code::Data);
}

/// Verifies: [DESIGN] RULES.md D665, D308: each of the join-order
/// workload's 100 queries gives the same rows with the phase and without.
#[test]
fn the_join_order_workload_agrees_with_transfer() {
    let db = imdb::load(300);
    let s = db.session("IMDB");
    for (name, sql) in imdb::queries() {
        let on = rows(&s, sql);
        exec(&s, "SET OPTIMIZER.PREDICATE_TRANSFER = 'OFF'").unwrap();
        assert_eq!(rows(&s, sql), on, "{name}");
        exec(&s, "SET OPTIMIZER.PREDICATE_TRANSFER = 'ON'").unwrap();
    }
}
