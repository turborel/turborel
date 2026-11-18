//! The curve observers whose results are the types of §16 and §17
//! through SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.10, §7.3.13,
//! §7.3.14, §7.4.7, §7.6.3, §7.6.6, §7.6.7, §7.7.4, §7.7.5, §7.8.3,
//! §7.9.3]: reference locations and their axis vectors, control points,
//! knots, angles and normals (RULES.md D124).

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

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.6.3, §7.8.3,
/// §7.9.3, §17.3] ST_RefLocation of an elliptical curve, a clothoid and
/// a spiral is an ST_AffinePlacement: its location, a point, and its
/// reference directions, the curve's axis vectors; an empty curve gives
/// NULL.
#[test]
fn reference_locations_are_placements() {
    let (_db, a) = session();
    let ellipse = format!(
        "ST_ELLIPTICFROMTXT('ELLIPTICALCURVE ({PLACED}, UAXISLENGTH 2, VAXISLENGTH 1, STARTANGLE RADIANS (0), ENDANGLE RADIANS (1.5))', 4326)"
    );
    let at = format!("{ellipse}.ST_REFLOCATION()");
    assert_eq!(
        one(&a, &format!("ST_ASTEXT({at}.ST_LOCATION()), {at}.ST_INDIMENSION(), {at}.ST_REFDIRECTIONS()[2].ST_X()")),
        "POINT (1 2),2,-1E0"
    );
    assert_eq!(one(&a, &format!("ST_SRID({at}.ST_LOCATION()), {at}.ST_REFDIRECTIONS()[1].ST_SRID()")), "4326,4326");
    let clothoid = format!("ST_CLOTHOIDFROMTXT('CLOTHOID ({PLACED}, SCALEFACTOR 10, STARTDISTANCE 0, ENDDISTANCE 5)')");
    assert_eq!(one(&a, &format!("{clothoid}.ST_REFLOCATION().ST_REFDIRECTIONS()[1].ST_Y()")), "1E0");
    let spiral = format!("ST_SPIRALFROMTXT('SPIRALCURVE ({PLACED}, LENGTH 10, STARTCURVATURE 0, ENDCURVATURE 0.1, SPIRALTYPE clothoid)')");
    assert_eq!(one(&a, &format!("ST_ASTEXT({spiral}.ST_REFLOCATION().ST_TRANSFORM(ST_POINT(1, 0)))")), "POINT (1 3)");
    assert_eq!(one(&a, "ST_CLOTHOIDFROMTXT('CLOTHOID EMPTY').ST_REFLOCATION() IS NULL"), "TRUE");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.7.4, §7.7.5,
/// §17.4, §17.5] ST_ControlPoints and ST_Knots of a NURBS curve are
/// arrays of ST_NURBSPoint and ST_Knot values.
#[test]
fn control_points_and_knots_are_records() {
    let (_db, a) = session();
    let nurbs = "ST_NURBSFROMTXT('NURBSCURVE (DEGREE 2, CONTROLPOINTS (NURBSPOINT (WEIGHTEDPOINT (0 0), WEIGHT 1), \
                 NURBSPOINT (WEIGHTEDPOINT (2 2), WEIGHT 2), NURBSPOINT (WEIGHTEDPOINT (2 0), WEIGHT 1)), \
                 KNOTS (KNOT (VALUE 0, MULTIPLICITY 3), KNOT (VALUE 1, MULTIPLICITY 3)))')";
    let points = format!("{nurbs}.ST_CONTROLPOINTS()");
    assert_eq!(
        one(&a, &format!("CARDINALITY({points}), ST_ASTEXT({points}[2].ST_WEIGHTEDPOINT()), {points}[2].ST_WEIGHT()")),
        "3,POINT (2 2),2E0"
    );
    let knots = format!("{nurbs}.ST_KNOTS()");
    assert_eq!(one(&a, &format!("CARDINALITY({knots}), {knots}[2].ST_VALUE(), {knots}[2].ST_MULTIPLICITY()")), "2,1E0,3");
    assert_eq!(state(&a, "VALUES ST_CONTROLPOINTS(ST_LINEFROMTEXT('LINESTRING (0 0, 1 1)'))"), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.10, §7.3.13,
/// §7.3.14, §7.4.7, §7.6.6, §7.6.7, §16.1, §17.2] ST_StartAngle and
/// ST_EndAngle of an arc and of an elliptical curve are ST_Angle values;
/// ST_BulgeNormal of an arc and ST_Normal of a circle are ST_Vector
/// values, a 2D arc's normal its chord's left perpendicular; an arc
/// string of more arcs raises 2FF76.
#[test]
fn angles_and_normals_are_records() {
    let (_db, a) = session();
    let arc = "ST_CIRCULARFROMTXT('CIRCULARSTRING (1 0, 0 1, -1 0)')";
    let angles =
        format!("CAST({arc}.ST_STARTANGLE().ST_DEGREES() AS DECIMAL(6, 2)), CAST({arc}.ST_ENDANGLE().ST_DEGREES() AS DECIMAL(6, 2))");
    assert_eq!(one(&a, &angles), "0.00,180.00");
    let normal = format!("{arc}.ST_BULGENORMAL()");
    assert_eq!(
        one(&a, &format!("CAST({normal}.ST_X() AS DECIMAL(6, 2)), CAST({normal}.ST_Y() AS DECIMAL(6, 2)), {normal}.ST_Z()")),
        "0.00,-1.00,NULL"
    );
    let circle = "ST_CIRCLEFROMTXT('CIRCLE Z (1 0 0, 0 1 0, -1 0 0)')";
    assert_eq!(one(&a, &format!("{circle}.ST_NORMAL().ST_Z()")), "1E0");
    let ellipse = format!(
        "ST_ELLIPTICFROMTXT('ELLIPTICALCURVE ({PLACED}, UAXISLENGTH 2, VAXISLENGTH 1, STARTANGLE RADIANS (0), ENDANGLE RADIANS (1.5))')"
    );
    assert_eq!(one(&a, &format!("{ellipse}.ST_STARTANGLE().ST_RADIANS(), {ellipse}.ST_ENDANGLE().ST_RADIANS()")), "0E0,1.5E0");
    let two = "ST_CIRCULARFROMTXT('CIRCULARSTRING (0 0, 1 1, 2 0, 3 -1, 4 0)')";
    assert_eq!(state(&a, &format!("VALUES {two}.ST_STARTANGLE()")), "2FF76");
}
