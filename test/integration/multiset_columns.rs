//! Multisets in columns, casts, assignments, aggregates and tables
//! [ISO/IEC 9075-2:2023 §4.12.3, §4.12.4, §6.13, §7.6, §9.2, §10.9,
//! §11.5]: kept across a close, cast to and from arrays, assigned,
//! collected, fused and intersected, unnested; nested in other
//! collections, of distinct types and as distinct types' sources
//! [Features S272, S281, S402]; parameters of routines [S202]; and
//! described in descriptor areas and the Information Schema.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{Scratch, after_crash, code, dynamic_host, failure, run_with as run, set_text as text, sorted};
use crate::multisets::{multisets, one, refused};

/// The database in `dir`, opened from its files by `fs`.
fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }
}

/// Verifies: [DESIGN] RULES.md D80, D25 multiset values, nested, with
/// null elements, kept in a base table are read the same after the
/// database is closed and opened, crashes, or crashes after a checkpoint.
#[test]
fn kept_after_reopening() {
    let table = "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, M VARCHAR(3) MULTISET MULTISET)";
    let expected = ["1,MULTISET[MULTISET['x', NULL], MULTISET[]]", "2,NULL", "3,MULTISET[]"];
    for (case, checkpoint, closed) in [("closed", false, true), ("crashed", false, false), ("checkpointed", true, false)] {
        let scratch = Scratch::new(&format!("multiset-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        before.schema(table).unwrap();
        let a = before.session("A");
        a.run("INSERT INTO T VALUES (1, MULTISET[MULTISET['x', CAST(NULL AS VARCHAR(3))], CAST(MULTISET[] AS VARCHAR(3) MULTISET)]), (2, NULL)").unwrap();
        a.commit().unwrap();
        if checkpoint {
            before.checkpoint().unwrap();
        }
        a.run("INSERT INTO T VALUES (3, MULTISET[])").unwrap();
        a.commit().unwrap();
        if closed {
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        assert_eq!(sorted(&db.session("A"), "SELECT * FROM T"), expected, "{case}");
        db.close().unwrap();
        drop((db, before));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 SR5, SR6, GR5, GR6,
/// §6.5 SR1] CAST takes a multiset to a multiset or an array, element by
/// element, and an array to a multiset; an array of a maximum cardinality
/// fewer than the elements is 2202F; MULTISET[] casts to any multiset
/// type; neither an array nor a multiset casts to or from another type.
#[test]
fn cast_6_13() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT CAST(M AS DECIMAL(3, 1) MULTISET) FROM T WHERE K = 1"), "MULTISET[1.0, 2.0, 2.0]", "GR6");
    assert_eq!(one(&a, "SELECT CAST(ARRAY[2, 1] AS INTEGER MULTISET) FROM T WHERE K = 1"), "MULTISET[2, 1]", "SR6");
    assert_eq!(one(&a, "SELECT CARDINALITY(CAST(M AS INTEGER ARRAY[3])) FROM T WHERE K = 1"), "3", "SR5");
    assert_eq!(refused(&a, "SELECT CAST(M AS INTEGER ARRAY[2]) FROM T WHERE K = 1"), (Code::Data, "2202F".to_owned()), "GR6b i");
    assert_eq!(one(&a, "SELECT CAST(MULTISET[] AS INTEGER MULTISET) FROM T WHERE K = 1"), "MULTISET[]", "§6.5 SR1");
    assert_eq!(code(&a, "SELECT CAST(K AS INTEGER MULTISET) FROM T"), Code::SyntaxRule, "SR5");
    assert_eq!(code(&a, "SELECT CAST(M AS INTEGER) FROM T"), Code::SyntaxRule, "SR5");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.12.4, §9.2 SR2, GR3b
/// xxiii, §11.5 SR4e] a multiset is stored in a multiset site alone, each
/// element assigned to its element type, a string too long 22001; an
/// array is not; a column's default may be MULTISET[], of a multiset type
/// alone, as ARRAY[] is of an array type alone.
#[test]
fn assigned_9_2() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE S (K INTEGER, M VARCHAR(2) MULTISET DEFAULT MULTISET[], A INTEGER ARRAY[2])")
        .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO S (K) VALUES (1)").unwrap();
    a.run("INSERT INTO S (K, M) VALUES (2, MULTISET['ab', 'c'])").unwrap();
    assert_eq!(sorted(&a, "SELECT K, M FROM S"), ["1,MULTISET[]", "2,MULTISET['ab', 'c ']"]);
    assert_eq!(failure(a.run("INSERT INTO S (K, M) VALUES (3, MULTISET['abc'])")), (Code::Data, "22001".to_owned()));
    assert_eq!(failure(a.run("INSERT INTO S (K, M) VALUES (3, ARRAY['a'])")).0, Code::SyntaxRule, "§4.12.4");
    assert_eq!(failure(a.run("INSERT INTO S (K, A) VALUES (3, MULTISET[1])")).0, Code::SyntaxRule, "§4.12.4");
    a.run("UPDATE S SET M = M MULTISET UNION MULTISET['d'] WHERE K = 2").unwrap();
    assert_eq!(one(&a, "SELECT CARDINALITY(M) FROM S WHERE K = 2"), "3");
    for ddl in ["CREATE TABLE B1 (M INTEGER MULTISET DEFAULT ARRAY[])", "CREATE TABLE B2 (A INTEGER ARRAY[2] DEFAULT MULTISET[])"] {
        assert_eq!(failure(a.run(ddl)).0, Code::SyntaxRule, "{ddl}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR7k to SR7n, GR7d x,
/// xi] COLLECT gathers its values, a null among them, as FUSION ( MULTISET
/// [ X ] ) would, DISTINCT once each; FUSION the elements of the
/// multisets, the null multisets left out; INTERSECTION as many of each
/// value as the multiset of fewest; each null of no rows. FUSION and
/// INTERSECTION take no DISTINCT and a multiset alone, and INTERSECTION's
/// elements are grouped.
#[test]
fn aggregated_10_9() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT CARDINALITY(COLLECT(CASE WHEN K = 3 THEN NULL ELSE K END)) FROM T"), "4", "SR7l");
    let distinct = "SELECT CARDINALITY(COLLECT(DISTINCT X)) FROM (VALUES 1, 1, 2, CAST(NULL AS INTEGER), CAST(NULL AS INTEGER)) AS V (X)";
    assert_eq!(one(&a, distinct), "3", "a null once");
    assert_eq!(one(&a, "SELECT COLLECT(K) FROM T WHERE K > 9"), "NULL", "GR7d ii");
    assert_eq!(one(&a, "SELECT FUSION(M) FROM T WHERE K <> 4"), "MULTISET[1, 2, 2]", "GR7d x");
    assert_eq!(one(&a, "SELECT CARDINALITY(FUSION(M)) FROM T"), "5", "GR7d x");
    assert_eq!(one(&a, "SELECT INTERSECTION(M) FROM T WHERE K IN (1, 3)"), "MULTISET[1, 2, 2]", "the null left out");
    let both = "SELECT INTERSECTION(X) FROM (VALUES MULTISET[1, 2, 2, 3], MULTISET[2, 2, 2, 1]) AS V (X)";
    assert_eq!(one(&a, both), "MULTISET[1, 2, 2]", "GR7d xi");
    assert_eq!(
        sorted(&a, "SELECT K, FUSION(M) FROM T GROUP BY K"),
        ["1,MULTISET[1, 2, 2]", "2,MULTISET[]", "3,NULL", "4,MULTISET[NULL, 3]"]
    );
    assert_eq!(code(&a, "SELECT FUSION(DISTINCT M) FROM T"), Code::SyntaxRule, "SR7m");
    assert_eq!(code(&a, "SELECT INTERSECTION(DISTINCT M) FROM T"), Code::SyntaxRule, "SR7n");
    assert_eq!(code(&a, "SELECT FUSION(K) FROM T"), Code::SyntaxRule, "SR7m");
    assert_eq!(code(&a, "SELECT INTERSECTION(MULTISET[ARRAY[K]]) FROM T"), Code::SyntaxRule, "§9.13 SR3");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6 SR8e i] UNNEST of a
/// multiset gives a row of each element, of a row type its fields; it is
/// unnested alone, and without WITH ORDINALITY.
#[test]
fn unnested_7_6() {
    let db = multisets();
    let a = db.session("A");
    assert_eq!(sorted(&a, "SELECT K, E FROM T, UNNEST(T.M) AS U (E)"), ["1,1", "1,2", "1,2", "4,3", "4,NULL"]);
    let rows = "SELECT X, Y FROM UNNEST(TABLE(SELECT K, K * 10 FROM T WHERE K < 3)) AS U (X, Y)";
    assert_eq!(sorted(&a, rows), ["1,10", "2,20"]);
    assert_eq!(code(&a, "SELECT E FROM T, UNNEST(T.M) WITH ORDINALITY AS U (E, N)"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT E FROM T, UNNEST(T.M, T.M) AS U (E, F)"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.4 GR1, ISO/IEC
/// 9075-2:2023 §11.5 SR4e] an SQL variable of a multiset type DEFAULT
/// MULTISET[] begins empty; of an array type it is refused.
#[test]
fn variables_of_multisets() {
    let db = multisets();
    let a = db.session("A");
    let body = "BEGIN DECLARE V INTEGER MULTISET DEFAULT MULTISET[]; SET V = V MULTISET UNION MULTISET[K]; RETURN CARDINALITY(V); END";
    a.run(&format!("CREATE FUNCTION F (K INTEGER) RETURNS INTEGER CONTAINS SQL {body}")).unwrap();
    assert_eq!(one(&a, "SELECT F(1) FROM T WHERE K = 1"), "1");
    let wrong = "CREATE FUNCTION G () RETURNS INTEGER CONTAINS SQL BEGIN DECLARE V INTEGER ARRAY[2] DEFAULT MULTISET[]; RETURN 1; END";
    assert_eq!(failure(a.run(wrong)).0, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1, §4.12.1] Feature S281:
/// arrays of multisets, multisets of arrays and multisets of multisets,
/// made, kept and read.
#[test]
fn nested_collections_s281() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE N (P INTEGER ARRAY[2] MULTISET, Q INTEGER MULTISET ARRAY[2], R INTEGER MULTISET MULTISET)")
        .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO N VALUES (MULTISET[ARRAY[1, 2], ARRAY[3]], ARRAY[MULTISET[4], MULTISET[5, 5]], MULTISET[MULTISET[6]])").unwrap();
    assert_eq!(one(&a, "SELECT P FROM N"), "MULTISET[ARRAY[1, 2], ARRAY[3]]");
    assert_eq!(one(&a, "SELECT CARDINALITY(Q[2]) FROM N"), "2");
    assert_eq!(one(&a, "SELECT ELEMENT(ELEMENT(R)) FROM N"), "6");
    assert_eq!(one(&a, "SELECT CARDINALITY(R MULTISET UNION ALL R) FROM N"), "2");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.51 SR5a, SR7a, §6.50
/// SR1, §6.31 SR1] Feature S402: a distinct type whose source type is a
/// multiset type is taken wherever a multiset value expression is, its
/// operators giving a multiset of its source's element type; Feature S272:
/// a multiset whose element type is a distinct type.
#[test]
fn distinct_types_s402_s272() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TYPE BAG AS INTEGER MULTISET FINAL CREATE TYPE MONEY AS DECIMAL(9, 2) FINAL
           CREATE TABLE D (B BAG, P MONEY MULTISET)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO D VALUES (BAG(MULTISET[1, 2, 2]), MULTISET[MONEY(1.50), MONEY(2)])").unwrap();
    assert_eq!(one(&a, "SELECT CARDINALITY(B) FROM D"), "3", "§6.31 SR1");
    assert_eq!(one(&a, "SELECT B MULTISET UNION DISTINCT MULTISET[3] FROM D"), "MULTISET[1, 2, 3]", "§6.50 SR1");
    assert_eq!(one(&a, "SELECT CASE WHEN 2 MEMBER OF B THEN 'T' END FROM D"), "T");
    assert_eq!(one(&a, "SELECT CASE WHEN B IS A SET THEN 'T' ELSE 'F' END FROM D"), "F");
    assert_eq!(sorted(&a, "SELECT E FROM D, UNNEST(D.B) AS U (E)"), ["1", "2", "2"]);
    assert_eq!(one(&a, "SELECT CARDINALITY(SET(B)) FROM D"), "2");
    assert_eq!(one(&a, "SELECT CARDINALITY(P) FROM D"), "2", "S272");
    assert_eq!(one(&a, "SELECT CAST(ELEMENT(P MULTISET EXCEPT MULTISET[MONEY(2)]) AS DECIMAL(9, 2)) FROM D"), "1.50", "S272");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60] Feature S202: an
/// SQL-invoked routine whose parameters and result are of multiset types.
#[test]
fn routines_of_multisets_s202() {
    let db = multisets();
    let a = db.session("A");
    a.run("CREATE FUNCTION TWICE (M INTEGER MULTISET) RETURNS INTEGER MULTISET CONTAINS SQL RETURN M MULTISET UNION ALL M").unwrap();
    assert_eq!(one(&a, "SELECT CARDINALITY(TWICE(M)) FROM T WHERE K = 1"), "6");
    assert_eq!(one(&a, "SELECT TWICE(MULTISET[7]) FROM T WHERE K = 1"), "MULTISET[7, 7]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16 SR1n, SR1o, SR1aq, §11.9
/// CR2] of a CHECK, a multiset element grouping operation of character
/// strings is a potential source of non-determinism, refused; of numbers
/// it is not.
#[test]
fn constraints_are_deterministic_9_16() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE C (S VARCHAR(3) MULTISET, I INTEGER MULTISET)").unwrap();
    let a = db.session("A");
    a.run("ALTER TABLE C ADD CONSTRAINT FINE CHECK (CARDINALITY(SET(I)) < 9)").unwrap();
    for check in ["CARDINALITY(SET(S)) < 9", "CARDINALITY(S MULTISET UNION DISTINCT S) < 9", "CARDINALITY(S MULTISET EXCEPT S) < 9"] {
        let sql = format!("ALTER TABLE C ADD CHECK ({check})");
        assert_eq!(failure(a.run(&sql)).0, Code::SyntaxRule, "{check}");
    }
    a.run("ALTER TABLE C ADD CHECK (CARDINALITY(S MULTISET UNION ALL S) < 9)").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.9 GR7, §20.1 Table 30,
/// §20.12 GR4b ii 3] DESCRIBE WITH NESTING gives a multiset's item TYPE 55
/// and its element's item after it, of LEVEL one more; EXECUTE INTO sets
/// its DATA whole and RETURNED_CARDINALITY its elements; the DATA of its
/// element is 0700C.
#[test]
fn described_20_9() {
    let db = multisets();
    let s = db.session("A");
    let mut p = dynamic_host().declare("MS", DataType::multiset(integer()), Value::Null);
    text(&mut p, "SELECT K, M FROM T WHERE K = 1");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D' WITH NESTING", &mut p).unwrap();
    run(&s, "GET DESCRIPTOR 'D' N = COUNT, VAL = TOP_LEVEL_COUNT", &mut p).unwrap();
    assert_eq!([p.value("N").to_string(), p.value("VAL").to_string()], ["3", "2"]);
    run(&s, "GET DESCRIPTOR 'D' VALUE 2 N = TYPE", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "55");
    run(&s, "GET DESCRIPTOR 'D' VALUE 3 N = LEVEL, VAL = TYPE", &mut p).unwrap();
    assert_eq!([p.value("N").to_string(), p.value("VAL").to_string()], ["1", "4"]);
    run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "GET DESCRIPTOR 'D' VALUE 2 MS = DATA, VAL = RETURNED_CARDINALITY", &mut p).unwrap();
    assert_eq!([p.value("MS").to_string(), p.value("VAL").to_string()], ["MULTISET[1, 2, 2]", "3"]);
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 3 N = DATA", &mut p), Err("0700C".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.5 GR4a, GR4b, §20.11 GR]
/// items set by hand, a multiset's of TYPE 55 and its element's after it
/// of LEVEL one more, give EXECUTE USING a multiset of the DATA set; the
/// DATA of its element is 0700D to set.
#[test]
fn written_using_items_set_20_11() {
    let db = multisets();
    let s = db.session("A");
    let bag = Value::Multiset(vec![Value::Exact(Decimal::new(8, 0)); 2].into());
    let mut p = dynamic_host().declare("MS", DataType::multiset(integer()), bag);
    p.set("K", Value::Exact(Decimal::new(9, 0)));
    text(&mut p, "INSERT INTO T VALUES (?, ?)");
    for sql in [
        "PREPARE I FROM TEXT",
        "ALLOCATE DESCRIPTOR 'E'",
        "SET DESCRIPTOR 'E' COUNT = 3",
        "SET DESCRIPTOR 'E' VALUE 1 TYPE = 4",
        "SET DESCRIPTOR 'E' VALUE 2 TYPE = 55",
        "SET DESCRIPTOR 'E' VALUE 3 LEVEL = 1, TYPE = 4",
        "SET DESCRIPTOR 'E' VALUE 1 DATA = K",
        "SET DESCRIPTOR 'E' VALUE 2 DATA = MS",
        "EXECUTE I USING SQL DESCRIPTOR 'E'",
    ] {
        if let Err(state) = run(&s, sql, &mut p) {
            panic!("{sql}: {state}");
        }
    }
    assert_eq!(run(&s, "SET DESCRIPTOR 'E' VALUE 3 DATA = K", &mut p), Err("0700D".to_owned()), "GR4a i");
    assert_eq!(one(&s, "SELECT M FROM T WHERE K = 9"), "MULTISET[8, 8]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.12.1, §9.9 SR2w] ISO/IEC
/// 9075-11 COLUMNS gives a multiset column DATA_TYPE 'MULTISET' and no
/// maximum cardinality, and ELEMENT_TYPES its element type; a distinct
/// type of a multiset is ordered by nothing.
#[test]
fn described_in_the_information_schema() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TYPE BAG AS INTEGER MULTISET FINAL CREATE TABLE T (K INTEGER, M VARCHAR(4) MULTISET)")
        .unwrap();
    let a = db.session("A");
    let columns =
        "SELECT COLUMN_NAME, DATA_TYPE, MAXIMUM_CARDINALITY, DTD_IDENTIFIER FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T'";
    assert_eq!(sorted(&a, columns), ["K,INTEGER,NULL,1", "M,MULTISET,NULL,2"]);
    let elements = "SELECT OBJECT_NAME, COLLECTION_TYPE_IDENTIFIER, DTD_IDENTIFIER, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH
        FROM INFORMATION_SCHEMA.ELEMENT_TYPES WHERE OBJECT_TYPE = 'TABLE' AND OBJECT_SCHEMA = 'A'";
    assert_eq!(sorted(&a, elements), ["T,2,2.1,CHARACTER VARYING,4"]);
    let types = "SELECT USER_DEFINED_TYPE_NAME, ORDERING_FORM FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES";
    assert_eq!(sorted(&a, types), ["BAG,NONE"]);
}
