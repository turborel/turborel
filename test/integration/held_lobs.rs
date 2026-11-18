//! Large objects and XML values in rows a statement holds past its work
//! area (RULES.md D38, D79, D86): an UPDATE's changes and an INSERT's
//! candidate rows, and the rows of sorts, joins, groupings and windows,
//! spilled, each value read back as it was.

use turborel::db::{Database, Session};
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::rows;

/// Rows of S: enough that a statement's held rows pass a work area of
/// 64 KiB.
const ROWS: i64 = 600;

/// The columns of S and T: a key, a CLOB, a BLOB and an XML value each
/// kept apart by its table, as each is longer than a record holds, an
/// array of XML, and G, the key's remainder by `GROUPS`.
const COLUMNS: &str = "(K INTEGER PRIMARY KEY, C CLOB, B BLOB, X XML(CONTENT(ANY)), AX XML(CONTENT(ANY)) ARRAY[2], G INTEGER)";

/// The groups of S by G: more than a work area of 64 KiB holds, each of
/// two rows, the second after every group's first.
const GROUPS: i64 = 300;

/// Row `k`'s CLOB: 5,000 characters, a two-octet one every ninth.
fn clob(k: i64) -> String {
    (0..5000).map(|i| if i % 9 == 0 { 'é' } else { char::from(b'a'.wrapping_add(u8::try_from(k % 26).unwrap())) }).collect()
}

/// Row `k`'s BLOB: 5,000 octets.
fn blob(k: i64) -> Vec<u8> {
    (0..5000_i64).map(|i| u8::try_from((i * 7 + k) % 251).unwrap()).collect()
}

/// Row `k`'s XML text: an element of 6,000 characters, and `tag`.
fn xml(k: i64, tag: &str) -> String {
    format!("<{tag} k=\"{k}\">{}</{tag}>", "x".repeat(6000))
}

/// A database of S holding `ROWS` rows, and T empty, committed.
fn database() -> Database {
    let db = Database::new();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE S {COLUMNS} CREATE TABLE T {COLUMNS}")).unwrap();
    let a = db.session("A");
    let p = a
        .prepare(
            "INSERT INTO S VALUES (?, ?, ?, XMLPARSE(CONTENT CAST(? AS CLOB) PRESERVE WHITESPACE), \
             ARRAY[XMLPARSE(CONTENT CAST(? AS CLOB) PRESERVE WHITESPACE)], ?)",
        )
        .unwrap();
    for k in 1..=ROWS {
        let params = [
            Value::Exact(Decimal::new(i128::from(k), 0)),
            Value::Lob(Lob::of_text(&clob(k))),
            Value::Lob(Lob::of_octets(&blob(k))),
            Value::Lob(Lob::of_text(&xml(k, "x"))),
            Value::Lob(Lob::of_text(&xml(k, "a"))),
            Value::Exact(Decimal::new(i128::from(k % GROUPS), 0)),
        ];
        a.execute_prepared(&p, &params).unwrap();
    }
    a.commit().unwrap();
    db
}

/// The octets of `v`, a large object.
fn octets(v: &Value) -> Vec<u8> {
    let Value::Lob(l) = v else { panic!("{v:?}") };
    l.reader().read(0, l.octets()).unwrap()
}

/// Checks that each row of `table` holds the values its row of S was made
/// with, its key `k` less `offset`.
fn check(s: &Session, table: &str, offset: i64) {
    let sql = format!("SELECT K, C, B, XMLSERIALIZE(CONTENT X AS CLOB), XMLSERIALIZE(CONTENT AX[1] AS CLOB) FROM {table}");
    let mut cursor = s.open_cursor(&sql).unwrap();
    let mut seen = 0;
    while let Some(row) = cursor.fetch().unwrap() {
        let Value::Exact(key) = &row[0] else { panic!("{row:?}") };
        let k = i64::try_from(key.units).unwrap() - offset;
        assert_eq!(String::from_utf8(octets(&row[1])).unwrap(), clob(k), "{table} C of {k}");
        assert_eq!(octets(&row[2]), blob(k), "{table} B of {k}");
        assert_eq!(String::from_utf8(octets(&row[3])).unwrap(), xml(k, "x"), "{table} X of {k}");
        assert_eq!(String::from_utf8(octets(&row[4])).unwrap(), xml(k, "a"), "{table} AX of {k}");
        seen += 1;
    }
    assert_eq!(seen, ROWS, "{table}");
}

