//! Prepared statements and their dynamic parameters [ISO/IEC 9075:1992
//! §17]: each `?`'s type deduced from where it stands [§17.6 GR5], refused
//! where nothing gives it one \[GR4\]; executed and opened with values cast to
//! those types [§17.9 GR5].

use turborel::db::{Cursor, Database, Session};
use turborel::error::Code;
use turborel::params::ParameterType;
use turborel::value::{DataType, Decimal, Value};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn text(s: &str) -> Value {
    Value::Char(s.into())
}

/// Every row of `cursor`, each its values joined by commas.
fn all(mut cursor: Cursor) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(r) = cursor.fetch().unwrap() {
        out.push(r.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// A session of X over T (K INTEGER NOT NULL, N CHARACTER(5), A DECIMAL(5,2)),
/// holding (1,'ANN',1.50), (2,'BOB',2.00), (3,'CAROL',NULL).
fn session() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, N CHARACTER(5), A DECIMAL(5,2))").unwrap();
    let s = db.session("X");
    for row in ["1, 'ANN', 1.50", "2, 'BOB', 2.00", "3, 'CAROL', NULL"] {
        s.run(&format!("INSERT INTO T VALUES ({row})")).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// The rows of the query `sql`, prepared, opened with `values`.
fn query(s: &Session, sql: &str, values: &[Value]) -> Vec<String> {
    let p = s.prepare(sql).unwrap();
    assert!(p.is_query(), "{sql}");
    all(s.open_prepared(&p, values).unwrap())
}

const INT: DataType = DataType::Exact { precision: 10, scale: 0 };

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6, §17.8, §17.9 GR3, GR4,
/// GR5]: a query prepared once is described, its parameters' types those
/// its context gives each and its result's columns, and opened as often as
/// asked with values, each cast to its parameter's type: a string to a
/// number as CAST does.
#[test]
fn a_prepared_query_is_described_and_opened_with_values() {
    let (_db, s) = session();
    let p = s.prepare("SELECT K, N, A FROM T WHERE K = ? OR A > ? ORDER BY K").unwrap();
    let types: Vec<DataType> = p.parameters().iter().map(|t| t.data_type()).collect();
    assert_eq!(types[0].to_string(), INT.to_string(), "the column's type");
    assert_eq!(types[1].to_string(), DataType::Exact { precision: 5, scale: 2 }.to_string());
    let named: Vec<Option<&str>> = p.columns().iter().map(|c| c.name.as_deref()).collect();
    assert_eq!(named, [Some("K"), Some("N"), Some("A")]);
    assert_eq!(all(s.open_prepared(&p, &[int(1), int(5)]).unwrap()), ["1,ANN,1.50"]);
    assert_eq!(all(s.open_prepared(&p, &[text(" 3 "), Value::Approx(1.9)]).unwrap()), ["2,BOB,2.00", "3,CAROL,NULL"]);
    assert_eq!(p.sql(), "SELECT K, N, A FROM T WHERE K = ? OR A > ? ORDER BY K");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR5h, i, j, k, l, m, q,
/// r, s, t]: a `?` takes the type of the other operand of an arithmetic
/// operator [h] or a comparison [m], of BETWEEN's [m], of an IN list's
/// value [r], of a subquery's column [q, t], of a CAST's target [i], of
/// COALESCE's first operand [j], of the first WHEN operand [k] and of
/// NULLIF's other [l]; in LIKE, CHARACTER VARYING [s], which is not
/// implemented: its value as given, unpadded, a number as its literal, at
/// most 32767 characters (SQLSTATE 22001 past).
#[test]
fn a_parameter_takes_its_type_from_where_it_stands() {
    let (_db, s) = session();
    assert_eq!(query(&s, "SELECT K + ? FROM T WHERE K = 1", &[int(10)]), ["11"]);
    assert_eq!(query(&s, "SELECT K FROM T WHERE K BETWEEN ? AND ? ORDER BY K", &[int(2), int(3)]), ["2", "3"]);
    assert_eq!(query(&s, "SELECT K FROM T WHERE ? BETWEEN K AND 9 ORDER BY K", &[int(2)]), ["1", "2"]);
    assert_eq!(query(&s, "SELECT K FROM T WHERE K IN (?, ?) ORDER BY K", &[int(3), int(1)]), ["1", "3"]);
    assert_eq!(query(&s, "SELECT K FROM T WHERE ? IN (2, K) ORDER BY K", &[int(2)]), ["1", "2", "3"]);
    assert_eq!(query(&s, "SELECT K FROM T WHERE ? IN (SELECT K FROM T WHERE K > 2)", &[int(3)]), ["1", "2", "3"]);
    assert_eq!(query(&s, "SELECT K FROM T WHERE ? > ALL (SELECT K FROM T) ", &[int(9)]).len(), 3);
    assert_eq!(query(&s, "SELECT CAST(? AS INTEGER) FROM T WHERE K = 1", &[text("42")]), ["42"]);
    assert_eq!(query(&s, "SELECT COALESCE(A, ?) FROM T WHERE K = 3", &[Value::Approx(9.5)]), ["9.50"]);
    assert_eq!(query(&s, "SELECT CASE K WHEN 1 THEN 'one' WHEN ? THEN 'two' END FROM T WHERE K = 2", &[int(2)]), ["two"]);
    assert_eq!(query(&s, "SELECT NULLIF(K, ?) FROM T WHERE K = 2", &[int(2)]), ["NULL"]);
    assert_eq!(query(&s, "SELECT K FROM T WHERE N LIKE ? ORDER BY K", &[text("%O%")]), ["2", "3"]);
    assert_eq!(query(&s, "SELECT K FROM T WHERE N LIKE ?", &[int(5)]).len(), 0, "a number, as its literal");
    assert_eq!(query(&s, "SELECT K FROM T WHERE N LIKE ?", &[Value::Null]).len(), 0, "unknown");
    let long = s.prepare("SELECT K FROM T WHERE N LIKE ?").unwrap();
    let too_long = s.open_prepared(&long, &[text(&"A".repeat(32_768))]).err().unwrap();
    assert_eq!((too_long.code, too_long.sqlstate().as_str()), (Code::Data, "22001"));
    assert_eq!(query(&s, "SELECT K FROM T WHERE (K, N) = (?, ?)", &[int(1), text("ANN")]), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR5b, c, d]: a string
/// function says what type a `?` takes as its argument: CHARACTER VARYING
/// for CHAR_LENGTH, POSITION's operands and concatenation's, NUMERIC(P,0)
/// for SUBSTRING's start and length.
#[test]
fn a_function_gives_its_parameters_their_types() {
    let (_db, s) = session();
    assert_eq!(query(&s, "SELECT CHAR_LENGTH(?) FROM T WHERE K = 1", &[text("four")]), ["4"]);
    assert_eq!(query(&s, "SELECT POSITION(? IN N) FROM T WHERE K = 3", &[text("RO")]), ["3"]);
    assert_eq!(query(&s, "SELECT ? || 'B' FROM T WHERE K = 1", &[text("A")]), ["AB"]);
    assert_eq!(query(&s, "SELECT SUBSTRING(N FROM ? FOR ?) FROM T WHERE K = 3", &[int(2), int(3)]), ["ARO"]);
    let p = s.prepare("SELECT CHAR_LENGTH(?) FROM T").unwrap();
    assert_eq!(p.parameters(), [ParameterType::Varying]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR5o, p, w; §17.10]: a
/// `?` of a table value constructor takes the type of the first row's
/// value in its place that is not one [o]; one in its place in every row
/// of an INSERT's, the column's [p]; an UPDATE's SET `?` its column's [w];
/// each executed as often as asked, its completion as `execute` gives
/// it; a schema definition, prepared, is processed as `direct` does.
#[test]
fn prepared_writes_take_their_columns_types() {
    let (db, s) = session();
    let shown = |p: &turborel::db::Prepared| p.parameters().iter().map(|t| t.data_type().to_string()).collect::<Vec<_>>();
    let insert = s.prepare("INSERT INTO T VALUES (?, ?, ?), (5, 'EVE', ?)").unwrap();
    assert_eq!(shown(&insert), ["NUMERIC(1,0)", "CHARACTER(3)", "NUMERIC(5,2)", "NUMERIC(5,2)"], "[o], and [p] for the third");
    let all_dynamic = s.prepare("INSERT INTO T VALUES (?, ?, ?)").unwrap();
    assert_eq!(shown(&all_dynamic), [INT.to_string(), "CHARACTER(5)".to_owned(), "NUMERIC(5,2)".to_owned()], "[p]");
    s.execute_prepared(&insert, &[int(4), text("DAN"), Value::Approx(4.25), int(5)]).unwrap();
    let update = s.prepare("UPDATE T SET A = ? WHERE K = ?").unwrap();
    s.execute_prepared(&update, &[Value::Null, int(4)]).unwrap();
    let missing = s.execute_prepared(&update, &[int(1), int(99)]).unwrap();
    assert_eq!(missing.sqlcode(), 100, "no row");
    assert_eq!(query(&s, "SELECT K, N, A FROM T WHERE K > ? ORDER BY K", &[int(3)]), ["4,DAN,NULL", "5,EVE,5.00"]);
    let delete = s.prepare("DELETE FROM T WHERE N = ?").unwrap();
    s.execute_prepared(&delete, &[text("EVE")]).unwrap();
    assert_eq!(query(&s, "SELECT COUNT(*) FROM T WHERE K >= ?", &[int(4)]), ["1"]);
    let defined = db.session("Y").prepare("CREATE SCHEMA AUTHORIZATION Y CREATE TABLE U (K INTEGER)").unwrap();
    assert!(!defined.is_query() && defined.parameters().is_empty());
    db.session("Y").execute_prepared(&defined, &[]).unwrap();
    assert_eq!(all(db.session("Y").open_cursor("SELECT COUNT(*) FROM U").unwrap()), ["0"]);
    let schema = s.prepare("SET SCHEMA ?").unwrap();
    assert_eq!(schema.parameters(), [ParameterType::Varying]);
    s.execute_prepared(&schema, &[text("X")]).unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR4]: a `?` where nothing
/// gives it a type is refused as the statement is prepared (SQLSTATE
/// 37000): in a select list [b], as both operands of one operator [c], with
/// a sign [d], as a set function's argument [e], in IS NULL [f], as
/// COALESCE's first operand or the first WHEN operand [h], on both sides
/// of a comparison or BETWEEN [i], as a table value constructor's value in
/// every row outside an INSERT [j], as IN's value and its list's first
/// [k], as both of POSITION's operands [l], of UPPER [n], TRIM [o] and
/// SUBSTRING's string [p]; and where GR5 gives none: a CASE result, and a
/// setting's value.
#[test]
fn a_parameter_with_no_type_is_refused() {
    let (_db, s) = session();
    let refused = [
        "SELECT ? FROM T",
        "SELECT K FROM T WHERE ? + ? = 1",
        "SELECT K FROM T WHERE -? = K",
        "SELECT COUNT(?) FROM T",
        "SELECT K FROM T WHERE ? IS NULL",
        "SELECT COALESCE(?, A) FROM T",
        "SELECT CASE K WHEN ? THEN 1 END FROM T",
        "SELECT K FROM T WHERE ? = ?",
        "SELECT K FROM T WHERE ? BETWEEN ? AND 3",
        "VALUES (?)",
        "SELECT K FROM T WHERE ? IN (?, 1)",
        "SELECT POSITION(? IN ?) FROM T",
        "SELECT UPPER(?) FROM T",
        "SELECT TRIM(?) FROM T",
        "SELECT SUBSTRING(? FROM 1) FROM T",
        "SELECT ? || ? FROM T",
        "SELECT CASE WHEN K = 1 THEN ? ELSE 'b' END FROM T",
        "SET LOCK_WAIT = ?",
    ];
    for sql in refused {
        let e = s.prepare(sql).unwrap_err();
        assert_eq!((e.code, e.sqlstate().as_str()), (Code::SyntaxRule, "37000"), "{sql}: {e}");
    }
    assert_eq!(s.prepare("SELECT BIT_LENGTH(?) FROM T").err().map(|e| e.sqlstate().as_str()), Some("37000"), "BIT VARYING");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.2 SR2, §17.6 GR4a, §17.9
/// GR5a, f, §17.10 GR2, §17.13 GR2]: a `?` outside a prepared statement is
/// refused (SQLCODE -103); OPEN, FETCH, CLOSE and SELECT ... INTO are not
/// preparable (37000); values not one for each parameter (07001), a query
/// executed (07003) and another statement opened as a cursor (07005) are
/// dynamic SQL errors (SQLCODE -921); a value CAST cannot make of the type,
/// CAST's error.
#[test]
fn prepared_statements_are_refused_saying_why() {
    let (_db, s) = session();
    assert_eq!(s.run("DELETE FROM T WHERE K = ?").err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(s.open_cursor("SELECT K FROM T WHERE K = ?").err().map(|e| e.code), Some(Code::SyntaxRule));
    for sql in ["OPEN C", "FETCH C INTO P", "CLOSE C"] {
        assert_eq!(s.prepare(sql).err().map(|e| e.sqlstate().as_str()), Some("37000"), "{sql}");
    }
    let q = s.prepare("SELECT K FROM T WHERE K = ?").unwrap();
    let w = s.prepare("DELETE FROM T WHERE K = ?").unwrap();
    let state = |e: turborel::error::SqlError| (e.code, e.sqlstate().as_str());
    assert_eq!(s.open_prepared(&q, &[]).err().map(state), Some((Code::Dynamic, "07001")));
    assert_eq!(s.open_prepared(&q, &[int(1), int(2)]).err().map(state), Some((Code::Dynamic, "07001")));
    assert_eq!(s.execute_prepared(&q, &[int(1)]).err().map(state), Some((Code::Dynamic, "07003")));
    assert_eq!(s.open_prepared(&w, &[int(1)]).err().map(state), Some((Code::Dynamic, "07005")));
    assert_eq!(s.open_prepared(&q, &[text("one")]).err().map(state), Some((Code::Data, "22018")), "CAST's");
    assert_eq!(s.execute_prepared(&w, &[int(1)]).unwrap().sqlcode(), 0, "values right, it runs");
}
