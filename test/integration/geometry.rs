//! GEOMETRY values through SQL, index keys, recovery and type contracts.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::geometry::{self, Geometry};
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, after_crash, exec, rows};

mod batches;
mod index;
mod prepared;

fn registered() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// Verifies: [EXTENSION] D53, D85, D-SPATIAL-FAMILY: GEOMETRY passes the
/// shared SqlType contract but for keys, which a type ordered by RELATIVE
/// has none of, on every supported shape, empty values and extreme SRIDs.
#[test]
fn geometry_keeps_the_shared_type_contract() {
    let mut values = Vec::new();
    for text in [
        "POINT EMPTY",
        "POINT (-0 0)",
        "POINT (1.5 -3)",
        "LINESTRING (1 2, 3 4)",
        "POLYGON ((0 0, 1 0, 1 1, 0 0))",
        "MULTIPOINT ((1 2), EMPTY)",
        "MULTILINESTRING EMPTY",
        "MULTIPOLYGON EMPTY",
        "GEOMETRYCOLLECTION EMPTY",
    ] {
        for srid in [i32::MIN, 0, 4326, i32::MAX] {
            values.push(Geometry::from_wkt(text, srid).unwrap().to_value());
        }
    }
    crate::type_contract::type_contract_unkeyed(&geometry::data_type(), &values);
    let fixed: Vec<_> = values.iter().map(|v| Geometry::from_value(v).unwrap().with_srid(4326).to_value()).collect();
    crate::type_contract::type_contract_unkeyed(&geometry::data_type_with_srid(4326).unwrap(), &fixed);
}

/// Verifies: [EXTENSION] D85: SQL constructors and observers round trip
/// WKT/WKB while keeping SRID separate. Empty is not NULL; failed
/// multirow construction leaves no insertion, on both storage engines.
#[test]
fn sql_geometry_values_and_failed_writes() {
    for engine in ["rowstore", "columnstore"] {
        let db = registered();
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER, G GEOMETRY) USING {engine}")).unwrap();
        let a = db.session("A");
        a.run("INSERT INTO SHAPES VALUES (1, ST_GEOMFROMTEXT('POINT (1 2)', 4326)), (2, ST_GEOMFROMTEXT('POINT EMPTY')), (3, NULL)")
            .unwrap();
        assert_eq!(
            rows(&a, "SELECT ID, ST_ASTEXT(G), ST_SRID(G), ST_ISEMPTY(G) FROM SHAPES ORDER BY ID").unwrap(),
            ["1,POINT (1 2),4326,0", "2,POINT EMPTY,0,1", "3,NULL,NULL,NULL"]
        );
        assert_eq!(
            rows(&a, "SELECT ST_ASTEXT(ST_GEOMFROMWKB(ST_ASBINARY(G), ST_SRID(G))) FROM SHAPES WHERE ID = 1").unwrap(),
            ["POINT (1 2)"]
        );
        assert_eq!(rows(&a, "SELECT ST_GEOMETRYTYPE(G) FROM SHAPES WHERE ID = 1").unwrap(), ["ST_Point"]);
        assert_eq!(
            exec(&a, "INSERT INTO SHAPES VALUES (4, ST_GEOMFROMTEXT('POINT (9 9)')), (5, ST_GEOMFROMTEXT('POINT (1)'))").err(),
            Some(Code::Data)
        );
        assert_eq!(rows(&a, "SELECT COUNT(*) FROM SHAPES").unwrap(), ["3"]);
        a.rollback().unwrap();
    }
}

