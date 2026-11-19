//! Simple feature access, SQL option [OGC 06-104r4], in its "SQL with
//! Geometry Types" environment: the views GEOMETRY_COLUMNS and
//! SPATIAL_REF_SYS [§7.1], its routines by their names without ST_
//! [§7.2], PolyhedSurface [§7.2.14] and the text and binary forms of OGC
//! 06-103r4 (RULES.md D136).

mod annotation;
mod blue_lake;
mod routines;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::rows;

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    db
}

fn run(s: &Session, all: &[&str]) {
    for sql in all {
        s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
}

/// The columns of a view of SF_INFORMTN_SCHEMA: name, type, nullable.
fn described(s: &Session, view: &str) -> Vec<String> {
    let sql = format!(
        "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
         WHERE TABLE_SCHEMA = 'SF_INFORMTN_SCHEMA' AND TABLE_NAME = '{view}' ORDER BY ORDINAL_POSITION"
    );
    rows(s, &sql).unwrap()
}

const COLUMNS: &str = "SELECT F_TABLE_SCHEMA, F_TABLE_NAME, F_GEOMETRY_COLUMN, G_TABLE_SCHEMA, G_TABLE_NAME, STORAGE_TYPE, \
     GEOMETRY_TYPE, COORD_DIMENSION, MAX_PPR, SRID FROM GEOMETRY_COLUMNS ORDER BY F_TABLE_NAME, F_GEOMETRY_COLUMN";

/// Verifies: [OGC 06-104r4 §7.1.3.2, §7.1.3.3, Table 4] GEOMETRY_COLUMNS
/// lists each column of a geometry type of a table or view, typed by a
/// name with ST_ or without, of an SRID or none, or registered with a
/// system defined: its feature table its geometry table, no storage type
/// or MAX_PPR, the type's code of Table 4 or of OGC 06-103r4's Table 7,
/// none of a type neither codes, with 1000 and COORD_DIMENSION 3 in a
/// geocentric system, and the SRID registered, else the type's, else 0;
/// it has the columns §7.1.3.2 names.
#[test]
fn geometry_columns_lists_every_geometry_column() {
    let db = database();
    let a = db.session("A");
    run(
        &a,
        &[
            blue_lake::UTM,
            "CREATE TABLE T (ID INTEGER, G GEOMETRY, P POINT(4326), C ST_GEOMETRY(4978), L ST_LINESTRING, M MULTICURVE, \
             S ST_BREPSOLID, N ST_TIN, H POLYHEDSURFACE, R POLYGON, W ST_CIRCLE)",
            "CREATE VIEW V AS SELECT P AS Q FROM T",
            "CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'R', 'UTM_ZONE_14N')",
        ],
    );
    assert_eq!(
        rows(&a, COLUMNS).unwrap(),
        [
            "A,T,C,A,T,NULL,1000,3,NULL,4978",
            "A,T,G,A,T,NULL,0,2,NULL,0",
            "A,T,H,A,T,NULL,15,2,NULL,0",
            "A,T,L,A,T,NULL,2,2,NULL,0",
            "A,T,M,A,T,NULL,11,2,NULL,0",
            "A,T,N,A,T,NULL,16,2,NULL,0",
            "A,T,P,A,T,NULL,1,2,NULL,4326",
            "A,T,R,A,T,NULL,3,2,NULL,101",
            "A,T,S,A,T,NULL,NULL,2,NULL,0",
            "A,T,W,A,T,NULL,NULL,2,NULL,0",
            "A,V,Q,A,V,NULL,1,2,NULL,4326",
        ]
    );
    let catalogs = "SELECT count(*) FROM GEOMETRY_COLUMNS WHERE F_TABLE_CATALOG = G_TABLE_CATALOG AND F_TABLE_CATALOG IS NOT NULL";
    assert_eq!(rows(&a, catalogs).unwrap(), ["11"]);
    let id = |n: &str| format!("{n},CHARACTER VARYING,NO,128");
    let int = |n: &str, null: &str| format!("{n},INTEGER,{null},NULL");
    let mut expected: Vec<String> = ["F_TABLE_CATALOG", "F_TABLE_SCHEMA", "F_TABLE_NAME", "F_GEOMETRY_COLUMN"].map(id).to_vec();
    expected.extend(["G_TABLE_CATALOG", "G_TABLE_SCHEMA", "G_TABLE_NAME"].map(id));
    expected.extend(["STORAGE_TYPE", "GEOMETRY_TYPE", "COORD_DIMENSION", "MAX_PPR"].map(|n| int(n, "YES")));
    expected.push(int("SRID", "NO"));
    assert_eq!(described(&a, "GEOMETRY_COLUMNS"), expected);
}

/// Verifies: [OGC 06-104r4 §7.1.1, §7.1.3.2] GEOMETRY_COLUMNS shows a
/// user the columns INFORMATION_SCHEMA.COLUMNS does; SELECT on the views
/// is everyone's and nothing else, nor is their base table read; an
/// unqualified name of a view names it where the default schema has no
/// table of that name, also as the qualifier of its columns and in a
/// view's query, and the table where it has one.
#[test]
fn the_views_are_read_as_the_information_schema_is() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    run(&a, &["CREATE TABLE T (G POINT)", "CREATE TABLE U (G LINESTRING)", "GRANT SELECT ON U TO B"]);
    a.commit().unwrap();
    assert_eq!(rows(&b, "SELECT F_TABLE_NAME FROM GEOMETRY_COLUMNS").unwrap(), ["U"]);
    assert_eq!(rows(&a, "SELECT GEOMETRY_COLUMNS.F_TABLE_NAME FROM GEOMETRY_COLUMNS ORDER BY 1").unwrap(), ["T", "U"]);
    assert_eq!(rows(&a, "SELECT count(*) FROM SF_INFORMTN_SCHEMA.GEOMETRY_COLUMNS").unwrap(), ["2"]);
    run(&a, &["CREATE VIEW W AS SELECT F_TABLE_NAME FROM geometry_columns"]);
    assert_eq!(rows(&a, "SELECT * FROM W ORDER BY 1").unwrap(), ["T", "U"]);
    assert_eq!(a.run("INSERT INTO SF_INFORMTN_SCHEMA.SPATIAL_REF_SYS VALUES (1, 'X', 1, 'Y')").unwrap_err().code, Code::Privilege);
    assert_eq!(a.run("DELETE FROM SF_INFORMTN_SCHEMA.GEOMETRY_COLUMNS").unwrap_err().code, Code::Privilege);
    assert_eq!(rows(&a, "SELECT * FROM ST_DEFINITION_SCHEMA.SF_GEOMETRY_COLUMNS").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT * FROM A.GEOMETRY_COLUMNS").unwrap_err(), Code::SyntaxRule);
    run(&a, &["CREATE TABLE GEOMETRY_COLUMNS (X INTEGER)"]);
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT count(*) FROM GEOMETRY_COLUMNS").unwrap(), ["0"]);
    assert_eq!(rows(&b, "SELECT count(*) FROM geometry_columns").unwrap(), ["1"]);
    let schemas = "SELECT count(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'SF_INFORMTN_SCHEMA'";
    assert_eq!(rows(&a, schemas).unwrap(), ["3"]);
}

/// Verifies: [OGC 06-104r4 §7.1.2.2, §7.1.1] SPATIAL_REF_SYS holds each
/// system supplied and each ST_DEFINESRS defines, of its organization's
/// name and identifier or none, under the columns §7.1.2.2 names; each
/// SRID of GEOMETRY_COLUMNS but 0 is one of its rows.
#[test]
fn spatial_ref_sys_holds_the_systems_supplied_and_defined() {
    let db = database();
    let a = db.session("A");
    run(
        &a,
        &[
            blue_lake::UTM,
            "CALL ST_DEFINESRS('LOCAL GRID', 900001, CAST(NULL AS VARCHAR(1)), CAST(NULL AS INTEGER), \
             'GEOGCS[\"grid\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"u\",1]]', 'a grid')",
            "CREATE TABLE T (P POINT(900001), Q POINT(4326), R POINT)",
        ],
    );
    let sql = "SELECT SRID, AUTH_NAME, AUTH_SRID, SUBSTRING(SRTEXT FROM 1 FOR 6) FROM SPATIAL_REF_SYS WHERE SRID IN (101, 4326, 900001) ORDER BY SRID";
    assert_eq!(rows(&a, sql).unwrap(), ["101,POSC,32214,PROJCS", "4326,EPSG,4326,GEOGCS", "900001,NULL,NULL,GEOGCS"]);
    let counts = "SELECT count(*) FROM SPATIAL_REF_SYS UNION ALL SELECT count(*) FROM ST_INFORMTN_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS";
    let both = rows(&a, counts).unwrap();
    assert_eq!(both.first(), both.get(1));
    let unknown = "SELECT count(*) FROM GEOMETRY_COLUMNS AS g WHERE g.SRID <> 0 AND NOT EXISTS (SELECT * FROM SPATIAL_REF_SYS AS s WHERE s.SRID = g.SRID)";
    assert_eq!(rows(&a, unknown).unwrap(), ["0"]);
    assert_eq!(
        described(&a, "SPATIAL_REF_SYS"),
        ["SRID,INTEGER,NO,NULL", "AUTH_NAME,CHARACTER VARYING,YES,128", "AUTH_SRID,INTEGER,YES,NULL", "SRTEXT,CHARACTER VARYING,NO,2048"]
    );
}
