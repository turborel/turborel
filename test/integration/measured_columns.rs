//! \[DESIGN\] A column read for its octet length alone (RULES.md D1267)
//! and a comparison of octets judged where a chunk holds them (D1268).
//! A column store's scan gives such a column late with its values'
//! lengths and makes no text of it. Each query gives what a table of
//! the row store gives and what it gives read a row at a time, of
//! dictionaries, texts coded by symbols and plain chunks, of nulls, the
//! empty string, strings of spaces and characters of many octets, of
//! rows frozen and not, on one thread and on several.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{both_ways, exec, rows};

/// Rows of each table frozen.
const ROWS: i64 = 6000;

/// The values of row `k`: K its key; G one of seven groups; W one of a
/// few words under BYTES, the empty string, a space and words of
/// characters of many octets among them, null of every 13th; U a text
/// of each row under BYTES, alike in its parts, empty of every 17th; P
/// the words under the default collation, which pads; S a CHARACTER
/// string.
fn row(k: i64) -> String {
    let words = ["'a'", "''", "' '", "'h\u{e9}llo'", "'\u{6f22}\u{5b57}'", "'green  '", "'a'", "''"];
    let word = words[usize::try_from(k * 7919 % 8).unwrap()];
    let w = if k % 13 == 0 { "NULL" } else { word };
    let u = if k % 17 == 0 { "''".to_owned() } else { format!("'http://site{}.example/p\u{e9}ge{}'", k % 30, k * 7919 % 5000) };
    format!("({k}, {}, {w}, {u}, {w}, 's{}')", k % 7, k % 31)
}

/// Schema A with R, of the row store, and C, of the column store in
/// groups of 2048 rows, alike, `ROWS` rows each, C's frozen, and 100
/// more rows each, not frozen; background work done only when asked.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "(K INTEGER, G INTEGER, W VARCHAR(20) COLLATE A.BYTES, U VARCHAR(80) COLLATE A.BYTES, P VARCHAR(20), S CHAR(6))";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE R {columns}
           CREATE TABLE C {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let a = db.session("A");
    let insert = |(from, to): (i64, i64)| {
        for table in ["R", "C"] {
            let values: Vec<String> = (from..to).map(row).collect();
            exec(&a, &format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    };
    (0..ROWS).step_by(500).for_each(|first| insert((first, first + 500)));
    a.commit().unwrap();
    db.maintain().unwrap();
    insert((ROWS, ROWS + 100));
    a.commit().unwrap();
    (db, a)
}

/// The queries each table answers alike, `{t}` its name: a column read
/// for its length alone, with the comparison with the empty string that
/// its lengths tell and each of its operators; with no filter; with a
/// filter of another column; projected, in arithmetic and as a sort's
/// key; a column read for its length and its text both; CHAR_LENGTH,
/// which no store keeps; a column under a collation that pads, whose
/// comparison its lengths do not tell; and a CHARACTER column, padded as
/// it is read.
const QUERIES: [&str; 23] = [
    "SELECT G, SUM(OCTET_LENGTH(U)), AVG(OCTET_LENGTH(U)), COUNT(*) FROM {t} WHERE U <> '' GROUP BY G",
    "SELECT G, SUM(OCTET_LENGTH(W)), COUNT(W), COUNT(*) FROM {t} WHERE W <> '' GROUP BY G",
    "SELECT SUM(OCTET_LENGTH(U)), SUM(OCTET_LENGTH(W)), COUNT(*) FROM {t}",
    "SELECT G, MIN(OCTET_LENGTH(W)), MAX(OCTET_LENGTH(U)) FROM {t} GROUP BY G",
    "SELECT COUNT(*) FROM {t} WHERE W <> ''",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(W)) FROM {t} WHERE W = ''",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(W)) FROM {t} WHERE W > ''",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(W)) FROM {t} WHERE W >= ''",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(W)) FROM {t} WHERE W < ''",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(W)) FROM {t} WHERE W <= ''",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(W)) FROM {t} WHERE W <> 'a'",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(U)) FROM {t} WHERE W <> '' AND U <> '' AND G < 5",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(U)) FROM {t} WHERE K < 90",
    "SELECT K, OCTET_LENGTH(U), OCTET_LENGTH(W) + 1 FROM {t} WHERE G = 3 AND K < 400",
    "SELECT K, OCTET_LENGTH(W) FROM {t} WHERE K BETWEEN 5990 AND 6040",
    "SELECT K FROM {t} WHERE K < 300 ORDER BY OCTET_LENGTH(U) DESC, K FETCH FIRST 7 ROWS ONLY",
    "SELECT G, SUM(OCTET_LENGTH(U)), MIN(U), MAX(W) FROM {t} WHERE U <> '' GROUP BY G",
    "SELECT W, SUM(OCTET_LENGTH(W)), COUNT(*) FROM {t} GROUP BY W",
    "SELECT G, SUM(CHAR_LENGTH(W)), SUM(CHAR_LENGTH(U)), SUM(OCTET_LENGTH(W)) FROM {t} WHERE U <> '' GROUP BY G",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(P)) FROM {t} WHERE P <> ''",
    "SELECT COUNT(*), SUM(OCTET_LENGTH(P)) FROM {t} WHERE P = ''",
    "SELECT G, SUM(OCTET_LENGTH(S)), SUM(OCTET_LENGTH(P)) FROM {t} GROUP BY G",
    "SELECT X.G, SUM(OCTET_LENGTH(X.U)) FROM {t} X JOIN R Y ON X.K = Y.K WHERE Y.G = 2 GROUP BY X.G",
];

