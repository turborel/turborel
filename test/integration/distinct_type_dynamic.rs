//! Distinct types in prepared statements and dynamic SQL [ISO/IEC
//! 9075-2:2023 §20.1, §20.7, §20.11, §20.12, §4.9.6; RULES.md D69]: a ?
//! taking a distinct type, an item described as of one, and values
//! crossing to and from the host as their source type's, by the transform
//! its definition makes (ID048).

use turborel::db::{Completion, Session};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{dynamic_host, money, rows, run_with as run, set_text as text};

/// The exact number `n` of scale `scale`.
fn exact(n: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(n, scale))
}

/// `money`'s session of A, T holding K 1 of P 5 and K 2 of P 7.50.
fn filled() -> (turborel::db::Database, Session) {
    let db = money();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 5)").unwrap();
    a.run("INSERT INTO T VALUES (2, 7.5)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// `dynamic_host` with NU, NUMERIC(10, 2), 3.25; IW, INTEGER, 3; and CH,
/// CHARACTER(10), '3'.
fn host() -> Params {
    let numeric = DataType::Exact { precision: 10, scale: 2, spelled: Spelled::Numeric };
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    dynamic_host().declare("NU", numeric, exact(325, 2)).declare("IW", integer, exact(3, 0)).declare(
        "CH",
        DataType::character(10),
        Value::Char("3".into()),
    )
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.7 SR; §9.18 SR10h i; §4.9.6] a ?
/// takes the type of the distinct type its context gives, and the value
/// given it is assigned to that type as to its source type; a ? argument of
/// FNUDT takes its parameter's type, the source's. A value no CAST makes a
/// number fails (22018).
#[test]
fn a_prepared_parameter_of_a_distinct_type_20_7() {
    let (_db, a) = filled();
    let query = a.prepare("SELECT K FROM T WHERE P = ?").unwrap();
    assert!(matches!(query.parameters(), [turborel::params::ParameterType::Of(DataType::Distinct(_))]));
    let mut c = a.open_prepared(&query, &[exact(75, 1)]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("2".to_owned()));
    drop(c);
    let insert = a.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    a.execute_prepared(&insert, &[exact(3, 0), exact(9, 0)]).unwrap();
    let e = a.execute_prepared(&insert, &[exact(4, 0), Value::Char("x".into())]).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22018");
    let call = a.prepare("SELECT MONEY(?) FROM T WHERE K = 1").unwrap();
    assert!(matches!(call.parameters(), [turborel::params::ParameterType::Of(DataType::Exact { precision: 10, scale: 2, .. })]));
    assert_eq!(rows(&a, "SELECT K, P FROM T ORDER BY K").unwrap(), ["1,5.00", "2,7.50", "3,9.00"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.1 Table 30, Table 34; §20.10 GR7d
/// viii; §20.1 SR6] DESCRIBE describes an item of a distinct type as TYPE
/// 17, USER_DEFINED_TYPE_CODE 1, and its name in USER_DEFINED_TYPE_CATALOG,
/// _SCHEMA and _NAME. Such an item is not valid, so a FETCH into it fails
/// (07002) and a USING of it (07001), until it is set to its source type.
#[test]
fn an_item_described_as_of_a_distinct_type_20_10() {
    let (_db, a) = filled();
    let mut p = host();
    text(&mut p, "SELECT K, P FROM T ORDER BY K");
    run(&a, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&a, "ALLOCATE DESCRIPTOR 'D' WITH MAX 5", &mut p).unwrap();
    run(&a, "DESCRIBE Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    let mut field = |f: &str, target: &str| {
        run(&a, &format!("GET DESCRIPTOR 'D' VALUE 2 {target} = {f}"), &mut p).unwrap();
        p.value(target).to_string().trim_end().to_owned()
    };
    assert_eq!([field("TYPE", "N"), field("USER_DEFINED_TYPE_CODE", "N")], ["17", "1"]);
    let named = ["USER_DEFINED_TYPE_CATALOG", "USER_DEFINED_TYPE_SCHEMA", "USER_DEFINED_TYPE_NAME"].map(|f| field(f, "TXT"));
    assert_eq!(named, ["TURBOREL", "A", "MONEY"]);
    a.declare_cursor("DECLARE R CURSOR FOR Q").unwrap();
    run(&a, "OPEN R", &mut p).unwrap();
    assert_eq!(run(&a, "FETCH R INTO SQL DESCRIPTOR 'D'", &mut p), Err("07002"));
    run(&a, "SET DESCRIPTOR 'D' VALUE 2 TYPE = 2, PRECISION = 10, SCALE = 2", &mut p).unwrap();
    assert_eq!(run(&a, "FETCH R INTO SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    run(&a, "GET DESCRIPTOR 'D' VALUE 2 NU = DATA", &mut p).unwrap();
    assert_eq!(p.value("NU").to_string(), "5.00", "the first row, as NUMERIC(10, 2): the FETCH refused fetched none");
    text(&mut p, "INSERT INTO T VALUES (?, ?)");
    run(&a, "PREPARE I FROM TEXT", &mut p).unwrap();
    run(&a, "DESCRIBE INPUT I USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(run(&a, "EXECUTE I USING SQL DESCRIPTOR 'D'", &mut p), Err("07001"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.11 GR6c iii, §20.12 GR5d iii; §4.9.6]
/// a value of a distinct type goes to a host target, and comes from a host
/// argument, as its source type's value, where the source type and the
/// host's are each assignable to the other. MC/DC: NUMERIC and INTEGER
/// hosts, both ways; a CHARACTER host neither way (07006 from it).
#[test]
fn values_cross_to_and_from_the_host_20_11() {
    let (_db, a) = filled();
    let mut p = host();
    text(&mut p, "SELECT P FROM T WHERE K = 1");
    run(&a, "PREPARE O FROM TEXT", &mut p).unwrap();
    run(&a, "EXECUTE O INTO NU", &mut p).unwrap();
    run(&a, "EXECUTE O INTO IW", &mut p).unwrap();
    assert_eq!([p.value("NU").to_string(), p.value("IW").to_string()], ["5.00", "5"]);
    assert!(run(&a, "EXECUTE O INTO CH", &mut p).is_err());
    p.set("NU", exact(325, 2));
    p.set("IW", exact(3, 0));
    text(&mut p, "INSERT INTO T VALUES (?, ?)");
    run(&a, "PREPARE I FROM TEXT", &mut p).unwrap();
    run(&a, "EXECUTE I USING IW, NU", &mut p).unwrap();
    run(&a, "EXECUTE I USING IW, IW", &mut p).unwrap();
    assert_eq!(run(&a, "EXECUTE I USING IW, CH", &mut p), Err("07006"));
    assert_eq!(rows(&a, "SELECT P FROM T WHERE K = 3 ORDER BY P").unwrap(), ["3.00", "3.25"]);
}
