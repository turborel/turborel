//! [OGC 06-104r4 Annex C.3.3] The conformance tests of the "SQL with
//! Geometry Types" environment: the Blue Lake schema and data, and the
//! queries T1 to T52 with their answers. POSITION, a key word of SQL, is
//! delimited; T37 compares in SRID 101, as the value it compares has it.

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::rows;

/// SRID 101, the POSC system of Annex C.3.3.2, as ST_DEFINESRS defines it.
pub const UTM: &str = "CALL ST_DEFINESRS('UTM_ZONE_14N', 101, 'POSC', 32214, 'PROJCS[\"UTM_ZONE_14N\", GEOGCS[\"World Geodetic System 72\", DATUM[\"WGS_72\", SPHEROID[\"NWL_10D\", 6378135, 298.26]], PRIMEM[\"Greenwich\", 0], UNIT[\"Meter\", 1.0]], PROJECTION[\"Transverse_Mercator\"], PARAMETER[\"False_Easting\", 500000.0], PARAMETER[\"False_Northing\", 0.0], PARAMETER[\"Central_Meridian\", -99.0], PARAMETER[\"Scale_Factor\", 0.9996], PARAMETER[\"Latitude_of_origin\", 0.0], UNIT[\"Meter\", 1.0]]', 'Blue Lake')";

/// [Annex C.3.3.1] The feature tables.
const SCHEMA: [&str; 10] = [
    "CREATE TABLE lakes (fid INTEGER NOT NULL PRIMARY KEY, name CHARACTER VARYING(64), shore POLYGON)",
    "CREATE TABLE road_segments (fid INTEGER NOT NULL PRIMARY KEY, name CHARACTER VARYING(64), aliases CHARACTER VARYING(64), num_lanes INTEGER, centerline LINESTRING)",
    "CREATE TABLE divided_routes (fid INTEGER NOT NULL PRIMARY KEY, name CHARACTER VARYING(64), num_lanes INTEGER, centerlines MULTILINESTRING)",
    "CREATE TABLE forests (fid INTEGER NOT NULL PRIMARY KEY, name CHARACTER VARYING(64), boundary MULTIPOLYGON)",
    "CREATE TABLE bridges (fid INTEGER NOT NULL PRIMARY KEY, name CHARACTER VARYING(64), \"POSITION\" POINT)",
    "CREATE TABLE streams (fid INTEGER NOT NULL PRIMARY KEY, name CHARACTER VARYING(64), centerline LINESTRING)",
    "CREATE TABLE buildings (fid INTEGER NOT NULL PRIMARY KEY, address CHARACTER VARYING(64), \"POSITION\" POINT, footprint POLYGON)",
    "CREATE TABLE ponds (fid INTEGER NOT NULL PRIMARY KEY, name CHARACTER VARYING(64), type CHARACTER VARYING(64), shores MULTIPOLYGON)",
    "CREATE TABLE named_places (fid INTEGER NOT NULL PRIMARY KEY, name CHARACTER VARYING(64), boundary POLYGON)",
    "CREATE TABLE map_neatlines (fid INTEGER NOT NULL PRIMARY KEY, neatline POLYGON)",
];

