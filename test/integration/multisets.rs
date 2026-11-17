//! Multisets [ISO/IEC 9075-2:2023 §4.12.3, §6.26, §6.31, §6.50 to §6.52,
//! §8.16 to §8.18]: made, compared, combined and tested. Their storage,
//! casts, aggregates, unnesting, user-defined types and description are
//! in `multiset_columns.rs`.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{code, failure, fetched, rows, sorted};

/// Schema A: table T of an INTEGER K and an INTEGER MULTISET M, holding
/// (1, {1, 2, 2}), (2, {}), (3, null) and (4, {null, 3}).
pub fn multisets() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, M INTEGER MULTISET)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, MULTISET[1, 2, 2]), (2, MULTISET[]), (3, NULL), (4, MULTISET[CAST(NULL AS INTEGER), 3])").unwrap();
    a.commit().unwrap();
    db
}

/// The one value of the one row of `sql`.
pub fn one(s: &Session, sql: &str) -> String {
    let found = rows(s, sql).unwrap();
    assert_eq!(found.len(), 1, "{sql}");
    found.into_iter().next().unwrap()
}

/// The truth value of the condition `c`, read of a row of T.
fn truth(s: &Session, c: &str) -> String {
    one(s, &format!("SELECT CASE WHEN {c} THEN 'T' WHEN NOT ({c}) THEN 'F' ELSE 'U' END FROM T WHERE K = 1"))
}

