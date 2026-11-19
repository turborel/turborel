//! The linear referencing values' text and GML read through the SQL
//! constructors [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14, §15.1.12
//! and its peers], the translating routines of §15.6.6 and §15.6.7, and
//! an LRM's constraints with a null element kept through each form.

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::{rows, state};

/// A session of a database of linear referencing routines, its table D
/// of one row.
fn session() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE D (X INTEGER) CREATE TABLE M (L ST_LRM)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO D VALUES (1)").unwrap();
    (db, a)
}

/// Checks each case: the value of the expression on the left, or, of an
/// expected text starting "ERR ", the SQLSTATE it raises.
fn check(a: &Session, cases: &[(&str, &str)]) {
    for (expr, expected) in cases {
        let sql = format!("SELECT {expr} FROM D");
        match expected.strip_prefix("ERR ") {
            Some(code) => assert_eq!(state(a, &sql), *code, "{expr}"),
            None => assert_eq!(rows(a, &sql).unwrap_or_else(|e| panic!("{expr}: {e:?}")), [*expected], "{expr}"),
        }
    }
}

/// The LRM 1 in metres, of no constraints.
const LRM: &str = "ST_LRM(1, 'km', 'absolute', 'metre', CAST(NULL AS VARCHAR(5) ARRAY[1]))";
/// Its text.
const LRM_TEXT: &str = "LRM LRMID 1, LRMNAME \"km\", LRMTYPE \"absolute\", LRMUOM \"metre\"";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.2 Description
/// 4b, §15.1.7 Description 2, 4b] ST_PrivateConstraints is set to
/// aconstraintarray as given, by the constructor and by ST_Constraints:
/// a null element in the middle stays null, read back by ST_Constraints,
/// through the text, the GML and a stored row. Regression test, the null
/// element was the empty string before the fix.
#[test]
fn a_null_constraint_stays_null_15_1_7() {
    let (_db, a) = session();
    let built = "ST_LRM(1, 'km', 'absolute', 'metre', ARRAY['a', CAST(NULL AS VARCHAR(5)), 'b'])";
    let changed = format!("{LRM}.ST_CONSTRAINTS(ARRAY['a', CAST(NULL AS VARCHAR(5)), 'b'])");
    let text = format!("{LRM_TEXT}, LRMCONSTRAINTS (\"a\", NULL, \"b\")");
    for value in [built, &changed] {
        check(
            &a,
            &[
                (value, &text),
                (&format!("{value}.ST_CONSTRAINTS()"), "ARRAY['a', NULL, 'b']"),
                (&format!("CARDINALITY({value}.ST_CONSTRAINTS())"), "3"),
            ],
        );
    }
    let gml = "<LRM><lrmID>1</lrmID><name>km</name><type>absolute</type><unitOfMeasure>metre</unitOfMeasure><constraints>\
               <constraint>a</constraint><constraint nil=\"true\"/><constraint>b</constraint></constraints></LRM>";
    check(
        &a,
        &[
            (&format!("ST_LRMFROMTEXT('{}').ST_CONSTRAINTS()", text.replace('\'', "''")), "ARRAY['a', NULL, 'b']"),
            (&format!("ST_LRMFROMGML('{gml}')"), &text),
            (&format!("ST_LRMFROMGML('{gml}').ST_CONSTRAINTS()"), "ARRAY['a', NULL, 'b']"),
        ],
    );
    a.run(&format!("INSERT INTO M VALUES ({built})")).unwrap();
    assert_eq!(rows(&a, "SELECT L.ST_CONSTRAINTS() FROM M").unwrap(), ["ARRAY['a', NULL, 'b']"]);
}
