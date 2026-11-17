//! The semantics of structured types and references [ISO/IEC 9075-2:2023
//! §4.9.3, §4.11, §9.18, §11.65]: method invocations bound and refused,
//! the EXECUTE an observer, a mutator or a method needs, the functions of
//! structured values as EXPLAIN shows them, the orderings of a subtype
//! family and of STATE, values and types kept as a database is closed
//! and opened, keyed values spilled, and references grouped and refused
//! an ordering.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, Scratch, after_crash, exec, rows};

/// Schema A: PT of X and Y with two methods M, of an INTEGER and of a
/// CHARACTER VARYING, and N of a SMALLINT; BOX of a PT and W; table T of
/// one row, (1, PT(1, 2), BOX(PT(5, NULL), NULL)). Schema B may read and
/// update T and use PT and BOX.
fn methods() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PT AS (X INTEGER, Y INTEGER) NOT FINAL METHOD M (A INTEGER) RETURNS INTEGER CONTAINS SQL, \
             METHOD M (A CHARACTER VARYING(5)) RETURNS INTEGER CONTAINS SQL, METHOD N (A SMALLINT) RETURNS INTEGER CONTAINS SQL",
            "CREATE METHOD M (A INTEGER) RETURNS INTEGER FOR PT RETURN A",
            "CREATE METHOD M (A CHARACTER VARYING(5)) RETURNS INTEGER FOR PT RETURN 2",
            "CREATE METHOD N (A SMALLINT) RETURNS INTEGER FOR PT RETURN A + SELF.X",
            "CREATE TYPE PT3 UNDER PT AS (Z INTEGER) NOT FINAL",
            "CREATE TYPE BOX AS (C PT, W INTEGER) NOT FINAL",
            "CREATE TABLE T (K INTEGER, P PT, B BOX)",
            "INSERT INTO T VALUES (1, NEW PT().X(1).Y(2), NEW BOX().C(NEW PT().X(5)))",
            "GRANT SELECT, UPDATE ON T TO B",
            "GRANT USAGE ON TYPE PT TO B",
            "GRANT USAGE ON TYPE BOX TO B",
        ],
    );
    (db, a)
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Asserts `sql` in `s`, read by a cursor if a query, is refused with
/// SQLCODE class `code`, SQLSTATE `state` and a message citing `cited`;
/// rolls back.
fn refused(s: &Session, sql: &str, (code, state, cited): (Code, &str, &str)) {
    let e = if sql.starts_with("SELECT") { s.open_cursor(sql).and_then(|mut c| c.fetch().map(|_| ())).err() } else { s.run(sql).err() };
    let e = e.unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    let msg = e.to_string();
    assert_eq!((e.code, e.sqlstate().as_str()), (code, state), "{sql}: {msg}");
    assert!(msg.contains(cited), "{sql}: {msg} does not cite {cited}");
}

