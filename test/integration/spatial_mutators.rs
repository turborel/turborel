//! The mutators of the geometry types and the observers of §5, §6, §8
//! and §10 through SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.2 to
//! §5.1.60, §6.1.3 to §6.1.7, §7.2.3 to §7.10.3, §8.1.10 to §8.6.7,
//! §10.3.3, §10.3.7]: each mutator returns SELF with one attribute
//! replaced, checked as its clause checks it (RULES.md D120).

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::state;

fn session() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE ONE (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO ONE VALUES (1)").unwrap();
    (db, a)
}

/// The one row `sql` selects, its values as text.
fn one(a: &Session, sql: &str) -> String {
    let query = format!("SELECT {sql} FROM ONE");
    let mut c = a.open_cursor(&query).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let row = c.fetch().unwrap_or_else(|e| panic!("{sql}: {e}")).unwrap_or_default();
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

const PLACED: &str = "REFERENCELOCATION AFFINEPLACEMENT (LOCATION (1 2), REFERENCEDIRECTIONS (VECTOR (0 1), VECTOR (-1 0)))";
const NULL_DOUBLE: &str = "CAST(NULL AS DOUBLE PRECISION)";

fn ellipse() -> String {
    format!(
        "ST_ELLIPTICFROMTXT('ELLIPTICALCURVE ({PLACED}, UAXISLENGTH 2, VAXISLENGTH 1, STARTANGLE RADIANS (0), ENDANGLE RADIANS (1.5))')"
    )
}

fn nurbs() -> &'static str {
    "ST_NURBSFROMTXT('NURBSCURVE (DEGREE 2, CONTROLPOINTS (NURBSPOINT (WEIGHTEDPOINT (0 0), WEIGHT 1), \
     NURBSPOINT (WEIGHTEDPOINT (2 2), WEIGHT 2), NURBSPOINT (WEIGHTEDPOINT (2 0), WEIGHT 1)), \
     KNOTS (KNOT (VALUE 0, MULTIPLICITY 3), KNOT (VALUE 1, MULTIPLICITY 3)))')"
}

fn clothoid() -> String {
    format!("ST_CLOTHOIDFROMTXT('CLOTHOID ({PLACED}, SCALEFACTOR 10, STARTDISTANCE 0, ENDDISTANCE 5)')")
}

