//! \[EXTENSION\] Indexes built whole by a load (RULES.md D532): an INSERT
//! of a batch of rows or more, or a bulk copy's executions, into a table
//! whose indexes are empty, each but a unique one held back and built from
//! its sorted entries as the load ends: answering, estimating, undone and
//! recovered as one its rows went into one at a time.

use turborel::db::{Database, Opened, Prepared, Session};
use turborel::error::Code;
use turborel::params::{Load, Params};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, count, exec, reopened_after_crash};

/// The rows a load gives T.
const ROWS: i64 = 12_000;

/// S.T, on the store `using`: a key, an index of (A, B) whose values come
/// in no order, a CHECK, and its statistics gathered only when asked.
fn table(using: &str) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let t = format!(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER NOT NULL, B INTEGER NOT NULL, \
         C INTEGER CHECK (C >= 0)) {using}"
    );
    db.schema(&t).unwrap();
    db.create_index("T_AB", "S.T", &["A", "B"], "btree").unwrap();
    db
}

/// The values of row `k` of T: A scrambled, so the index is written out of
/// key order.
fn values(k: i64) -> [i64; 4] {
    [k, (k * 7919) % 101, k % 7, k % 37]
}

/// Inserts T's rows `from..to` by statements of `each` rows of values.
fn inserted(s: &Session, (from, to): (i64, i64), each: i64) {
    for start in (from..to).step_by(usize::try_from(each).unwrap()) {
        let rows: Vec<String> =
            (start..to.min(start + each)).map(|k| format!("({})", values(k).map(|v| v.to_string()).join(", "))).collect();
        s.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
    }
}

/// The rows of T found through its index of (A, B), and by a scan.
fn found(s: &Session) -> (usize, usize) {
    (count(s, "SELECT K FROM T WHERE A = 3 AND B = 3") + count(s, "SELECT K FROM T WHERE A < 50"), count(s, "SELECT K FROM T"))
}

/// The plans of queries through T's index and not, once their statistics
/// are gathered.
fn plans(db: &Database) -> Vec<String> {
    let s = db.session("S");
    let queries = [
        "SELECT K FROM T WHERE A = 3 AND B = 3",
        "SELECT K FROM T WHERE A < 20",
        "SELECT K FROM T WHERE A BETWEEN 40 AND 60",
        "SELECT K FROM T",
    ];
    for q in queries {
        s.explain(q, &Params::new()).unwrap();
    }
    db.run_due().unwrap();
    queries.iter().map(|q| s.explain(q, &Params::new()).unwrap()).collect()
}

/// A plan's line up to its estimate, and the rows it estimates.
fn estimates(line: &str) -> (String, f64) {
    let rows = line.split("est. rows=").nth(1).and_then(|r| r.split([',', ')']).next()).and_then(|n| n.parse().ok());
    (line.split("(est.").next().unwrap_or_default().to_owned(), rows.unwrap_or_else(|| panic!("{line}")))
}

