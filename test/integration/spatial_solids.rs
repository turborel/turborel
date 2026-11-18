//! The routines in space of SQL/MM Spatial [ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §5.1, §7.1, §8.1, §8.4, §8.6, §9.1, §9.2, §10.3, §10.5]:
//! the 3D set operations and relationships, 3D measures of curves,
//! surfaces and solids, the shells of a B-rep solid, and the TIN's
//! maximum side length, clip and tables, through SQL as functions and as
//! methods, and stored values across a reopen.

use std::fmt::Write as _;

use turborel::db::{Database, OpenOptions, Session};
use turborel::geometry::model::{Geom, write_wkb};
use turborel::spatial::formats::wkt::{read_geom, write_geom};
use turborel::value::Value;

use crate::common::{Scratch, after_crash, rows, state};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// The constructor of the value `text` writes, of SRID `srid`.
fn geo(text: &str, srid: i32) -> String {
    let hex = write_wkb(&read_geom(text, srid).unwrap()).iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02X}");
        out
    });
    format!("ST_GEOMFROMWKB(X'{hex}', {srid})")
}

/// The faces of the box from `low` to `high`, facing out, as a polyhedral
/// surface's text.
fn box_faces(low: [f64; 3], high: [f64; 3]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (low, high);
    let faces = [
        [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
        [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
        [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
    ];
    let ring = |f: &[[f64; 3]; 4]| {
        let points: Vec<String> = f.iter().chain(std::iter::once(&f[0])).map(|p| format!("{} {} {}", p[0], p[1], p[2])).collect();
        format!("POLYGON (({}))", points.join(", "))
    };
    format!("POLYHEDRALSURFACE Z (PATCHES ({}))", faces.iter().map(ring).collect::<Vec<_>>().join(", "))
}

/// A cube's constructor.
fn cube(low: [f64; 3], high: [f64; 3]) -> String {
    geo(&format!("BREPSOLID Z ({})", box_faces(low, high)), 0)
}

/// The first value of the first row of `sql`.
fn value(a: &Session, sql: &str) -> Value {
    let mut cursor = a.open_cursor(sql).unwrap();
    let row = cursor.fetch().unwrap().unwrap();
    row.into_iter().next().unwrap()
}

/// The well-known text of the geometry `sql` gives.
fn text(a: &Session, sql: &str) -> String {
    write_geom(&Geom::from_value(&value(a, sql)).unwrap()).unwrap()
}

/// The number `sql` gives.
fn number(a: &Session, sql: &str) -> f64 {
    match value(a, sql) {
        Value::Approx(f) => f,
        other => panic!("{other:?}"),
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31 to §5.1.49]
/// ST_3DIntersection, ST_3DUnion, ST_3DDifference and
/// ST_3DSymDifference of two solids measure as the overlap says; the
/// relationships and ST_3DDistance in space and in a named unit; a value
/// without z raises 2FF72, an unknown unit 2FF14.
#[test]
fn solids_combine_and_relate_in_space() {
    let db = database();
    let a = db.session("A");
    let (one, two, far) = (cube([0.0; 3], [2.0; 3]), cube([1.0; 3], [3.0; 3]), cube([5.0, 0.0, 0.0], [6.0, 1.0, 1.0]));
    for (f, v) in [("ST_3DUNION", 15.0), ("ST_3DINTERSECTION", 1.0), ("ST_3DDIFFERENCE", 7.0), ("ST_3DSYMDIFFERENCE", 14.0)] {
        assert!(close(number(&a, &format!("VALUES ST_3DVOLUME({f}({one}, {two}))")), v), "{f}");
    }
    let relations = format!(
        "VALUES (ST_3DINTERSECTS({one}, {two}), ST_3DDISJOINT({one}, {far}), ST_3DEQUALS({one}, {one}), ST_3DEQUALS({one}, {two}))"
    );
    assert_eq!(rows(&a, &relations).unwrap(), ["1,1,1,0"]);
    assert!(close(number(&a, &format!("VALUES ST_3DDISTANCE({one}, {far})")), 3.0));
    assert!(close(number(&a, &format!("VALUES ST_3DDISTANCE({one}, {far}, 'KILOMETRE')")), 0.003));
    let line = geo("LINESTRING Z (-1 1 1, 3 1 1)", 0);
    assert_eq!(text(&a, &format!("VALUES ST_3DINTERSECTION({line}, {one})")), "LINESTRING Z (0 1 1, 2 1 1)");
    assert_eq!(state(&a, &format!("VALUES ST_3DUNION({one}, ST_POINT(1, 2))")), "2FF72");
    assert_eq!(state(&a, &format!("VALUES ST_3DDISTANCE({one}, {far}, 'parsec')")), "2FF14");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §9.1.2 to §9.1.5,
/// §9.2.2, §9.2.3 to §9.2.6, §8.1.13] a B-rep solid stored in an ST_BRepSolid
/// column keeps its measures and shells across a reopen: its volume,
/// surface area, centroid, a point in it, its shells as methods, a cavity
/// added by ST_InteriorShells or made by ST_BRepSolid, and one outside
/// it refused (2FF02).
#[test]
fn stored_solids_measure_and_keep_their_shells() {
    let scratch = Scratch::new("spatial-solids");
    let dir = scratch.path("db");
    let open = || {
        let db = database();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, S ST_BREPSOLID)").unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES (1, TREAT({} AS ST_BREPSOLID))", cube([0.0; 3], [3.0; 3]))).unwrap();
    a.commit().unwrap();
    drop((a, db));
    let db = open();
    let a = db.session("A");
    assert!(close(number(&a, "SELECT S.ST_3DVOLUME() FROM T"), 27.0));
    assert!(close(number(&a, "SELECT S.ST_3DSURFACEAREA() FROM T"), 54.0));
    assert_eq!(text(&a, "SELECT S.ST_3DCENTROID() FROM T"), "POINT Z (1.5 1.5 1.5)");
    assert_eq!(text(&a, "SELECT S.ST_3DPOINTONSOLID() FROM T"), "POINT Z (1.5 1.5 1.5)");
    assert_eq!(
        rows(&a, "SELECT S.ST_NUMINTSHELLS(), S.ST_EXTERIORSHELL().ST_ISSHELL(), CARDINALITY(S.ST_INTERIORSHELLS()) FROM T").unwrap(),
        ["0,1,0"]
    );
    let cavity = geo(&box_faces([1.0; 3], [2.0; 3]), 0);
    a.run(&format!("UPDATE T SET S = S.ST_INTERIORSHELLS(ARRAY[TREAT({cavity} AS ST_SURFACE)])")).unwrap();
    assert!(close(number(&a, "SELECT S.ST_3DVOLUME() FROM T"), 26.0));
    assert_eq!(rows(&a, "SELECT S.ST_NUMINTSHELLS(), S.ST_INTERIORSHELLN(2) IS NULL FROM T").unwrap(), ["1,TRUE"]);
    let outside = geo(&box_faces([4.0; 3], [5.0; 3]), 0);
    let made =
        format!("ST_BREPSOLID(TREAT({} AS ST_SURFACE), ARRAY[TREAT({cavity} AS ST_SURFACE)])", geo(&box_faces([0.0; 3], [3.0; 3]), 0));
    assert!(close(number(&a, &format!("VALUES ST_3DVOLUME({made})")), 26.0));
    assert_eq!(state(&a, &format!("SELECT S.ST_INTERIORSHELLS(ARRAY[TREAT({outside} AS ST_SURFACE)]) FROM T")), "2FF02");
    a.commit().unwrap();
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.3, §7.1.7,
/// §7.1.9, §7.1.12, §7.1.16, §5.1.14, §5.1.16, §10.3.4, §10.3.6] the
/// curve measures in space, and the parts of a measured line string
/// within a range of measures, z kept.
#[test]
fn curves_measure_in_space() {
    let db = database();
    let a = db.session("A");
    let line = geo("LINESTRING Z (0 0 0, 3 4 12)", 0);
    assert!(close(number(&a, &format!("VALUES ST_3DLENGTH({line})")), 13.0));
    assert!(close(number(&a, &format!("VALUES ST_3DLENGTH({line}, 'FOOT')")), 13.0 / 0.3048));
    let ring = geo("LINESTRING Z (0 0 0, 1 0 1, 1 1 0, 0 0 0)", 0);
    assert_eq!(rows(&a, &format!("VALUES (ST_3DISCLOSED({line}), ST_3DISCLOSED({ring}), ST_3DISRING({ring}))")).unwrap(), ["0,1,1"]);
    assert_eq!(text(&a, &format!("VALUES ST_3DPTATDISTANCE({line}, 6.5)")), "POINT Z (1.5 2 6)");
    assert!(close(number(&a, &format!("VALUES ST_3DDISTANCETOPT({line}, TREAT({} AS ST_POINT))", geo("POINT Z (1.5 2 6)", 0))), 6.5));
    let measured = geo("LINESTRING ZM (0 0 0 0, 10 0 5 10, 10 10 5 20)", 0);
    assert_eq!(
        text(&a, &format!("VALUES ST_3DLOCATEBETWEEN({measured}, 5, 15)")),
        "MULTILINESTRING ZM ((5 0 2.5 5, 10 0 5 10, 10 5 5 15))"
    );
    assert_eq!(text(&a, &format!("VALUES ST_3DLOCATEALONG({measured}, 20)")), "MULTIPOINT ZM ((10 10 5 20))");
    assert_eq!(state(&a, &format!("VALUES ST_3DLOCATEBETWEEN({measured}, 15, 5)")), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.3, §8.1.5,
/// §8.1.1, §8.1.7, §8.1.11, §8.1.13, §8.4.4, §5.1.9, §5.1.18] a surface's area,
/// perimeter and centroid in space, a box's faces a closed simple shell
/// whose boundary is empty, an open one's boundary its rim, and the slope
/// of a triangle; a vertical triangle has none (2FF02).
#[test]
fn surfaces_measure_in_space() {
    let db = database();
    let a = db.session("A");
    let tilted = geo("POLYGON Z ((0 0 0, 3 0 4, 3 1 4, 0 1 0, 0 0 0))", 0);
    assert!(close(number(&a, &format!("VALUES ST_3DAREA({tilted})")), 5.0));
    assert!(close(number(&a, &format!("VALUES ST_3DPERIMETER({tilted})")), 12.0));
    assert_eq!(text(&a, &format!("VALUES ST_3DCENTROID({tilted})")), "POINT Z (1.5 0.5 2)");
    let shell = geo(&box_faces([0.0; 3], [1.0; 3]), 0);
    let checks = format!(
        "VALUES (ST_ISSHELL({shell}), ST_3DISCLOSED({shell}), ST_IS3DCLOSED({shell}), ST_3DISSIMPLE({shell}), ST_ISSHELL({tilted}))"
    );
    assert_eq!(rows(&a, &checks).unwrap(), ["1,1,1,1,0"]);
    assert_eq!(text(&a, &format!("VALUES ST_3DBOUNDARY({tilted})")), "LINESTRING Z (0 0 0, 3 0 4, 3 1 4, 0 1 0, 0 0 0)");
    assert!(close(number(&a, &format!("VALUES ST_3DSLOPE({})", geo("TRIANGLE Z (0 0 0, 1 0 1, 0 1 0)", 0))), 1.0));
    assert_eq!(state(&a, &format!("VALUES ST_3DSLOPE({})", geo("TRIANGLE Z (0 0 0, 1 0 0, 0 0 1)", 0))), "2FF02");
}

/// The square of two triangles, z of x + y, its corners random points.
const SQUARE: &str = "TIN Z (PATCHES ((0 0 0, 2 0 2, 2 2 4), (0 0 0, 2 2 4, 0 2 2)) ELEMENTS (POINTS ID 1 TAG \"p\" MULTIPOINT Z ((0 0 0), (2 0 2), (2 2 4), (0 2 2))))";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.4 to §8.6.6]
/// a TIN's maximum side length set and retriangulated, null of one unset;
/// a null or nonpositive length raises 2FF03 or 2FF02; ST_Clip keeps the
/// part within the polygon; ST_TINTable() gives its rows, from which
/// ST_TINTable(table, columns..., length) builds it again.
#[test]
fn tins_clip_and_become_tables() {
    let db = database();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (ITEM VARCHAR(30), ORDERNUMBER INTEGER, X DOUBLE PRECISION, Y DOUBLE PRECISION, \
         Z DOUBLE PRECISION, TRIANGLE INTEGER ARRAY[3], VISIBILITY INTEGER, ELEMENTID INTEGER, ELEMENTTAG VARCHAR(64), ELEMENT INTEGER ARRAY[100])",
    )
    .unwrap();
    let a = db.session("A");
    let tin = format!("TREAT({} AS ST_TIN)", geo(SQUARE, 0));
    assert_eq!(
        rows(&a, &format!("VALUES ({tin}.ST_MAXSIDELENGTH() IS NULL, {tin}.ST_MAXSIDELENGTH(5, 1).ST_MAXSIDELENGTH())")).unwrap(),
        ["TRUE,5E0"]
    );
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_MAXSIDELENGTH(CAST(NULL AS DOUBLE PRECISION), 0)")), "2FF03");
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_MAXSIDELENGTH(0, 0)")), "2FF02");
    let window = format!("TREAT({} AS ST_POLYGON)", geo("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))", 0));
    assert!(close(number(&a, &format!("VALUES ST_3DAREA({tin}.ST_CLIP({window}))")), 3.0_f64.sqrt()));
    let listed = rows(&a, &format!("SELECT ITEM, ORDERNUMBER, ZCOORD, TRIANGLE, ELEMENT FROM TABLE(ST_TINTABLE({tin})) AS T")).unwrap();
    assert_eq!(listed[0], "point,1,0E0,NULL,NULL");
    assert_eq!(listed[4], "triangle,1,NULL,ARRAY[1, 2, 4],NULL");
    assert_eq!(listed[6], "random points,1,NULL,NULL,ARRAY[1, 3, 4, 2]");
    a.run(&format!("INSERT INTO R SELECT * FROM TABLE(ST_TINTABLE({tin})) AS T")).unwrap();
    let built = format!(
        "{tin}.ST_TINTABLE('R', 'ITEM', 'ORDERNUMBER', 'X', 'Y', 'Z', 'TRIANGLE', 'VISIBILITY', 'ELEMENTID', 'ELEMENTTAG', 'ELEMENT', 7)"
    );
    assert_eq!(rows(&a, &format!("VALUES ({built}.ST_MAXSIDELENGTH(), ST_3DAREA({built}) > 0)")).unwrap(), ["7E0,TRUE"]);
    assert_eq!(state(&a, &format!("VALUES {}", built.replace("'R'", "'NOWHERE'"))), "2FF02");
    assert_eq!(state(&a, &format!("VALUES {}", built.replace("'ELEMENTTAG'", "'NOTAG'"))), "2FF02");
}