/// The Syntax Rule `cited` broken (SQLCODE -103).
fn syntax(cited: &str) -> (Code, &str, &str) {
    (Code::SyntaxRule, "2A000", cited)
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR10, SR10l iii, §14.15 SR7]
/// An attribute's name with two arguments names no mutator, and no
/// method of the name takes them; a mutator's argument not assignable to
/// its attribute is refused; a SET clause of an attribute the type lacks
/// is refused; a SET clause through an attribute of an attribute reads
/// the first by its observer and sets the last by its mutator.
#[test]
fn methods_and_attributes_are_bound_9_18() {
    let (_db, a) = methods();
    refused(&a, "SELECT P.X(1, 2) FROM T", syntax("no method X of A.PT takes these arguments [ISO/IEC 9075-2:2023 §9.18 SR10]"));
    refused(&a, "SELECT P.X('abc') FROM T", syntax("attribute X is of INTEGER"));
    refused(&a, "SELECT P.X('abc') FROM T", syntax("§9.18 SR10l iii]"));
    refused(&a, "UPDATE T SET P.Q = 1", syntax("type A.PT has no attribute Q [ISO/IEC 9075-2:2023 §9.18 SR10]"));
    assert_eq!(query(&a, "SELECT P.M(7), P.M('A'), P.N(CAST(3 AS SMALLINT)) FROM T"), ["7,2,4"]);
    ok(&a, &["UPDATE T SET B.C.X = 9, B.W = 4"]);
    assert_eq!(query(&a, "SELECT B FROM T"), ["A.BOX(A.PT(9, NULL), 4)"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR10, §9.6, SR10l iii] A dynamic
/// parameter as a method's argument leaves two methods M alike, which is
/// refused as the statement is prepared; of one method N, it is typed as
/// its parameter and the method is invoked with its value.
#[test]
fn a_dynamic_argument_chooses_one_method_or_none_9_18() {
    let (_db, a) = methods();
    let e = a.prepare("SELECT P.M(?) FROM T").err().unwrap();
    assert_eq!((e.code, e.sqlstate().as_str()), (Code::SyntaxRule, "37000"), "{e}");
    assert!(e.to_string().contains("more than one method M of A.PT is alike [ISO/IEC 9075-2:2023 §9.18 SR10, §9.6]"), "{e}");
    let p = a.prepare("SELECT P.N(?) FROM T").unwrap();
    let mut c = a.open_prepared(&p, &[Value::Exact(Decimal::new(5, 0))]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("6".to_owned()));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR6] An observer, a mutator and
/// a method are invoked by one who holds EXECUTE on them alone (SQLCODE
/// -104): of an attribute read, an attribute set, an attribute read on
/// the way to one set, and a method; the column itself is read, and the
/// type's owner invokes each.
#[test]
fn observers_mutators_and_methods_need_execute_9_18() {
    let (db, a) = methods();
    let b = db.session("B");
    let privilege = |cited| (Code::Privilege, "2A000", cited);
    let x = "no EXECUTE is held on the method of attribute X of A.PT [ISO/IEC 9075-2:2023 §9.18 SR6]";
    refused(&b, "SELECT P.X FROM A.T", privilege(x));
    refused(&b, "UPDATE A.T SET P.X = 3", privilege(x));
    refused(&b, "UPDATE A.T SET B.C.X = 3", privilege("no EXECUTE is held on the method of attribute C of A.BOX"));
    refused(&b, "SELECT P.N(1) FROM A.T", privilege("no EXECUTE is held on method N of A.PT [ISO/IEC 9075-2:2023 §9.18 SR6]"));
    assert_eq!(query(&b, "SELECT K FROM A.T"), ["1"]);
    assert_eq!(query(&a, "SELECT P.X, P.N(1) FROM T"), ["1,2"]);
}

/// Verifies: [EXTENSION] [DESIGN] RULES.md D81 EXPLAIN names each
/// function of a structured value, of the observer and the mutator of an
/// attribute, the constructor function, TREAT, the type predicate and
/// SPECIFICTYPE [ISO/IEC 9075-2:2023 §4.9.3, §6.17, §6.33, §8.19], and of
/// a reference, TREAT and the instance DEREF makes [§6.24]; SPECIFICTYPE
/// gives the most specific type's name, each part quoted [§6.33 GR15].
#[test]
fn explain_names_the_functions_of_structured_values_6_17() {
    let (_db, a) = methods();
    let sql = "SELECT P.X, P.X(3), NEW PT(), TREAT(P AS PT3), P IS OF (PT3), P.SPECIFICTYPE FROM T WHERE P.Y = 2";
    let plan = a.explain(sql, &Params::new()).unwrap();
    for shown in ["X(T.P)", "X(T.P, 3)", "PT(NULL, NULL)", "TREAT(T.P)", "(IS OF(T.P))", "SPECIFICTYPE(T.P)"] {
        assert!(plan.contains(shown), "{shown} in {plan}");
    }
    assert_eq!(query(&a, "SELECT P.SPECIFICTYPE, P IS OF (PT3) FROM T"), ["\"TURBOREL\".\"A\".\"PT\",FALSE"]);
    ok(
        &a,
        &[
            "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TYPE EMP UNDER PERSON AS (PAY INTEGER) NOT FINAL",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
            "CREATE TABLE STAFF OF EMP UNDER PEOPLE",
            "CREATE TABLE PETS (OWNER REF(PERSON) SCOPE PEOPLE, N INTEGER)",
        ],
    );
    let plan = a.explain("SELECT DEREF(OWNER), TREAT(OWNER AS REF(EMP)) FROM PETS", &Params::new()).unwrap();
    for shown in ["EMP(A.DEREF(PETS.OWNER))", "PERSON(A.DEREF(PETS.OWNER))", "TREAT(PETS.OWNER, "] {
        assert!(plan.contains(shown), "{shown} in {plan}");
    }
}

/// Schema A: PT, PT3 under it and PT4 under PT3, and functions to order
/// them: PK and PK3 to map, PR and PR3 relative, PL mapping to a large
/// object.
fn family() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PT AS (X INTEGER, Y INTEGER) NOT FINAL",
            "CREATE TYPE PT3 UNDER PT AS (Z INTEGER) NOT FINAL",
            "CREATE TYPE PT4 UNDER PT3 AS (W INTEGER) NOT FINAL",
            "CREATE FUNCTION PK (P PT) RETURNS INTEGER CONTAINS SQL DETERMINISTIC RETURN P.X",
            "CREATE FUNCTION PK3 (P PT3) RETURNS INTEGER CONTAINS SQL DETERMINISTIC RETURN P.X",
            "CREATE FUNCTION PR (P PT, Q PT) RETURNS INTEGER CONTAINS SQL DETERMINISTIC RETURN 0",
            "CREATE FUNCTION PR3 (P PT3, Q PT3) RETURNS INTEGER CONTAINS SQL DETERMINISTIC RETURN 0",
            "CREATE FUNCTION PL (P PT) RETURNS CHARACTER LARGE OBJECT(10) CONTAINS SQL DETERMINISTIC RETURN 'A'",
        ],
    );
    (db, a)
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.65 SR1, SR3, SR4, SR5, SR8, §9.14
/// SR3] CREATE ORDERING of no type is refused; a subtype's ordering is of
/// its direct supertype's comparison form, so refused while that has
/// none; of a type not maximal it is by MAP, and by MAP only if its
/// supertype's is; a supertype's ordering agrees with its subtypes'; the
/// MAP function of an ordering FULL returns no large object, as one of
/// EQUALS ONLY may.
#[test]
fn orderings_agree_across_a_subtype_family_11_65() {
    let (_db, a) = family();
    refused(
        &a,
        "CREATE ORDERING FOR NOPE ORDER FULL BY STATE",
        syntax("there is no user-defined type A.NOPE [ISO/IEC 9075-2:2023 §11.65 SR1]"),
    );
    let sr3 = "the ordering of A.PT3 is of its direct supertype A.PT's comparison form [ISO/IEC 9075-2:2023 §11.65 SR3]";
    refused(&a, "CREATE ORDERING FOR PT3 ORDER FULL BY MAP WITH FUNCTION PK3 (PT3)", syntax(sr3));
    refused(&a, "CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PL (PT)", syntax("[ISO/IEC 9075-2:2023 §11.65 SR8, §9.14 SR3]"));
    ok(&a, &["CREATE ORDERING FOR PT ORDER FULL BY RELATIVE WITH FUNCTION PR (PT, PT)"]);
    let sr5 = "type A.PT3's direct supertype A.PT compares by MAP, so it does too [ISO/IEC 9075-2:2023 §11.65 SR5]";
    refused(&a, "CREATE ORDERING FOR PT3 ORDER FULL BY MAP WITH FUNCTION PK3 (PT3)", syntax(sr5));
    let sr4 = "the ordering of A.PT3, no maximal supertype, is by MAP [ISO/IEC 9075-2:2023 §11.65 SR4]";
    refused(&a, "CREATE ORDERING FOR PT3 ORDER FULL BY RELATIVE WITH FUNCTION PR3 (PT3, PT3)", syntax(sr4));
    ok(
        &a,
        &[
            "DROP ORDERING FOR PT RESTRICT",
            "CREATE ORDERING FOR PT EQUALS ONLY BY MAP WITH FUNCTION PL (PT)",
            "CREATE ORDERING FOR PT3 EQUALS ONLY BY MAP WITH FUNCTION PK3 (PT3)",
        ],
    );
    refused(&a, "CREATE ORDERING FOR PT4 EQUALS ONLY BY STATE", syntax("the ordering of A.PT4, no maximal supertype, is by MAP"));
    ok(&a, &["DROP ORDERING FOR PT RESTRICT"]);
    let subtype = "subtype A.PT3's ordering is not of this form by MAP [ISO/IEC 9075-2:2023 §11.65 SR3, SR5]";
    refused(&a, "CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PK (PT)", syntax(subtype));
    let forms = "SELECT USER_DEFINED_TYPE_NAME, ORDERING_FORM, ORDERING_CATEGORY FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES \
                 WHERE USER_DEFINED_TYPE_SCHEMA = 'A' ORDER BY 1";
    assert_eq!(query(&a, forms), ["PT,NONE,NULL", "PT3,EQUALS,MAP", "PT4,NONE,NULL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.65 SR6a iii, §4.2.4] An ordering
/// by STATE is refused of a type with an attribute of a type of no
/// comparison type: a structured type of no ordering, an array, a
/// multiset or a row of one, or a distinct type whose ordering is
/// dropped; an attribute of a type ordered, or of a reference type,
/// does not refuse it.
#[test]
fn state_orderings_need_comparable_attributes_11_65() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE N AS (X INTEGER) NOT FINAL",
            "CREATE TYPE MONEY AS DECIMAL(9,2) FINAL",
            "DROP ORDERING FOR MONEY RESTRICT",
            "CREATE TYPE E7 AS (A INTEGER) NOT FINAL",
            "CREATE ORDERING FOR E7 EQUALS ONLY BY STATE",
        ],
    );
    for (name, attribute) in [("E1", "N"), ("E2", "N ARRAY[2]"), ("E3", "N MULTISET"), ("E4", "ROW(F INTEGER, G N)"), ("E5", "MONEY")] {
        ok(&a, &[&format!("CREATE TYPE {name} AS (A {attribute}) NOT FINAL")]);
        let cited = format!("attribute A of A.{name} is of a type of no comparison type [ISO/IEC 9075-2:2023 §11.65 SR6a iii]");
        refused(&a, &format!("CREATE ORDERING FOR {name} EQUALS ONLY BY STATE"), syntax(&cited));
    }
    ok(&a, &["CREATE TYPE E8 AS (A E7, B REF(E7)) NOT FINAL", "CREATE ORDERING FOR E8 EQUALS ONLY BY STATE"]);
    let e8 = "SELECT ORDERING_FORM, ORDERING_CATEGORY FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES WHERE USER_DEFINED_TYPE_NAME = 'E8'";
    assert_eq!(query(&a, e8), ["EQUALS,STATE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.65 SR6a iv] (UV102) The specific
/// name of a STATE ordering's function, when none is written, is the
/// type's name and _EQUALS, numbered past a routine's of that name; a
/// name written is of the type's schema and no routine's or other
/// ordering's, else refused; the Information Schema shows each.
#[test]
fn state_orderings_have_specific_names_11_65() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE E6 AS (A INTEGER) NOT FINAL",
            "CREATE FUNCTION F () RETURNS INTEGER SPECIFIC E6_EQUALS CONTAINS SQL RETURN 1",
            "CREATE ORDERING FOR E6 EQUALS ONLY BY STATE",
            "CREATE TYPE E7 AS (A INTEGER) NOT FINAL",
        ],
    );
    for (written, at) in [("E6_EQUALS_1", "A.E6_EQUALS_1"), ("E6_EQUALS", "A.E6_EQUALS"), ("B.E7_EQ", "B.E7_EQ")] {
        let cited = format!("{at} is of schema A and no routine's [ISO/IEC 9075-2:2023 §11.65 SR6a iv]");
        refused(&a, &format!("CREATE ORDERING FOR E7 EQUALS ONLY BY STATE {written}"), syntax(&cited));
    }
    ok(&a, &["CREATE ORDERING FOR E7 EQUALS ONLY BY STATE E7_EQ"]);
    let named = "SELECT USER_DEFINED_TYPE_NAME, ORDERING_ROUTINE_SCHEMA, ORDERING_ROUTINE_NAME \
                 FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES WHERE USER_DEFINED_TYPE_SCHEMA = 'A' ORDER BY 1";
    assert_eq!(query(&a, named), ["E6,A,E6_EQUALS_1", "E7,A,E7_EQ"]);
}