/// Verifies: [EXTENSION] D85: prepared constructors bind text and SRID
/// independently, reject fractional/out-of-range SRIDs, compare by
/// ST_OrderingEquals, and refuse a UNIQUE B-tree key of geometry values,
/// which have no key of their own (D-SPATIAL-FAMILY).
#[test]
fn prepared_values_have_exact_srid_and_representation_keys() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (G GEOMETRY) CREATE TABLE KEYED (G GEOMETRY UNIQUE)").unwrap();
    let a = db.session("A");
    let insert = a.prepare("INSERT INTO SHAPES VALUES (ST_GEOMFROMTEXT(?, ?))").unwrap();
    let args = [Value::Char("POINT (-0 2)".into()), Value::Exact(Decimal::new(4326, 0))];
    a.execute_prepared(&insert, &args).unwrap();
    assert_eq!(exec(&a, "INSERT INTO KEYED VALUES (ST_GEOMFROMTEXT('POINT (0.0 2e0)', 4326))").err(), Some(Code::Unsupported));
    a.run("INSERT INTO SHAPES VALUES (ST_GEOMFROMTEXT('POINT (0 2)', 28992))").unwrap();
    assert_eq!(rows(&a, "SELECT ST_SRID(G) FROM SHAPES WHERE G = ST_GEOMFROMTEXT('POINT (0 2)', 4326)").unwrap(), ["4326"]);
    assert_eq!(rows(&a, "VALUES ST_GEOMFROMTEXT('POINT EMPTY', 2147483648)").unwrap_err(), Code::Data);
    assert_eq!(rows(&a, "VALUES ST_GEOMFROMTEXT('POINT EMPTY', 1.5)").unwrap_err(), Code::SyntaxRule);
    a.rollback().unwrap();
    assert_eq!(exec(&a, "CREATE INDEX SPATIAL ON SHAPES (G) USING rtree").err(), Some(Code::Unsupported));
}

/// Verifies: [EXTENSION] D53, D85: constructor NULL propagation, CAST
/// default SRID, invalid type parameters and argument type/arity checks.
#[test]
fn nulls_casts_and_signatures_are_explicit() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for query in [
        "VALUES ST_X(CAST(NULL AS GEOMETRY))",
        "VALUES ST_POINT(CAST(NULL AS DOUBLE PRECISION), 2)",
        "VALUES ST_ASTEXT(CAST(NULL AS GEOMETRY))",
        "VALUES ST_GEOMFROMTEXT(CAST(NULL AS VARCHAR(20)))",
        "VALUES ST_GEOMFROMTEXT('POINT (1 2)', CAST(NULL AS INTEGER))",
    ] {
        assert_eq!(rows(&a, query).unwrap(), ["NULL"]);
    }
    assert_eq!(rows(&a, "VALUES ST_SRID(CAST('POINT (1 2)' AS GEOMETRY))").unwrap(), ["0"]);
    for query in [
        "VALUES ST_ASTEXT(1)",
        "VALUES ST_SRID()",
        "VALUES ST_GEOMFROMWKB('POINT (1 2)')",
        "VALUES ST_ASTEXT(CAST('POINT EMPTY' AS GEOMETRY), 1)",
    ] {
        assert_eq!(rows(&a, query).unwrap_err(), Code::SyntaxRule, "{query}");
    }
    assert_eq!(exec(&a, "CREATE TABLE BAD (G GEOMETRY(4326, 1))").err(), Some(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] D85: SQL users construct points from numeric
/// columns and inspect or relabel them without assembling WKT strings.
#[test]
fn point_functions_work_with_numeric_columns_and_prepared_parameters() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE XY (X DOUBLE PRECISION, Y DOUBLE PRECISION)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO XY VALUES (4.9, 52.37), (NULL, 2)").unwrap();
    assert_eq!(rows(&a, "SELECT ST_ASTEXT(ST_POINT(X, Y, 4326)) FROM XY WHERE X IS NOT NULL").unwrap(), ["POINT (4.9 52.37)"]);
    assert_eq!(rows(&a, "VALUES ST_X(ST_POINT(4.9, 52.37)), ST_Y(ST_POINT(4.9, 52.37))").unwrap(), ["4.9E0", "5.237E1"]);
    assert_eq!(rows(&a, "VALUES ST_SRID(ST_SETSRID(ST_POINT(4.9, 52.37), 4326))").unwrap(), ["4326"]);
    assert_eq!(rows(&a, "VALUES ST_ASTEXT(ST_SETSRID(ST_POINT(4.9, 52.37), 4326))").unwrap(), ["POINT (4.9 52.37)"]);
    let query = a.prepare("VALUES ST_ASTEXT(ST_POINT(?, ?, ?))").unwrap();
    let mut cursor = a.open_prepared(&query, &[Value::Approx(1.0), Value::Approx(2.0), Value::Exact(Decimal::new(4326, 0))]).unwrap();
    assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), "POINT (1 2)");
    assert!(cursor.fetch().unwrap().is_none());
    assert_eq!(rows(&a, "VALUES ST_POINT(1, 2, 1.5)").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "VALUES ST_Y(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)'))").unwrap_err(), Code::Data);
}

