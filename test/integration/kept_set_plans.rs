//! Kept plans of set operations (RULES.md D45), filled anew at each
//! execution, and a row compared with a row subquery's one row
//! [X3.135-1986 §5.11].

use turborel::db::{Cursor, Database, Session};
use turborel::error::Code;
use turborel::value::{Decimal, Value};

/// Schema X with T (K, N) of rows K 1 to 6, N K mod 3.
fn schema() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, N INTEGER)").unwrap();
    let s = db.session("X");
    for k in 1..=6 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {})", k % 3)).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// Every row of `cursor`, its values joined by commas.
fn all(mut cursor: Cursor) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(r) = cursor.fetch().unwrap() {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// The integer `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [DESIGN] RULES.md D45 [ISO/IEC 9075:1992 §7.10] a prepared
/// UNION ALL, plain and ordered, keeps its plan and fills each operand's
/// values at each execution, giving what the statement written with them
/// gives.
#[test]
fn a_kept_union_is_filled_at_each_execution_7_10() {
    let (_db, s) = schema();
    let plain = s.prepare("SELECT K FROM T WHERE K = ? UNION ALL SELECT K FROM T WHERE N = ?").unwrap();
    let ordered = s.prepare("SELECT K FROM T WHERE K < ? UNION ALL SELECT K FROM T WHERE N = ? ORDER BY 1").unwrap();
    for k in 1..=3 {
        let mut got = all(s.open_prepared(&plain, &[int(k), int(k % 3)]).unwrap());
        got.sort();
        let mut direct =
            all(s.open_cursor(&format!("SELECT K FROM T WHERE K = {k} UNION ALL SELECT K FROM T WHERE N = {}", k % 3)).unwrap());
        direct.sort();
        assert_eq!(got, direct, "K = {k}");
        let sorted = all(s.open_prepared(&ordered, &[int(k), int(k % 3)]).unwrap());
        let written = format!("SELECT K FROM T WHERE K < {k} UNION ALL SELECT K FROM T WHERE N = {} ORDER BY 1", k % 3);
        assert_eq!(sorted, all(s.open_cursor(&written).unwrap()), "K = {k}");
    }
}

/// Verifies: [X3.135-1986 §5.11 GR1, GR2] a row compared with a row
/// subquery is compared with its one row, unknown if it has none, and
/// refused (SQLCODE -811) if it has more than one.
#[test]
fn a_row_subquery_of_a_comparison_has_one_row_5_11() {
    let (_db, s) = schema();
    let one = all(s.open_cursor("SELECT K FROM T WHERE (K, N) = (SELECT K, N FROM T WHERE K = 4)").unwrap());
    assert_eq!(one, ["4"]);
    let none = all(s.open_cursor("SELECT K FROM T WHERE NOT ((K, N) = (SELECT K, N FROM T WHERE K = 0)) IS NOT UNKNOWN").unwrap());
    assert_eq!(none.len(), 6);
    let many = s.open_cursor("SELECT K FROM T WHERE (K, N) = (SELECT K, N FROM T)").and_then(|c| {
        let mut c = c;
        while c.fetch()?.is_some() {}
        Ok(())
    });
    assert_eq!(many.unwrap_err().code, Code::Cardinality);
}
