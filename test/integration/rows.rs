//! Row types [ISO/IEC 9075-2:2023 §4.10, §6.2, §6.6, §6.16, §7.1, §7.16,
//! §8.2]: rows made, their fields referenced by `<field reference>` and by
//! `<identifier chain>`, spread by `<all fields reference>`, compared,
//! ordered, grouped, cast and assigned field by field.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{code, failure, fetched, rows, sorted};

/// Schema B: table R of an INTEGER K, P ROW(X INTEGER, Y VARCHAR(5)) and
/// Q ROW(A INTEGER, B ROW(C INTEGER, D INTEGER)), holding (1, (1, 'a'),
/// (1, (2, 3))), (2, (2, 'b'), (5, (6, 7))) and (3, null, null).
fn rows_b() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE R (K INTEGER, P ROW(X INTEGER, Y VARCHAR(5)), Q ROW(A INTEGER, B ROW(C INTEGER, D INTEGER)))").unwrap();
    let b = db.session("B");
    b.run("INSERT INTO R VALUES (1, ROW(1, 'a'), ROW(1, ROW(2, 3))), (2, (2, 'b'), (5, (6, 7))), (3, NULL, NULL)").unwrap();
    b.commit().unwrap();
    db
}

/// The rows of `sql`, in order, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.1 SR, §6.16 SR2, SR4, GR1] a row is
/// made by ROW ( ... ) or by a parenthesized list of two or more, and kept
/// whole; a `<field reference>` is the field of its row, null of a null
/// row, of a nested row too; one of no field of the row, or of a value of
/// no row type, is refused. A row in parentheses as an element of VALUES
/// is referenced too.
#[test]
fn field_reference_6_16() {
    let db = rows_b();
    let b = db.session("B");
    assert_eq!(sorted(&b, "SELECT K, P, Q FROM R"), ["1,ROW(1, 'a'),ROW(1, ROW(2, 3))", "2,ROW(2, 'b'),ROW(5, ROW(6, 7))", "3,NULL,NULL"]);
    assert_eq!(sorted(&b, "SELECT K, (P).Y, (Q).B.C, (R.Q).B.D FROM R"), ["1,a,2,3", "2,b,6,7", "3,NULL,NULL,NULL"]);
    assert_eq!(query(&b, "SELECT (ROW(K, 'z')).F2 FROM R WHERE K = 1"), ["z"], "UV077: F and its place");
    assert_eq!(code(&b, "SELECT (P).Z FROM R"), Code::SyntaxRule, "SR4");
    assert_eq!(code(&b, "SELECT (K).Z FROM R"), Code::SyntaxRule, "SR2");
    assert_eq!(query(&b, "VALUES (ROW(4, 'w')).F2"), ["w"], "§7.1 SR2: one element, not a list");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.6 SR9b iv, v, SR9c, SR11] an
