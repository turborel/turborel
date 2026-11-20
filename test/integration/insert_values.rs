//! The rows of an INSERT's VALUES [ISO/IEC 9075-2:2023 §7.3, §14.11]:
//! a `?` of the object column's type whatever the other rows hold
//! [§20.7 GR5a xix 1 A], a row's value kept whole when another row's is
//! shorter or longer, assignment past a column's length refused [§9.2],
//! and a `?` cut by its cast warned of [§20.11 GR6c ii 2 B] (RULES.md D390,
//! D391).

use turborel::db::{Completion, Database, Session};
use turborel::error::SqlState;
use turborel::value::lob::Lob;
use turborel::value::{Binary, Value};

use crate::common::rows;

/// Schema A: table SCANS of an INTEGER ID, a BLOB(10M) B, a CLOB(1M) C, a
/// VARCHAR(3) V, a CHAR(5) F and a VARBINARY(3) VB.
fn scans() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE SCANS \
         (ID INTEGER, B BLOB(10M), C CLOB(1M), V VARCHAR(3), F CHAR(5), VB VARBINARY(3))",
    )
    .unwrap();
    db
}

/// The octets of row `id`'s column `column`, a large object.
fn octets(s: &Session, id: i64, column: &str) -> Vec<u8> {
    let row = s.open_cursor(&format!("SELECT {column} FROM SCANS WHERE ID = {id}")).unwrap().fetch().unwrap().unwrap();
    let Value::Lob(l) = &row[0] else { panic!("{row:?}") };
    l.reader().read(0, l.octets()).unwrap()
}

/// `n` octets, each of its place.
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

