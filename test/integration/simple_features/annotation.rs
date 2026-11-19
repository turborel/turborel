//! [OGC 06-104r4 §7.2.20] The text types of Simple feature access,
//! ANNOTATION_TEXT, ANNOTATION_TEXT_ELEMENT and
//! ANNOTATION_TEXT_ELEMENT_ARRAY, their routines, and the metadata view
//! ANNOTATION_TEXT_METADATA [§6.2.9.2], the class tS [Annex A.2.6.2].

use turborel::error::Code;

use super::{database, described, run};
use crate::common::rows;

/// An element of text 'Blue Lake' at (60 15), with a leader line, in the
/// unspecified system.
const LAKE: &str = "ANNOTATION_TEXT_ELEMENT('Blue Lake', GeomFromText('POINT (60 15)'), \
     GeomFromText('LINESTRING (60 15, 62 18)'), '<font size=\"12\"/>')";
/// An element of text 'Route 75' at (10 40), with no leader line.
const ROAD: &str = "(NEW ANNOTATION_TEXT_ELEMENT()).Value('Route 75').Location(GeomFromText('POINT (10 40)'))";

fn one(sql: &str) -> Vec<String> {
    let db = database();
    rows(&db.session("A"), sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Verifies: [OGC 06-104r4 §7.2.20] An ANNOTATION_TEXT_ELEMENT is made by
/// its constructor of four attributes, by AnnotationTextElement or by NEW,
/// and each attribute is read by its method and changed by its method of
/// one argument; the constructor function's element has every attribute
/// null [ISO/IEC 9075-2:2023 §4.9.3.3]; each routine gives the null value
/// of a null argument.
#[test]
fn text_elements_are_made_read_and_changed() {
    let sql = format!("SELECT e.Value(), AsText(e.Location()), AsText(e.LeaderLine()), e.TextAttributes() FROM (VALUES ({LAKE})) AS t (e)");
    assert_eq!(one(&sql), ["Blue Lake,POINT (60 15),LINESTRING (60 15, 62 18),<font size=\"12\"/>"]);
    let sql = "SELECT e.Value() FROM (VALUES (AnnotationTextElement('x', GeomFromText('POINT (1 1)'), \
               GeomFromText('POINT (1 1)'), ''))) AS t (e)";
    assert_eq!(one(sql), ["x"]);
    let sql = format!("SELECT e.Value(), AsText(e.Location()), e.LeaderLine(), e.TextAttributes() FROM (VALUES ({ROAD})) AS t (e)");
    assert_eq!(one(&sql), ["Route 75,POINT (10 40),NULL,NULL"]);
    let sql = format!("SELECT e.TextAttributes('<b/>').TextAttributes(), e.Value('Lake').Value() FROM (VALUES ({LAKE})) AS t (e)");
    assert_eq!(one(&sql), ["<b/>,Lake"]);
    let sql = "SELECT e.Value() IS NULL, e.Location() IS NULL FROM (VALUES (NEW ANNOTATION_TEXT_ELEMENT())) AS t (e)";
    assert_eq!(one(sql), ["TRUE,TRUE"]);
    let sql =
        "VALUES (ANNOTATION_TEXT_ELEMENT(CAST(NULL AS VARCHAR(1)), GeomFromText('POINT (1 1)'), GeomFromText('POINT (1 1)'), '') IS NULL)";
    assert_eq!(one(sql), ["TRUE"]);
}

/// Verifies: [OGC 06-104r4 §7.2.20] An ANNOTATION_TEXT_ELEMENT_ARRAY is
/// made of an array of elements; ElementN(n) is element n, the null value
/// past its ends, and ElementN(element, n) replaces element n or adds one
/// after the last; a position further, or a null element, is refused.
#[test]
fn element_arrays_are_read_and_changed_by_position() {
    let array = format!("ANNOTATION_TEXT_ELEMENT_ARRAY(ARRAY[{LAKE}, {ROAD}])");
    let sql = format!(
        "SELECT a.ElementN(1).Value(), a.ElementN(2).Value(), a.ElementN(3) IS NULL, a.ElementN(0) IS NULL FROM (VALUES ({array})) AS t (a)"
    );
    assert_eq!(one(&sql), ["Blue Lake,Route 75,TRUE,TRUE"]);
    let sql = format!(
        "SELECT a.ElementN({ROAD}, 1).ElementN(1).Value(), a.ElementN({ROAD}, 3).ElementN(3).Value() FROM (VALUES ({array})) AS t (a)"
    );
    assert_eq!(one(&sql), ["Route 75,Route 75"]);
    let sql = format!("SELECT (NEW ANNOTATION_TEXT_ELEMENT_ARRAY()).ElementN({ROAD}, 1).ElementN(1).Value() FROM (VALUES (1)) AS t (x)");
    assert_eq!(one(&sql), ["Route 75"]);
    let db = database();
    let a = db.session("A");
    let far = format!("SELECT a.ElementN({ROAD}, 4) FROM (VALUES ({array})) AS t (a)");
    assert_eq!(rows(&a, &far).unwrap_err(), Code::Data);
    let null = format!("VALUES (ANNOTATION_TEXT_ELEMENT_ARRAY(ARRAY[{LAKE}, CAST(NULL AS ANNOTATION_TEXT_ELEMENT)]))");
    assert_eq!(rows(&a, &null).unwrap_err(), Code::Data);
}

/// Verifies: [OGC 06-104r4 §7.2.20] An ANNOTATION_TEXT holds its elements,
/// ELEMENT_ARRAY, and the envelope of their locations and leader lines,
/// ENVELOPE, null of none; CONCAT appends another text's elements and
/// widens the envelope; elements of two SRIDs are refused.
#[test]
fn texts_hold_their_elements_and_envelope() {
    let text = format!("ANNOTATION_TEXT(ANNOTATION_TEXT_ELEMENT_ARRAY(ARRAY[{LAKE}]))");
    let road = format!("ANNOTATION_TEXT(ANNOTATION_TEXT_ELEMENT_ARRAY(ARRAY[{ROAD}]))");
    let sql = format!("SELECT AsText(t.ENVELOPE()), t.ELEMENT_ARRAY().ElementN(1).Value() FROM (VALUES ({text})) AS v (t)");
    assert_eq!(one(&sql), ["POLYGON ((60 15, 62 15, 62 18, 60 18, 60 15)),Blue Lake"]);
    let sql = format!(
        "SELECT AsText(t.CONCAT({road}).ENVELOPE()), t.CONCAT({road}).ELEMENT_ARRAY().ElementN(2).Value() FROM (VALUES ({text})) AS v (t)"
    );
    assert_eq!(one(&sql), ["POLYGON ((10 15, 62 15, 62 40, 10 40, 10 15)),Route 75"]);
    let empty = "SELECT t.ENVELOPE() IS NULL, t.ELEMENT_ARRAY() IS NULL FROM (VALUES (NEW ANNOTATION_TEXT())) AS v (t)";
    assert_eq!(one(empty), ["TRUE,TRUE"]);
    let bare = "VALUES (ANNOTATION_TEXT(NEW ANNOTATION_TEXT_ELEMENT_ARRAY()).ENVELOPE() IS NULL)";
    assert_eq!(one(bare), ["TRUE"]);
    let db = database();
    let other = "(NEW ANNOTATION_TEXT_ELEMENT()).Location(GeomFromText('POINT (1 1)', 4326))";
    let mixed = format!("VALUES (ANNOTATION_TEXT(ANNOTATION_TEXT_ELEMENT_ARRAY(ARRAY[{LAKE}, {other}])))");
    assert_eq!(rows(&db.session("A"), &mixed).unwrap_err(), Code::Data);
}

/// Verifies: [OGC 06-104r4 §6.2.9.2, §7.2.20, Annex A.2.6.2] A column of
/// type ANNOTATION_TEXT keeps its values, shown as the constructors that
/// make them, and is listed by ANNOTATION_TEXT_METADATA, of the columns
/// Table 3 names, with no defaults, and not by GEOMETRY_COLUMNS.
#[test]
fn text_columns_are_kept_and_listed() {
    let db = database();
    let a = db.session("A");
    let text = format!("ANNOTATION_TEXT(ANNOTATION_TEXT_ELEMENT_ARRAY(ARRAY[{LAKE}]))");
    run(
        &a,
        &[
            "CREATE TABLE LABELS (ID INTEGER, LABEL ANNOTATION_TEXT, SHAPE POINT)",
            &format!("INSERT INTO LABELS VALUES (1, {text}, PointFromText('POINT (60 15)'))"),
        ],
    );
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT l.LABEL.ELEMENT_ARRAY().ElementN(1).Value() FROM LABELS AS l").unwrap(), ["Blue Lake"]);
    let shown = rows(&a, "SELECT CAST(LABEL AS VARCHAR(400)) FROM LABELS").unwrap();
    assert!(
        shown
            .first()
            .is_some_and(|s| s.starts_with("ANNOTATION_TEXT(ANNOTATION_TEXT_ELEMENT_ARRAY(ARRAY[ANNOTATION_TEXT_ELEMENT('Blue Lake'")),
        "{shown:?}"
    );
    let metadata = "SELECT F_TABLE_SCHEMA, F_TABLE_NAME, F_TEXT_COLUMN, A_TEXT_DEFAULT_MAP_BASE_SCALE, A_TEXT_DEFAULT_EXPRESSION, \
                    A_TEXT_DEFAULT_ATTRIBUTES FROM ANNOTATION_TEXT_METADATA";
    assert_eq!(rows(&a, metadata).unwrap(), ["A,LABELS,LABEL,NULL,NULL,NULL"]);
    assert_eq!(rows(&a, "SELECT F_GEOMETRY_COLUMN FROM GEOMETRY_COLUMNS WHERE F_TABLE_NAME = 'LABELS'").unwrap(), ["SHAPE"]);
    assert_eq!(rows(&db.session("B"), metadata).unwrap(), Vec::<String>::new());
    let id = |n: &str| format!("{n},CHARACTER VARYING,NO,128");
    let default = |n: &str| format!("{n},CHARACTER VARYING,YES,4096");
    let mut expected: Vec<String> = ["F_TABLE_CATALOG", "F_TABLE_SCHEMA", "F_TABLE_NAME", "F_TEXT_COLUMN"].map(id).to_vec();
    expected.extend(["A_TEXT_DEFAULT_MAP_BASE_SCALE", "A_TEXT_DEFAULT_EXPRESSION", "A_TEXT_DEFAULT_ATTRIBUTES"].map(default));
    assert_eq!(described(&a, "ANNOTATION_TEXT_METADATA"), expected);
}