/// The rows of `sql`, sorted.
fn read(s: &Session, sql: &str) -> Vec<String> {
    let mut found = rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    found.sort();
    found
}

/// Asserts C answers each query as R does, and as it does read a row at
/// a time, `what` saying when.
fn alike(s: &Session, what: &str) {
    for q in QUERIES {
        let batched = read(s, &q.replace("{t}", "C"));
        let (_, by_rows) = both_ways(s, &q.replace("{t}", "C"));
        assert_eq!(batched, read(s, &q.replace("{t}", "R")), "{what}: {q}");
        assert_eq!(batched, by_rows, "{what}, a row at a time: {q}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR5] RULES.md D1267, D1268: a
/// column store table gives what a table of the row store gives, and
/// what it gives read a row at a time, where a column is read for its
/// octet length alone, filtered by the comparison with the empty string
/// its lengths tell, by another comparison of its octets, by another
/// column or by none; where it is read for its length and its text
/// both; of CHAR_LENGTH; of a column whose collation pads, of which a
/// string of spaces equals the empty string; and of a CHARACTER column.
/// Its rows frozen and not, on several threads and on one.
#[test]
fn measured_columns_answer_as_rows() {
    let (_db, a) = tables();
    alike(&a, "as planned");
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(&a, set).unwrap();
    }
    alike(&a, "on several threads");
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    alike(&a, "on one thread");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR5] RULES.md D1267, the values
/// themselves: the octet length of a string of characters of many octets
/// counts its octets, of the empty string is 0 and of a null is null,
/// and under a collation of NO PAD a space is not the empty string,
/// while under one that pads it is. Normal range and boundaries.
#[test]
fn lengths_are_of_octets_and_spaces_compare_as_their_collation_says() {
    let (_db, a) = tables();
    let one = |sql: &str| read(&a, sql).join("|");
    // k = 1: 7919 % 8 = 7, the empty string; k = 3: 23757 % 8 = 5, 'green  '; k = 5: 39595 % 8 = 3, 'h\u{e9}llo'.
    assert_eq!(one("SELECT OCTET_LENGTH(W) FROM C WHERE K = 1"), "0");
    assert_eq!(one("SELECT OCTET_LENGTH(U) FROM C WHERE K = 1"), "30", "http://site1.example/p\u{e9}ge2919");
    assert_eq!(one("SELECT OCTET_LENGTH(W) FROM C WHERE K = 5"), "6");
    assert_eq!(one("SELECT CHAR_LENGTH(W) FROM C WHERE K = 5"), "5");
    assert_eq!(one("SELECT OCTET_LENGTH(W) FROM C WHERE K = 3"), "7");
    assert_eq!(one("SELECT OCTET_LENGTH(S) FROM C WHERE K = 3"), "6", "a CHARACTER string is padded as it is read");
    assert_eq!(one("SELECT OCTET_LENGTH(W) FROM C WHERE K = 0"), "NULL");
    assert_eq!(one("SELECT OCTET_LENGTH(U) FROM C WHERE K = 0"), "0");
    assert_eq!(one("SELECT SUM(OCTET_LENGTH(U)) FROM C WHERE K < 2"), "30");
    let spaces = one("SELECT COUNT(*) FROM C WHERE W = ' '");
    assert_ne!(spaces, "0");
    assert_eq!(one("SELECT COUNT(*) FROM C WHERE W = ' ' AND W <> ''"), spaces, "NO PAD: a space is not the empty string");
    assert_eq!(one("SELECT COUNT(*) FROM C WHERE P = ' ' AND P <> ''"), "0", "PAD SPACE: a space equals it");
    assert_eq!(one("SELECT COUNT(*) FROM C WHERE OCTET_LENGTH(P) = 1 AND P = ''"), spaces);
}

/// Verifies: [X3.135-1986 §5.9 GR5] RULES.md D1267, an expression of a
/// column read for its length that its vectors cannot give: a division
/// by zero raises what a table of the row store raises, and a product
/// past 64 bits gives what it gives, the batch then evaluated a row at a
/// time, of the column's values decoded. Robustness.
#[test]
fn an_expression_of_a_length_raises_as_rows_raise() {
    let (_db, a) = tables();
    let of = |sql: &str, table: &str| rows(&a, &sql.replace("{t}", table));
    // Row 1's text is of 30 octets.
    let zero = "SELECT SUM(K / (OCTET_LENGTH(U) - 30)) FROM {t} WHERE K < 600";
    assert_eq!((of(zero, "R"), of(zero, "C")), (Err(Code::Data), Err(Code::Data)));
    for sql in [
        "SELECT SUM(K / (OCTET_LENGTH(U) + 1)) FROM {t} WHERE K < 600",
        "SELECT SUM(OCTET_LENGTH(U) * 100000000000000000) FROM {t} WHERE K < 3",
        "SELECT G, MAX(OCTET_LENGTH(U) * 100000000000000000) FROM {t} WHERE U <> '' GROUP BY G",
    ] {
        let (mut row_store, mut column_store) = (of(sql, "R"), of(sql, "C"));
        row_store.iter_mut().chain(column_store.iter_mut()).for_each(|found| found.sort());
        assert_eq!(column_store, row_store, "{sql}");
    }
}