/// Verifies: [EXTENSION] D25, D53, D85: checkpoint and log recovery keep
/// geometry coordinates, empty values and SRIDs. Uncommitted updates
/// disappear after crash; the types are built in, so no registration is
/// needed to open the files.
#[test]
fn geometry_survives_checkpoint_and_crash_recovery() {
    let scratch = Scratch::new("geometry-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = registered();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER, G GEOMETRY)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO SHAPES VALUES (1, ST_GEOMFROMTEXT('POLYGON ((0 0, 4 0, 4 4, 0 0))', 28992))").unwrap();
    a.commit().unwrap();
    db.checkpoint().unwrap();
    a.run("INSERT INTO SHAPES VALUES (2, ST_GEOMFROMTEXT('POINT EMPTY', 4326))").unwrap();
    a.commit().unwrap();
    a.run("UPDATE SHAPES SET G = ST_GEOMFROMTEXT('POINT (99 99)') WHERE ID = 1").unwrap();
    db.checkpoint().unwrap();
    let crashed = a;
    drop(db);
    let db = open();
    let a = db.session("A");
    assert_eq!(
        rows(&a, "SELECT ID, ST_ASTEXT(G), ST_SRID(G) FROM SHAPES ORDER BY ID").unwrap(),
        ["1,POLYGON ((0 0, 4 0, 4 4, 0 0)),28992", "2,POINT EMPTY,4326"]
    );
    a.rollback().unwrap();
    db.close().unwrap();
    drop((a, db));
    let db = open();
    assert_eq!(rows(&db.session("A"), "SELECT COUNT(*) FROM SHAPES").unwrap(), ["2"]);
    drop(crashed);
}

/// Verifies: [EXTENSION] D85: integer-valued spatial relationships work
/// in SQL filters and joins, preserve NULL and EMPTY semantics, and bind
/// prepared geometry parameters. D15 errors survive other filters.
#[test]
fn sql_relationships_refine_shapes_and_preserve_errors() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER PRIMARY KEY, G GEOMETRY)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO SHAPES VALUES (1, ST_POINT(1, 1)), (2, ST_POINT(3, 3)), (3, ST_GEOMFROMTEXT('POINT EMPTY')), (4, NULL)").unwrap();
    let query = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_GEOMFROMTEXT('POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))')) = 1 ORDER BY ID";
    assert_eq!(rows(&a, query).unwrap(), ["1"]);
    assert_eq!(
        rows(&a, "SELECT ST_INTERSECTS(G, G), ST_DISJOINT(G, G) FROM SHAPES ORDER BY ID").unwrap(),
        ["1,0", "1,0", "0,1", "NULL,NULL"]
    );
    assert_eq!(
        rows(&a, "SELECT S.ID, T.ID FROM SHAPES S JOIN SHAPES T ON ST_INTERSECTS(S.G, T.G) = 1 ORDER BY S.ID, T.ID").unwrap(),
        ["1,1", "2,2"]
    );
    let prepared = a.prepare("SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ?) = 1").unwrap();
    let point = Geometry::point(3.0, 3.0, 0).unwrap().to_value();
    let mut cursor = a.open_prepared(&prepared, &[point]).unwrap();
    assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), "2");
    assert!(cursor.fetch().unwrap().is_none());
    drop(cursor);
    a.run("UPDATE SHAPES SET G = ST_SETSRID(G, 4326) WHERE ID = 2").unwrap();
    assert_eq!(rows(&a, "SELECT ID FROM SHAPES WHERE ID = 1 AND ST_INTERSECTS(G, ST_POINT(1, 1)) = 1").unwrap_err(), Code::Data);
    assert_eq!(rows(&a, "VALUES ST_INTERSECTS(ST_POINT(0, 0), 1)").unwrap_err(), Code::SyntaxRule);
}