/// Asserts the plans of `loaded` are those of `alone`, of the same shape,
/// each estimate within a tenth of the other's.
fn estimated_alike(loaded: &Database, alone: &Database, using: &str) {
    for (one, other) in plans(loaded).iter().zip(plans(alone)) {
        let lines = |p: &str| p.lines().filter(|l| l.contains("est.")).map(estimates).collect::<Vec<_>>();
        for ((shape, near), (shape_alone, exact)) in lines(one).into_iter().zip(lines(&other)) {
            assert_eq!(shape, shape_alone, "{using}");
            assert!((near - exact).abs() <= (exact / 10.0).max(2.0), "{using}: {one} against {other}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D34, D35, D532: an index built whole by a
/// load answers as one its rows went into one at a time; its plans are of
/// the same shape, its count of distinct keys the same, and its estimate
/// of each range within a tenth of the other's, its pages being fuller.
/// An index not empty as a load begins takes its entries in the rows'
/// order, and is estimated alike exactly. Of each store.
#[test]
fn an_index_built_by_a_load_is_estimated_as_one_built_row_by_row() {
    for using in ["", "USING columnstore"] {
        let (loaded, alone) = (table(using), table(using));
        inserted(&loaded.session("S"), (0, ROWS), ROWS);
        inserted(&alone.session("S"), (0, ROWS), 1);
        for db in [&loaded, &alone] {
            db.session("S").commit().unwrap();
        }
        assert_eq!(found(&loaded.session("S")), found(&alone.session("S")), "{using}");
        estimated_alike(&loaded, &alone, using);
        // A second load finds the index full: row by row, estimated alike.
        inserted(&loaded.session("S"), (ROWS, 2 * ROWS), ROWS);
        inserted(&alone.session("S"), (ROWS, 2 * ROWS), ROWS);
        assert_eq!(found(&loaded.session("S")), found(&alone.session("S")), "{using}");
    }
}

/// `n` as an exact numeric value.
fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// T's rows `from..to`, as a copy gives an INSERT's parameters.
fn given(from: i64, to: i64) -> Vec<Vec<Value>> {
    (from..to).map(|k| values(k).into_iter().map(int).collect()).collect()
}

/// The prepared INSERT a copy into T executes.
fn insert(s: &Session) -> Prepared {
    s.prepare("INSERT INTO T (K, A, B, C) VALUES (?, ?, ?, ?)").unwrap()
}

/// Verifies: [X3.135-1986 §8.7 GR1c, §4.16]; [EXTENSION] RULES.md D532: a
/// load of many executions holds its index's entries back until its last,
/// which may be given no rows; an execution that fails in it is undone
/// whole, its entries with it, and the load goes on; a rollback undoes a
/// load whole, and the index, empty, is loaded again; a duplicate key is
/// refused as each row goes in, the unique index being held back never.
/// Of each store.
#[test]
fn a_load_of_many_executions_builds_its_index_as_it_ends() {
    for using in ["", "USING columnstore"] {
        let db = table(using);
        let s = db.session("S");
        let p = insert(&s);
        s.execute_prepared_rows(&p, &given(0, 5000), Load::Going).unwrap();
        let mut broken = given(5000, 6000);
        broken.push(vec![int(6000), int(1), int(1), int(-1)]);
        assert_eq!(s.execute_prepared_rows(&p, &broken, Load::Going).map_err(|e| e.code), Err(Code::Constraint), "{using}");
        let duplicate = [given(5000, 5100), given(10, 11)].concat();
        assert_eq!(s.execute_prepared_rows(&p, &duplicate, Load::Going).map_err(|e| e.code), Err(Code::Constraint), "{using}");
        s.execute_prepared_rows(&p, &given(5000, ROWS), Load::Going).unwrap();
        s.execute_prepared_rows(&p, &[], Load::Ending).unwrap();
        let expected = found(&s);
        s.rollback().unwrap();
        assert_eq!(found(&s), (0, 0), "{using}: the load undone whole");
        s.execute_prepared_rows(&p, &given(0, ROWS), Load::Going).unwrap();
        s.execute_prepared_rows(&p, &given(ROWS, ROWS + 1), Load::Ending).unwrap();
        s.commit().unwrap();
        let alone = table(using);
        inserted(&alone.session("S"), (0, ROWS + 1), 1);
        assert_eq!(found(&s), found(&alone.session("S")), "{using}");
        assert_eq!(expected.1, usize::try_from(ROWS).unwrap(), "{using}");
    }
}

/// Verifies: [DESIGN] RULES.md D25, D532: a database crashed with a load
/// committed and another under way, its index's entries held back, is
/// recovered to the committed load, found through the index and by a
/// scan, and none of the other's rows; recovered from the log alone and
/// from a checkpoint and the log after it. Of each store.
#[test]
fn a_load_crashed_midway_leaves_its_index_whole() {
    for (using, checkpoint) in [("", false), ("", true), ("USING columnstore", false), ("USING columnstore", true)] {
        let scratch = Scratch::new("bulk-load-crash");
        let dir = scratch.path("db");
        let (crashed, _) = reopened_after_crash(&dir);
        crashed
            .schema(
                "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER NOT NULL, B INTEGER NOT NULL, C INTEGER)",
            )
            .unwrap();
        crashed.create_index("T_AB", "S.T", &["A", "B"], "btree").unwrap();
        let s = crashed.session("S");
        let p = insert(&s);
        s.execute_prepared_rows(&p, &given(0, 5000), Load::Going).unwrap();
        s.execute_prepared_rows(&p, &given(5000, 6000), Load::Ending).unwrap();
        s.commit().unwrap();
        if checkpoint {
            crashed.checkpoint().unwrap();
        }
        s.execute_prepared_rows(&p, &given(6000, ROWS), Load::Going).unwrap();
        let (db, opened) = reopened_after_crash(&dir);
        assert!(matches!(opened, Opened::Recovered(_)), "{using}: {opened:?}");
        let r = db.session("S");
        let alone = table("");
        inserted(&alone.session("S"), (0, 6000), 1);
        assert_eq!(found(&r), found(&alone.session("S")), "{using}, checkpoint {checkpoint}");
        assert_eq!(exec(&r, "INSERT INTO T VALUES (7000, 1, 1, 1)").err(), None, "{using}: the open load's keys free");
        drop((db, s, crashed));
    }
}

/// The rows of T a session finds through its index of (A, B), and by
/// conditions no index answers.
fn found_both_ways(s: &Session) -> (usize, usize) {
    (count(s, "SELECT K FROM T WHERE A = 3 AND B = 3"), count(s, "SELECT K FROM T WHERE A + 0 = 3 AND B + 0 = 3"))
}

/// Verifies: [X3.135-1986 §4.16, §8.7 GR1c]; [EXTENSION] RULES.md D532: a
/// copy failing at its 5000th row, its savepoint rolled back as a copy's
/// is, leaves its table as it was and the index it held back empty, which
/// then takes a load, rows one at a time and a load after, as an index no load
/// failed into does, estimated alike. A reader in another transaction
/// meanwhile is not kept waiting and finds none of an open load's rows,
/// by the index or by a scan, and every row through both once it commits.
/// A statement of the loading transaction that is no part of the load
/// ends it first, so it finds through the index every row it inserted.
/// Of each store.
#[test]
fn a_failed_copy_leaves_its_index_empty_and_no_reader_misses_an_entry() {
    for using in ["", "USING columnstore"] {
        let db = table(using);
        let (s, reader) = (db.session("S"), db.session("S"));
        let p = insert(&s);
        s.run("SAVEPOINT COPY_1").unwrap();
        s.execute_prepared_rows(&p, &given(0, 4096), Load::Going).unwrap();
        assert_eq!((found(&reader), found_both_ways(&reader)), ((0, 0), (0, 0)), "{using}: none of an open load's rows");
        reader.commit().unwrap();
        let mut broken = given(4096, 5000);
        broken.push(vec![int(5000), int(1), int(1), int(-1)]);
        assert_eq!(s.execute_prepared_rows(&p, &broken, Load::Going).map_err(|e| e.code), Err(Code::Constraint), "{using}");
        s.run("ROLLBACK TO SAVEPOINT COPY_1").unwrap();
        assert_eq!((found(&s), found_both_ways(&s)), ((0, 0), (0, 0)), "{using}: the table as it was");
        s.execute_prepared_rows(&p, &given(0, 3000), Load::Going).unwrap();
        let (through, scanned) = found_both_ways(&s);
        assert!(through > 0 && through == scanned, "{using}: a statement no part of the load ends it");
        inserted(&s, (3000, 3100), 1);
        s.execute_prepared_rows(&p, &given(3100, ROWS), Load::Going).unwrap();
        s.execute_prepared_rows(&p, &[], Load::Ending).unwrap();
        assert_eq!(found(&reader), (0, 0), "{using}");
        s.commit().unwrap();
        let alone = table(using);
        inserted(&alone.session("S"), (0, ROWS), 1);
        assert_eq!(found(&reader), found(&alone.session("S")), "{using}");
        let (through, scanned) = found_both_ways(&reader);
        assert_eq!(through, scanned, "{using}");
        estimated_alike(&db, &alone, using);
    }
}

/// Verifies: [X3.135-1986 §4.16]; [EXTENSION] RULES.md D532: a load of
/// many executions committed without the one that ends it is ended as it
/// commits, so its index holds every row it inserted.
#[test]
fn a_load_committed_unended_is_ended_as_it_commits() {
    for using in ["", "USING columnstore"] {
        let db = table(using);
        let s = db.session("S");
        let p = insert(&s);
        s.execute_prepared_rows(&p, &given(0, ROWS), Load::Going).unwrap();
        s.commit().unwrap();
        let (through, scanned) = found_both_ways(&s);
        assert!(through > 0 && through == scanned, "{using}: {through} through the index, {scanned} by a scan");
    }
}
