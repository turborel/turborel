//! SQL access through Cartesian R-link indexes: results, parameter slots,
//! visibility, rollback, recovery and nullable coordinates (D72).

use crate::common::{Scratch, after_crash, rows, serializable};
use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Value;

mod joins;
mod packing;

fn populate(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE P (ID INTEGER NOT NULL PRIMARY KEY, X DOUBLE PRECISION, Y DOUBLE PRECISION)")
        .unwrap();
    let session = db.session("S");
    for start in (0..2_000_i32).step_by(200) {
        let values: Vec<_> = (start..start + 200).map(|id| format!("({id}, {}, {})", id % 37, id % 43)).collect();
        session.run(&format!("INSERT INTO P VALUES {}", values.join(","))).unwrap();
    }
    session.run("INSERT INTO P VALUES (2000, 3, NULL), (2001, NULL, 4), (2002, NULL, NULL), (2003, 0.1, 0.5)").unwrap();
    session.commit().unwrap();
}

fn database() -> Database {
    let db = Database::new();
    populate(&db);
    db.create_index("P_XY", "S.P", &["X", "Y"], "rtree").unwrap();
    db
}

/// Verifies: [EXTENSION] Independent X/Y bounds use the spatial index
/// and return the same rows as a sequential scan, with strict and closed
/// boundaries, contradictory bounds and exact decimal constants. A
/// predicate leaving a nullable coordinate unconstrained cannot use an
/// index that omits that row. SQL ORDER BY remains a separate sort.
#[test]
fn spatial_filters_equal_scans_including_nulls_and_numeric_boundaries() {
    let db = database();
    let session = db.session("S");
    let conditions = [
        "X BETWEEN 2 AND 3 AND Y BETWEEN 4 AND 5",
        "X > 2 AND X < 3 AND Y >= 4 AND Y <= 5",
        "X >= 0.1 AND X <= 0.10000000000000001 AND Y = 0.5",
        "X = 0.1 AND Y = 0.5",
        "X > 10 AND X < 3 AND Y >= 0",
        "Y = 4 AND X < 3",
        "X = 3",
        "Y = 4",
        "X IS NULL OR Y IS NULL",
    ];
    let plan = session.physical_plan(&format!("SELECT ID FROM P WHERE {}", conditions[0]), &Params::new()).unwrap();
    assert!(plan.contains("P_XY"), "{plan}");
    let queries: Vec<_> = conditions.iter().map(|c| format!("SELECT ID, X, Y FROM P WHERE {c} ORDER BY X, Y, ID")).collect();
    let indexed: Vec<_> = queries.iter().map(|sql| rows(&session, sql).unwrap()).collect();
    session.commit().unwrap();
    db.drop_index("P_XY").unwrap();
    for (sql, expected) in queries.iter().zip(indexed) {
        assert_eq!(rows(&session, sql).unwrap(), expected, "{sql}");
    }
}

/// Verifies: [EXTENSION] Reused prepared plans replace every spatial
/// comparison's parameter, including NULL and inverted intervals; values
/// from earlier executions cannot remain in a key's independent bounds.
#[test]
fn prepared_spatial_windows_replace_all_parameters() {
    let db = database();
    let session = db.session("S");
    let prepared = session.prepare("SELECT ID FROM P WHERE X >= ? AND X <= ? AND Y >= ? AND Y <= ? ORDER BY ID").unwrap();
    for n in 0..24_i32 {
        let values = [n % 9, n % 9 + 1, n % 7, n % 7 + 1].map(|x| Value::Approx(f64::from(x)));
        let mut cursor = session.open_prepared(&prepared, &values).unwrap();
        let mut found = Vec::new();
        while let Some(row) = cursor.fetch().unwrap() {
            found.push(row[0].to_string());
        }
        let expected = rows(
            &session,
            &format!(
                "SELECT ID FROM P WHERE X + 0 >= {} AND X <= {} AND Y >= {} AND Y <= {} ORDER BY ID",
                values[0], values[1], values[2], values[3]
            ),
        )
        .unwrap();
        assert_eq!(found, expected);
        session.commit().unwrap();
    }
    for values in [
        [Value::Null, Value::Approx(10.0_f64), Value::Approx(0.0_f64), Value::Approx(10.0_f64)],
        [Value::Approx(10.0_f64), Value::Approx(0.0_f64), Value::Approx(0.0_f64), Value::Approx(10.0_f64)],
    ] {
        assert!(session.open_prepared(&prepared, &values).unwrap().fetch().unwrap().is_none());
        session.commit().unwrap();
    }
}

