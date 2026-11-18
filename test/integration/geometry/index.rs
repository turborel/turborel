//! Geometry index candidates through SQL and MVCC visibility (D85).

use super::registered;
use crate::common::{Scratch, after_crash, rows, serializable};
use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::geometry::Geometry;
use turborel::params::Params;
use turborel::value::Value;

mod estimates;
mod joins;
mod nearest;
mod windows;

fn populated() -> Database {
    let db = registered();
    populate(&db);
    db
}

fn populate(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER PRIMARY KEY, G GEOMETRY(4326))").unwrap();
    let session = db.session("A");
    for start in (0..2_000_i32).step_by(200) {
        let values: Vec<_> = (start..start + 200).map(|id| format!("({id}, ST_POINT({id}, {id}, 4326))")).collect();
        session.run(&format!("INSERT INTO SHAPES VALUES {}", values.join(","))).unwrap();
    }
    session.run("INSERT INTO SHAPES VALUES (2000, ST_GEOMFROMTEXT('POINT EMPTY', 4326)), (2001, NULL), (2002, ST_GEOMFROMTEXT('POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 2 8, 8 8, 8 2, 2 2))', 4326))").unwrap();
    session.commit().unwrap();
    db.create_index("SHAPES_G", "A.SHAPES", &["G"], "rtree").unwrap();
}

/// Verifies: [EXTENSION] D85: ST_Intersects = 1 in either orientation
/// uses envelope candidates and rechecks polygon holes. Adding/removing
/// the index preserves results for NULL, EMPTY, boundaries and equality.
#[test]
fn geometry_index_filters_refine_candidates_and_equal_scans() {
    let db = populated();
    let a = db.session("A");
    let conditions = [
        "ST_INTERSECTS(G, ST_POINT(5, 5, 4326)) = 1",
        "1 = ST_INTERSECTS(ST_POINT(5, 5, 4326), G)",
        "ST_INTERSECTS(G, ST_POINT(2, 5, 4326)) = 1",
        "ST_INTERSECTS(G, ST_GEOMFROMTEXT('POINT EMPTY', 4326)) = 1",
        "ST_INTERSECTS(G, CAST(NULL AS GEOMETRY)) = 1",
        "G = ST_POINT(5, 5, 4326)",
        "ST_DISJOINT(G, ST_POINT(5, 5, 4326)) = 1",
    ];
    let mut results = Vec::new();
    for (at, condition) in conditions.iter().enumerate() {
        let query = format!("SELECT ID FROM SHAPES WHERE {condition} ORDER BY ID");
        if at < 3 {
            let plan = a.physical_plan(&query, &Params::new()).unwrap();
            assert!(plan.contains("SHAPES_G"), "{plan}");
            assert!(plan.contains("Filter"), "{plan}");
        }
        results.push(rows(&a, &query).unwrap());
    }
    assert_eq!(results[0], ["5"]);
    assert_eq!(results[1], ["5"]);
    assert_eq!(results[2], ["2002"]);
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    for (condition, expected) in conditions.iter().zip(results) {
        assert_eq!(rows(&a, &format!("SELECT ID FROM SHAPES WHERE {condition} ORDER BY ID")).unwrap(), expected);
    }
}

/// Verifies: [DESIGN] D15, D85: prepared geometry lookups replace NULL,
/// EMPTY and reference-changing values. An SRID mismatch outside every
/// stored envelope must still raise, even when the query geometry is empty.
#[test]
fn geometry_index_prepared_values_preserve_srid_errors() {
    let db = populated();
    let a = db.session("A");
    let query = a.prepare("SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ?) = 1 ORDER BY ID").unwrap();
    for id in 100..124_i32 {
        let point = Geometry::point(f64::from(id), f64::from(id), 4326).unwrap().to_value();
        let mut cursor = a.open_prepared(&query, &[point]).unwrap();
        assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), id.to_string());
        assert!(cursor.fetch().unwrap().is_none());
    }
    for value in [Value::Null, Geometry::from_wkt("POINT EMPTY", 4326).unwrap().to_value()] {
        assert!(a.open_prepared(&query, &[value]).unwrap().fetch().unwrap().is_none());
    }
    for text in ["POINT (10000 10000)", "POINT EMPTY"] {
        let value = Geometry::from_wkt(text, 0).unwrap().to_value();
        assert_eq!(a.open_prepared(&query, &[value]).and_then(|mut c| c.fetch()).unwrap_err().code, Code::Data);
    }
    let bad = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(5, 5, 4326)) = 1 AND 1 / (ID - 10) > 0";
    let plan = a.physical_plan(bad, &Params::new()).unwrap();
    assert!(!plan.contains("SHAPES_G"), "{plan}");
    assert_eq!(rows(&a, bad).unwrap_err(), Code::Data);
}

