//! Translating positions between linear elements in SQL [ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §15.2.7, §15.2.8]: ST_TranslateToInst
//! and ST_TranslateToType, of the LRMs and registered columns of the
//! §21 catalog.

use turborel::db::Database;
use turborel::value::Value;

use crate::common::{rows, state};

/// Schema A: T of a road along (0 0) to (100 0) and a rail along (0 0)
/// to (200 0), each a curve element of LRM 1, the rail of type "rail",
/// in a registered column; LRM 1 defined, absolute, in metres.
fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, C ST_LRCURVE)").unwrap();
    let a = db.session("A");
    a.run("CALL ST_DEFINELRM('KM', 1, 'ACME', 7, 'LRM LRMID 1, LRMNAME \"km\", LRMTYPE \"absolute\", LRMUOM \"metre\"', 'posts')").unwrap();
    let curve = |id: i32, kind: &str, to: i32| {
        format!(
            "NEW ST_LRCurve({id}, 1, ST_LRMeasure(0), {kind}, CAST(NULL AS ST_STARTVALUE ARRAY[2]), \
             ST_GEOMFROMTEXT('LINESTRING (0 0, {to} 0)'))"
        )
    };
    a.run(&format!("INSERT INTO T VALUES (1, {}), (2, {})", curve(1, "'road'", 100), curve(2, "'rail'", 200))).unwrap();
    a.run("CALL ST_REGISTERLRCOLUMN('A', 'T', 'C', 1)").unwrap();
    a.commit().unwrap();
    db
}

/// The position 25 metres along the road, naming it by value and LRM 1
/// by id.
const AT_25: &str = "ST_PositionExp(R.C, 1, ST_DistanceExp(ST_LRMeasure(25, 'metre')))";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.7 Description
/// 2] ST_TranslateToInst gives the distance along the target element of
/// the place a position on SELF locates; of a null SELF, position or LRM
/// it gives the null value, as a routine of RETURNS NULL ON NULL INPUT
/// does; an LRM the catalog does not hold raises 2FF87.
#[test]
fn translate_to_an_instance_15_2_7() {
    let db = database();
    let a = db.session("A");
    let query = |lrm: &str| {
        format!(
            "SELECT R.C.ST_TRANSLATETOINST({AT_25}, S.C, {lrm}).ST_DISTANCEALONG().ST_MEASURE() \
             FROM T AS R, T AS S WHERE R.ID = 1 AND S.ID = 2"
        )
    };
    assert_eq!(rows(&a, &query("1")).unwrap(), ["2.5E1"]);
    assert_eq!(rows(&a, &query("CAST(NULL AS INTEGER)")).unwrap(), ["NULL"]);
    assert_eq!(state(&a, &query("9")), "2FF87");
    let null_self = "SELECT CAST(NULL AS ST_LRCURVE).ST_TRANSLATETOINST(CAST(NULL AS ST_POSITIONEXP), C, 1) FROM T WHERE ID = 2";
    assert_eq!(rows(&a, null_self).unwrap(), ["NULL"]);
    let null_position = "SELECT C.ST_TRANSLATETOINST(CAST(NULL AS ST_POSITIONEXP), C, 1) FROM T WHERE ID = 2";
    assert_eq!(rows(&a, null_position).unwrap(), ["NULL"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.8 Description
/// 2] ST_TranslateToType gives a position on each element of the type
/// named, of the registered columns, that holds the place; a type no
/// element holding it is of raises 2FF87.
#[test]
fn translate_to_a_type_15_2_8() {
    let db = database();
    let a = db.session("A");
    let query = |kind: &str| format!("SELECT CARDINALITY(R.C.ST_TRANSLATETOTYPE({AT_25}, '{kind}', 1)) FROM T AS R WHERE R.ID = 1");
    assert_eq!(rows(&a, &query("rail")).unwrap(), ["1"]);
    let found = format!("SELECT R.C.ST_TRANSLATETOTYPE({AT_25}, 'rail', 1)[1].ST_LINEARELEMENTID() FROM T AS R WHERE R.ID = 1");
    assert_eq!(rows(&a, &found).unwrap(), ["2"]);
    assert_eq!(state(&a, &query("canal")), "2FF87");
}