/// `<identifier chain>` whose basis is a column of a row type, its second
/// identifier, is that column's fields in turn, of three identifiers or
/// more; of four whose first is the catalog's name, a column of the table
/// the next two name. Exactly one candidate basis is taken: of none the
/// chain is refused, a first part of four that is neither a correlation
/// name nor the catalog's 3D000; of two, both refused. A chain whose basis
/// is outward is an outer reference.
#[test]
fn identifier_chains_6_6() {
    let db = rows_b();
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE S (K INTEGER, Q ROW(B ROW(D INTEGER, E ROW(F INTEGER))))").unwrap();
    let c = db.session("C");
    c.run("INSERT INTO S VALUES (1, ROW(ROW(1, ROW(9)))), (2, ROW(ROW(7, ROW(8))))").unwrap();
    assert_eq!(sorted(&c, "SELECT S.Q.B.D, S.Q.B.E.F FROM S"), ["1,9", "7,8"], "SR9b iv 2");
    assert_eq!(sorted(&c, "SELECT TURBOREL.C.S.K FROM C.S"), ["1", "2"], "SR9b v");
    assert_eq!(failure(fetched(&c, "SELECT X.C.S.K FROM C.S")), (Code::SyntaxRule, "3D000".to_owned()), "no candidate");
    assert_eq!(code(&c, "SELECT S.K.B.D FROM S"), Code::SyntaxRule, "K is of no row type");
    assert_eq!(code(&c, "SELECT S.Q.B.Z FROM S"), Code::SyntaxRule, "no field Z");
    let outer = "SELECT K, (SELECT COUNT(*) FROM S AS T WHERE T.K = S.Q.B.D) FROM S";
    assert_eq!(sorted(&c, outer), ["1,1", "2,0"], "an outer reference");
    let b = db.session("B");
    assert_eq!(sorted(&b, "SELECT K, R.P.Y, R.Q.B FROM R"), ["1,a,ROW(2, 3)", "2,b,ROW(6, 7)", "3,NULL,NULL"]);
    c.run("CREATE TABLE W (C ROW(S ROW(K INTEGER)))").unwrap();
    let both = "SELECT TURBOREL.C.S.K FROM C.S, W AS TURBOREL";
    let e = fetched(&c, both).unwrap_err();
    assert_eq!((e.code, e.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §6.6 SR9c")), "a column and a field");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 SR4; ISO/IEC 9075-2:2023 §6.6 SR9b
/// iv 2] a set function's argument that is an identifier chain whose
/// basis is a column of a row type of the subquery's own table is no outer
/// reference, of three identifiers or four. A regression test: such a
/// chain was taken as a column of a table its first two name, found in no
/// table, so as an outer reference that made the enclosing query
/// aggregated.
#[test]
fn a_chain_in_a_set_function_is_local() {
    let db = rows_b();
    let b = db.session("B");
    assert_eq!(sorted(&b, "SELECT K, (SELECT MAX(S.P.X) FROM R AS S) FROM R"), ["1,2", "2,2", "3,2"]);
    assert_eq!(sorted(&b, "SELECT K, (SELECT MAX(S.Q.B.D) FROM R AS S) FROM R"), ["1,7", "2,7", "3,7"]);
    assert_eq!(code(&b, "SELECT K, (SELECT MAX(R.P.X) FROM R AS S) FROM R"), Code::SyntaxRule, "outer: R aggregated, K not grouped");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.16 SR10f, SR10g ii, SR11] an
/// `<asterisked identifier chain>` .* whose basis is a column of a row
/// type, and an `<all fields reference>`, are each of its fields, named as
/// the fields or as AS names them, as many names as fields [SR11a]. A
/// column alone is no basis [SR10f vi], and a value of no row type has no
/// fields.
#[test]
fn all_fields_7_16() {
    let db = rows_b();
    let b = db.session("B");
    assert_eq!(sorted(&b, "SELECT R.P.* FROM R"), ["1,a", "2,b", "NULL,NULL"]);
    assert_eq!(sorted(&b, "SELECT (Q).* FROM R"), ["1,ROW(2, 3)", "5,ROW(6, 7)", "NULL,NULL"]);
    assert_eq!(sorted(&b, "SELECT M, N FROM (SELECT R.P.* AS (M, N) FROM R) AS V"), ["1,a", "2,b", "NULL,NULL"], "T053");
    assert_eq!(code(&b, "SELECT P.* FROM R"), Code::SyntaxRule, "SR10f vi");
    assert_eq!(code(&b, "SELECT R.P.* AS (M) FROM R"), Code::SyntaxRule, "SR11a");
    assert_eq!(code(&b, "SELECT (K).* FROM R"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 SR5b iv, GR1b i, §7.2] rows are
/// compared field by field, a value of a row type in place of a row value
/// constructor as its fields, on either side. MC/DC of < of (1, null)
/// against (2, 0): True, the first fields deciding; of (2, 0) against (1,
/// 5): False, the first deciding though the second is less; of (null, 1)
/// against (2, 0): Unknown, the first unknown; of = with a null field:
/// Unknown, and False if another is unequal.
#[test]
fn compared_8_2() {
    let db = rows_b();
    let b = db.session("B");
    assert_eq!(query(&b, "SELECT K FROM R WHERE P = ROW(1, 'a')"), ["1"]);
    assert_eq!(query(&b, "SELECT K FROM R WHERE P = (2, 'b')"), ["2"]);
    assert_eq!(query(&b, "SELECT K FROM R WHERE (1, 'a') = P"), ["1"]);
    assert_eq!(query(&b, "SELECT K FROM R WHERE P < ROW(2, 'a')"), ["1"]);
    assert_eq!(query(&b, "SELECT K FROM R WHERE P IN (ROW(2, 'b'), ROW(9, 'z'))"), ["2"]);
    let truth = |c: &str| query(&b, &format!("SELECT CASE WHEN {c} THEN 'T' WHEN NOT ({c}) THEN 'F' ELSE 'U' END FROM R WHERE K = 1"));
    let null = "CAST(NULL AS INTEGER)";
    assert_eq!(truth(&format!("ROW(1, {null}) < ROW(2, 0)")), ["T"], "the first fields decide");
    assert_eq!(truth("ROW(2, 0) < ROW(1, 5)"), ["F"], "the first fields decide, greater");
    assert_eq!(truth(&format!("ROW({null}, 1) < ROW(2, 0)")), ["U"], "the first unknown");
    assert_eq!(truth(&format!("ROW(1, {null}) = ROW(1, 0)")), ["U"], "a field unknown");
    assert_eq!(truth(&format!("ROW(1, {null}) = ROW(2, 0)")), ["F"], "a field unequal");
    assert_eq!(code(&b, "SELECT K FROM R WHERE P = ROW(1, 'a', 2)"), Code::SyntaxRule, "degrees differ");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10, §9.12, §9.14, NOTE 377] rows
/// are ordered field by field, the null value after the others, so sorted,
/// grouped, made distinct and brought together by UNION; GREATEST takes
/// them; MAX and MIN of a row type are refused, as rows are not totally
/// ordered.
#[test]
fn ordered_and_grouped_10_10() {
    let db = rows_b();
    let b = db.session("B");
    assert_eq!(query(&b, "SELECT K, P FROM R ORDER BY P DESC"), ["3,NULL", "2,ROW(2, 'b')", "1,ROW(1, 'a')"]);
    assert_eq!(sorted(&b, "SELECT P, COUNT(*) FROM R GROUP BY P"), ["NULL,1", "ROW(1, 'a'),1", "ROW(2, 'b'),1"]);
    assert_eq!(fetched(&b, "SELECT DISTINCT Q FROM R").unwrap(), 3);
    assert_eq!(fetched(&b, "SELECT P FROM R UNION SELECT P FROM R").unwrap(), 3);
    assert_eq!(sorted(&b, "SELECT GREATEST(P, ROW(2, 'a')) FROM R"), ["NULL", "ROW(2, 'a')", "ROW(2, 'b')"]);
    assert_eq!(code(&b, "SELECT MAX(P) FROM R"), Code::SyntaxRule, "NOTE 377");
    assert_eq!(code(&b, "SELECT MIN(Q) FROM R"), Code::SyntaxRule, "NOTE 377");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 SR5, GR, §9.2 GR3] a row is cast
/// field by field to a row type of its degree, and stored so; to no other
/// type, nor to a row type of another degree; a field too long for its
/// target is 22001.
#[test]
fn cast_and_assigned_6_13() {
    let db = rows_b();
    let b = db.session("B");
    let cast = "SELECT CAST(P AS ROW(X DECIMAL(3,1), Y CHARACTER(3))) FROM R";
    assert_eq!(sorted(&b, cast), ["NULL", "ROW(1.0, 'a  ')", "ROW(2.0, 'b  ')"]);
    assert_eq!(code(&b, "SELECT CAST(P AS ROW(X INTEGER)) FROM R"), Code::SyntaxRule);
    assert_eq!(code(&b, "SELECT CAST(P AS INTEGER) FROM R"), Code::SyntaxRule);
    b.run("INSERT INTO R VALUES (4, ROW(4.0, 'd'), NULL)").unwrap();
    assert_eq!(query(&b, "SELECT P FROM R WHERE K = 4"), ["ROW(4, 'd')"]);
    assert_eq!(failure(b.run("INSERT INTO R VALUES (5, ROW(5, 'longer'), NULL)")), (Code::Data, "22001".to_owned()));
    assert_eq!(failure(b.run("INSERT INTO R VALUES (5, ROW(5), NULL)")).0, Code::SyntaxRule);
}