/// Verifies: [DESIGN] D85: old snapshots see their own envelope and shape;
/// equal-envelope versions retain one counted entry and different-envelope
/// versions are rechecked. NULL updates and rollback cannot leave ghosts.
#[test]
fn geometry_index_rechecks_visible_projection_and_shape() {
    let db = populated();
    let (reader, writer) = (db.session("A"), db.session("A"));
    writer.run("UPDATE SHAPES SET G = ST_GEOMFROMTEXT('LINESTRING (0 0, 4 4)', 4326) WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    let diagonal = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(1, 1, 4326)) = 1 AND ID = 0";
    assert!(reader.physical_plan(diagonal, &Params::new()).unwrap().contains("SHAPES_G"));
    assert_eq!(rows(&reader, diagonal).unwrap(), ["0"]);
    writer.run("UPDATE SHAPES SET G = ST_GEOMFROMTEXT('LINESTRING (0 4, 4 0)', 4326) WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    assert_eq!(rows(&reader, diagonal).unwrap(), ["0"]);
    assert_eq!(rows(&writer, diagonal).unwrap(), Vec::<String>::new());
    writer.commit().unwrap();
    writer.run("UPDATE SHAPES SET G = ST_POINT(9999, 9999, 4326) WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    assert_eq!(rows(&reader, diagonal).unwrap(), ["0"]);
    let far = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(9999, 9999, 4326)) = 1";
    assert_eq!(rows(&reader, far).unwrap(), Vec::<String>::new());
    reader.commit().unwrap();
    assert_eq!(rows(&reader, far).unwrap(), ["0"]);
    reader.commit().unwrap();
    writer.run("UPDATE SHAPES SET G = NULL WHERE ID = 0").unwrap();
    assert_eq!(rows(&writer, far).unwrap(), Vec::<String>::new());
    writer.rollback().unwrap();
    assert_eq!(rows(&reader, far).unwrap(), ["0"]);
    reader.commit().unwrap();
    writer.run("UPDATE SHAPES SET G = NULL WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    assert_eq!(rows(&reader, far).unwrap(), Vec::<String>::new());
}

/// Verifies: [DESIGN] D85: geometry candidates retain serializable read
/// tracking. Two readers of an empty window cannot both insert there and
/// commit, even though the R-tree returned neither inserted row initially.
#[test]
fn geometry_index_reads_track_serializable_phantoms() {
    let db = populated();
    serializable(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    let query = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(3000, 3000, 4326)) = 1";
    assert_eq!(rows(&a, query).unwrap(), Vec::<String>::new());
    assert_eq!(rows(&b, query).unwrap(), Vec::<String>::new());
    a.run("INSERT INTO SHAPES VALUES (3000, ST_POINT(3000, 3000, 4326))").unwrap();
    b.run("INSERT INTO SHAPES VALUES (3001, ST_POINT(3000, 3000, 4326))").unwrap();
    let results = [a.commit(), b.commit()];
    assert!(results.iter().any(Result::is_err));
    for error in results.into_iter().filter_map(Result::err) {
        assert_eq!(error.code, Code::Conflict);
    }
}

/// Verifies: [DESIGN] D85: recovery preserves geometry projections and
/// their version counts. A checkpoint containing an uncommitted update
/// is undone while committed same-envelope and new-envelope changes survive.
#[test]
fn geometry_index_recovers_projected_versions_and_open_writes() {
    let scratch = Scratch::new("geometry-index-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = registered();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let original = open();
    populate(&original);
    original.checkpoint().unwrap();
    let writer = original.session("A");
    writer.run("UPDATE SHAPES SET G = ST_GEOMFROMTEXT('LINESTRING (0 0, 4 4)', 4326) WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    writer.run("UPDATE SHAPES SET G = ST_GEOMFROMTEXT('LINESTRING (0 4, 4 0)', 4326) WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    writer.run("UPDATE SHAPES SET G = ST_POINT(9000, 9000, 4326) WHERE ID = 1").unwrap();
    writer.commit().unwrap();
    writer.run("UPDATE SHAPES SET G = NULL WHERE ID IN (0, 1)").unwrap();
    original.checkpoint().unwrap();
    drop(original);
    for _ in 0..2 {
        let db = open();
        let a = db.session("A");
        let far = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(9000, 9000, 4326)) = 1";
        assert!(a.physical_plan(far, &Params::new()).unwrap().contains("SHAPES_G"));
        assert_eq!(rows(&a, far).unwrap(), ["1"]);
        assert_eq!(rows(&a, "SELECT ST_ASTEXT(G) FROM SHAPES WHERE ID = 0").unwrap(), ["LINESTRING (0 4, 4 0)"]);
        let cross = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(1, 3, 4326)) = 1 ORDER BY ID";
        assert_eq!(rows(&a, cross).unwrap(), ["0", "2002"]);
        assert_eq!(rows(&a, "SELECT COUNT(*) FROM SHAPES").unwrap(), ["2003"]);
        a.commit().unwrap();
        db.checkpoint().unwrap();
    }
    drop(writer);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.7 SR1, §11.8, §9.12 SR3] [ISO/IEC
/// 13249-3:2016 (DIS, SC32 N2593) §5.1.65] D120: a geometry
/// type is ST-EC-ordered, so a UNIQUE, PRIMARY KEY or referential
/// constraint of a geometry column is refused when it is declared, by
/// CREATE TABLE and ALTER TABLE, whatever the unique index method; an
/// R-tree of the column is still made.
#[test]
fn geometry_columns_are_no_key_of_a_constraint() {
    let db = registered();
    db.set_setting("TABLES.UNIQUE_INDEX_METHOD", &Value::Char("rtree".into())).unwrap();
    for table in ["P (ID INTEGER, G GEOMETRY(4326) UNIQUE)", "P (G ST_POINT PRIMARY KEY)", "P (ID INTEGER, G GEOMETRY, UNIQUE (ID, G))"] {
        assert_eq!(
            db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE {table}")).unwrap_err().code,
            Code::SyntaxRule,
            "{table}"
        );
    }
    db.set_setting("TABLES.UNIQUE_INDEX_METHOD", &Value::Char("btree".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (ID INTEGER PRIMARY KEY, G GEOMETRY(4326))").unwrap();
    let a = db.session("A");
    for alter in ["ALTER TABLE P ADD UNIQUE (G)", "ALTER TABLE P ADD CONSTRAINT R FOREIGN KEY (G) REFERENCES P (G)"] {
        assert_eq!(a.run(alter).unwrap_err().code, Code::SyntaxRule, "{alter}");
    }
    a.rollback().unwrap();
    db.create_index("P_G", "A.P", &["G"], "rtree").unwrap();
}

/// Verifies: [DESIGN] D15, D85: an index does not manufacture an SRID
/// error on an empty/null-only table, but an EMPTY geometry still has a
/// reference. Invalid later writes roll back every earlier spatial entry.
#[test]
fn geometry_index_empty_inputs_and_failed_writes_keep_sql_semantics() {
    for engine in ["rowstore", "columnstore"] {
        let db = registered();
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER, G GEOMETRY(4326)) USING {engine}")).unwrap();
        db.create_index("G_INDEX", "A.SHAPES", &["G"], "rtree").unwrap();
        let a = db.session("A");
        let mismatch = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(1000, 1000)) = 1";
        assert_eq!(rows(&a, mismatch).unwrap(), Vec::<String>::new());
        a.run("INSERT INTO SHAPES VALUES (1, NULL)").unwrap();
        assert_eq!(rows(&a, mismatch).unwrap(), Vec::<String>::new());
        a.run("INSERT INTO SHAPES VALUES (2, ST_GEOMFROMTEXT('POINT EMPTY', 4326))").unwrap();
        assert_eq!(rows(&a, mismatch).unwrap_err(), Code::Data);
        let failed = "INSERT INTO SHAPES VALUES (3, ST_POINT(5, 5, 4326)), (4, ST_POINT(6, 6))";
        assert_eq!(a.run(failed).unwrap_err().code, Code::Data);
        assert_eq!(rows(&a, "SELECT COUNT(*) FROM SHAPES").unwrap(), ["2"]);
        assert_eq!(rows(&a, "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(5, 5, 4326)) = 1").unwrap(), Vec::<String>::new());
    }
}

/// Verifies: [DESIGN] D49, D85: a temporary index forwards its geometry
/// projection even before its pages exist. Different sessions own distinct
/// entries; ON COMMIT DELETE recreates the same projection after clearing.
#[test]
fn geometry_index_temporary_instances_keep_projection_and_ownership() {
    for retention in ["PRESERVE", "DELETE"] {
        let db = registered();
        db.schema(&format!(
            "CREATE SCHEMA AUTHORIZATION A CREATE GLOBAL TEMPORARY TABLE T (ID INTEGER, G GEOMETRY(4326)) ON COMMIT {retention} ROWS"
        ))
        .unwrap();
        db.create_index("TEMP_G", "A.T", &["G"], "rtree").unwrap();
        let (a, b) = (db.session("A"), db.session("A"));
        let query = "SELECT ID FROM T WHERE ST_INTERSECTS(G, ST_POINT(5, 5, 4326)) = 1";
        assert_eq!(rows(&a, query).unwrap(), Vec::<String>::new());
        a.commit().unwrap();
        for (id, session) in [(1_i32, &a), (2_i32, &b)] {
            for start in (100..1_100_i32).step_by(100) {
                let values: Vec<_> = (start..start + 100).map(|n| format!("({n}, ST_POINT({n}, {n}, 4326))")).collect();
                session.run(&format!("INSERT INTO T VALUES {}", values.join(","))).unwrap();
            }
            session.run(&format!("INSERT INTO T VALUES ({id}, ST_POINT(5, 5, 4326))")).unwrap();
            assert!(session.physical_plan(query, &Params::new()).unwrap().contains("TEMP_G"));
            assert_eq!(rows(session, query).unwrap(), [id.to_string()]);
            session.commit().unwrap();
            let expected = if retention == "PRESERVE" { vec![id.to_string()] } else { Vec::new() };
            assert_eq!(rows(session, query).unwrap(), expected);
            session.commit().unwrap();
        }
    }
}