/// Verifies: [DESIGN] D15, D85: constant preparation preserves results
/// with folding disabled, sees new parameters after prepared plans have
/// learned their shape, and never raises a skipped branch's runtime error.
#[test]
fn prepared_relationships_preserve_parameter_reuse_and_lazy_errors() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER, G GEOMETRY)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO SHAPES VALUES (1, ST_POINT(1, 1)), (2, ST_POINT(2, 2))").unwrap();
    for folding in ["ON", "OFF"] {
        a.run(&format!("SET OPTIMIZER.CONSTANT_FOLDING = '{folding}'")).unwrap();
        let query = a.prepare("SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ?) = 1 ORDER BY ID").unwrap();
        for n in 0_u32..16 {
            let coordinate = 1_u32 + n % 2;
            let point = Geometry::point(f64::from(coordinate), f64::from(coordinate), 0).unwrap().to_value();
            let mut cursor = a.open_prepared(&query, &[point]).unwrap();
            assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), coordinate.to_string());
            assert!(cursor.fetch().unwrap().is_none());
        }
        assert_eq!(
            rows(&a, "SELECT CASE WHEN ID = 1 THEN 7 ELSE ST_INTERSECTS(G, ST_POINT(0, 0, 4326)) END FROM SHAPES WHERE ID = 1").unwrap(),
            ["7"]
        );
        for sql in [
            "SELECT CASE WHEN ID = 1 THEN 7 ELSE ST_INTERSECTS(G, ST_GEOMFROMTEXT(?)) END FROM SHAPES WHERE ID = 1",
            "SELECT ST_INTERSECTS(G, ST_GEOMFROMTEXT(?)) FROM SHAPES WHERE ID = 99",
        ] {
            let query = a.prepare(sql).unwrap();
            for _ in 0..16 {
                let mut cursor = a.open_prepared(&query, &[Value::Char("bad".into())]).unwrap();
                if sql.starts_with("SELECT CASE") {
                    assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), "7");
                }
                assert!(cursor.fetch().unwrap().is_none());
            }
        }
        assert_eq!(rows(&a, "SELECT ST_INTERSECTS(G, ST_POINT(0, 0, 4326)) FROM SHAPES").unwrap_err(), Code::Data);
    }
}

