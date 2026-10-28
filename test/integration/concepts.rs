//! Concepts [X3.135-1986 §4], `<predicate>` and `<table expression>`
//! [X3.135-1986 §5.10, §5.19], and the two levels [X3.135-1986 §9]: what the
//! definitions require, observed through statements.

use turborel::db::{Completion, Database};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Value};

use crate::common::{code, count, exec, rows, setup};
use turborel::value::Spelled;

/// A database with schema `M` holding `D (K INTEGER, V CHAR(2))`, no
/// constraints, and no rows.
fn multisets() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION M CREATE TABLE D (K INTEGER, V CHAR(2))").unwrap();
    db
}

/// Verifies: [X3.135-1986 §4.1, §4.4] a table is a multi-set of rows and may
/// be empty: equal rows are kept apart, and a query of an empty table, or one
/// that keeps no rows, gives none.
#[test]
fn tables_are_multisets_and_may_be_empty() {
    let db = multisets();
    let s = db.session("M");
    assert_eq!(count(&s, "SELECT K FROM D"), 0);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM D"), Ok(vec!["0".into()]));
    for _ in 0..2 {
        s.run("INSERT INTO D VALUES (1, 'A')").unwrap();
    }
    assert_eq!(count(&s, "SELECT K, V FROM D"), 2);
    assert_eq!(count(&s, "SELECT DISTINCT K, V FROM D"), 1);
    assert_eq!(count(&s, "SELECT K FROM D WHERE K = 2"), 0);
}

/// Verifies: [X3.135-1986 §4.4] every row has a value of every column, in the
/// columns' order: a column an INSERT leaves out holds the null value, and "*"
/// gives the columns in ordinal position.
#[test]
fn every_row_has_a_value_of_every_column() {
    let db = multisets();
    let s = db.session("M");
    s.run("INSERT INTO D (V) VALUES ('B')").unwrap();
    assert_eq!(rows(&s, "SELECT * FROM D"), Ok(vec!["NULL,B ".into()]));
}

/// Verifies: [X3.135-1986 §4.2] the null value is distinct from every nonnull
/// value, and a character string and a number are not comparable values.
#[test]
fn null_is_distinct_and_strings_and_numbers_do_not_compare() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME IS NULL"), Ok(vec!["3".into()]));
    assert_eq!(count(&s, "SELECT ID FROM T WHERE NAME = 'ANN' OR NAME <> 'ANN'"), 2, "null is neither equal nor unequal");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME = 1"), Code::SyntaxRule);
}

/// Verifies: [X3.135-1986 §4.2.1] all character strings are comparable, and
/// two are identical if they are equal under the comparison rules, which pad
/// the shorter with spaces.
#[test]
fn character_strings_are_identical_when_equal() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME = 'ANN   '"), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, "SELECT COUNT(DISTINCT NAME) FROM T WHERE NAME = 'ANN' OR NAME = 'BOB'"), Ok(vec!["2".into()]));
}

/// Verifies: [X3.135-1986 §4.2.2] a number assigned to an exact column keeps
/// its leading significant digits and takes the column's scale; one that
/// would lose leading digits is refused; any number assigned to an
/// approximate column becomes an approximate value.
#[test]
fn numbers_take_the_type_of_their_target() {
    let db = setup();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID, AMT, F) VALUES (4, 1.239, 7)").unwrap();
    assert_eq!(rows(&s, "SELECT AMT, F FROM T WHERE ID = 4"), Ok(vec!["1.23,7E0".into()]));
    assert_eq!(exec(&s, "INSERT INTO T (ID, AMT) VALUES (5, 12345.6)"), Err(Code::Data));
}

/// Verifies: [X3.135-1986 §4.3] a column has a description: its data type,
/// whether it is constrained to nonnull values, and, for a named column, its
/// name. A result column that is a column specification inherits it.
#[test]
fn columns_have_descriptions() {
    let db = setup();
    let s = db.session("APP");
    let cursor = s.open_cursor("SELECT ID, NAME, AMT + 1 FROM T").unwrap();
    let described: Vec<(Option<String>, DataType, bool)> =
        cursor.columns.iter().map(|c| (c.name.clone(), c.data_type, c.not_null)).collect();
    assert_eq!(described[0], (Some("ID".into()), DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric }, true));
    assert_eq!(described[1], (Some("NAME".into()), DataType::Character { length: 6 }, false));
    assert_eq!((described[2].0.clone(), described[2].2), (None, false), "an expression is an unnamed column");
}

/// Verifies: [X3.135-1986 §4.6, §4.7] the tables a `<schema>` defines are
/// owned by its authorization identifier, and the database holds the tables
/// of every schema: each is reached by its qualified name, and only its owner
/// may read it [X3.135-1986 §4.15].
#[test]
fn schemas_own_their_tables_in_one_database() {
    let db = multisets();
    db.schema("CREATE SCHEMA AUTHORIZATION N CREATE TABLE D (K INTEGER)").unwrap();
    db.session("N").run("INSERT INTO D VALUES (7)").unwrap();
    assert_eq!(rows(&db.session("N"), "SELECT K FROM N.D"), Ok(vec!["7".into()]));
    assert_eq!(count(&db.session("M"), "SELECT K FROM D"), 0, "M.D is a different table");
    assert_eq!(code(&db.session("M"), "SELECT K FROM N.D"), Code::Privilege);
}

