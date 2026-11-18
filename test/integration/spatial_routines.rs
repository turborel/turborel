//! The routines of SQL/MM Spatial's curves, surfaces and representations
//! through SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.13 to
//! §5.1.15, §5.1.56 to §5.1.64, §6 to §10]: well-known text, binary and
//! GML of every instantiable type and each type's constructors of them,
//! the curve observers, the surface measures, rings and patches, and the
//! measure locators (RULES.md D124).

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::{rows, state};

fn session() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE ONE (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO ONE VALUES (1)").unwrap();
    (db, a)
}

/// The one value `sql` selects, as text.
fn one(a: &Session, sql: &str) -> String {
    let query = format!("SELECT {sql} FROM ONE");
    let mut c = a.open_cursor(&query).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let row = c.fetch().unwrap_or_else(|e| panic!("{sql}: {e}")).unwrap_or_default();
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

/// The value `sql` gives and the warning it raises.
fn warned(a: &Session, sql: &str) -> String {
    let value = rows(a, &format!("SELECT {sql} FROM ONE")).unwrap_or_default().join(",");
    format!("{value} {}", a.warning().map(|w| w.as_str().to_owned()).unwrap_or_default())
}

/// The well-known text of the value `sql` gives.
fn wkt(a: &Session, sql: &str) -> String {
    one(a, &format!("CAST(ST_ASTEXT({sql}) AS VARCHAR(4000))"))
}

/// One well-known text of each of the 23 instantiable types.
const TEXTS: [(&str, &str); 23] = [
    ("ST_POINTFROMTEXT", "POINT Z (1 2 3)"),
    ("ST_LINEFROMTEXT", "LINESTRING M (0 0 1, 1 1 2)"),
    ("ST_CIRCULARFROMTXT", "CIRCULARSTRING (0 0, 1 1, 2 0)"),
    ("ST_CIRCLEFROMTXT", "CIRCLE (0 0, 1 1, 2 0)"),
    ("ST_GEODESICFROMTXT", "GEODESICSTRING (0 0, 1 1)"),
    (
        "ST_ELLIPTICFROMTXT",
        "ELLIPTICALCURVE (REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), REFERENCEDIRECTIONS (VECTOR (1 0), VECTOR (0 1))), UAXISLENGTH 2, VAXISLENGTH 1, STARTANGLE RADIANS (0), ENDANGLE RADIANS (1.5))",
    ),
    (
        "ST_NURBSFROMTXT",
        "NURBSCURVE (DEGREE 2, CONTROLPOINTS (NURBSPOINT (WEIGHTEDPOINT (0 0), WEIGHT 1), NURBSPOINT (WEIGHTEDPOINT (2 2), WEIGHT 2), \
         NURBSPOINT (WEIGHTEDPOINT (2 0), WEIGHT 1)), KNOTS (KNOT (VALUE 0, MULTIPLICITY 3), KNOT (VALUE 1, MULTIPLICITY 3)))",
    ),
    (
        "ST_CLOTHOIDFROMTXT",
        "CLOTHOID (REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), REFERENCEDIRECTIONS (VECTOR (1 0), VECTOR (0 1))), SCALEFACTOR 10, STARTDISTANCE 0, ENDDISTANCE 5)",
    ),
    (
        "ST_SPIRALFROMTXT",
        "SPIRALCURVE (REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), REFERENCEDIRECTIONS (VECTOR (1 0), VECTOR (0 1))), LENGTH 10, STARTCURVATURE 0, ENDCURVATURE 0.1, SPIRALTYPE clothoid)",
    ),
    ("ST_COMPOUNDFROMTXT", "COMPOUNDCURVE (LINESTRING (0 0, 1 0), CIRCULARSTRING (1 0, 2 1, 3 0))"),
    ("ST_CPOLYFROMTEXT", "CURVEPOLYGON (CIRCULARSTRING (0 0, 2 2, 4 0, 2 -2, 0 0))"),
    ("ST_POLYFROMTEXT", "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 2, 2 2, 2 1, 1 1))"),
    ("ST_TRIFROMTEXT", "TRIANGLE (0 0, 1 0, 0 1)"),
    ("ST_PHSFROMTEXT", "POLYHEDRALSURFACE Z (PATCHES (POLYGON ((0 0 0, 1 0 0, 1 1 0, 0 0 0)), POLYGON ((0 0 0, 1 1 0, 0 1 0, 0 0 0))))"),
    ("ST_TINFROMTEXT", "TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)) MAXSIDELENGTH 3)"),
    ("ST_COMPSURFFROMTXT", "COMPOUNDSURFACE (POLYGON ((0 0, 1 0, 1 1, 0 0)), POLYGON ((0 0, 1 1, 0 1, 0 0)))"),
    (
        "ST_BREPFROMTEXT",
        "BREPSOLID Z (POLYHEDRALSURFACE Z (PATCHES (POLYGON ((0 0 0, 1 0 0, 0 1 0, 0 0 0)), POLYGON ((0 0 0, 0 1 0, 0 0 1, 0 0 0)), \
         POLYGON ((0 0 0, 0 0 1, 1 0 0, 0 0 0)), POLYGON ((1 0 0, 0 0 1, 0 1 0, 1 0 0)))))",
    ),
    ("ST_GEOMCOLLFROMTXT", "GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 1 1))"),
    ("ST_MPOINTFROMTEXT", "MULTIPOINT ((1 1), (2 2))"),
    ("ST_MCURVEFROMTEXT", "MULTICURVE (CIRCULARSTRING (0 0, 1 1, 2 0), LINESTRING (0 0, 1 1))"),
    ("ST_MLINEFROMTEXT", "MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))"),
    ("ST_MSURFACEFROMTXT", "MULTISURFACE (CURVEPOLYGON (CIRCULARSTRING (0 0, 2 2, 4 0, 2 -2, 0 0)), POLYGON ((5 5, 6 5, 6 6, 5 5)))"),
    ("ST_MPOLYFROMTEXT", "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))"),
];

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.57, §5.1.62,
/// §6.1.8, §7.2.8 to §7.10.8, §8.2.8 to §8.7.6, §9.2.7, §10.1.6 to
/// §10.6.4] each type's FromText function reads its text, and ST_AsText
/// writes back a text that reads to the same value.
#[test]
fn every_type_reads_and_writes_its_well_known_text() {
    let (_db, a) = session();
    for (f, text) in TEXTS {
        let written = one(&a, &format!("CAST(ST_ASTEXT({f}('{text}', 4326)) AS VARCHAR(2000))"));
        let again = one(&a, &format!("CAST(ST_ASTEXT(ST_GEOMFROMTEXT('{written}', 4326)) AS VARCHAR(2000))"));
        assert_eq!(written, again, "{f}");
        assert_eq!(one(&a, &format!("ST_SRID({f}('{text}', 4326))")), "4326", "{f}");
        assert_eq!(one(&a, &format!("ST_SRID(ST_WKTTOSQL('{text}'))")), "0", "{f}");
    }
    assert_eq!(one(&a, "CAST(ST_ASTEXT(ST_POINTFROMTEXT('POINT Z (1 2 3)')) AS VARCHAR(100))"), "POINT Z (1 2 3)");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6.1.8 Description
/// 2, §6.1.9, §6.1.10] a typed constructor of a value of another type
/// raises the representation's exception: 2FF22 of text, 2FF23 of
/// binary, 2FF24 of GML.
#[test]
fn typed_constructors_refuse_other_types() {
    let (_db, a) = session();
    assert_eq!(state(&a, "VALUES ST_POINTFROMTEXT('LINESTRING (0 0, 1 1)')"), "2FF22");
    assert_eq!(state(&a, "VALUES ST_POINTFROMWKB(ST_ASBINARY(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)')))"), "2FF23");
    assert_eq!(state(&a, "VALUES ST_POINTFROMGML(ST_ASGML(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)')))"), "2FF24");
    assert_eq!(state(&a, "VALUES ST_CURVEN(ST_COMPOUNDFROMTXT('NOT TEXT'), 1)"), "2FF22");
    assert_eq!(one(&a, "ST_GEOMETRYTYPE(ST_CPOLYFROMTEXT('POLYGON ((0 0, 1 0, 1 1, 0 0))'))"), "ST_Polygon");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.58, §5.1.59,
/// §5.1.63, §6.1.9 to §10.6.5] each type's FromWKB function reads the
/// well-known binary ST_AsBinary writes.
#[test]
fn every_type_reads_its_well_known_binary() {
    let (_db, a) = session();
    for (f, text) in TEXTS {
        let wkb = f.replace("FROMTEXT", "FROMWKB").replace("FROMTXT", "FROMWKB");
        let expected = wkt(&a, &format!("ST_GEOMFROMTEXT('{text}')"));
        assert_eq!(wkt(&a, &format!("{wkb}(ST_ASBINARY(ST_GEOMFROMTEXT('{text}')), 26918)")), expected, "{wkb}");
        assert_eq!(one(&a, &format!("ST_SRID({wkb}(ST_ASBINARY(ST_GEOMFROMTEXT('{text}')), 26918))")), "26918", "{wkb}");
        assert_eq!(wkt(&a, &format!("ST_WKBTOSQL(ST_ASBINARY(ST_GEOMFROMTEXT('{text}')))")), expected, "{wkb}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60, §5.1.61,
/// §5.1.64, §6.1.10 to §10.6.6] each type's FromGML function reads the
/// GML ST_AsGML writes, its srsName that of the reference system of the
/// value's SRID as the spatial reference systems view gives it.
#[test]
fn every_type_reads_and_writes_its_gml() {
    let (_db, a) = session();
    for (f, text) in TEXTS {
        let gml = f.replace("FROMTEXT", "FROMGML").replace("FROMTXT", "FROMGML");
        let written = format!("ST_ASGML(ST_GEOMFROMTEXT('{text}', 4326))");
        let again = one(&a, &format!("CAST(ST_ASGML({gml}({written})) AS VARCHAR(4000))"));
        assert_eq!(again, one(&a, &format!("CAST({written} AS VARCHAR(4000))")), "{gml}");
        assert_eq!(one(&a, &format!("ST_SRID({gml}(ST_ASGML(ST_GEOMFROMTEXT('{text}', 4326))))")), "4326", "{gml}");
    }
    let point = "ST_POINTFROMTEXT('POINT (1 2)', 4326)";
    let gml = one(&a, &format!("CAST(ST_ASGML({point}) AS VARCHAR(400))"));
    assert!(gml.contains("srsName=\"EPSG:4326\""), "{gml}");
    assert_eq!(one(&a, &format!("ST_SRID(ST_GMLTOSQL(ST_ASGML({point})))")), "4326");
    assert_eq!(one(&a, &format!("ST_SRID(ST_GEOMFROMGML(ST_ASGML({point}), 0))")), "0");
    let method = one(&a, &format!("CAST({point}.ST_ASGML() AS VARCHAR(400))"));
    assert_eq!(method, gml);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.2 to §7.1.15]
/// the observers of every curve: length, end points, closure, ring,
/// linearization, the distance to and the point at a distance, and the
/// perpendicular projections, as methods and as functions.
#[test]
fn curve_observers_run_as_methods() {
    let (_db, a) = session();
    let arc = "ST_CIRCULARFROMTXT('CIRCULARSTRING (0 0, 1 1, 2 0)')";
    assert_eq!(one(&a, &format!("CAST({arc}.ST_LENGTH() AS DECIMAL(10, 6))")), "3.141593");
    assert_eq!(wkt(&a, &format!("{arc}.ST_STARTPOINT()")), "POINT (0 0)");
    assert_eq!(wkt(&a, &format!("ST_ENDPOINT({arc})")), "POINT (2 0)");
    assert_eq!(one(&a, &format!("{arc}.ST_ISCLOSED()")), "0");
    let circle = "ST_CIRCLEFROMTXT('CIRCLE (0 0, 1 1, 2 0)')";
    assert_eq!(one(&a, &format!("{circle}.ST_ISCLOSED(), {circle}.ST_ISRING()")), "1,1");
    let line = one(&a, &format!("{arc}.ST_CURVETOLINE().ST_NUMPOINTS() > 3"));
    assert_eq!(line, "TRUE");
    let quarter = format!("{arc}.ST_POINTATDISTANCE(1.5707963267948966E0)");
    assert_eq!(one(&a, &format!("CAST(ST_X({quarter}) AS DECIMAL(10, 6)), CAST(ST_Y({quarter}) AS DECIMAL(10, 6))")), "1.000000,1.000000");
    assert_eq!(state(&a, &format!("VALUES {arc}.ST_POINTATDISTANCE(10E0)")), "2FF79");
    let on = format!("{arc}.ST_DISTANCETOPOINT(ST_POINT(2, 0))");
    assert_eq!(one(&a, &format!("CAST({on} AS DECIMAL(10, 6))")), "3.141593");
    assert_eq!(state(&a, &format!("VALUES {arc}.ST_DISTANCETOPOINT(ST_POINT(5, 5))")), "2FF80");
    let foot = format!("TREAT({arc}.ST_PERPPOINTS(ST_POINT(1, 3)) AS ST_POINT)");
    assert_eq!(one(&a, &format!("CAST(ST_X({foot}) AS DECIMAL(10, 6)), CAST(ST_Y({foot}) AS DECIMAL(10, 6))")), "1.000000,1.000000");
    let whole = format!("{arc}.ST_PERPPOINTS(ST_POINT(1, 0))");
    assert_eq!(one(&a, &format!("ST_GEOMETRYTYPE({whole}), TREAT({whole} AS ST_CIRCULARSTRING).ST_NUMPOINTS()")), "ST_CircularString,5");
    assert_eq!(one(&a, "ST_LINEFROMTEXT('LINESTRING EMPTY').ST_LENGTH()"), "NULL");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.2.3 to §7.5.5,
/// §8.4.3] the point lists of line, circular and geodesic strings,
/// circles and triangles, with the warning 01F01 of a position outside
/// them; the segments, middle points, bulge, center and radius of arcs.
#[test]
fn point_lists_and_arcs() {
    let (_db, a) = session();
    let line = "ST_LINEFROMTEXT('LINESTRING (0 0, 1 1, 2 0)')";
    assert_eq!(one(&a, &format!("{line}.ST_NUMPOINTS(), CARDINALITY({line}.ST_POINTS())")), "3,3");
    assert_eq!(wkt(&a, &format!("{line}.ST_POINTN(2)")), "POINT (1 1)");
    assert_eq!(warned(&a, &format!("{line}.ST_POINTN(9)")), "NULL 01F01");
    let two = "ST_CIRCULARFROMTXT('CIRCULARSTRING (0 0, 1 1, 2 0, 3 -1, 4 0)')";
    assert_eq!(one(&a, &format!("{two}.ST_NUMSEGMENTS()")), "2");
    assert_eq!(wkt(&a, &format!("{two}.ST_SEGMENTN(2)")), "CIRCULARSTRING (2 0, 3 -1, 4 0)");
    assert_eq!(one(&a, &format!("CARDINALITY({two}.ST_MIDPOINTREP())")), "5");
    assert_eq!(state(&a, &format!("VALUES {two}.ST_RADIUS()")), "2FF76");
    let arc = "ST_CIRCULARFROMTXT('CIRCULARSTRING (0 0, 1 1, 2 0)')";
    assert_eq!(one(&a, &format!("{arc}.ST_RADIUS(), {arc}.ST_BULGE()")), "1E0,1E0");
    assert_eq!(wkt(&a, &format!("{arc}.ST_CENTER()")), "POINT (1 0)");
    let circle = "ST_CIRCLEFROMTXT('CIRCLE (0 0, 1 1, 2 0)')";
    assert_eq!(wkt(&a, &format!("{circle}.ST_CENTER()")), "POINT (1 0)");
    assert_eq!(wkt(&a, &format!("{circle}.ST_POINTN(3)")), "POINT (2 0)");
    let geodesic = "ST_GEODESICFROMTXT('GEODESICSTRING (0 0, 1 1)')";
    assert_eq!(one(&a, &format!("{geodesic}.ST_NUMPOINTS()")), "2");
    assert_eq!(one(&a, "CARDINALITY(ST_TRIFROMTEXT('TRIANGLE (0 0, 1 0, 0 1)').ST_POINTS())"), "3");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.6.8, §7.7.3,
/// §7.8.4 to §7.8.8, §7.9.5 to §7.9.9, §7.10.3 to §7.10.5] the parameters
/// of the parametric curves and the parts of a compound curve.
#[test]
fn parametric_curves_and_compound_curves() {
    let (_db, a) = session();
    let placement = "REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), REFERENCEDIRECTIONS (VECTOR (1 0), VECTOR (0 1)))";
    let clothoid =
        format!("ST_CLOTHOIDFROMTXT('CLOTHOID M ({placement}, SCALEFACTOR 10, STARTDISTANCE 0, ENDDISTANCE 5, STARTM 1, ENDM 2)')");
    let got = one(&a, &format!("{clothoid}.ST_SCALEFACTOR(), {clothoid}.ST_STARTDISTANCE(), {clothoid}.ST_ENDDISTANCE()"));
    assert_eq!(got, "1E1,0E0,5E0");
    assert_eq!(one(&a, &format!("{clothoid}.ST_STARTM(), {clothoid}.ST_ENDM()")), "1E0,2E0");
    let spiral = format!("ST_SPIRALFROMTXT('SPIRALCURVE ({placement}, LENGTH 10, STARTCURVATURE 0, ENDCURVATURE 0.1, SPIRALTYPE bloss)')");
    let got = one(&a, &format!("{spiral}.ST_STARTCURVATURE(), {spiral}.ST_ENDCURVATURE(), {spiral}.ST_SPIRALTYPE(), {spiral}.ST_STARTM()"));
    assert_eq!(got, "0E0,1E-1,bloss,NULL");
    assert_eq!(one(&a, &format!("CAST({spiral}.ST_LENGTH() AS DECIMAL(10, 6))")), "10.000000");
    let nurbs = "ST_NURBSFROMTXT('NURBSCURVE (DEGREE 1, CONTROLPOINTS (NURBSPOINT (WEIGHTEDPOINT (0 0), WEIGHT 1), \
                 NURBSPOINT (WEIGHTEDPOINT (1 0), WEIGHT 1)), KNOTS (KNOT (VALUE 0, MULTIPLICITY 2), KNOT (VALUE 1, MULTIPLICITY 2)))')";
    assert_eq!(one(&a, &format!("{nurbs}.ST_DEGREE()")), "1");
    let compound = "ST_COMPOUNDFROMTXT('COMPOUNDCURVE (LINESTRING (0 0, 1 0), CIRCULARSTRING (1 0, 2 1, 3 0))')";
    assert_eq!(one(&a, &format!("{compound}.ST_NUMCURVES(), CARDINALITY({compound}.ST_CURVES())")), "2,2");
    assert_eq!(wkt(&a, &format!("{compound}.ST_CURVEN(2)")), "CIRCULARSTRING (1 0, 2 1, 3 0)");
    assert_eq!(warned(&a, &format!("{compound}.ST_CURVEN(3)")), "NULL 01F01");
    assert_eq!(state(&a, &format!("VALUES ST_SCALEFACTOR({spiral})")), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.2 to §8.1.8,
/// §10.5.2 to §10.5.8] area, perimeter, centroid and a point on the
/// surface of curve polygons, polygons, polyhedral surfaces, TINs,
/// compound surfaces and multisurfaces.
#[test]
fn surface_measures() {
    let (_db, a) = session();
    let disc = "ST_CPOLYFROMTEXT('CURVEPOLYGON (CIRCULARSTRING (0 0, 2 2, 4 0, 2 -2, 0 0))')";
    assert_eq!(one(&a, &format!("CAST({disc}.ST_AREA() AS DECIMAL(10, 6))")), "12.566371");
    assert_eq!(one(&a, &format!("CAST({disc}.ST_PERIMETER() AS DECIMAL(10, 6))")), "12.566371");
    let centroid = format!("{disc}.ST_CENTROID()");
    assert_eq!(
        one(&a, &format!("CAST(ST_X({centroid}) AS DECIMAL(10, 6)), CAST(ST_Y({centroid}) AS DECIMAL(10, 6))")),
        "2.000000,0.000000"
    );
    let square = "ST_POLYFROMTEXT('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 2, 2 2, 2 1, 1 1))')";
    assert_eq!(one(&a, &format!("{square}.ST_AREA(), {square}.ST_PERIMETER()")), "1.5E1,2E1");
    assert_eq!(one(&a, &format!("ST_INTERSECTS({square}.ST_POINTONSURFACE(), {square})")), "1");
    let phs = "ST_PHSFROMTEXT('POLYHEDRALSURFACE (PATCHES (POLYGON ((0 0, 1 0, 1 1, 0 0)), POLYGON ((0 0, 1 1, 0 1, 0 0))))')";
    assert_eq!(one(&a, &format!("{phs}.ST_AREA(), {phs}.ST_PERIMETER()")), "1E0,4E0");
    assert_eq!(wkt(&a, &format!("{phs}.ST_CENTROID()")), "POINT (0.5 0.5)");
    let multi = "ST_MPOLYFROMTEXT('MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((2 0, 3 0, 3 1, 2 1, 2 0)))')";
    assert_eq!(one(&a, &format!("{multi}.ST_AREA(), {multi}.ST_PERIMETER()")), "2E0,8E0");
    let middle = format!("{multi}.ST_CENTROID()");
    assert_eq!(one(&a, &format!("CAST(ST_X({middle}) AS DECIMAL(10, 6)), CAST(ST_Y({middle}) AS DECIMAL(10, 6))")), "1.500000,0.500000");
    let curved =
        "ST_MSURFACEFROMTXT('MULTISURFACE (CURVEPOLYGON (CIRCULARSTRING (0 0, 2 2, 4 0, 2 -2, 0 0)), ((5 0, 6 0, 6 1, 5 1, 5 0)))')";
    assert_eq!(one(&a, &format!("CAST({curved}.ST_AREA() AS DECIMAL(10, 6))")), "13.566371");
    let compound = "ST_COMPSURFFROMTXT('COMPOUNDSURFACE (POLYGON ((0 0, 1 0, 1 1, 0 0)), POLYGON ((0 0, 1 1, 0 1, 0 0)))')";
    assert_eq!(one(&a, &format!("{compound}.ST_AREA(), {compound}.ST_PERIMETER()")), "1E0,4E0");
    assert_eq!(state(&a, "VALUES ST_AREA(ST_POINT(1, 1))"), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.2.3 to §8.2.7,
/// §8.3.3 to §8.3.5, §8.5.3 to §8.5.5, §8.6.4, §8.6.7, §8.7.3 to §8.7.5]
/// the rings of polygons, the patches of polyhedral surfaces and TINs,
/// the TIN's maximum side length and the parts of compound surfaces.
#[test]
fn rings_patches_and_parts() {
    let (_db, a) = session();
    let square = "ST_POLYFROMTEXT('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 2, 2 2, 2 1, 1 1))')";
    assert_eq!(wkt(&a, &format!("{square}.ST_EXTERIORRING()")), "LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0)");
    assert_eq!(one(&a, &format!("{square}.ST_NUMINTERIORRING(), CARDINALITY({square}.ST_INTERIORRINGS())")), "1,1");
    assert_eq!(wkt(&a, &format!("{square}.ST_INTERIORRINGN(1)")), "LINESTRING (1 1, 1 2, 2 2, 2 1, 1 1)");
    assert_eq!(warned(&a, &format!("{square}.ST_INTERIORRINGN(2)")), "NULL 01F01");
    let disc = "ST_CPOLYFROMTEXT('CURVEPOLYGON (CIRCULARSTRING (0 0, 2 2, 4 0, 2 -2, 0 0))')";
    assert_eq!(one(&a, &format!("ST_GEOMETRYTYPE({disc}.ST_CURVEPOLYTOPOLY())")), "ST_Polygon");
    assert_eq!(wkt(&a, &format!("{disc}.ST_EXTERIORRING()")), "CIRCULARSTRING (0 0, 2 2, 4 0, 2 -2, 0 0)");
    let phs = "ST_PHSFROMTEXT('POLYHEDRALSURFACE (PATCHES (POLYGON ((0 0, 1 0, 1 1, 0 0)), POLYGON ((0 0, 1 1, 0 1, 0 0))))')";
    assert_eq!(one(&a, &format!("{phs}.ST_NUMPATCHES(), CARDINALITY({phs}.ST_PATCHES())")), "2,2");
    assert_eq!(wkt(&a, &format!("{phs}.ST_PATCHN(2)")), "POLYGON ((0 0, 1 1, 0 1, 0 0))");
    let tin = "ST_TINFROMTEXT('TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)) MAXSIDELENGTH 3)')";
    assert_eq!(one(&a, &format!("{tin}.ST_MAXSIDELENGTH(), {tin}.ST_NUMPATCHES()")), "3E0,1");
    assert_eq!(one(&a, &format!("ST_GEOMETRYTYPE({tin}.ST_PATCHN(1))")), "ST_Triangle");
    let compound = "ST_COMPSURFFROMTXT('COMPOUNDSURFACE (POLYGON ((0 0, 1 0, 1 1, 0 0)), POLYGON ((0 0, 1 1, 0 1, 0 0)))')";
    assert_eq!(one(&a, &format!("{compound}.ST_NUMSURFACES(), CARDINALITY({compound}.ST_SURFACES())")), "2,2");
    assert_eq!(wkt(&a, &format!("{compound}.ST_SURFACEN(1)")), "POLYGON ((0 0, 1 0, 1 1, 0 0))");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.13, §5.1.15]
/// ST_LocateAlong and ST_LocateBetween of measured points and curves: a
/// multipoint, a multicurve, the empty point of a value without m, and
/// 2FF02 of a reversed range.
#[test]
fn measures_are_located() {
    let (_db, a) = session();
    let line = "ST_LINEFROMTEXT('LINESTRING M (0 0 0, 10 0 10, 10 10 20)')";
    assert_eq!(wkt(&a, &format!("{line}.ST_LOCATEALONG(5E0)")), "MULTIPOINT M ((5 0 5))");
    assert_eq!(wkt(&a, &format!("{line}.ST_LOCATEBETWEEN(5E0, 15E0)")), "MULTICURVE M ((5 0 5, 10 0 10, 10 5 15))");
    assert_eq!(wkt(&a, "ST_LINEFROMTEXT('LINESTRING (0 0, 1 1)').ST_LOCATEALONG(1E0)"), "POINT EMPTY");
    assert_eq!(wkt(&a, &format!("{line}.ST_LOCATEBETWEEN(25E0, 30E0)")), "POINT M EMPTY");
    assert_eq!(state(&a, &format!("VALUES {line}.ST_LOCATEBETWEEN(2E0, 1E0)")), "2FF02");
    let points = "ST_MPOINTFROMTEXT('MULTIPOINT M ((1 1 1), (2 2 5))')";
    assert_eq!(wkt(&a, &format!("ST_LOCATEBETWEEN({points}, 0E0, 2E0)")), "MULTIPOINT M ((1 1 1))");
}

/// The TIN of five spot heights, a breakline across it, a maximum side.
const SPOTS: &str = "ST_TINELEMENT('random points', 7, 'spots', \
                     ST_MPOINTFROMTEXT('MULTIPOINT Z ((0 0 0), (4 0 1), (4 4 2), (0 4 3), (2 2 5))'))";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.2 to §17.1.6]
/// ST_TINElement's constructors check the type against the geometry
/// (2FF74) and the type's name (2FF02); its observers read and its
/// mutators replace each part.
#[test]
fn tin_elements_are_built_and_read() {
    let (_db, a) = session();
    let got = one(&a, &format!("ST_ELEMENTTYPE({SPOTS}), ST_ELEMENTID({SPOTS}), ST_ELEMENTTAG({SPOTS})"));
    assert_eq!(got, "random points,7,spots");
    assert_eq!(one(&a, &format!("ST_GEOMETRYTYPE(ST_ELEMENTGEOMETRY({SPOTS}))")), "ST_MultiPoint");
    assert_eq!(one(&a, &format!("ST_ELEMENTID(ST_ELEMENTID({SPOTS}, 9)), ST_ELEMENTTAG(ST_ELEMENTTAG({SPOTS}, 'x'))")), "9,x");
    assert_eq!(one(&a, "ST_ELEMENTID(ST_TINELEMENT('stop line', 'cut', ST_LINEFROMTEXT('LINESTRING (0 0, 1 1)')))"), "0");
    assert_eq!(state(&a, "VALUES ST_TINELEMENT('breakline', 'b', ST_LINEFROMTEXT('LINESTRING (0 0, 1 1)'))"), "2FF74");
    assert_eq!(state(&a, "VALUES ST_TINELEMENT('boundary', 'b', ST_LINEFROMTEXT('LINESTRING Z (0 0 0, 1 1 1)'))"), "2FF74");
    assert_eq!(state(&a, "VALUES ST_TINELEMENT('wall', 'b', ST_LINEFROMTEXT('LINESTRING Z (0 0 0, 1 1 1)'))"), "2FF02");
    assert_eq!(state(&a, &format!("VALUES ST_ELEMENTTYPE({SPOTS}, 'hole')")), "2FF74");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.2, §8.6.3,
/// §8.6.4, §8.6.7] ST_TIN of elements is their constrained Delaunay
/// triangulation, of triangles those triangles; ST_TINElements gives
/// the elements back and ST_MaxSideLength the side length.
#[test]
fn tins_are_built_from_elements_and_triangles() {
    let (_db, a) = session();
    let tin = format!("ST_TIN(ARRAY[{SPOTS}], 0E0, 4326)");
    assert_eq!(one(&a, &format!("{tin}.ST_NUMPATCHES(), ST_SRID({tin}), {tin}.ST_MAXSIDELENGTH()")), "4,4326,NULL");
    assert_eq!(one(&a, &format!("CARDINALITY({tin}.ST_TINELEMENTS()), ST_ELEMENTTAG({tin}.ST_TINELEMENTS()[1])")), "1,spots");
    assert_eq!(one(&a, &format!("CAST({tin}.ST_AREA() AS DECIMAL(10, 6))")), "16.000000");
    let kept = format!("ST_TIN(ARRAY[{SPOTS}], 5E0)");
    assert_eq!(one(&a, &format!("{kept}.ST_NUMPATCHES(), {kept}.ST_MAXSIDELENGTH()")), "4,5E0");
    let peeled = format!("ST_TIN(ARRAY[{SPOTS}], 3E0)");
    assert_eq!(one(&a, &format!("{peeled}.ST_ISEMPTY()")), "1");
    let triangles = "ARRAY[ST_TRIFROMTEXT('TRIANGLE Z (0 0 0, 1 0 0, 0 1 1)')]";
    let given = format!("ST_TIN({triangles}, ARRAY[{SPOTS}], 0E0)");
    assert_eq!(one(&a, &format!("{given}.ST_NUMPATCHES(), CARDINALITY({given}.ST_TINELEMENTS())")), "1,1");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.3 Description
/// 4] ST_TINElements(elements, triangulate) replaces a TIN's elements,
/// keeping its triangles at 0 and triangulating the new elements at 1;
/// an element of another system raises 2FF10.
#[test]
fn tin_elements_are_replaced() {
    let (_db, a) = session();
    let tin = "ST_TINFROMTEXT('TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)))')";
    let kept = format!("{tin}.ST_TINELEMENTS(ARRAY[{SPOTS}], 0)");
    assert_eq!(one(&a, &format!("{kept}.ST_NUMPATCHES(), CARDINALITY({kept}.ST_TINELEMENTS())")), "1,1");
    let again = format!("{tin}.ST_TINELEMENTS(ARRAY[{SPOTS}], 1)");
    assert_eq!(one(&a, &format!("{again}.ST_NUMPATCHES(), ST_ELEMENTTAG({again}.ST_TINELEMENTS()[1])")), "4,spots");
    let elsewhere = "ST_TINELEMENT('random points', 'far', ST_MPOINTFROMTEXT('MULTIPOINT Z ((0 0 0), (4 0 1), (4 4 2))', 4326))";
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_TINELEMENTS(ARRAY[{elsewhere}], 1)")), "2FF10");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.11, §7.1.13,
/// §7.3.12, §7.4.5, §7.6.4, §7.6.5, §7.8.5, §7.8.6] The unit forms of
/// ST_DistanceToPoint, ST_PointAtDistance, ST_Radius, ST_UAxisLength,
/// ST_VAxisLength, ST_StartDistance and ST_EndDistance, in a unit of
/// ST_UNITS_OF_MEASURE; SRID 0 measures in metres, and a unit that is no
/// linear unit raises 2FF14.
#[test]
fn unit_forms_measure_in_a_unit() {
    let (_db, a) = session();
    let line = "ST_LINEFROMTEXT('LINESTRING (0 0, 1000 0)')";
    assert_eq!(one(&a, &format!("{line}.ST_DISTANCETOPOINT(ST_POINT(500, 0), 'KILOMETRE')")), "5E-1");
    assert_eq!(wkt(&a, &format!("{line}.ST_POINTATDISTANCE(5E-1, 'KILOMETRE')")), "POINT (500 0)");
    let circle = "ST_CIRCLEFROMTXT('CIRCLE (0 0, 1000 1000, 2000 0)')";
    assert_eq!(one(&a, &format!("{circle}.ST_RADIUS(), {circle}.ST_RADIUS('KILOMETRE')")), "1E3,1E0");
    let arc = "ST_CIRCULARFROMTXT('CIRCULARSTRING (0 0, 1000 1000, 2000 0)')";
    assert_eq!(one(&a, &format!("{arc}.ST_RADIUS('KILOMETRE')")), "1E0");
    let ellipse = "ST_ELLIPTICFROMTXT('ELLIPTICALCURVE (REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), \
                   REFERENCEDIRECTIONS (VECTOR (1 0), VECTOR (0 1))), UAXISLENGTH 2000, VAXISLENGTH 1000, STARTANGLE RADIANS (0), \
                   ENDANGLE RADIANS (1.5))')";
    let axes = format!("{ellipse}.ST_UAXISLENGTH(), {ellipse}.ST_UAXISLENGTH('KILOMETRE'), {ellipse}.ST_VAXISLENGTH('KILOMETRE')");
    assert_eq!(one(&a, &axes), "2E3,2E0,1E0");
    let clothoid = "ST_CLOTHOIDFROMTXT('CLOTHOID (REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), \
                    REFERENCEDIRECTIONS (VECTOR (1 0), VECTOR (0 1))), SCALEFACTOR 10, STARTDISTANCE 0, ENDDISTANCE 500)')";
    assert_eq!(one(&a, &format!("{clothoid}.ST_STARTDISTANCE('KILOMETRE'), {clothoid}.ST_ENDDISTANCE('KILOMETRE')")), "0E0,5E-1");
    assert_eq!(state(&a, &format!("VALUES {circle}.ST_RADIUS('RADIAN')")), "2FF14");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60 DR 1,
/// §5.1.64; ISO/IEC 9075-2:2023 §9.2 GR 3b vii] GML of more characters
/// than ST_MaxGeometryAsGML, which ST_SIZINGS gives, raises 22001.
#[test]
fn gml_is_bounded_by_its_sizing() {
    let (_db, a) = session();
    let sizing = "SELECT SUPPORTED_VALUE FROM ST_INFORMTN_SCHEMA.ST_SIZINGS WHERE VARIABLE_NAME = 'ST_MaxGeometryAsGML'";
    let limit = turborel::geometry::MAX_GEOMETRY_AS_GML;
    assert_eq!(rows(&a, sizing).unwrap(), [limit.to_string()]);
    let point = "<gml:Point><gml:pos>1 2</gml:pos></gml:Point>";
    let padded = format!("{point}{}", " ".repeat(usize::try_from(limit).unwrap().saturating_sub(point.len())));
    assert_eq!(wkt(&a, &format!("ST_GEOMFROMGML('{padded}')")), "POINT (1 2)");
    assert_eq!(state(&a, &format!("VALUES ST_GEOMFROMGML('{padded}x')")), "22001");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.1
/// Description 3, §17.1.3 to §17.1.7] ST_TINElement() is the empty set,
/// whose ST_IsEmpty is 1 and whose observers give NULL; a mutator gives
/// it a part, so it is no longer empty; an element made whole is 0.
#[test]
fn tin_elements_can_be_empty() {
    let (_db, a) = session();
    assert_eq!(
        one(&a, "ST_TINELEMENT().ST_ISEMPTY(), ST_TINELEMENT().ST_ELEMENTTYPE() IS NULL, ST_TINELEMENT().ST_ELEMENTID() IS NULL"),
        "1,TRUE,TRUE"
    );
    assert_eq!(one(&a, "ST_TINELEMENT().ST_ELEMENTTAG('t').ST_ISEMPTY(), ST_TINELEMENT().ST_ELEMENTTAG('t').ST_ELEMENTTAG()"), "0,t");
    let whole = "ST_TINELEMENT('breakline', 7, 'b', ST_LINEFROMTEXT('LINESTRING Z (0 0 1, 1 1 1)'))";
    assert_eq!(one(&a, &format!("{whole}.ST_ISEMPTY(), {whole}.ST_ELEMENTID()")), "0,7");
}