/// Verifies: [DESIGN] The spatial index retains keys for an old snapshot,
/// rejects invisible newer keys through the ordinary MVCC recheck, and
/// undoes an update on rollback. Repeated equal coordinate versions do
/// not lose their counted index entry when an old version is collected.
#[test]
fn spatial_keys_follow_snapshot_visibility_and_rollback() {
    let db = database();
    let (reader, writer) = (db.session("S"), db.session("S"));
    reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    let old = "SELECT ID FROM P WHERE X = 0 AND Y = 0 ORDER BY ID";
    let before = rows(&reader, old).unwrap();
    writer.run("UPDATE P SET X = 999, Y = 999 WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    assert_eq!(rows(&reader, old).unwrap(), before);
    assert_eq!(rows(&reader, "SELECT ID FROM P WHERE X = 999 AND Y = 999").unwrap(), Vec::<String>::new());
    reader.commit().unwrap();
    assert_eq!(rows(&reader, "SELECT ID FROM P WHERE X = 999 AND Y = 999").unwrap(), ["0"]);
    reader.commit().unwrap();
    writer.run("UPDATE P SET X = 500, Y = 500 WHERE ID = 0").unwrap();
    writer.rollback().unwrap();
    writer.run("UPDATE P SET X = 999, Y = 999 WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    assert_eq!(rows(&reader, "SELECT ID FROM P WHERE X = 999 AND Y = 999").unwrap(), ["0"]);
    assert_eq!(rows(&reader, "SELECT ID FROM P WHERE X = 500 AND Y = 500").unwrap(), Vec::<String>::new());
}

/// Verifies: [DESIGN] Spatial predicate reads participate in serializable
/// validation: two transactions reading an empty window cannot each
/// insert into it and both commit. The index does not bypass read tracking.
#[test]
fn spatial_windows_track_serializable_phantoms() {
    let db = database();
    serializable(&db);
    let (a, b) = (db.session("S"), db.session("S"));
    let query = "SELECT ID FROM P WHERE X BETWEEN 100 AND 101 AND Y BETWEEN 100 AND 101";
    assert_eq!(rows(&a, query).unwrap(), Vec::<String>::new());
    assert_eq!(rows(&b, query).unwrap(), Vec::<String>::new());
    a.run("INSERT INTO P VALUES (3000, 100, 100)").unwrap();
    b.run("INSERT INTO P VALUES (3001, 101, 101)").unwrap();
    let first = a.commit();
    let second = b.commit();
    assert!(first.is_err() || second.is_err());
    for error in [first.err(), second.err()].into_iter().flatten() {
        assert_eq!(error.code, Code::Conflict);
    }
}

/// Verifies: [EXTENSION] Rectangle keys retain all four coordinates.
/// A spanning rectangle is found even when min X <= 3 and max X >= 7
/// cross in point space. An invalid later row rolls back the entire
/// statement, including earlier index entries successfully added.
#[test]
fn rectangle_keys_and_failed_statements_preserve_sql_results() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE R (ID INTEGER, LX DOUBLE PRECISION NOT NULL, LY DOUBLE PRECISION NOT NULL, HX DOUBLE PRECISION NOT NULL, HY DOUBLE PRECISION NOT NULL)").unwrap();
    let session = db.session("S");
    session.run("CREATE INDEX R_BOUNDS ON R (LX, LY, HX, HY) USING RTREE").unwrap();
    session.run("INSERT INTO R VALUES (1, 0, 0, 10, 10), (2, 3, 4, 7, 8), (3, 0, 0, 1, 1)").unwrap();
    session.commit().unwrap();
    let query = "SELECT ID FROM R WHERE LX <= 3 AND HX >= 7 AND LY <= 4 AND HY >= 8 ORDER BY ID";
    assert_eq!(rows(&session, query).unwrap(), ["1", "2"]);
    let error = session.run("INSERT INTO R VALUES (4, 0, 0, 10, 10), (5, 9, 0, 1, 10)").unwrap_err();
    assert_eq!(error.code, Code::Api);
    assert_eq!(rows(&session, query).unwrap(), ["1", "2"]);
    session.commit().unwrap();
    db.drop_index("R_BOUNDS").unwrap();
    assert_eq!(rows(&session, query).unwrap(), ["1", "2"]);
}

/// Verifies: [DESIGN] A file database recovers committed spatial changes
/// and rolls back uncommitted changes represented in a checkpoint. A
/// second reopen retains the same keys. This exercises logical WAL replay
/// and index mending with R-link pages, rather than a separate spatial log.
#[test]
fn spatial_index_recovers_commits_and_undoes_checkpointed_open_writes() {
    let scratch = Scratch::new("spatial-recovery");
    let dir = scratch.path("db");
    let original = Database::new();
    original.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    populate(&original);
    original.create_index("P_XY", "S.P", &["X", "Y"], "rtree").unwrap();
    original.checkpoint().unwrap();
    let committed = original.session("S");
    committed.run("UPDATE P SET X = 800, Y = 800 WHERE ID < 200").unwrap();
    committed.commit().unwrap();
    let open = original.session("S");
    open.run("UPDATE P SET X = 900, Y = 900 WHERE ID >= 200 AND ID < 400").unwrap();
    original.checkpoint().unwrap();
    drop((committed, original));
    let recovered = Database::new();
    assert!(matches!(recovered.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap(), Opened::Recovered(_)));
    let session = recovered.session("S");
    assert_eq!(rows(&session, "SELECT COUNT(*) FROM P WHERE X = 800 AND Y = 800").unwrap(), ["200"]);
    assert_eq!(rows(&session, "SELECT COUNT(*) FROM P WHERE X = 900 AND Y = 900").unwrap(), ["0"]);
    assert_eq!(rows(&session, "SELECT COUNT(*) FROM P").unwrap(), ["2004"]);
    session.commit().unwrap();
    recovered.checkpoint().unwrap();
    drop((session, recovered));
    let reopened = Database::new();
    reopened.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(rows(&reopened.session("S"), "SELECT COUNT(*) FROM P WHERE X = 800 AND Y = 800").unwrap(), ["200"]);
    drop(open);
}

/// Verifies: [EXTENSION] Temporary spatial indexes hold only the owning
/// session's rows. The built-in registration and temporary-table method
/// selection preserve the same two-coordinate window semantics.
#[test]
fn temporary_spatial_indexes_are_private_to_each_session() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE GLOBAL TEMPORARY TABLE P (ID INTEGER, X DOUBLE PRECISION NOT NULL, Y DOUBLE PRECISION NOT NULL) ON COMMIT PRESERVE ROWS").unwrap();
    db.create_index("TEMP_XY", "S.P", &["X", "Y"], "rtree").unwrap();
    let (a, b) = (db.session("S"), db.session("S"));
    a.run("INSERT INTO P VALUES (1, 5, 5)").unwrap();
    a.commit().unwrap();
    b.run("INSERT INTO P VALUES (2, 5, 5)").unwrap();
    b.commit().unwrap();
    let query = "SELECT ID FROM P WHERE X = 5 AND Y = 5";
    assert_eq!(rows(&a, query).unwrap(), ["1"]);
    assert_eq!(rows(&b, query).unwrap(), ["2"]);
}

/// Verifies: [EXTENSION] An R-tree can back a two-coordinate unique key:
/// the coordinator checks uniqueness when the index reports no shortcut.
/// Keys with NULLS NOT DISTINCT are explicitly unsupported (-900), not
/// silently omitted. Duplicate insertion rolls back the whole statement.
#[test]
fn coordinate_uniqueness_is_checked_and_null_equal_keys_are_refused() {
    let db = Database::new();
    db.set_setting("TABLES.UNIQUE_INDEX_METHOD", &Value::Char("rtree".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE P (X DOUBLE PRECISION, Y DOUBLE PRECISION, UNIQUE (X, Y))").unwrap();
    let session = db.session("S");
    assert_eq!(session.run("INSERT INTO P VALUES (1, 2), (1, 2)").err().unwrap().code, Code::Constraint);
    assert_eq!(rows(&session, "SELECT COUNT(*) FROM P").unwrap(), ["0"]);
    session.commit().unwrap();
    let rejected = session.run("CREATE TABLE N (X DOUBLE PRECISION, Y DOUBLE PRECISION, UNIQUE NULLS NOT DISTINCT (X, Y))");
    assert_eq!(rejected.err().unwrap().code, Code::Unsupported);
}