/// Verifies: [X3.135-1986 §4.8, §4.9, §4.10, §4.10.1, §4.10.2] a session plays
/// the part of a `<module>` and `execute` of a `<procedure>` call: the
/// parameters supply and receive values, the status says whether the call
/// completed, and an indicator parameter says whether a value is null.
#[test]
fn a_call_passes_parameters_and_returns_a_status() {
    let db = setup();
    let s = db.session("APP");
    let mut p = Params::new()
        .declare("K", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Exact(Decimal::new(3, 0)))
        .declare("N", DataType::Character { length: 6 }, Value::Null)
        .indicator("NI", 0);
    let found = s.execute("SELECT NAME INTO N NI FROM T WHERE ID = K", &mut p).unwrap();
    assert_eq!((found.sqlcode(), p.indicator_value("NI")), (0, Some(-1)), "found, and the name is null");
    p.set("K", Value::Exact(Decimal::new(9, 0)));
    assert_eq!(s.execute("SELECT NAME INTO N NI FROM T WHERE ID = K", &mut p).unwrap(), Completion::NoData);
}

/// Verifies: [X3.135-1986 §4.13] the `<SQL statement>`s operate on tables:
/// INSERT adds rows, UPDATE changes values in rows, DELETE removes rows, and
/// SELECT ... INTO fetches values.
#[test]
fn statements_operate_on_tables() {
    let db = multisets();
    let s = db.session("M");
    assert_eq!(exec(&s, "INSERT INTO D VALUES (1, 'A')"), Ok(Completion::Success));
    assert_eq!(exec(&s, "UPDATE D SET V = 'B' WHERE K = 1"), Ok(Completion::Success));
    let mut p = Params::new().declare("OUT", DataType::Character { length: 2 }, Value::Null);
    s.execute("SELECT V INTO OUT FROM D", &mut p).unwrap();
    assert_eq!(p.value("OUT").to_string(), "B ");
    assert_eq!(exec(&s, "DELETE FROM D WHERE K = 1"), Ok(Completion::Success));
    assert_eq!(count(&s, "SELECT K FROM D"), 0);
}

/// Verifies: [X3.135-1986 §5.10 GR1] a `<predicate>` is applied to each row on
/// its own: each row is kept or not by its own values.
#[test]
fn a_predicate_applies_to_each_row() {
    let db = setup();
    let s = db.session("APP");
    let mut kept = rows(&s, "SELECT ID FROM T WHERE AMT > 0 OR NAME IS NULL").unwrap();
    kept.sort();
    assert_eq!(kept, vec!["1".to_string(), "3".to_string()]);
}

/// Verifies: [X3.135-1986 §5.19 GR1] with no optional clause the table is the
/// FROM result; otherwise each clause applies to the result of the one before:
/// WHERE to the rows, GROUP BY to what WHERE kept, HAVING to the groups.
#[test]
fn table_expression_clauses_apply_in_order() {
    let db = multisets();
    let s = db.session("M");
    for (k, v) in [(1, "A"), (1, "A"), (2, "A"), (3, "B")] {
        s.run(&format!("INSERT INTO D VALUES ({k}, '{v}')")).unwrap();
    }
    assert_eq!(count(&s, "SELECT K FROM D"), 4);
    let query = "SELECT V, COUNT(*) FROM D WHERE K < 3 GROUP BY V HAVING COUNT(*) > 1";
    assert_eq!(rows(&s, query), Ok(vec!["A ,3".into()]), "K = 3 is gone before grouping, so B has no group");
}

/// Verifies: [X3.135-1986 §9] this implementation processes Level 2, the
/// complete language: the facilities Level 1 leaves out work, among them an
/// `<identifier>` longer than 12 characters (rule 2), a `<table name>` with an
/// `<authorization identifier>` (rule 3), USER and indicator parameters
/// (rule 4), ALL in a set function and a `<distinct set function>` of AVG
/// (rules 6, 7), the `<comp op>` <> (rule 8), a `<schema>` and a `<unique
/// constraint definition>` (rules 13, 14).
#[test]
fn level_2_facilities_are_processed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION LEVELTWO CREATE TABLE THIRTEEN_CHARS (K INTEGER NOT NULL, UNIQUE (K))").unwrap();
    let s = db.session("LEVELTWO");
    s.run("INSERT INTO LEVELTWO.THIRTEEN_CHARS VALUES (2)").unwrap();
    s.run("INSERT INTO THIRTEEN_CHARS VALUES (4)").unwrap();
    let query = "SELECT SUM(ALL K), AVG(DISTINCT K), USER FROM LEVELTWO.THIRTEEN_CHARS WHERE K <> 0";
    assert_eq!(rows(&s, query), Ok(vec!["6,3.00,LEVELTWO".into()]));
    let mut p =
        Params::new().declare("K", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Null).indicator("KI", -1);
    assert_eq!(s.execute("INSERT INTO THIRTEEN_CHARS VALUES (K KI)", &mut p).map_err(|e| e.code), Err(Code::Constraint));
}
