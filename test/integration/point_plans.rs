//! Probes that fix a whole unique key: chosen without pricing another path
//! once a table is large, with the same rows as any other plan. These are
//! `[DESIGN]` requirements: the standard does not govern plans, only results.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

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