/// The SQLSTATE of `sql`'s failure.
fn refused(s: &Session, sql: &str) -> String {
    s.run(sql).unwrap_err().sqlstate().as_str().to_owned()
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.7 GR5a xix 1 A, §14.11 SR16, §7.3
/// SR3] RULES.md D390: a `?` in a row of an INSERT's VALUES of several rows
/// takes its object column's type, not that of another row's literal, so a
/// 4 MiB BLOB and an 80 KB CLOB bound beside a row of X'0102' and 'short'
/// are stored whole and read back octet for octet. So too as an operand of
/// a UNION ALL of VALUES, and a VARCHAR(3)'s `?` beside a literal 'a'.
/// Regression test: each was cut to the literal's length, BINARY(2) and
/// CHAR(5), with no error.
#[test]
fn a_parameter_takes_its_object_columns_type_20_7() {
    let db = scans();
    let s = db.session("A");
    let (blob, clob) = (pattern(4 << 20), "é€x".repeat(13_000));
    let bound = [Value::Lob(Lob::of_octets(&blob)), Value::Lob(Lob::of_text(&clob))];
    let p = s.prepare("INSERT INTO SCANS (ID, B, C) VALUES (1, ?, ?), (2, X'0102', 'short')").unwrap();
    assert_eq!(s.execute_prepared(&p, &bound).unwrap(), Completion::Success);
    assert_eq!(octets(&s, 1, "B"), blob);
    assert_eq!(octets(&s, 1, "C"), clob.as_bytes());
    assert_eq!((octets(&s, 2, "B"), octets(&s, 2, "C")), (vec![1, 2], b"short".to_vec()));
    let p = s.prepare("INSERT INTO SCANS (ID, B, C) VALUES (3, X'0102', 'short') UNION ALL VALUES (4, ?, ?)").unwrap();
    s.execute_prepared(&p, &bound).unwrap();
    assert_eq!((octets(&s, 4, "B"), octets(&s, 4, "C")), (blob, clob.into_bytes()));
    let p = s.prepare("INSERT INTO SCANS (ID, V) VALUES (5, 'a'), (6, ?)").unwrap();
    s.execute_prepared(&p, &[Value::Char("abc".into())]).unwrap();
    assert_eq!(rows(&s, "SELECT V FROM SCANS WHERE ID IN (5, 6) ORDER BY ID").unwrap(), ["a", "abc"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.3 SR3, §9.5 SR3a, SR3b, §14.11 SR16]
/// RULES.md D390: the rows of VALUES, a longer literal in the first and a
/// shorter one in the second or the other way round, and the operands of
/// an INSERT's UNION ALL, each store their values whole.
#[test]
fn rows_of_other_lengths_store_whole_7_3() {
    let db = scans();
    let s = db.session("A");
    let long = "X'0102030405060708', 'a longer text', 'abc', 'abcde'";
    s.run(&format!("INSERT INTO SCANS (ID, B, C, V, F) VALUES (1, {long}), (2, X'01', 'a', 'a', 'a')")).unwrap();
    s.run(&format!("INSERT INTO SCANS (ID, B, C, V, F) VALUES (3, X'01', 'a', 'a', 'a'), (4, {long})")).unwrap();
    s.run("INSERT INTO SCANS (ID, B, C, VB) SELECT 5, X'01020304', 'four', X'010203' FROM SCANS WHERE ID = 1 UNION ALL VALUES (6, X'01', 'a', X'01')")
        .unwrap();
    let eight = [1, 2, 3, 4, 5, 6, 7, 8];
    for (id, b, c) in [
        (1, &eight[..], "a longer text"),
        (2, &[1], "a"),
        (3, &[1], "a"),
        (4, &eight[..], "a longer text"),
        (5, &[1, 2, 3, 4], "four"),
        (6, &[1], "a"),
    ] {
        assert_eq!((octets(&s, id, "B"), octets(&s, id, "C")), (b.to_vec(), c.as_bytes().to_vec()), "row {id}");
    }
    let kept = rows(&s, "SELECT ID, V, F, OCTET_LENGTH(VB) FROM SCANS ORDER BY ID").unwrap();
    assert_eq!(kept, ["1,abc,abcde,NULL", "2,a,a    ,NULL", "3,a,a    ,NULL", "4,abc,abcde,NULL", "5,NULL,NULL,3", "6,NULL,NULL,1"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.2 GR3b iii, v, xii, §14.11 SR16]
/// RULES.md D390: a
/// value longer than its column, in either row of VALUES of several rows,
/// or in an operand of a UNION ALL, is refused with SQLSTATE 22001 and no
/// row is stored, so no value is cut to fit.
#[test]
fn assignment_past_the_length_is_refused_9_2() {
    let db = scans();
    let s = db.session("A");
    for sql in [
        "INSERT INTO SCANS (ID, V) VALUES (1, 'abcdef'), (2, 'a')",
        "INSERT INTO SCANS (ID, V) VALUES (1, 'a'), (2, 'abcdef')",
        "INSERT INTO SCANS (ID, F) VALUES (1, 'abcdefgh'), (2, 'a')",
        "INSERT INTO SCANS (ID, VB) VALUES (1, X'01020304'), (2, X'01')",
        "INSERT INTO SCANS (ID, V) VALUES (1, 'a') UNION ALL VALUES (2, 'abcdef')",
        "INSERT INTO SCANS (ID, V) SELECT * FROM (VALUES (1, 'abcdef'), (2, 'a')) AS X",
    ] {
        assert_eq!(refused(&s, sql), "22001", "{sql}");
    }
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM SCANS").unwrap(), ["0"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.11 GR6c ii 2 B, §6.13 GR12d ii,
/// GR13b ii] RULES.md D391: a `?`'s value longer than its type is cast to
/// it, cut, with the warning string data, right truncation (01004), in a
/// row of one or of several, executed again from its kept plan too, so
/// the cut is never silent; one that fits raises none. Regression test: the warning was dropped.
#[test]
fn a_parameter_cut_by_its_cast_is_warned_of_20_11() {
    let db = scans();
    let s = db.session("A");
    let warned = Completion::Warning(SqlState::STRING_TRUNCATED);
    let p = s.prepare("INSERT INTO SCANS (ID, V) VALUES (1, ?)").unwrap();
    for value in ["abcdef", "uvwxyz"] {
        assert_eq!(s.execute_prepared(&p, &[Value::Char(value.into())]).unwrap(), warned, "bound, then a kept plan's slot");
    }
    let p = s.prepare("INSERT INTO SCANS (ID, VB) VALUES (2, X'01'), (3, ?)").unwrap();
    assert_eq!(s.execute_prepared(&p, &[Value::Binary(Binary::new(vec![1, 2, 3, 4]))]).unwrap(), warned);
    let p = s.prepare("INSERT INTO SCANS (ID, V) VALUES (4, ?)").unwrap();
    assert_eq!(s.execute_prepared(&p, &[Value::Char("ab".into())]).unwrap(), Completion::Success);
    assert_eq!(
        rows(&s, "SELECT ID, V, OCTET_LENGTH(VB) FROM SCANS ORDER BY ID, V").unwrap(),
        ["1,abc,NULL", "1,uvw,NULL", "2,NULL,1", "3,NULL,3", "4,ab,NULL"]
    );
}