/// [Annex C.3.3.2] The data.
const DATA: [&str; 19] = [
    "INSERT INTO lakes VALUES (101, 'Blue Lake', PolyFromText('POLYGON((52 18,66 23,73 9,48 6,52 18),(59 18,67 18,67 13,59 13,59 18))', 101))",
    "INSERT INTO road_segments VALUES (102, 'Route 5', NULL, 2, LineFromText('LINESTRING( 0 18, 10 21, 16 23, 28 26, 44 31 )', 101))",
    "INSERT INTO road_segments VALUES (103, 'Route 5', 'Main Street', 4, LineFromText('LINESTRING( 44 31, 56 34, 70 38 )', 101))",
    "INSERT INTO road_segments VALUES (104, 'Route 5', NULL, 2, LineFromText('LINESTRING( 70 38, 72 48 )', 101))",
    "INSERT INTO road_segments VALUES (105, 'Main Street', NULL, 4, LineFromText('LINESTRING( 70 38, 84 42 )', 101))",
    "INSERT INTO road_segments VALUES (106, 'Dirt Road by Green Forest', NULL, 1, LineFromText('LINESTRING( 28 26, 28 0 )', 101))",
    "INSERT INTO divided_routes VALUES (119, 'Route 75', 4, MLineFromText('MULTILINESTRING((10 48,10 21,10 0),(16 0,16 23,16 48))', 101))",
    "INSERT INTO forests VALUES (109, 'Green Forest', MPolyFromText('MULTIPOLYGON(((28 26,28 0,84 0,84 42,28 26),(52 18,66 23,73 9,48 6,52 18)),((59 18,67 18,67 13,59 13,59 18)))', 101))",
    "INSERT INTO bridges VALUES (110, 'Cam Bridge', PointFromText('POINT( 44 31 )', 101))",
    "INSERT INTO streams VALUES (111, 'Cam Stream', LineFromText('LINESTRING( 38 48, 44 41, 41 36, 44 31, 52 18 )', 101))",
    "INSERT INTO streams VALUES (112, NULL, LineFromText('LINESTRING( 76 0, 78 4, 73 9 )', 101))",
    "INSERT INTO buildings VALUES (113, '123 Main Street', PointFromText('POINT( 52 30 )', 101), PolyFromText('POLYGON( ( 50 31, 54 31, 54 29, 50 29, 50 31) )', 101))",
    "INSERT INTO buildings VALUES (114, '215 Main Street', PointFromText('POINT( 64 33 )', 101), PolyFromText('POLYGON( ( 66 34, 62 34, 62 32, 66 32, 66 34) )', 101))",
    "INSERT INTO ponds VALUES (120, NULL, 'Stock Pond', MPolyFromText('MULTIPOLYGON( ( ( 24 44, 22 42, 24 40, 24 44) ), ( ( 26 44, 26 40, 28 42, 26 44) ) )', 101))",
    "INSERT INTO named_places VALUES (117, 'Ashton', PolyFromText('POLYGON( ( 62 48, 84 48, 84 30, 56 30, 56 34, 62 48) )', 101))",
    "INSERT INTO named_places VALUES (118, 'Goose Island', PolyFromText('POLYGON( ( 67 13, 67 18, 59 18, 59 13, 67 13) )', 101))",
    "INSERT INTO map_neatlines VALUES (115, PolyFromText('POLYGON( ( 0 0, 0 48, 84 48, 84 0, 0 0 ) )', 101))",
    "CALL ST_REGISTERGEOMETRYCOLUMN('A', 'STREAMS', 'CENTERLINE', 'UTM_ZONE_14N')",
    "CALL ST_REGISTERGEOMETRYCOLUMN('A', 'LAKES', 'SHORE', 'UTM_ZONE_14N')",
];

/// A database of the Blue Lake data in schema A, and A's session.
pub fn blue_lake() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for sql in std::iter::once(UTM).chain(SCHEMA).chain(DATA) {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    (db, a)
}