/// The SQLCODE class and SQLSTATE of the query `sql`, which must fail.
pub fn refused(s: &Session, sql: &str) -> (Code, String) {
    failure(fetched(s, sql))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1, §6.52 GR2, §6.5 GR1,
/// §6.31 GR1b] a multiset column keeps the multisets inserted, an empty
/// one, a null one and null elements among them, each counted by
/// CARDINALITY, null of a null multiset.
#[test]
fn multisets_are_kept_6_52() {
    let db = multisets();
    let a = db.session("A");
    let kept = sorted(&a, "SELECT K, M, CARDINALITY(M) FROM T");
    assert_eq!(kept, ["1,MULTISET[1, 2, 2],3", "2,MULTISET[],0", "3,NULL,NULL", "4,MULTISET[NULL, 3],2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.52 SR3, GR2, §9.5, §6.5
/// SR1] a `<multiset value constructor by enumeration>` is of the type its
/// elements bring together and may nest; MULTISET[] has a type only where
/// its context gives one, and elements of no common type are refused.
#[test]
fn constructed_by_enumeration_6_52() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT MULTISET[1, 2.5] FROM T WHERE K = 1"), "MULTISET[1.0, 2.5]", "§9.5");
    assert_eq!(one(&a, "SELECT MULTISET[MULTISET[1], MULTISET[2, 3]] FROM T WHERE K = 1"), "MULTISET[MULTISET[1], MULTISET[2, 3]]");
    assert_eq!(code(&a, "SELECT MULTISET[] FROM T"), Code::SyntaxRule, "§6.5 SR1");
    assert_eq!(code(&a, "SELECT MULTISET[1, 'a'] FROM T"), Code::SyntaxRule, "§9.5");
    assert_eq!(one(&a, "SELECT CARDINALITY(CAST(MULTISET[] AS INTEGER MULTISET)) FROM T WHERE K = 1"), "0", "§6.5 SR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.52 SR1, SR4, GR3] a
/// `<multiset value constructor by query>` is the multiset of the values
/// of its query's rows, empty of none; its query is of degree one and has
/// no ORDER BY. A `<table value constructor by query>` is the multiset of
/// a ROW of each row's columns.
#[test]
fn constructed_by_query_6_52() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(truth(&a, "MULTISET(SELECT K FROM T) = MULTISET[4, 3, 2, 1]"), "T", "GR3");
    assert_eq!(one(&a, "SELECT MULTISET(SELECT K FROM T WHERE K > 9) FROM T WHERE K = 1"), "MULTISET[]", "GR3");
    assert_eq!(refused(&a, "SELECT MULTISET(SELECT K, M FROM T) FROM T").0, Code::SyntaxRule, "SR4a");
    assert_eq!(refused(&a, "SELECT MULTISET(SELECT K FROM T ORDER BY K) FROM T").0, Code::SyntaxRule, "SR4b");
    assert_eq!(refused(&a, "SELECT TABLE(SELECT K FROM T ORDER BY K) FROM T").0, Code::SyntaxRule, "SR1b");
    assert_eq!(one(&a, "SELECT TABLE(SELECT K, K * 10 FROM T WHERE K < 3) FROM T WHERE K = 1"), "MULTISET[ROW(1, 10), ROW(2, 20)]", "SR1e");
    assert_eq!(truth(&a, "ROW(2, 20) MEMBER OF TABLE(SELECT K, K * 10 FROM T)"), "T", "SR1e");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.26 SR1, §7.11 GR1] ELEMENT
/// is the one element of a multiset of one; null of an empty or a null
/// multiset, as the scalar subquery of no row; a cardinality violation,
/// 21000, of more than one.
#[test]
fn element_reference_6_26() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT ELEMENT(MULTISET[5]) FROM T WHERE K = 1"), "5");
    assert_eq!(sorted(&a, "SELECT K, ELEMENT(M) FROM T WHERE K IN (2, 3)"), ["2,NULL", "3,NULL"]);
    assert_eq!(refused(&a, "SELECT ELEMENT(M) FROM T WHERE K = 1"), (Code::Cardinality, "21000".to_owned()));
    assert_eq!(code(&a, "SELECT ELEMENT(K) FROM T"), Code::SyntaxRule, "§6.50 SR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.51 SR2] SET keeps one
/// element of each that are not distinct from one another, nulls among
/// them; null of a null multiset.
#[test]
fn set_function_6_51() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(
        one(&a, "SELECT SET(MULTISET[2, 1, 2, CAST(NULL AS INTEGER), CAST(NULL AS INTEGER)]) FROM T WHERE K = 1"),
        "MULTISET[2, 1, NULL]"
    );
    assert_eq!(sorted(&a, "SELECT K, SET(M) FROM T"), ["1,MULTISET[1, 2]", "2,MULTISET[]", "3,NULL", "4,MULTISET[NULL, 3]"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.50 SR2 to SR4, §7.17 GR3]
/// MULTISET UNION, EXCEPT and INTERSECT, ALL or DISTINCT, keep as many of
/// each value as the query of UNNEST of each their rules give, nulls not
/// distinct from one another; null if either operand is; INTERSECT binds
/// tighter than UNION and EXCEPT, which go left to right; of a multiset
/// of the element types brought together.
#[test]
fn multiset_operators_6_50() {
    let db = multisets();
    let a = db.session("A");
    let (x, y) = ("MULTISET[1, 1, 2, CAST(NULL AS INTEGER)]", "MULTISET[1, 3, CAST(NULL AS INTEGER), CAST(NULL AS INTEGER)]");
    let of = |op: &str| one(&a, &format!("SELECT {x} MULTISET {op} {y} FROM T WHERE K = 1"));
    assert_eq!(of("UNION"), "MULTISET[1, 1, 2, NULL, 1, 3, NULL, NULL]", "ALL implicit");
    assert_eq!(of("UNION ALL"), "MULTISET[1, 1, 2, NULL, 1, 3, NULL, NULL]");
    assert_eq!(of("UNION DISTINCT"), "MULTISET[1, 2, NULL, 3]");
    assert_eq!(of("EXCEPT ALL"), "MULTISET[1, 2]");
    assert_eq!(of("EXCEPT DISTINCT"), "MULTISET[2]");
    assert_eq!(of("INTERSECT ALL"), "MULTISET[1, NULL]");
    assert_eq!(of("INTERSECT DISTINCT"), "MULTISET[1, NULL]");
    let precedence = "MULTISET[9] MULTISET UNION MULTISET[1, 2] MULTISET INTERSECT MULTISET[2]";
    assert_eq!(one(&a, &format!("SELECT {precedence} FROM T WHERE K = 1")), "MULTISET[9, 2]", "INTERSECT first");
    let left = "MULTISET[1, 2] MULTISET EXCEPT MULTISET[1] MULTISET UNION MULTISET[1]";
    assert_eq!(one(&a, &format!("SELECT {left} FROM T WHERE K = 1")), "MULTISET[2, 1]", "left to right");
    assert_eq!(one(&a, "SELECT MULTISET[1] MULTISET UNION MULTISET[2.5] FROM T WHERE K = 1"), "MULTISET[1.0, 2.5]", "§9.5");
    assert_eq!(sorted(&a, "SELECT K, M MULTISET UNION MULTISET[7] FROM T WHERE K IN (2, 3)"), ["2,MULTISET[7]", "3,NULL"]);
    assert_eq!(code(&a, "SELECT M MULTISET UNION K FROM T"), Code::SyntaxRule, "SR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR1b iii, §8.15 GR1c vi,
/// §4.12.4] two multisets are equal if some pairing makes each pair
/// equal, unknown if one makes each pair equal or unknown, else false;
/// distinct if some value is not distinct from more elements of one; and
/// compared for equality alone [§9.14 SR3].
#[test]
fn multisets_compared_8_2() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(truth(&a, "MULTISET[1, 2, 2] = MULTISET[2, 1, 2]"), "T");
    assert_eq!(truth(&a, "MULTISET[1, 2, 2] = MULTISET[1, 1, 2]"), "F");
    assert_eq!(truth(&a, "MULTISET[1, 2] = MULTISET[1, 2, 2]"), "F", "cardinality");
    assert_eq!(truth(&a, "MULTISET[1, CAST(NULL AS INTEGER)] = MULTISET[2, 1]"), "U", "a pairing of equal or unknown pairs");
    assert_eq!(truth(&a, "MULTISET[1, CAST(NULL AS INTEGER)] = MULTISET[2, 3]"), "F", "1 pairs with no element");
    assert_eq!(truth(&a, "MULTISET[1, 2] <> MULTISET[2, 1]"), "F");
    assert_eq!(
        truth(&a, "MULTISET[CAST(NULL AS INTEGER), 1] IS NOT DISTINCT FROM MULTISET[1, CAST(NULL AS INTEGER)]"),
        "T",
        "§8.15 GR1c vi"
    );
    assert_eq!(truth(&a, "MULTISET[CAST(NULL AS INTEGER), 1] IS DISTINCT FROM MULTISET[1, 1]"), "T", "§8.15 GR1c vi");
    assert_eq!(truth(&a, "MULTISET[1, 1, 2] IS DISTINCT FROM MULTISET[1, 2, 2]"), "T", "other counts");
    assert_eq!(code(&a, "SELECT K FROM T WHERE M < MULTISET[1]"), Code::SyntaxRule, "§9.14 SR3");
    assert_eq!(code(&a, "SELECT K FROM T WHERE M = ARRAY[1]"), Code::SyntaxRule, "§4.12.4");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.16 GR3] MEMBER, MC/DC of
/// its cases: false of an empty multiset (even of a null X) \[GR3a\];
/// unknown of a null X or multiset \[GR3b\]; true of an equal element
/// \[GR3c i\]; unknown of a null element and none equal \[GR3c ii\]; false
/// else \[GR3c iii\]. NOT MEMBER negates it \[SR5\], OF is optional, and a
/// row X is compared with rows.
#[test]
fn member_predicate_8_16() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(truth(&a, "CAST(NULL AS INTEGER) MEMBER OF CAST(MULTISET[] AS INTEGER MULTISET)"), "F", "GR3a");
    assert_eq!(truth(&a, "CAST(NULL AS INTEGER) MEMBER OF MULTISET[1]"), "U", "GR3b, X");
    assert_eq!(truth(&a, "1 MEMBER OF CAST(NULL AS INTEGER MULTISET)"), "U", "GR3b, MV");
    assert_eq!(truth(&a, "2 MEMBER OF MULTISET[1, 2]"), "T", "GR3c i");
    assert_eq!(truth(&a, "2 MEMBER OF MULTISET[1, CAST(NULL AS INTEGER)]"), "U", "GR3c ii");
    assert_eq!(truth(&a, "3 MEMBER OF MULTISET[1, 2]"), "F", "GR3c iii");
    assert_eq!(truth(&a, "2 MEMBER MULTISET[1, CAST(NULL AS INTEGER), 2]"), "T", "OF optional; equal before unknown");
    assert_eq!(truth(&a, "3 NOT MEMBER OF MULTISET[1, 2]"), "T", "SR5");
    assert_eq!(truth(&a, "(1, 2) MEMBER OF MULTISET[ROW(1, 2), ROW(3, 4)]"), "T", "SR2b");
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE 2 MEMBER OF M"), ["1"]);
    assert_eq!(code(&a, "SELECT K FROM T WHERE 'a' MEMBER OF M"), Code::SyntaxRule, "SR3");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.17 GR3] SUBMULTISET, MC/DC
/// of its cases: true of an empty CV (even of a null MV) \[GR3a\]; unknown
/// of a null CV or MV \[GR3b\]; false of a CV of more elements \[GR3c i\];
/// true of a pairing of equal pairs \[GR3c ii\]; unknown of one of equal or
/// unknown pairs \[GR3c iii\]; false else \[GR3c iv\].
#[test]
fn submultiset_predicate_8_17() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(truth(&a, "CAST(MULTISET[] AS INTEGER MULTISET) SUBMULTISET OF CAST(NULL AS INTEGER MULTISET)"), "T", "GR3a");
    assert_eq!(truth(&a, "MULTISET[1] SUBMULTISET OF CAST(NULL AS INTEGER MULTISET)"), "U", "GR3b");
    assert_eq!(truth(&a, "MULTISET[1, 1, 1] SUBMULTISET OF MULTISET[1, 1]"), "F", "GR3c i");
    assert_eq!(truth(&a, "MULTISET[2, 1] SUBMULTISET OF MULTISET[1, 3, 2]"), "T", "GR3c ii");
    assert_eq!(truth(&a, "MULTISET[1, CAST(NULL AS INTEGER)] SUBMULTISET OF MULTISET[3, 1]"), "U", "GR3c iii");
    assert_eq!(truth(&a, "MULTISET[1, 1] SUBMULTISET OF MULTISET[1, 2]"), "F", "GR3c iv");
    assert_eq!(truth(&a, "MULTISET[1, 1] NOT SUBMULTISET MULTISET[1, 2]"), "T", "SR5, OF optional");
    assert_eq!(code(&a, "SELECT K FROM T WHERE (M, M) SUBMULTISET OF M"), Code::SyntaxRule, "SR1");
    assert_eq!(code(&a, "SELECT K FROM T WHERE M SUBMULTISET OF MULTISET['a']"), Code::SyntaxRule, "SR3");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.18 SR3, SR4] IS A SET is
/// whether no two elements are not distinct from each other, nulls not
/// distinct from one another; unknown of a null multiset; IS NOT A SET its
/// negation.
#[test]
fn set_predicate_8_18() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(truth(&a, "MULTISET[1, 2] IS A SET"), "T");
    assert_eq!(truth(&a, "MULTISET[1, CAST(NULL AS INTEGER), CAST(NULL AS INTEGER)] IS A SET"), "F");
    assert_eq!(truth(&a, "CAST(NULL AS INTEGER MULTISET) IS A SET"), "U");
    assert_eq!(truth(&a, "MULTISET[1, 1] IS NOT A SET"), "T", "SR3");
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE M IS A SET"), ["2", "4"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.13 SR3, §9.12 SR3, §9.14
/// SR3] a multiset is grouped and ordered by no operation, and a multiset
/// element grouping operation refuses a multiset of arrays, whose
/// elements nothing groups, while UNION ALL, which groups none, takes one.
#[test]
fn grouping_refused_9_13() {
    let db = multisets();
    let a = db.session("A");
    for sql in ["SELECT M FROM T ORDER BY M", "SELECT M FROM T GROUP BY M", "SELECT DISTINCT M FROM T", "SELECT MAX(M) FROM T"] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    let arrays = "MULTISET[ARRAY[1]]";
    for sql in [
        format!("SELECT SET({arrays}) FROM T"),
        format!("SELECT {arrays} MULTISET UNION DISTINCT {arrays} FROM T"),
        format!("SELECT {arrays} MULTISET EXCEPT ALL {arrays} FROM T"),
        format!("SELECT {arrays} MULTISET INTERSECT ALL {arrays} FROM T"),
        format!("SELECT K FROM T WHERE {arrays} = {arrays}"),
        format!("SELECT K FROM T WHERE {arrays} IS A SET"),
        format!("SELECT K FROM T WHERE {arrays} SUBMULTISET OF {arrays}"),
    ] {
        assert_eq!(code(&a, &sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(one(&a, &format!("SELECT {arrays} MULTISET UNION ALL {arrays} FROM T WHERE K = 1")), "MULTISET[ARRAY[1], ARRAY[1]]");
}

/// Verifies: [DESIGN] RULES.md D80: MULTISET, MEMBER and SUBMULTISET are no
/// key words, so a column may be named so, read as one before UNION of a
/// query and as an operand of the predicates.
#[test]
fn words_are_names_d80() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE W (MULTISET INTEGER, MEMBER INTEGER, SUBMULTISET INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO W VALUES (1, 2, 3)").unwrap();
    assert_eq!(one(&a, "SELECT MULTISET + MEMBER + SUBMULTISET FROM W"), "6");
    assert_eq!(sorted(&a, "SELECT MULTISET FROM W UNION SELECT MEMBER FROM W"), ["1", "2"]);
    assert_eq!(sorted(&a, "SELECT MEMBER FROM W WHERE MEMBER > MULTISET UNION SELECT SUBMULTISET FROM W"), ["2", "3"]);
    assert_eq!(sorted(&a, "SELECT MEMBER FROM W WHERE MEMBER > MULTISET UNION ALL VALUES 5"), ["2", "5"]);
    assert_eq!(one(&a, "SELECT MEMBER FROM W WHERE MEMBER MEMBER OF MULTISET[2]"), "2");
}