fn spiral() -> String {
    format!("ST_SPIRALFROMTXT('SPIRALCURVE ({PLACED}, LENGTH 10, STARTCURVATURE 0, ENDCURVATURE 0.1, SPIRALTYPE clothoid)')")
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6.1.3 to §6.1.7]
/// ST_X, ST_Y, ST_Z and ST_M of a number set a point's coordinate, ST_Z
/// and ST_M adding the dimension; the null value raises 2FF03 of x and y
/// and 2FF16 of a point that is not empty; ST_X of the empty point
/// raises 2FF18; ST_Z(), ST_M() and ST_ExplicitPoint() observe it.
#[test]
fn point_mutators_set_coordinates() {
    let (_db, a) = session();
    let p = "ST_POINTFROMTEXT('POINT ZM (1 2 3 4)')";
    assert_eq!(one(&a, &format!("ST_ASTEXT({p}.ST_X(5E0)), ST_ASTEXT({p}.ST_Y(5E0))")), "POINT ZM (5 2 3 4),POINT ZM (1 5 3 4)");
    assert_eq!(one(&a, "ST_ASTEXT(ST_POINT(1, 2).ST_Z(5E0)), ST_ASTEXT(ST_POINT(1, 2).ST_M(6E0))"), "POINT Z (1 2 5),POINT M (1 2 6)");
    assert_eq!(one(&a, &format!("{p}.ST_Z(), {p}.ST_M(), {p}.ST_EXPLICITPOINT()")), "3E0,4E0,ARRAY[1E0, 2E0, 3E0, 4E0]");
    assert_eq!(one(&a, "ST_POINT(1, 2).ST_Z() IS NULL, ST_POINTFROMTEXT('POINT EMPTY').ST_EXPLICITPOINT() IS NULL"), "TRUE,TRUE");
    assert_eq!(one(&a, &format!("ST_ASTEXT(ST_POINTFROMTEXT('POINT EMPTY').ST_Z({NULL_DOUBLE}))")), "POINT EMPTY");
    assert_eq!(state(&a, &format!("VALUES ST_POINT(1, 2).ST_X({NULL_DOUBLE})")), "2FF03");
    assert_eq!(state(&a, &format!("VALUES {p}.ST_Z({NULL_DOUBLE})")), "2FF16");
    assert_eq!(state(&a, "VALUES ST_POINTFROMTEXT('POINT EMPTY').ST_X(1E0)"), "2FF18");
    assert_eq!(state(&a, "VALUES ST_POINTFROMTEXT('POINT EMPTY').ST_Z(1E0)"), "2FF03");
    assert_eq!(one(&a, "CAST(NULL AS ST_POINT).ST_X(1E0) IS NULL"), "TRUE");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.2.3, §7.3.3,
/// §7.4.3, §7.5.3, §8.4.3, §18.1.6, §18.1.7] ST_Points(ST_Point ARRAY)
/// replaces a curve's or a triangle's points: the array's nulls raise
/// 2FF03 and 2FF09, equal neighbours 2FF05, another system 2FF10, mixed
/// dimensions 2FF25; an arc of equal ends 2FF05, a circle of other than
/// three points 2FF77 or of collinear ones 2FF78, a triangle of other than
/// four 2FF02.
#[test]
fn points_mutators_check_their_arrays() {
    let (_db, a) = session();
    let line = "ST_LINEFROMTEXT('LINESTRING (0 0, 1 1)')";
    assert_eq!(
        one(&a, &format!("ST_ASTEXT({line}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(2, 0), ST_POINT(2, 2)]))")),
        "LINESTRING (0 0, 2 0, 2 2)"
    );
    let z = "ARRAY[ST_POINTFROMTEXT('POINT Z (0 0 0)'), ST_POINTFROMTEXT('POINT Z (1 0 1)')]";
    assert_eq!(one(&a, &format!("ST_ASTEXT({line}.ST_POINTS({z}))")), "LINESTRING Z (0 0 0, 1 0 1)");
    assert_eq!(state(&a, &format!("VALUES {line}.ST_POINTS(CAST(NULL AS ST_POINT ARRAY[4]))")), "2FF03");
    assert_eq!(state(&a, &format!("VALUES {line}.ST_POINTS(ARRAY[ST_POINT(0, 0), CAST(NULL AS ST_POINT)])")), "2FF09");
    assert_eq!(state(&a, &format!("VALUES {line}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(0, 0)])")), "2FF05");
    assert_eq!(state(&a, &format!("VALUES {line}.ST_POINTS(ARRAY[ST_POINT(0, 0, 4326), ST_POINT(1, 0, 4326)])")), "2FF10");
    assert_eq!(state(&a, &format!("VALUES {line}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINTFROMTEXT('POINT Z (1 0 1)')])")), "2FF25");
    let arc = "ST_CIRCULARFROMTXT('CIRCULARSTRING (1 0, 0 1, -1 0)')";
    assert_eq!(
        one(&a, &format!("ST_ASTEXT({arc}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(1, 1), ST_POINT(2, 0)]))")),
        "CIRCULARSTRING (0 0, 1 1, 2 0)"
    );
    assert_eq!(state(&a, &format!("VALUES {arc}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(1, 1), ST_POINT(0, 0)])")), "2FF05");
    let circle = "ST_CIRCLEFROMTXT('CIRCLE (1 0, 0 1, -1 0)')";
    assert_eq!(
        one(&a, &format!("ST_ASTEXT({circle}.ST_POINTS(ARRAY[ST_POINT(2, 0), ST_POINT(0, 2), ST_POINT(-2, 0)]))")),
        "CIRCLE (2 0, 0 2, -2 0)"
    );
    assert_eq!(state(&a, &format!("VALUES {circle}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(1, 1)])")), "2FF77");
    assert_eq!(state(&a, &format!("VALUES {circle}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(1, 1), ST_POINT(2, 2)])")), "2FF78");
    assert_eq!(state(&a, &format!("VALUES {circle}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(1, 1), ST_POINT(0, 0)])")), "2FF05");
    let geodesic = "ST_GEODESICFROMTXT('GEODESICSTRING (0 0, 1 1)', 4326)";
    assert_eq!(
        one(&a, &format!("ST_ASTEXT({geodesic}.ST_POINTS(ARRAY[ST_POINT(0, 0, 4326), ST_POINT(5, 5, 4326)]))")),
        "GEODESICSTRING (0 0, 5 5)"
    );
    let triangle = "ST_TRIFROMTEXT('TRIANGLE (0 0, 1 0, 0 1)')";
    let four = "ARRAY[ST_POINT(0, 0), ST_POINT(2, 0), ST_POINT(0, 2), ST_POINT(0, 0)]";
    assert_eq!(one(&a, &format!("ST_ASTEXT({triangle}.ST_POINTS({four}))")), "TRIANGLE (0 0, 2 0, 0 2)");
    assert_eq!(state(&a, &format!("VALUES {triangle}.ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(2, 0), ST_POINT(0, 0)])")), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.10.3]
/// ST_Curves(ST_Curve ARRAY) replaces a compound curve's curves, which
/// meet end to start or raise 2FF11.
#[test]
fn curves_mutator_replaces_compound_parts() {
    let (_db, a) = session();
    let compound = "ST_COMPOUNDFROMTXT('COMPOUNDCURVE ((0 0, 1 0), CIRCULARSTRING (1 0, 2 1, 3 0))')";
    let parts = "ARRAY[ST_LINEFROMTEXT('LINESTRING (0 0, 5 0)'), ST_CIRCULARFROMTXT('CIRCULARSTRING (5 0, 6 1, 7 0)')]";
    assert_eq!(one(&a, &format!("ST_ASTEXT({compound}.ST_CURVES({parts}))")), "COMPOUNDCURVE ((0 0, 5 0), CIRCULARSTRING (5 0, 6 1, 7 0))");
    let apart = "ARRAY[ST_LINEFROMTEXT('LINESTRING (0 0, 1 0)'), ST_LINEFROMTEXT('LINESTRING (5 0, 6 0)')]";
    assert_eq!(state(&a, &format!("VALUES {compound}.ST_CURVES({apart})")), "2FF11");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.2.3, §8.2.4,
/// §8.3.3, §8.3.4, §8.4.5, §8.4.6] ST_ExteriorRing(ST_Curve) and
/// ST_InteriorRings(ST_Curve ARRAY) replace a surface's rings: another
/// system raises 2FF10, a curve not closed or rings not making a valid
/// polygon 2FF02, an empty surface's interior rings 2FF07; a polygon's
/// ring not a linestring 2FF12 or 2FF08, a triangle's of other than four
/// points 2FF75, a triangle's holes 2FF66.
#[test]
fn ring_mutators_check_their_rings() {
    let (_db, a) = session();
    let curved = "ST_CPOLYFROMTEXT('CURVEPOLYGON ((0 0, 8 0, 8 8, 0 0))')";
    assert_eq!(
        one(&a, &format!("ST_ASTEXT({curved}.ST_EXTERIORRING(ST_CIRCLEFROMTXT('CIRCLE (1 0, 0 1, -1 0)')))")),
        "CURVEPOLYGON (CIRCLE (1 0, 0 1, -1 0))"
    );
    let hole = "ARRAY[ST_LINEFROMTEXT('LINESTRING (5 1, 7 1, 7 3, 5 1)')]";
    assert_eq!(one(&a, &format!("{curved}.ST_INTERIORRINGS({hole}).ST_NUMINTERIORRING()")), "1");
    assert_eq!(state(&a, &format!("VALUES {curved}.ST_EXTERIORRING(ST_LINEFROMTEXT('LINESTRING (0 0, 8 0, 8 8)'))")), "2FF02");
    assert_eq!(state(&a, &format!("VALUES {curved}.ST_EXTERIORRING(ST_LINEFROMTEXT('LINESTRING (0 0, 8 0, 8 8, 0 0)', 4326))")), "2FF10");
    let small = "ST_LINEFROMTEXT('LINESTRING (0 0, 4 0, 4 4, 0 0)')";
    assert_eq!(state(&a, &format!("VALUES {curved}.ST_INTERIORRINGS({hole}).ST_EXTERIORRING({small})")), "2FF02");
    assert_eq!(state(&a, &format!("VALUES ST_CPOLYFROMTEXT('CURVEPOLYGON EMPTY').ST_INTERIORRINGS({hole})")), "2FF07");
    let polygon = "ST_POLYFROMTEXT('POLYGON ((0 0, 8 0, 8 8, 0 0))')";
    assert_eq!(one(&a, &format!("ST_ASTEXT({polygon}.ST_EXTERIORRING({small}))")), "POLYGON ((0 0, 4 0, 4 4, 0 0))");
    assert_eq!(state(&a, &format!("VALUES {polygon}.ST_EXTERIORRING(ST_CIRCLEFROMTXT('CIRCLE (1 0, 0 1, -1 0)'))")), "2FF12");
    assert_eq!(state(&a, &format!("VALUES {polygon}.ST_INTERIORRINGS(ARRAY[ST_CIRCLEFROMTXT('CIRCLE (5 1, 5.5 1.5, 6 1)')])")), "2FF08");
    let outside = "ARRAY[ST_LINEFROMTEXT('LINESTRING (10 10, 11 10, 11 11, 10 10)')]";
    assert_eq!(state(&a, &format!("VALUES {polygon}.ST_INTERIORRINGS({outside})")), "2FF02");
    let triangle = "ST_TRIFROMTEXT('TRIANGLE (0 0, 1 0, 0 1)')";
    assert_eq!(
        one(&a, &format!("ST_ASTEXT({triangle}.ST_EXTERIORRING(ST_LINEFROMTEXT('LINESTRING (0 0, 3 0, 0 3, 0 0)')))")),
        "TRIANGLE (0 0, 3 0, 0 3)"
    );
    assert_eq!(state(&a, &format!("VALUES {triangle}.ST_EXTERIORRING(ST_LINEFROMTEXT('LINESTRING (0 0, 3 0, 3 3, 0 3, 0 0)'))")), "2FF75");
    assert_eq!(state(&a, &format!("VALUES {triangle}.ST_INTERIORRINGS({hole})")), "2FF66");
    assert_eq!(one(&a, &format!("ST_ASTEXT({triangle}.ST_INTERIORRINGS(CAST(ARRAY[] AS ST_CURVE ARRAY[4])))")), "TRIANGLE (0 0, 1 0, 0 1)");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.5.3, §8.6.7]
/// ST_Patches replaces a polyhedral surface's patches, 3D ones each with
/// an edge of another and none of three, else 2FF02; a TIN's are
/// triangles, else 2FF67.
#[test]
fn patches_mutators_check_adjacency() {
    let (_db, a) = session();
    let surface = "ST_PHSFROMTEXT('POLYHEDRALSURFACE Z (PATCHES (POLYGON Z ((0 0 0, 1 0 0, 0 1 0, 0 0 0))))')";
    let lower = "ST_POLYFROMTEXT('POLYGON Z ((0 0 0, 1 0 0, 1 1 0, 0 0 0))')";
    let upper = "ST_POLYFROMTEXT('POLYGON Z ((0 0 0, 1 1 0, 0 1 0, 0 0 0))')";
    assert_eq!(one(&a, &format!("{surface}.ST_PATCHES(ARRAY[{lower}, {upper}]).ST_NUMPATCHES()")), "2");
    let apart = "ST_POLYFROMTEXT('POLYGON Z ((5 5 0, 6 5 0, 5 6 0, 5 5 0))')";
    assert_eq!(state(&a, &format!("VALUES {surface}.ST_PATCHES(ARRAY[{lower}, {apart}])")), "2FF02");
    let third = "ST_POLYFROMTEXT('POLYGON Z ((0 0 0, 1 1 0, 1 1 1, 0 0 0))')";
    assert_eq!(state(&a, &format!("VALUES {surface}.ST_PATCHES(ARRAY[{lower}, {upper}, {third}])")), "2FF02");
    assert_eq!(state(&a, &format!("VALUES {surface}.ST_PATCHES(ARRAY[ST_POLYFROMTEXT('POLYGON ((0 0, 1 0, 1 1, 0 0))')])")), "2FF02");
    let tin = "ST_TINFROMTEXT('TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)))')";
    let triangle = "ST_TRIFROMTEXT('TRIANGLE Z (0 0 0, 2 0 0, 0 2 1)')";
    assert_eq!(one(&a, &format!("ST_ASTEXT({tin}.ST_PATCHES(ARRAY[{triangle}]))")), "TIN Z (PATCHES ((0 0 0, 2 0 0, 0 2 1)))");
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_PATCHES(ARRAY[{lower}])")), "2FF67");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.6.3 to §7.6.9]
/// The mutators of an elliptical curve: its placement, 2D or 3D as SELF
/// (2FF96) of two directions (2FF86); axis lengths, in a unit converted
/// to SELF's; angles; and measures, both or none, set together and
/// removed by the null value. A null argument raises 2FF03, an empty
/// curve 2FF02.
#[test]
fn elliptical_mutators_set_attributes() {
    let (_db, a) = session();
    let e = ellipse();
    assert_eq!(one(&a, &format!("{e}.ST_UAXISLENGTH(3E0).ST_UAXISLENGTH(), {e}.ST_VAXISLENGTH(2E0).ST_VAXISLENGTH()")), "3E0,2E0");
    assert_eq!(one(&a, &format!("{e}.ST_UAXISLENGTH(3E0, 'KILOMETRE').ST_UAXISLENGTH(), {e}.ST_UAXISLENGTH('METRE')")), "3E3,2E0");
    assert_eq!(one(&a, &format!("{e}.ST_STARTANGLE(NEW ST_ANGLE(1E0)).ST_STARTANGLE().ST_RADIANS()")), "1E0");
    assert_eq!(one(&a, &format!("{e}.ST_ENDANGLE(NEW ST_ANGLE(1E0)).ST_ENDANGLE().ST_RADIANS()")), "1E0");
    assert_eq!(
        one(&a, &format!("{e}.ST_STARTM(5E0).ST_STARTM(), {e}.ST_STARTM(5E0).ST_ENDM(), {e}.ST_STARTM(5E0).ST_ENDM(7E0).ST_ENDM()")),
        "5E0,5E0,7E0"
    );
    assert_eq!(one(&a, &format!("{e}.ST_STARTM(5E0).ST_ENDM({NULL_DOUBLE}).ST_ISMEASURED()")), "0");
    let moved = "NEW ST_AFFINEPLACEMENT(ST_POINT(5, 6), ARRAY[NEW ST_VECTOR(1E0, 0E0), NEW ST_VECTOR(0E0, 1E0)])";
    assert_eq!(one(&a, &format!("ST_ASTEXT({e}.ST_REFLOCATION({moved}).ST_REFLOCATION().ST_LOCATION())")), "POINT (5 6)");
    let lifted =
        "NEW ST_AFFINEPLACEMENT(ST_POINTFROMTEXT('POINT Z (0 0 0)'), ARRAY[NEW ST_VECTOR(1E0, 0E0, 0E0), NEW ST_VECTOR(0E0, 1E0, 0E0)])";
    assert_eq!(state(&a, &format!("VALUES {e}.ST_REFLOCATION({lifted})")), "2FF96");
    let one_way = "NEW ST_AFFINEPLACEMENT(ST_POINT(0, 0), ARRAY[NEW ST_VECTOR(1E0, 0E0)])";
    assert_eq!(state(&a, &format!("VALUES {e}.ST_REFLOCATION({one_way})")), "2FF86");
    assert_eq!(state(&a, &format!("VALUES {e}.ST_UAXISLENGTH({NULL_DOUBLE})")), "2FF03");
    assert_eq!(state(&a, &format!("VALUES {e}.ST_STARTANGLE(CAST(NULL AS ST_ANGLE))")), "2FF03");
    assert_eq!(state(&a, "VALUES ST_ELLIPTICFROMTXT('ELLIPTICALCURVE EMPTY').ST_UAXISLENGTH(3E0)"), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.7.4 to §7.7.7]
/// ST_ControlPoints and ST_Knots of a NURBS curve set them, agreeing
/// with each other else 2FF02; ST_StartM and ST_EndM measure it.
#[test]
fn nurbs_mutators_set_attributes() {
    let (_db, a) = session();
    let n = nurbs();
    let moved =
        "ARRAY[NEW ST_NURBSPOINT(ST_POINT(0, 0), 1E0), NEW ST_NURBSPOINT(ST_POINT(3, 3), 2E0), NEW ST_NURBSPOINT(ST_POINT(3, 0), 1E0)]";
    assert_eq!(one(&a, &format!("ST_ASTEXT({n}.ST_CONTROLPOINTS({moved}).ST_CONTROLPOINTS()[2].ST_WEIGHTEDPOINT())")), "POINT (3 3)");
    assert_eq!(one(&a, &format!("{n}.ST_KNOTS(ARRAY[NEW ST_KNOT(0E0, 3), NEW ST_KNOT(2E0, 3)]).ST_KNOTS()[2].ST_VALUE()")), "2E0");
    assert_eq!(state(&a, &format!("VALUES {n}.ST_CONTROLPOINTS(ARRAY[NEW ST_NURBSPOINT(ST_POINT(0, 0), 1E0)])")), "2FF02");
    assert_eq!(state(&a, &format!("VALUES {n}.ST_KNOTS(ARRAY[NEW ST_KNOT(0E0, 3)])")), "2FF02");
    assert_eq!(one(&a, &format!("{n}.ST_STARTM(1E0).ST_ENDM(2E0).ST_ENDM(), {n}.ST_STARTM(1E0).ST_ISMEASURED()")), "2E0,1");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.8.3 to §7.8.8,
/// §7.9.3 to §7.9.9] The mutators of a clothoid and a spiral: scale
/// factor, distances and length, in a unit if given; curvatures; a
/// spiral's type, a known one else 2FF02; ST_Length of a unit name still
/// its length in that unit.
#[test]
fn clothoid_and_spiral_mutators_set_attributes() {
    let (_db, a) = session();
    let k = clothoid();
    assert_eq!(one(&a, &format!("{k}.ST_SCALEFACTOR(3E0).ST_SCALEFACTOR(), {k}.ST_ENDDISTANCE(4E0).ST_ENDDISTANCE()")), "3E0,4E0");
    assert_eq!(one(&a, &format!("{k}.ST_STARTDISTANCE(1E0, 'KILOMETRE').ST_STARTDISTANCE(), {k}.ST_ENDDISTANCE('METRE')")), "1E3,5E0");
    assert_eq!(one(&a, &format!("{k}.ST_STARTM(1E0).ST_ENDM(9E0).ST_ENDM()")), "9E0");
    let s = spiral();
    assert_eq!(
        one(&a, &format!("{s}.ST_LENGTH(20E0).ST_LENGTH(), {s}.ST_LENGTH(1E0, 'KILOMETRE').ST_LENGTH(), {s}.ST_LENGTH('METRE')")),
        "2E1,1E3,1E1"
    );
    assert_eq!(
        one(&a, &format!("{s}.ST_STARTCURVATURE(0.01E0).ST_STARTCURVATURE(), {s}.ST_ENDCURVATURE(0.2E0).ST_ENDCURVATURE()")),
        "1E-2,2E-1"
    );
    assert_eq!(one(&a, &format!("{s}.ST_SPIRALTYPE('bloss').ST_SPIRALTYPE()")), "bloss");
    assert_eq!(state(&a, &format!("VALUES {s}.ST_SPIRALTYPE('nonsense')")), "2FF02");
    let moved = "NEW ST_AFFINEPLACEMENT(ST_POINT(0, 0), ARRAY[NEW ST_VECTOR(1E0, 0E0), NEW ST_VECTOR(0E0, 1E0)])";
    assert_eq!(one(&a, &format!("ST_ASTEXT({s}.ST_REFLOCATION({moved}).ST_STARTPOINT())")), "POINT (0 0)");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.2, §5.1.3,
/// §5.1.11, §5.1.12, §5.1.19, §5.1.20, §5.1.25 to §5.1.28] The observers
/// of a value's dimensions and extent: ST_Dimension, -1 of the empty
/// set; ST_CoordDim, ST_Is3D, ST_IsMeasured; ST_Envelope in x and y,
/// widened where flat; ST_EnvelopeAsPts with z and m; ST_MinZ to ST_MaxM,
/// null without the dimension.
#[test]
fn dimension_and_extent_observers() {
    let (_db, a) = session();
    let line = "ST_LINEFROMTEXT('LINESTRING ZM (0 0 1 7, 2 1 2 9)')";
    assert_eq!(one(&a, &format!("{line}.ST_DIMENSION(), {line}.ST_COORDDIM(), {line}.ST_IS3D(), {line}.ST_ISMEASURED()")), "1,4,1,1");
    assert_eq!(one(&a, "ST_GEOMFROMTEXT('GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 1 1))').ST_DIMENSION()"), "1");
    assert_eq!(one(&a, "ST_GEOMFROMTEXT('GEOMETRYCOLLECTION EMPTY').ST_DIMENSION()"), "-1");
    assert_eq!(one(&a, &format!("ST_ASTEXT({line}.ST_ENVELOPE())")), "POLYGON ((0 0, 2 0, 2 1, 0 1, 0 0))");
    assert_eq!(one(&a, "ST_ASTEXT(ST_POINT(1, 1).ST_ENVELOPE()) <> 'POLYGON ((1 1, 1 1, 1 1, 1 1, 1 1))'"), "TRUE");
    assert_eq!(one(&a, "ST_POINT(1, 1).ST_ENVELOPE().ST_AREA() > 0"), "TRUE");
    assert_eq!(
        one(&a, &format!("ST_ASTEXT({line}.ST_ENVELOPEASPTS()[1]), ST_ASTEXT({line}.ST_ENVELOPEASPTS()[2])")),
        "POINT ZM (0 0 1 7),POINT ZM (2 1 2 9)"
    );
    assert_eq!(one(&a, &format!("{line}.ST_MINZ(), {line}.ST_MAXZ(), {line}.ST_MINM(), {line}.ST_MAXM()")), "1E0,2E0,7E0,9E0");
    assert_eq!(one(&a, "ST_POINT(1, 1).ST_MINZ() IS NULL, ST_LINEFROMTEXT('LINESTRING EMPTY').ST_ENVELOPE() IS NULL"), "TRUE,TRUE");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.10, §10.3.3,
/// §10.3.7, §5.1.56, §5.1.58, §5.1.60] ST_IsWorld of a surface, 0 of a
/// bounded one; ST_IsClosed of a multicurve, its boundary empty;
/// ST_PerpPoints of a multicurve, the feet on each curve or the point on
/// one; the ToSQL methods, the value of their representation.
#[test]
fn surface_collection_and_representation_methods() {
    let (_db, a) = session();
    assert_eq!(one(&a, "ST_POLYFROMTEXT('POLYGON ((0 0, 4 0, 4 4, 0 0))').ST_ISWORLD()"), "0");
    assert_eq!(one(&a, "ST_MCURVEFROMTEXT('MULTICURVE ((0 0, 2 0), (2 0, 0 0))').ST_ISCLOSED()"), "1");
    assert_eq!(
        one(&a, "ST_MCURVEFROMTEXT('MULTICURVE ((0 0, 2 0))').ST_ISCLOSED(), ST_MCURVEFROMTEXT('MULTICURVE EMPTY').ST_ISCLOSED()"),
        "0,0"
    );
    let two = "ST_MCURVEFROMTEXT('MULTICURVE ((0 0, 2 0), (0 2, 2 2))')";
    assert_eq!(one(&a, &format!("ST_ASTEXT({two}.ST_PERPPOINTS(ST_POINT(1, 1)))")), "MULTIPOINT ((1 0), (1 2))");
    assert_eq!(one(&a, &format!("ST_ASTEXT({two}.ST_PERPPOINTS(ST_POINT(1, 0)))")), "POINT (1 0)");
    assert_eq!(one(&a, "ST_ASTEXT(ST_POINT(0, 0).ST_WKTTOSQL('POINT (1 2)'))"), "POINT (1 2)");
    assert_eq!(one(&a, "ST_ASTEXT(ST_POINT(0, 0).ST_WKBTOSQL(ST_POINT(3, 4).ST_ASBINARY()))"), "POINT (3 4)");
    let gml = "'<gml:Point srsName=\"EPSG:4326\"><gml:pos>1 2</gml:pos></gml:Point>'";
    assert_eq!(one(&a, &format!("ST_POINT(0, 0).ST_GMLTOSQL({gml}).ST_SRID()")), "4326");
}