/// [Annex C.3.3.3] Each query and its answer.
const QUERIES: [(&str, &str); 47] = [
    ("SELECT f_geometry_column FROM geometry_columns WHERE f_table_name = 'STREAMS'", "CENTERLINE"),
    ("SELECT coord_dimension FROM geometry_columns WHERE f_table_name = 'STREAMS'", "2"),
    ("SELECT srid FROM geometry_columns WHERE f_table_name = 'STREAMS'", "101"),
    ("SELECT Dimension(shore) FROM lakes WHERE name = 'Blue Lake'", "2"),
    ("SELECT GeometryType(centerlines) FROM divided_routes WHERE name = 'Route 75'", "MULTILINESTRING"),
    ("SELECT AsText(boundary) FROM named_places WHERE name = 'Goose Island'", "POLYGON ((67 13, 67 18, 59 18, 59 13, 67 13))"),
    (
        "SELECT AsText(PolyFromWKB(AsBinary(boundary), 101)) FROM named_places WHERE name = 'Goose Island'",
        "POLYGON ((67 13, 67 18, 59 18, 59 13, 67 13))",
    ),
    ("SELECT SRID(boundary) FROM named_places WHERE name = 'Goose Island'", "101"),
    ("SELECT IsEmpty(centerline) FROM road_segments WHERE name = 'Route 5' AND aliases = 'Main Street'", "0"),
    ("SELECT IsSimple(shore) FROM lakes WHERE name = 'Blue Lake'", "1"),
    ("SELECT AsText(Boundary(boundary)) FROM named_places WHERE name = 'Goose Island'", "LINESTRING (67 13, 67 18, 59 18, 59 13, 67 13)"),
    ("SELECT AsText(Envelope(boundary)) FROM named_places WHERE name = 'Goose Island'", "POLYGON ((59 13, 67 13, 67 18, 59 18, 59 13))"),
    ("SELECT X(\"POSITION\") FROM bridges WHERE name = 'Cam Bridge'", "4.4E1"),
    ("SELECT Y(\"POSITION\") FROM bridges WHERE name = 'Cam Bridge'", "3.1E1"),
    ("SELECT AsText(StartPoint(centerline)) FROM road_segments WHERE fid = 102", "POINT (0 18)"),
    ("SELECT AsText(EndPoint(centerline)) FROM road_segments WHERE fid = 102", "POINT (44 31)"),
    ("SELECT IsClosed(LineFromWKB(AsBinary(Boundary(boundary)), SRID(boundary))) FROM named_places WHERE name = 'Goose Island'", "1"),
    ("SELECT IsRing(LineFromWKB(AsBinary(Boundary(boundary)), SRID(boundary))) FROM named_places WHERE name = 'Goose Island'", "1"),
    ("SELECT Length(centerline) FROM road_segments WHERE fid = 106", "2.6E1"),
    ("SELECT NumPoints(centerline) FROM road_segments WHERE fid = 102", "5"),
    ("SELECT AsText(PointN(centerline, 1)) FROM road_segments WHERE fid = 102", "POINT (0 18)"),
    ("SELECT AsText(Centroid(boundary)) FROM named_places WHERE name = 'Goose Island'", "POINT (63 15.5)"),
    ("SELECT Contains(boundary, PointOnSurface(boundary)) FROM named_places WHERE name = 'Goose Island'", "1"),
    ("SELECT Area(boundary) FROM named_places WHERE name = 'Goose Island'", "4E1"),
    ("SELECT AsText(ExteriorRing(shore)) FROM lakes WHERE name = 'Blue Lake'", "LINESTRING (52 18, 66 23, 73 9, 48 6, 52 18)"),
    ("SELECT NumInteriorRing(shore) FROM lakes WHERE name = 'Blue Lake'", "1"),
    ("SELECT AsText(InteriorRingN(shore, 1)) FROM lakes WHERE name = 'Blue Lake'", "LINESTRING (59 18, 67 18, 67 13, 59 13, 59 18)"),
    ("SELECT NumGeometries(centerlines) FROM divided_routes WHERE name = 'Route 75'", "2"),
    ("SELECT AsText(GeometryN(centerlines, 2)) FROM divided_routes WHERE name = 'Route 75'", "LINESTRING (16 0, 16 23, 16 48)"),
    ("SELECT IsClosed(centerlines) FROM divided_routes WHERE name = 'Route 75'", "0"),
    ("SELECT Length(centerlines) FROM divided_routes WHERE name = 'Route 75'", "9.6E1"),
    ("SELECT AsText(Centroid(shores)) FROM ponds WHERE fid = 120", "POINT (25 42)"),
    ("SELECT Contains(shores, PointOnSurface(shores)) FROM ponds WHERE fid = 120", "1"),
    ("SELECT Area(shores) FROM ponds WHERE fid = 120", "8E0"),
    (
        "SELECT Equals(boundary, PolyFromText('POLYGON( ( 67 13, 67 18, 59 18, 59 13, 67 13) )', 101)) FROM named_places WHERE name = 'Goose Island'",
        "1",
    ),
    (
        "SELECT Disjoint(centerlines, boundary) FROM divided_routes, named_places WHERE divided_routes.name = 'Route 75' AND named_places.name = 'Ashton'",
        "1",
    ),
    ("SELECT Touches(centerline, shore) FROM streams, lakes WHERE streams.name = 'Cam Stream' AND lakes.name = 'Blue Lake'", "1"),
    (
        "SELECT Within(boundary, footprint) FROM named_places, buildings WHERE named_places.name = 'Ashton' AND buildings.address = '215 Main Street'",
        "0",
    ),
    (
        "SELECT Overlaps(forests.boundary, named_places.boundary) FROM forests, named_places WHERE forests.name = 'Green Forest' AND named_places.name = 'Ashton'",
        "1",
    ),
    (
        "SELECT Crosses(road_segments.centerline, divided_routes.centerlines) FROM road_segments, divided_routes WHERE road_segments.fid = 102 AND divided_routes.name = 'Route 75'",
        "1",
    ),
    (
        "SELECT Intersects(road_segments.centerline, divided_routes.centerlines) FROM road_segments, divided_routes WHERE road_segments.fid = 102 AND divided_routes.name = 'Route 75'",
        "1",
    ),
    (
        "SELECT Contains(forests.boundary, named_places.boundary) FROM forests, named_places WHERE forests.name = 'Green Forest' AND named_places.name = 'Ashton'",
        "0",
    ),
    (
        "SELECT Relate(forests.boundary, named_places.boundary, 'TTTTTTTTT') FROM forests, named_places WHERE forests.name = 'Green Forest' AND named_places.name = 'Ashton'",
        "1",
    ),
    (
        "SELECT Distance(\"POSITION\", boundary) FROM bridges, named_places WHERE bridges.name = 'Cam Bridge' AND named_places.name = 'Ashton'",
        "1.2E1",
    ),
    (
        "SELECT AsText(Intersection(centerline, shore)) FROM streams, lakes WHERE streams.name = 'Cam Stream' AND lakes.name = 'Blue Lake'",
        "POINT (52 18)",
    ),
    (
        "SELECT AsText(Difference(named_places.boundary, forests.boundary)) FROM named_places, forests WHERE named_places.name = 'Ashton' AND forests.name = 'Green Forest'",
        "POLYGON ((84 48, 62 48, 56 34, 84 42, 84 48))",
    ),
    (
        "SELECT AsText(Union(shore, boundary)) FROM lakes, named_places WHERE lakes.name = 'Blue Lake' AND named_places.name = 'Goose Island'",
        "POLYGON ((66 23, 52 18, 48 6, 73 9, 66 23))",
    ),
];