/// Verifies: [EXTENSION] D85: ST_DISTANCE measures geometry in filters,
/// joins and deterministic nearest ordering; prepared constants preserve
/// NULL/EMPTY results and D15 errors when other filters would skip a row.
#[test]
fn sql_distance_filters_joins_and_nearest_ordering() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER, G GEOMETRY)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO SHAPES VALUES (1, ST_POINT(0, 0)), (2, ST_POINT(3, 4)), (3, ST_GEOMFROMTEXT('LINESTRING (10 0, 10 10)')), (4, ST_GEOMFROMTEXT('POINT EMPTY')), (5, NULL)").unwrap();
    assert_eq!(
        rows(
            &a,
            "SELECT ID, ST_DISTANCE(G, ST_POINT(0, 0)) AS D FROM SHAPES WHERE ST_ISEMPTY(G) = 0 ORDER BY D, ID FETCH FIRST 2 ROWS ONLY"
        )
        .unwrap(),
        ["1,0E0", "2,5E0"]
    );
    assert_eq!(rows(&a, "SELECT ID FROM SHAPES WHERE ST_DISTANCE(G, ST_POINT(0, 0)) <= 7 ORDER BY ID").unwrap(), ["1", "2"]);
    assert_eq!(
        rows(&a, "SELECT S.ID, T.ID FROM SHAPES S JOIN SHAPES T ON ST_DISTANCE(S.G, T.G) = 5 ORDER BY S.ID, T.ID").unwrap(),
        ["1,2", "2,1"]
    );
    assert_eq!(rows(&a, "SELECT ST_DISTANCE(G, ST_POINT(0, 0)) FROM SHAPES WHERE ID >= 4 ORDER BY ID").unwrap(), ["NULL", "NULL"]);
    for folding in ["ON", "OFF"] {
        a.run(&format!("SET OPTIMIZER.CONSTANT_FOLDING = '{folding}'")).unwrap();
        let query = a.prepare("SELECT ST_DISTANCE(G, ?) FROM SHAPES WHERE ID = 1").unwrap();
        for n in 0_u32..16 {
            let x = f64::from(n);
            let point = Geometry::point(x, 0.0, 0).unwrap().to_value();
            let mut cursor = a.open_prepared(&query, &[point]).unwrap();
            assert_eq!(cursor.fetch().unwrap().unwrap()[0].as_f64(), Some(x));
            assert!(cursor.fetch().unwrap().is_none());
        }
        assert_eq!(rows(&a, "SELECT ST_DISTANCE(G, ST_GEOMFROMTEXT('bad')) FROM SHAPES WHERE ID = 99").unwrap(), Vec::<String>::new());
    }
    a.run("UPDATE SHAPES SET G = ST_SETSRID(G, 4326) WHERE ID = 2").unwrap();
    assert_eq!(rows(&a, "SELECT ID FROM SHAPES WHERE ID = 1 AND ST_DISTANCE(G, ST_POINT(0, 0)) < 7").unwrap_err(), Code::Data);
}

/// Verifies: [EXTENSION] D53, D85: an optional column SRID rejects mixed
/// references atomically in both engines, accepts NULL/EMPTY, applies to
/// prepared writes and CAST, and survives checkpoint and log recovery.
#[test]
fn constrained_geometry_columns_enforce_srid_after_recovery() {
    for engine in ["rowstore", "columnstore"] {
        let scratch = Scratch::new("geometry-srid-recovery");
        let dir = scratch.path("db");
        let open = || {
            let db = registered();
            db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
            db
        };
        let db = open();
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER, G GEOMETRY(4326)) USING {engine}")).unwrap();
        let a = db.session("A");
        a.run("INSERT INTO SHAPES VALUES (1, CAST('POINT (1 2)' AS GEOMETRY(4326))), (2, ST_GEOMFROMTEXT('POINT EMPTY', 4326)), (3, NULL)")
            .unwrap();
        a.commit().unwrap();
        db.checkpoint().unwrap();
        a.run("INSERT INTO SHAPES VALUES (4, ST_POINT(4, 5, 4326))").unwrap();
        a.commit().unwrap();
        drop((a, db));
        let db = open();
        let a = db.session("A");
        assert_eq!(rows(&a, "SELECT ID, ST_SRID(G) FROM SHAPES ORDER BY ID").unwrap(), ["1,4326", "2,4326", "3,NULL", "4,4326"]);
        assert_eq!(exec(&a, "INSERT INTO SHAPES VALUES (5, ST_POINT(5, 6, 4326)), (6, ST_POINT(1, 2))").unwrap_err(), Code::Data);
        assert_eq!(exec(&a, "UPDATE SHAPES SET G = ST_SETSRID(G, 0) WHERE ID = 2").unwrap_err(), Code::Data);
        let insert = a.prepare("INSERT INTO SHAPES VALUES (?, ?)").unwrap();
        let wrong = Geometry::point(1.0, 2.0, 0).unwrap().to_value();
        assert_eq!(a.execute_prepared(&insert, &[Value::Exact(Decimal::new(7, 0)), wrong]).unwrap_err().code, Code::Data);
        assert_eq!(rows(&a, "SELECT COUNT(*) FROM SHAPES").unwrap(), ["4"]);
        for parameters in ["2147483648", "4326, 1"] {
            assert_eq!(exec(&a, &format!("CREATE TABLE BAD (G GEOMETRY({parameters}))")).unwrap_err(), Code::SyntaxRule);
        }
    }
}
