//! The methods of SQL/MM Spatial's types [ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §5 to §10]: each type's own and those it inherits, the
//! overriding ST_Geometries of the collection types, `(v AS T).m()`,
//! SPECIFICTYPE of a geometry value, and the large objects ST_AsText and
//! ST_AsBinary return (RULES.md D-SPATIAL-FAMILY).

use turborel::db::Database;
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{rows, state};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G ST_GEOMETRY, C ST_GEOMCOLLECTION, M ST_MULTIPOINT, P ST_POINT)")
        .unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO T VALUES (1, ST_POINT(1, 2), TREAT(ST_GEOMFROMTEXT('GEOMETRYCOLLECTION (POINT (1 2), POINT (3 4))') AS ST_GEOMCOLLECTION), \
         TREAT(ST_GEOMFROMTEXT('MULTIPOINT ((5 6))') AS ST_MULTIPOINT), ST_POINT(7, 8))",
    )
    .unwrap();
    a.commit().unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR10b, §9.19 SR3] [ISO/IEC
/// 13249-3:2016 (DIS, SC32 N2593) §5.1.1, §6.1.1] a method is invoked of a
/// value whose declared type, or a supertype of it, declares it: ST_X of
/// an ST_Point, ST_SRID and ST_AsText of every type; ST_X of a value
/// declared ST_Geometry is refused (SQLCODE -103) until TREAT makes it an
/// ST_Point.
#[test]
fn methods_are_their_types_and_inherited() {
    let db = database();
    let a = db.session("A");
    assert_eq!(rows(&a, "SELECT P.ST_X(), P.ST_SRID(), M.ST_SRID(), G.ST_GEOMETRYTYPE() FROM T").unwrap(), ["7E0,0,0,ST_Point"]);
    assert_eq!(rows(&a, "SELECT G.ST_X() FROM T").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT TREAT(G AS ST_POINT).ST_Y() FROM T").unwrap(), ["2E0"]);
    assert_eq!(rows(&a, "SELECT P.ST_SRID(4326).ST_SRID() FROM T").unwrap(), ["4326"]);
    assert_eq!(rows(&a, "SELECT P.ST_INTERSECTS(ST_POINT(7, 8)), P.ST_DISTANCE(ST_POINT(7, 5)) FROM T").unwrap(), ["1,3E0"]);
    assert_eq!(rows(&a, "SELECT P.ST_NUMGEOMETRIES() FROM T").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT P.ST_SRID(1, 2) FROM T").unwrap_err(), Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §10.1.3 to §10.1.5,
/// §10.2.3] [ISO/IEC 9075-2:2023 §9.18 SR10b, GR3b] ST_NumGeometries,
/// ST_GeometryN and ST_Geometries of a collection: ST_MultiPoint's
/// overriding ST_Geometries returns ST_Point ARRAY, so its elements have
/// ST_X; `(M AS ST_GEOMCOLLECTION)` invokes ST_GeomCollection's, of
/// ST_Geometry ARRAY; a position out of range gives null and the warning
/// 01F01; the mutator checks its array (2FF03, 2FF08, 2FF10).
#[test]
fn collections_override_their_elements() {
    let db = database();
    let a = db.session("A");
    assert_eq!(rows(&a, "SELECT C.ST_NUMGEOMETRIES(), M.ST_NUMGEOMETRIES() FROM T").unwrap(), ["2,1"]);
    assert_eq!(rows(&a, "SELECT C.ST_GEOMETRYN(2).ST_ASTEXT() FROM T").unwrap(), ["POINT (3 4)"]);
    assert_eq!(rows(&a, "SELECT M.ST_GEOMETRIES()[1].ST_X() FROM T").unwrap(), ["5E0"]);
    assert_eq!(rows(&a, "SELECT (M AS ST_GEOMCOLLECTION).ST_GEOMETRIES()[1].ST_X() FROM T").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT CARDINALITY((M AS ST_GEOMCOLLECTION).ST_GEOMETRIES()) FROM T").unwrap(), ["1"]);
    assert_eq!(rows(&a, "SELECT (P AS ST_GEOMCOLLECTION).ST_NUMGEOMETRIES() FROM T").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT (C AS ST_MULTIPOINT).ST_NUMGEOMETRIES() FROM T").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT C.ST_GEOMETRYN(3) FROM T").unwrap(), ["NULL"]);
    assert_eq!(a.warning().map(|w| w.as_str().to_owned()).as_deref(), Some("01F01"));
    assert_eq!(rows(&a, "SELECT C.ST_GEOMETRYN(0) IS NULL FROM T").unwrap(), ["TRUE"]);
    let mutated = "SELECT M.ST_GEOMETRIES(ARRAY[ST_POINT(1, 1), ST_POINT(2, 2)]).ST_ASTEXT() FROM T";
    assert_eq!(rows(&a, mutated).unwrap(), ["MULTIPOINT ((1 1), (2 2))"]);
    assert_eq!(state(&a, "SELECT M.ST_GEOMETRIES(ARRAY[ST_POINT(1, 1), CAST(NULL AS ST_POINT)]) FROM T"), "2FF03");
    assert_eq!(state(&a, "SELECT M.ST_GEOMETRIES(ARRAY[ST_POINT(1, 1, 4326)]) FROM T"), "2FF10");
    let line = "ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)')";
    assert_eq!(state(&a, &format!("SELECT M.ST_GEOMETRIES(ARRAY[{line}]) FROM T")), "2FF08");
    assert_eq!(rows(&a, &format!("SELECT C.ST_GEOMETRIES(ARRAY[{line}]).ST_GEOMETRYTYPE() FROM T")).unwrap(), ["ST_GeomCollection"]);
    assert_eq!(state(&a, "SELECT ST_NUMGEOMETRIES(G) FROM T"), "2FF02");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.33 GR15] SPECIFICTYPE of a geometry
/// value names its most specific type, in the catalog's INFORMATION_SCHEMA;
/// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.47, §5.1.48] ST_AsText is a
/// CHARACTER LARGE OBJECT and ST_AsBinary a BINARY LARGE OBJECT.
#[test]
fn specific_types_and_large_object_results() {
    let db = database();
    let a = db.session("A");
    assert_eq!(
        rows(&a, "SELECT G.SPECIFICTYPE, T.C.SPECIFICTYPE() FROM T").unwrap(),
        ["\"TURBOREL\".\"INFORMATION_SCHEMA\".\"ST_POINT\",\"TURBOREL\".\"INFORMATION_SCHEMA\".\"ST_GEOMCOLLECTION\""]
    );
    let types = a.prepare("SELECT ST_ASTEXT(G), G.ST_ASBINARY() FROM T").unwrap();
    assert_eq!(
        types.columns().iter().map(|c| c.data_type.to_string()).collect::<Vec<_>>(),
        ["CHARACTER LARGE OBJECT(16777216)", "BINARY LARGE OBJECT(16777216)"]
    );
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(P.ST_ASTEXT()), OCTET_LENGTH(P.ST_ASBINARY()) FROM T").unwrap(), ["11,21"]);
}