/// [Annex C.3.3.3] The queries past T49, T1 and T5.
const MORE: [(&str, &str); 4] = [
    (
        "SELECT AsText(SymDifference(shore, boundary)) FROM lakes, named_places WHERE lakes.name = 'Blue Lake' AND named_places.name = 'Ashton'",
        "MULTIPOLYGON (((66 23, 52 18, 48 6, 73 9, 66 23), (59 18, 67 18, 67 13, 59 13, 59 18)), ((84 48, 62 48, 56 34, 56 30, 84 30, 84 48)))",
    ),
    ("SELECT count(*) FROM buildings, bridges WHERE Contains(Buffer(bridges.\"POSITION\", 15.0), buildings.footprint) = 1", "1"),
    ("SELECT AsText(ConvexHull(shore)) FROM lakes WHERE lakes.name = 'Blue Lake'", "POLYGON ((48 6, 73 9, 66 23, 52 18, 48 6))"),
    ("SELECT auth_name || ':' || CAST(auth_srid AS VARCHAR(9)) FROM SPATIAL_REF_SYS WHERE SRID = 101", "POSC:32214"),
];

/// Verifies: [OGC 06-104r4 Annex C.3.3.3 T1 to T52] Each conformance
/// query of the "SQL with Geometry Types" environment over the Blue Lake
/// data, by the names Simple feature access writes, has its answer.
#[test]
fn blue_lake_queries_have_their_answers() {
    let (_db, a) = blue_lake();
    let tables = rows(&a, "SELECT f_table_name FROM geometry_columns ORDER BY 1, f_geometry_column").unwrap();
    let expected = [
        "BRIDGES",
        "BUILDINGS",
        "BUILDINGS",
        "DIVIDED_ROUTES",
        "FORESTS",
        "LAKES",
        "MAP_NEATLINES",
        "NAMED_PLACES",
        "PONDS",
        "ROAD_SEGMENTS",
        "STREAMS",
    ];
    assert_eq!(tables, expected);
    let text = rows(&a, "SELECT srtext FROM SPATIAL_REF_SYS WHERE SRID = 101").unwrap();
    assert!(text.first().is_some_and(|t| t.starts_with("PROJCS[\"UTM_ZONE_14N\"")), "{text:?}");
    for (sql, answer) in QUERIES.iter().chain(&MORE) {
        assert_eq!(rows(&a, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}")), [*answer], "{sql}");
    }
}
