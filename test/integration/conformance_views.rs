//! The views of what this implementation supports [ISO/IEC 9075-11:2006
//! draft §5.52 to §5.57]: SQL_FEATURES shows a feature supported exactly
//! where CONFORMANCE.md claims it, SQL_PARTS the parts, SQL_PACKAGES none,
//! SQL_SIZING each limit ISO/IEC 9075-2:2023 Annex B lists as the code
//! keeps it, and SQL_IMPLEMENTATION_INFO and SQL_SIZING_PROFILES none.

use std::collections::BTreeMap;

use turborel::db::{Database, Session};

use crate::common::{failure, rows};

/// The document whose table "Features claimed" SQL_FEATURES shows.
const CONFORMANCE: &str = include_str!("../../CONFORMANCE.md");

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Each feature CONFORMANCE.md claims, and the title of the section that
/// says how it is done.
fn claimed() -> BTreeMap<String, String> {
    let section = CONFORMANCE.split("\n## Features claimed\n").nth(1).unwrap().split("\n#").next().unwrap();
    let cells = |row: &str| row.trim_matches('|').split('|').map(|c| c.trim().to_owned()).collect::<Vec<_>>();
    let rows = section.lines().filter(|l| l.starts_with("| ") && !l.starts_with("| Feature ")).map(cells);
    rows.map(|c| (c[0].clone(), c[2].trim_start_matches('[').split("](").next().unwrap().to_owned())).collect()
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.52, §6.43 Description 4 to 8]
/// SQL_FEATURES has a row of each feature and subfeature, YES exactly
/// where CONFORMANCE.md claims it, or claims the feature a subfeature is
/// of, its COMMENTS the section saying how, else NO and null; IS_VERIFIED_BY
/// is null; a feature's SUB_FEATURE_ID and SUB_FEATURE_NAME are a single
/// space, a subfeature's FEATURE_NAME its feature's.
#[test]
fn features_are_supported_as_claimed_5_52() {
    let s = Database::new().session("ANY");
    let claims = claimed();
    assert!(claims.len() > 300 && claims.contains_key("T031") && claims.contains_key("F501"));
    let found = query(
        &s,
        "SELECT FEATURE_ID || '|' || SUB_FEATURE_ID || '|' || IS_SUPPORTED || '|' || COALESCE(IS_VERIFIED_BY, '-') || '|' \
         || COALESCE(COMMENTS, '-') || '|' || FEATURE_NAME || '|' || SUB_FEATURE_NAME FROM INFORMATION_SCHEMA.SQL_FEATURES",
    );
    let mut supported = 0;
    for row in &found {
        let [id, sub, is, verified, comments, name, sub_name] = row.split('|').collect::<Vec<_>>()[..].try_into().unwrap();
        let claim = if sub == " " { claims.get(id) } else { claims.get(&format!("{id}-{sub}")).or_else(|| claims.get(id)) };
        assert_eq!(is, if claim.is_some() { "YES" } else { "NO" }, "{row}");
        assert_eq!(comments, claim.map_or("-".to_owned(), |c| format!("CONFORMANCE.md, {c}")), "{row}");
        assert_eq!(verified, "-", "{row}");
        assert!(!name.trim().is_empty() && (sub != " " || sub_name == " "), "{row}");
        supported += usize::from(claim.is_some());
    }
    assert!(found.len() > 700 && supported > claims.len(), "{} rows, {supported} supported", found.len());
    let one = |id: &str, sub: &str| {
        query(
            &s,
            &format!(
                "SELECT FEATURE_NAME, SUB_FEATURE_NAME, IS_SUPPORTED, COMMENTS FROM INFORMATION_SCHEMA.SQL_FEATURES WHERE FEATURE_ID = '{id}' AND SUB_FEATURE_ID = '{sub}'"
            ),
        )
    };
    assert_eq!(one("T031", " "), ["BOOLEAN data type, ,YES,CONFORMANCE.md, Truth values"]);
    assert_eq!(one("T011", " "), ["Timestamp in Information Schema, ,NO,NULL"]);
    assert_eq!(one("T321", "06"), ["Basic SQL-invoked routines,ROUTINES view,YES,CONFORMANCE.md, SQL-invoked routines"]);
    assert_eq!(one("E011", "01"), ["Numeric data types,INTEGER and SMALLINT data types (including all spellings),NO,NULL"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.54, §5.55, §6.43 Description
/// 1] SQL_PARTS has a row of each part of ISO/IEC 9075 that ISO/IEC
/// 9075-2:2023 names, none claimed whole, as CONFORMANCE.md claims none;
/// SQL_PACKAGES has none, as no text defines a package.
#[test]
fn parts_and_packages_5_54_5_55() {
    let s = Database::new().session("ANY");
    let parts =
        query(&s, "SELECT PART, IS_SUPPORTED, IS_VERIFIED_BY, COMMENTS FROM INFORMATION_SCHEMA.SQL_PARTS ORDER BY CAST(PART AS INTEGER)");
    let numbers: Vec<&str> = parts.iter().map(|p| p.split(',').next().unwrap()).collect();
    assert_eq!(numbers, ["1", "2", "3", "4", "9", "10", "11", "13", "14", "15", "16"]);
    assert!(parts.iter().all(|p| p.ends_with(",NO,NULL,NULL")), "{parts:?}");
    assert_eq!(
        query(&s, "SELECT NAME FROM INFORMATION_SCHEMA.SQL_PARTS WHERE PART = '11'"),
        ["Information and Definition Schemas (SQL/Schemata)"]
    );
    assert_eq!(query(&s, "SELECT * FROM INFORMATION_SCHEMA.SQL_PACKAGES"), Vec::<String>::new());
}

/// The value SQL_SIZING shows of item `id` in `s`.
fn supported(s: &Session, id: u32) -> String {
    query(s, &format!("SELECT SUPPORTED_VALUE FROM INFORMATION_SCHEMA.SQL_SIZING WHERE SIZING_ID = {id}")).remove(0)
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.56, §6.45 Description 3;
/// ISO/IEC 9075-2:2023 Annex B] SQL_SIZING has a row of each limit Annex
/// B lists, numbered as it numbers them, of the value the code keeps: a
/// CHARACTER of that length is valid and one longer is SQLCODE -900, so
/// for a BINARY, a NUMERIC's precision, a TIME's fractional seconds and
/// the savepoints of a transaction (3B002 past them); of an item with no
/// limit 0, of one no feature here needs null.
#[test]
fn sizing_items_are_the_limits_kept_5_56() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    let ids = query(&s, "SELECT SIZING_ID FROM INFORMATION_SCHEMA.SQL_SIZING ORDER BY SIZING_ID");
    assert_eq!(ids.len(), 40);
    assert_eq!((ids[0].as_str(), ids[39].as_str()), ("1", "208"));
    assert_eq!(
        query(&s, "SELECT SIZING_NAME, SUPPORTED_VALUE, COMMENTS FROM INFORMATION_SCHEMA.SQL_SIZING WHERE SIZING_ID = 2"),
        ["The maximum length of a large object character string type.,4294967296,IL002, in characters"]
    );
    assert_eq!((supported(&s, 39), supported(&s, 66)), ("0".to_owned(), "NULL".to_owned()));
    for (id, valid) in [(1, "CHARACTER({})"), (3, "BINARY({})"), (9, "NUMERIC({})"), (45, "TIME({})")] {
        let n: u32 = supported(&s, id).parse().unwrap();
        s.run(&format!("CREATE TABLE T{id} (C {})", valid.replace("{}", &n.to_string()))).unwrap();
        let past = s.run(&format!("CREATE TABLE U{id} (C {})", valid.replace("{}", &(n + 1).to_string())));
        assert_eq!(past.expect_err(valid).sqlcode(), -900, "IL{id:03}");
    }
    s.commit().unwrap();
    let most: u32 = supported(&s, 208).parse().unwrap();
    for n in 0..most {
        s.run(&format!("SAVEPOINT S{n}")).unwrap();
    }
    assert_eq!(failure(s.run("SAVEPOINT MORE")).1, "3B002");
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.53, §5.57, §6.44, §6.46]
/// SQL_IMPLEMENTATION_INFO and SQL_SIZING_PROFILES have the draft's
/// columns and no row, as no implementation information item and no
/// profile is defined; each view of what is supported has the draft's
/// columns, in its order.
#[test]
fn the_views_have_the_drafts_columns_5_52_to_5_57() {
    let s = Database::new().session("ANY");
    let columns = |view: &str| {
        let sql = format!(
            "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = 'INFORMATION_SCHEMA' AND TABLE_NAME = '{view}' ORDER BY ORDINAL_POSITION"
        );
        query(&s, &sql).join(" ")
    };
    let views = [
        ("SQL_FEATURES", "FEATURE_ID FEATURE_NAME SUB_FEATURE_ID SUB_FEATURE_NAME IS_SUPPORTED IS_VERIFIED_BY COMMENTS"),
        ("SQL_IMPLEMENTATION_INFO", "IMPLEMENTATION_INFO_ID IMPLEMENTATION_INFO_NAME INTEGER_VALUE CHARACTER_VALUE COMMENTS"),
        ("SQL_PACKAGES", "ID NAME IS_SUPPORTED IS_VERIFIED_BY COMMENTS"),
        ("SQL_PARTS", "PART NAME IS_SUPPORTED IS_VERIFIED_BY COMMENTS"),
        ("SQL_SIZING", "SIZING_ID SIZING_NAME SUPPORTED_VALUE COMMENTS"),
        ("SQL_SIZING_PROFILES", "SIZING_ID SIZING_NAME PROFILE_ID PROFILE_NAME REQUIRED_VALUE COMMENTS"),
    ];
    for (view, names) in views {
        assert_eq!(columns(view), names, "{view}");
    }
    for view in ["SQL_IMPLEMENTATION_INFO", "SQL_SIZING_PROFILES"] {
        assert_eq!(query(&s, &format!("SELECT * FROM INFORMATION_SCHEMA.{view}")), Vec::<String>::new(), "{view}");
    }
}