/// The database in `dir`, opened from its files by `fs`.
fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.9.3, §4.11] [DESIGN] RULES.md D81
/// columns of a structured type of a supertype, of reference types with
/// and without a scope, and of a multiset and an array of a structured
/// type, are described and read the same after the database is closed
/// and opened, or crashes.
#[test]
fn structured_and_reference_columns_are_kept_4_9_3() {
    for (case, closed) in [("closed", true), ("crashed", false)] {
        let scratch = Scratch::new(&format!("structured-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        before.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        ok(
            &before.session("A"),
            &[
                "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED",
                "CREATE TYPE EMP UNDER PERSON AS (PAY INTEGER) NOT FINAL",
                "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
                "CREATE TABLE PETS (N INTEGER, OWNER REF(PERSON) SCOPE PEOPLE, LOOSE REF(EMP), WHO EMP, MANY EMP MULTISET, ARR EMP ARRAY[2])",
                "INSERT INTO PEOPLE (NAME) VALUES ('ANN')",
                "INSERT INTO PETS SELECT 1, ID, CAST(NULL AS REF(EMP)), NEW EMP().NAME('X').PAY(3), MULTISET[NEW EMP().PAY(1)], \
                 ARRAY[NEW EMP().PAY(2)] FROM PEOPLE",
            ],
        );
        if closed {
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        let a = db.session("A");
        let read = query(&a, "SELECT N, OWNER->NAME, LOOSE, WHO, MANY, ARR FROM PETS");
        assert_eq!(read, ["1,ANN,NULL,A.EMP('X', 3),MULTISET[A.EMP(NULL, 1)],ARRAY[A.EMP(NULL, 2)]"], "{case}");
        let types = "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'PETS' ORDER BY ORDINAL_POSITION";
        assert_eq!(query(&a, types), ["N,INTEGER", "OWNER,REF", "LOOSE,REF", "WHO,USER-DEFINED", "MANY,MULTISET", "ARR,ARRAY"], "{case}");
        a.commit().unwrap();
        db.close().unwrap();
        drop((db, before));
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.12, §9.14, §11.65] [DESIGN] RULES.md
/// D38, D81 with MEMORY.WORK_BYTES at its least, a window's sort and a
/// grouping of values ordered by MAP spill those values with their keys,
/// and give what they give in memory.
#[test]
fn keyed_values_are_spilled_9_14() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PT AS (X INTEGER, Y INTEGER) NOT FINAL",
            "CREATE FUNCTION PK (P PT) RETURNS INTEGER CONTAINS SQL DETERMINISTIC RETURN P.X",
            "CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PK (PT)",
            "CREATE TABLE T (K INTEGER, P PT)",
            "INSERT INTO T WITH RECURSIVE R (K) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT K + 1 FROM R WHERE K < 1500) \
             SELECT K, NEW PT().X(K / 3).Y(K) FROM R",
        ],
    );
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let cases = [
        (
            "SELECT COUNT(*), MIN(K), MAX(K) FROM (SELECT K, ROW_NUMBER() OVER (ORDER BY P, K) AS RN FROM T) AS D WHERE RN = K",
            "1500,1,1500",
        ),
        ("SELECT COUNT(*), MAX(C) FROM (SELECT P, COUNT(*) AS C FROM T GROUP BY P) AS G", "501,3"),
    ];
    for (sql, expected) in cases {
        assert_eq!(query(&a, sql), [expected], "{sql}");
    }
    assert_eq!(counted.taken(), 0, "the default MEMORY.WORK_BYTES holds them");
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for (sql, expected) in cases {
        let before = counted.taken();
        assert_eq!(query(&a, sql), [expected], "{sql}, spilled");
        assert!(counted.taken() > before, "{sql} spilled");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR9, §9.12, §9.14 SR3, §6.13]
/// References are grouped and made distinct by their octets; an ordering
/// comparison of them is refused, naming the reference type and its
/// scope; a CAST of one to a number is refused with SQLSTATE 22005.
#[test]
fn references_group_by_their_octets_8_2() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
            "CREATE TABLE PETS (OWNER REF(PERSON) SCOPE PEOPLE, N INTEGER)",
            "INSERT INTO PEOPLE (NAME) VALUES ('ANN'), ('BOB')",
            "INSERT INTO PETS SELECT ID, 1 FROM PEOPLE",
            "INSERT INTO PETS SELECT ID, 2 FROM PEOPLE WHERE NAME = 'ANN'",
        ],
    );
    assert_eq!(query(&a, "SELECT OWNER->NAME, COUNT(*) FROM PETS GROUP BY OWNER ORDER BY 1"), ["ANN,2", "BOB,1"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT OWNER) FROM PETS"), ["2"]);
    let ordered = "the comparison orders values of REF(A.PERSON) SCOPE A.PEOPLE, and references are compared for equality alone \
                   [ISO/IEC 9075-2:2023 §9.14 SR3]";
    refused(&a, "SELECT OWNER < OWNER FROM PETS", syntax(ordered));
    refused(
        &a,
        "SELECT CAST(OWNER AS INTEGER) FROM PETS",
        (Code::Data, "22005", "cannot be cast to INTEGER [ISO/IEC 9075:1992 §6.10 SR4]"),
    );
}
