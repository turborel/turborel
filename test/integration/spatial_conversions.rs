//! The cast methods of ST_Geometry and the constructors of a compound
//! surface and a B-rep solid [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §5.1.55, §8.7.2, §8.7.3, §9.2.2], through SQL as functions, methods
//! and NEW.

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::{rows, state};

fn session() -> Session {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.session("A")
}

/// The text of the value `f` makes of the geometry of `wkt`.
fn cast(a: &Session, f: &str, wkt: &str) -> String {
    made(a, &format!("{f}(ST_GEOMFROMTEXT('{wkt}', 4326))"))
}

/// The text of the value of `wkt`, as the writer spells it.
fn canon(a: &Session, wkt: &str) -> String {
    made(a, &format!("ST_GEOMFROMTEXT('{wkt}')"))
}

/// The SQLSTATE of `f` of the geometry of `wkt`.
fn refused(a: &Session, f: &str, wkt: &str) -> String {
    state(a, &format!("VALUES {f}(ST_GEOMFROMTEXT('{wkt}'))"))
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.55 Description
/// 1 to 39] SELF of the type is returned; a curve's line string, a
/// curve polygon's polygon and a compound curve of a curve are made; the
/// one element of a collection, or of a compound curve, is cast again;
/// another value raises 2FF16, and the empty set gives the type's empty
/// value of SELF's SRID.
#[test]
fn curves_and_polygons_cast_by_their_methods() {
    let a = session();
    let arc = "CIRCULARSTRING (0 0, 1 1, 2 0)";
    assert_eq!(cast(&a, "ST_TOPOINT", "MULTIPOINT ((1 2))"), "POINT (1 2)");
    assert_eq!(cast(&a, "ST_TOPOINT", "GEOMETRYCOLLECTION (POINT (1 2))"), "POINT (1 2)");
    assert_eq!(refused(&a, "ST_TOPOINT", "MULTIPOINT ((1 2), (3 4))"), "2FF16");
    assert_eq!(refused(&a, "ST_TOPOINT", "LINESTRING (0 0, 1 1)"), "2FF16");
    assert_eq!(cast(&a, "ST_TOPOINT", "LINESTRING EMPTY"), "POINT EMPTY");
    assert_eq!(rows(&a, "VALUES ST_SRID(ST_TOPOINT(ST_GEOMFROMTEXT('LINESTRING EMPTY', 4326)))").unwrap(), ["4326"]);
    assert!(cast(&a, "ST_TOLINESTRING", arc).starts_with("LINESTRING (0 0, "));
    assert!(cast(&a, "ST_TOLINESTRING", &format!("GEOMETRYCOLLECTION ({arc})")).starts_with("LINESTRING (0 0, "));
    assert_eq!(cast(&a, "ST_TOCIRCULAR", &format!("COMPOUNDCURVE ({arc})")), arc);
    assert_eq!(refused(&a, "ST_TOCIRCULAR", "COMPOUNDCURVE ((0 0, 1 0), CIRCULARSTRING (1 0, 2 1, 3 0))"), "2FF16");
    assert_eq!(cast(&a, "ST_TOCOMPOUND", "LINESTRING (0 0, 1 1)"), "COMPOUNDCURVE ((0 0, 1 1))");
    let square = "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))";
    assert_eq!(cast(&a, "ST_TOCURVEPOLY", square), square);
    assert_eq!(refused(&a, "ST_TOCURVEPOLY", "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 0)), ((5 5, 6 5, 6 6, 5 5)))"), "2FF16");
    let disc = "CURVEPOLYGON (CIRCULARSTRING (0 0, 2 2, 4 0, 2 -2, 0 0))";
    assert!(cast(&a, "ST_TOPOLYGON", disc).starts_with("POLYGON ((0 0, "));
    assert!(cast(&a, "ST_TOPOLYGON", &format!("GEOMETRYCOLLECTION ({disc})")).starts_with("POLYGON ((0 0, "));
    let triangle = "TRIANGLE (0 0, 1 0, 0 1)";
    assert_eq!(cast(&a, "ST_TOTRIANGLE", &format!("GEOMETRYCOLLECTION ({triangle})")), canon(&a, triangle));
    assert_eq!(cast(&a, "ST_TOPOLYGON", triangle), canon(&a, triangle));
    assert_eq!(refused(&a, "ST_TOTRIANGLE", square), "2FF16");
    assert_eq!(rows(&a, "VALUES ST_TOPOINT(CAST(NULL AS ST_GEOMETRY))").unwrap(), ["NULL"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.55 Description
/// 40 to 69] the surfaces, the solid, the collection and the multi types:
/// a compound surface and a collection hold SELF, a multi type a
/// collection's elements or SELF, and an element of no type it holds
/// raises 2FF08; ST_ToNURBS and ST_ToSpiral run as ST_ToNURBSCurve and
/// ST_ToSpiralCurve; each is a method of ST_Geometry.
#[test]
fn surfaces_solids_and_collections_cast_by_their_methods() {
    let a = session();
    let tin = "TIN (PATCHES ((0 0, 1 0, 0 1)))";
    assert_eq!(cast(&a, "ST_TOPOLYHDRLSURF", tin), canon(&a, tin));
    assert_eq!(cast(&a, "ST_TOTIN", &format!("GEOMETRYCOLLECTION ({tin})")), canon(&a, tin));
    assert_eq!(refused(&a, "ST_TOPOLYHDRLSURF", "POLYGON ((0 0, 1 0, 0 1, 0 0))"), "2FF16");
    let square = "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))";
    assert_eq!(cast(&a, "ST_TOCOMPSURFACE", square), canon(&a, &format!("COMPOUNDSURFACE ({square})")));
    let compound_tin = canon(&a, &format!("COMPOUNDSURFACE ({tin})"));
    assert_eq!(cast(&a, "ST_TOCOMPSURFACE", &format!("GEOMETRYCOLLECTION ({tin})")), compound_tin);
    assert_eq!(refused(&a, "ST_TOBREPSOLID", square), "2FF16");
    assert_eq!(cast(&a, "ST_TOBREPSOLID", "POINT EMPTY"), "BREPSOLID Z EMPTY");
    assert_eq!(cast(&a, "ST_TOGEOMCOLL", "POINT (1 2)"), "GEOMETRYCOLLECTION (POINT (1 2))");
    assert_eq!(cast(&a, "ST_TOGEOMCOLL", "MULTIPOINT ((1 2))"), "MULTIPOINT ((1 2))");
    assert_eq!(cast(&a, "ST_TOMULTIPOINT", "GEOMETRYCOLLECTION (POINT (1 2), POINT (3 4))"), "MULTIPOINT ((1 2), (3 4))");
    assert_eq!(cast(&a, "ST_TOMULTIPOINT", "POINT (1 2)"), "MULTIPOINT ((1 2))");
    assert_eq!(refused(&a, "ST_TOMULTIPOINT", "GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))"), "2FF08");
    assert_eq!(cast(&a, "ST_TOMULTICURVE", "CIRCULARSTRING (0 0, 1 1, 2 0)"), "MULTICURVE (CIRCULARSTRING (0 0, 1 1, 2 0))");
    assert!(cast(&a, "ST_TOMULTILINE", "CIRCULARSTRING (0 0, 1 1, 2 0)").starts_with("MULTILINESTRING ((0 0, "));
    assert_eq!(cast(&a, "ST_TOMULTISURFACE", square), canon(&a, &format!("MULTISURFACE ({square})")));
    assert_eq!(cast(&a, "ST_TOMULTIPOLYGON", "TRIANGLE (0 0, 1 0, 0 1)"), "MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))");
    assert_eq!(cast(&a, "ST_TOMULTIPOLYGON", "MULTIPOINT EMPTY"), "MULTIPOLYGON EMPTY");
    for f in ["ST_TONURBS", "ST_TONURBSCURVE", "ST_TOSPIRAL", "ST_TOSPIRALCURVE", "ST_TOCLOTHOID", "ST_TOELLIPTICAL"] {
        assert_eq!(refused(&a, f, "POINT (1 2)"), "2FF16", "{f}");
    }
    let method = "VALUES ST_GEOMFROMTEXT('GEOMETRYCOLLECTION (POINT (1 2))').ST_TOPOINT().ST_X()";
    assert_eq!(rows(&a, method).unwrap(), ["1E0"]);
    let typed = "VALUES ST_GEOMFROMTEXT('POINT (1 2)').ST_TOMULTIPOINT().SPECIFICTYPE";
    assert_eq!(rows(&a, typed).unwrap(), ["\"TURBOREL\".\"INFORMATION_SCHEMA\".\"ST_MULTIPOINT\""]);
}

/// The text of a constructor's value `sql` gives.
fn made(a: &Session, sql: &str) -> String {
    let sql = format!("VALUES ST_ASTEXT({sql})");
    let mut cursor = a.open_cursor(&sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let row = cursor.fetch().unwrap_or_else(|e| panic!("{sql}: {e}")).unwrap();
    rows(a, &sql).unwrap_or_else(|e| panic!("{sql}: {e:?} {row:?}")).into_iter().next().unwrap()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.7.1 Description
/// 9, §8.7.2, §8.7.3] ST_CompoundSurface of no argument is empty; of
/// text, GML and WKB it reads the value; of a surface or an array of
/// them, with an SRID or 0, it holds them; ST_Surfaces(ST_Surface ARRAY)
/// replaces them. Two next surfaces that do not meet in a curve raise
/// 2FF85, mixed systems 2FF10, mixed dimensions 2FF25, a null array
/// 2FF03 though SELF is null.
#[test]
fn compound_surfaces_are_constructed_and_mutated() {
    let a = session();
    let (one, two) = ("POLYGON ((0 0, 1 0, 1 1, 0 0))", "POLYGON ((0 0, 1 1, 0 1, 0 0))");
    let written = format!("COMPOUNDSURFACE ({one}, {two})");
    let both = canon(&a, &written);
    assert_eq!(made(&a, "NEW ST_COMPOUNDSURFACE()"), "COMPOUNDSURFACE EMPTY");
    assert_eq!(made(&a, &format!("ST_COMPOUNDSURFACE('{written}')")), both);
    assert_eq!(made(&a, &format!("ST_COMPOUNDSURFACE(ST_ASBINARY(ST_GEOMFROMTEXT('{written}')))")), both);
    let gml = format!("ST_COMPOUNDSURFACE(ST_ASGML(ST_GEOMFROMTEXT('{written}')))");
    assert_eq!(made(&a, &gml), both);
    let array = format!("ARRAY[ST_POLYFROMTEXT('{one}', 4326), ST_POLYFROMTEXT('{two}', 4326)]");
    assert_eq!(made(&a, &format!("NEW ST_COMPOUNDSURFACE({array}, 4326)")), both);
    assert_eq!(rows(&a, &format!("VALUES ST_SRID(ST_COMPOUNDSURFACE({array}, 4326))")).unwrap(), ["4326"]);
    assert_eq!(made(&a, &format!("ST_COMPOUNDSURFACE(ST_POLYFROMTEXT('{one}'))")), canon(&a, &format!("COMPOUNDSURFACE ({one})")));
    let replaced = format!("ST_COMPOUNDSURFACE(ST_POLYFROMTEXT('{one}')).ST_SURFACES(ARRAY[ST_POLYFROMTEXT('{two}')])");
    assert_eq!(made(&a, &replaced), canon(&a, &format!("COMPOUNDSURFACE ({two})")));
    assert_eq!(state(&a, &format!("VALUES ST_COMPOUNDSURFACE({array})")), "2FF10");
    let apart = "ARRAY[ST_POLYFROMTEXT('POLYGON ((0 0, 1 0, 1 1, 0 0))'), ST_POLYFROMTEXT('POLYGON ((5 5, 6 5, 6 6, 5 5))')]";
    assert_eq!(state(&a, &format!("VALUES ST_COMPOUNDSURFACE({apart})")), "2FF85");
    let mixed = format!("ARRAY[ST_POLYFROMTEXT('{one}'), ST_POLYFROMTEXT('POLYGON Z ((0 0 0, 1 1 0, 0 1 0, 0 0 0))')]");
    assert_eq!(state(&a, &format!("VALUES ST_COMPOUNDSURFACE({mixed})")), "2FF25");
    let null_array = "CAST(NULL AS ST_COMPOUNDSURFACE).ST_SURFACES(CAST(NULL AS ST_SURFACE ARRAY[2]))";
    assert_eq!(state(&a, &format!("VALUES {null_array}")), "2FF03");
    assert_eq!(state(&a, "VALUES ST_COMPOUNDSURFACE('POINT (1 2)')"), "2FF22");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §9.2.1 Description
/// 9, §9.2.2 Description 1 to 8] ST_BRepSolid of no argument is the empty
/// solid, and of text, GML and WKB reads the solid, of the SRID given;
/// a text of another type raises 2FF22.
#[test]
fn brep_solids_are_constructed_of_representations() {
    let a = session();
    let faces = [
        "POLYGON Z ((0 0 0, 0 1 0, 1 0 0, 0 0 0))",
        "POLYGON Z ((0 0 0, 1 0 0, 0 0 1, 0 0 0))",
        "POLYGON Z ((0 0 0, 0 0 1, 0 1 0, 0 0 0))",
        "POLYGON Z ((1 0 0, 0 1 0, 0 0 1, 1 0 0))",
    ];
    let written = format!("BREPSOLID Z (POLYHEDRALSURFACE Z (PATCHES ({})))", faces.join(", "));
    let solid = canon(&a, &written);
    assert_eq!(made(&a, "NEW ST_BREPSOLID()"), "BREPSOLID Z EMPTY");
    assert_eq!(made(&a, &format!("ST_BREPSOLID('{written}')")), solid);
    assert_eq!(rows(&a, &format!("VALUES ST_SRID(ST_BREPSOLID('{written}', 4326))")).unwrap(), ["4326"]);
    assert_eq!(made(&a, &format!("NEW ST_BREPSOLID(ST_ASBINARY(ST_GEOMFROMTEXT('{written}')), 4326)")), solid);
    assert_eq!(made(&a, &format!("ST_BREPSOLID(ST_ASGML(ST_GEOMFROMTEXT('{written}')))")), solid);
    assert_eq!(state(&a, "VALUES ST_BREPSOLID('POINT (1 2)')"), "2FF22");
}
