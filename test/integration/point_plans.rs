//! Probes that fix a whole unique key: chosen without pricing another path
//! once a table is large, with the same rows as any other plan. These are
//! `[DESIGN]` requirements: the standard does not govern plans, only results.

use turborel::db::{Database, Prepared, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use crate::common::{code, rows, setup};

/// A table of 1,200 rows, past the size at which such a probe goes unpriced,
/// on a two-column key, its column `D` the same in every row.
fn keyed(db: &Database) -> Session {
    let s = db.session("APP");
    s.run("CREATE TABLE P (A INTEGER NOT NULL, B INTEGER NOT NULL, C INTEGER, D INTEGER NOT NULL, PRIMARY KEY (A, B))").unwrap();
    s.run("COMMIT").unwrap();
    s.run("CREATE INDEX P_D ON P (D)").unwrap();
    for start in (0..1200_i32).step_by(100) {
        let values: Vec<String> = (start..start + 100).map(|i| format!("({}, {}, {}, 7)", i.div_euclid(10), i.rem_euclid(10), i)).collect();
        s.run(&format!("INSERT INTO P VALUES {}", values.join(", "))).unwrap();
    }
    s.run("COMMIT").unwrap();
    s
}

fn plan(s: &Session, sql: &str) -> String {
    s.explain(sql, &Params::new()).unwrap()
}

/// Verifies: [DESIGN] RULES.md D45, access paths follow the values of each
/// execution: a probe fixing a whole unique key is chosen, and answers
/// as the scan does, for a key present, a key absent, and a conjunct more
/// that it must still test.
#[test]
fn a_whole_unique_key_is_probed_with_the_rows_of_a_scan() {
    let db = setup();
    let s = keyed(&db);
    assert!(plan(&s, "SELECT C FROM P WHERE A = 5 AND B = 6").contains("IndexScan"));
    assert_eq!(rows(&s, "SELECT C FROM P WHERE A = 5 AND B = 6"), Ok(vec!["56".into()]));
    assert_eq!(rows(&s, "SELECT C FROM P WHERE A = 5 AND B = 60"), Ok(vec![]));
    assert_eq!(rows(&s, "SELECT C FROM P WHERE A = 5 AND B = 6 AND C = 1"), Ok(vec![]));
    assert_eq!(rows(&s, "SELECT C FROM P WHERE A = 5 AND B = 6 AND C = 56 ORDER BY C"), Ok(vec!["56".into()]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM P WHERE A = 5"), Ok(vec!["10".into()]));
}

/// Verifies: [DESIGN] RULES.md D45: an equal key of an index that is not
/// unique finds any number of rows, so it is priced, and where it finds
/// them all the table is scanned.
#[test]
fn a_key_that_is_not_unique_is_still_priced() {
    let db = setup();
    let s = keyed(&db);
    assert!(plan(&s, "SELECT C FROM P WHERE D = 7").contains("SeqScan"));
}

/// Verifies: [DESIGN] RULES.md D15: a conjunct that can raise an error sees
/// every row, so even a whole unique key is not probed past it.
#[test]
fn a_conjunct_that_can_raise_keeps_its_scan() {
    let db = setup();
    let s = keyed(&db);
    let sql = "SELECT C FROM P WHERE A = 5 AND B = 6 AND 1 / (C - 56) = 1";
    assert!(plan(&s, sql).contains("SeqScan"));
    assert_eq!(code(&s, sql), Code::Data);
}

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The rows of `p`, a query, opened with `values`, each as text.
fn opened(s: &Session, p: &Prepared, values: &[Value]) -> Vec<String> {
    let mut cursor = s.open_prepared(p, values).unwrap();
    let mut out = Vec::new();
    while let Some(r) = cursor.fetch().unwrap() {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [DESIGN] RULES.md D45: a prepared query whose `?`s fix a
/// whole unique key, written on either side of the column and with a
/// conjunct more, answers each execution as the statement written with its
/// values does, for a key present, a key absent, a null, a value cast to
/// the column's type, and a row inserted or deleted since, making one
/// plan.
#[test]
fn a_prepared_probe_of_a_whole_unique_key_is_kept_and_filled() {
    let db = setup();
    let s = keyed(&db);
    let query = s.prepare("SELECT C FROM P WHERE A = ? AND B = ?").unwrap();
    assert_eq!(opened(&s, &query, &[int(5), int(6)]), vec!["56"]);
    assert_eq!(opened(&s, &query, &[int(11), int(9)]), vec!["119"]);
    assert_eq!(opened(&s, &query, &[int(5), int(60)]), Vec::<String>::new());
    assert_eq!(opened(&s, &query, &[int(5), Value::Null]), Vec::<String>::new());
    assert_eq!(opened(&s, &query, &[Value::Char(" 7 ".into()), int(0)]), vec!["70"], "cast to INTEGER");
    s.run("INSERT INTO P VALUES (200, 0, 2000, 7)").unwrap();
    s.run("DELETE FROM P WHERE A = 5 AND B = 6").unwrap();
    assert_eq!(opened(&s, &query, &[int(200), int(0)]), vec!["2000"]);
    assert_eq!(opened(&s, &query, &[int(5), int(6)]), Vec::<String>::new());
    assert_eq!(query.plans_made(), 1);
    let more = s.prepare("SELECT C FROM P WHERE ? = B AND A = ? AND C < ?").unwrap();
    assert_eq!(opened(&s, &more, &[int(7), int(11), int(1000)]), vec!["117"]);
    assert_eq!(opened(&s, &more, &[int(7), int(11), int(100)]), Vec::<String>::new());
    assert_eq!(opened(&s, &more, &[int(8), int(11), int(1000)]), vec!["118"]);
    assert_eq!(more.plans_made(), 1);
}