/// Verifies: [DESIGN] RULES.md D38, D79, D86: an UPDATE whose changes pass
/// a work area of 64 KiB writes back the CLOB, BLOB and XML values its
/// rows held, as they were; an INSERT of rows read from another table,
/// past the work area, copies each of that table's values whole, not its
/// ids; the values read back equal those written, and still do after the
/// source rows are deleted. A regression test: spilled rows named their
/// table's objects by id alone, read back as references no table resolved,
/// so the UPDATE failed (SQLCODE -916).
#[test]
fn held_rows_past_the_work_area_keep_their_large_objects() {
    let db = database();
    let a = db.session("A");
    a.run("SET MEMORY.WORK_BYTES = 65536").unwrap();
    a.run("UPDATE S SET K = K + 1000").unwrap();
    a.run("INSERT INTO T SELECT * FROM S").unwrap();
    a.commit().unwrap();
    check(&a, "S", 1000);
    check(&a, "T", 1000);
    a.run("DELETE FROM S").unwrap();
    a.commit().unwrap();
    check(&a, "T", 1000);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM S").unwrap(), ["0"]);
}

/// The values of `row`, of a key, a CLOB, a BLOB and an XML value, are
/// those row `k` of S was made with, the key `k`; and `k`.
fn made(row: &[Value], what: &str) -> i64 {
    let Value::Exact(key) = &row[0] else { panic!("{what}: {row:?}") };
    let k = i64::try_from(key.units).unwrap();
    assert_eq!(String::from_utf8(octets(&row[1])).unwrap(), clob(k), "{what}: C of {k}");
    assert_eq!(octets(&row[2]), blob(k), "{what}: B of {k}");
    let Value::Xml(x) = &row[3] else { panic!("{what}: {row:?}") };
    assert_eq!(x.text().unwrap(), xml(k, "x"), "{what}: X of {k}");
    k
}

/// Verifies: [DESIGN] RULES.md D38, D79, D86: the CLOB, BLOB and XML values
/// kept apart of rows an operator spills past a work area of 64 KiB are
/// read back as they were, each row once: a sort's runs, a hash join's
/// right operand, an outer join's, a product's operand read again, a
/// grouping of a group-invariant column's values and one of ANY_VALUE's,
/// a window's partition, and a sort of a union. A regression test: each
/// named a table's objects by id alone, read back as no octets or as
/// another object's.
#[test]
fn operators_past_the_work_area_keep_their_large_objects() {
    let db = database();
    let a = db.session("A");
    a.run("SET MEMORY.WORK_BYTES = 65536").unwrap();
    let frame = "OVER (ORDER BY K ROWS BETWEEN CURRENT ROW AND CURRENT ROW)";
    let window = format!("SELECT K, FIRST_VALUE(C) {frame}, FIRST_VALUE(B) {frame}, FIRST_VALUE(X) {frame} FROM S");
    let queries = [
        ("SELECT K, C, B, X FROM S ORDER BY K DESC", ROWS, 1),
        ("SELECT S2.K, S2.C, S2.B, S2.X FROM S JOIN S AS S2 ON S.K = S2.K", ROWS, 1),
        ("SELECT S2.K, S2.C, S2.B, S2.X FROM S RIGHT JOIN S AS S2 ON S.K = S2.K + 0", ROWS, 1),
        ("SELECT S2.K, S2.C, S2.B, S2.X FROM (VALUES (1)) AS V (N) CROSS JOIN S AS S2", ROWS, 1),
        ("SELECT S.K, S.C, S.B, S.X FROM S GROUP BY S.K", ROWS, 1),
        ("SELECT ANY_VALUE(K), ANY_VALUE(C), ANY_VALUE(B), ANY_VALUE(X) FROM S GROUP BY G", GROUPS, 1),
        (window.as_str(), ROWS, 1),
        ("SELECT K, C, B, X FROM S UNION ALL SELECT K, C, B, X FROM S ORDER BY 1", ROWS, 2),
    ];
    for (sql, keys, each) in queries {
        let mut cursor = a.open_cursor(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let mut seen = std::collections::BTreeMap::new();
        while let Some(row) = cursor.fetch().unwrap_or_else(|e| panic!("{sql}: {e}")) {
            *seen.entry(made(&row, sql)).or_insert(0) += 1;
        }
        assert_eq!(seen.len(), usize::try_from(keys).unwrap(), "{sql}");
        assert!(seen.values().all(|&n| n == each), "{sql}: {seen:?}");
    }
}
