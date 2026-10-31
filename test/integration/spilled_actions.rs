//! Referential checks and actions, bounded (RULES.md D38): whether a row
//! has a match, and NO ACTION's look for rows left without one, stop at
//! the first row found [ISO/IEC 9075:1992 §11.8 GR2]; what referential
//! actions keep spills past MEMORY.WORK_BYTES, every outcome as it is in
//! memory [GR4-GR7].

use std::sync::mpsc::Receiver;

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{Counted, arm, gated, rows, sorted};
use crate::spilling::{SMALL, bytes};

/// Rows of the referencing table, all matching one referenced row.
const CHILDREN: i64 = 200;

/// Whether a scan of a gated table reached its second row since the gate
/// was armed: its message heard. The gate was told beforehand to go on, so
/// the scan never waits.
fn read_past_first(heard: &Receiver<()>) -> bool {
    heard.try_recv().is_ok()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR2]; [DESIGN] RULES.md
/// D38. Regression test (R-TEST-7): NO ACTION gathered every row matching
/// the row deleted only to ask whether there was one. A row of P that
/// `CHILDREN` rows of the gated table G reference, deleted, is refused
/// (SQLCODE -201), the scan of G ending at its first row.
#[test]
fn no_action_stops_at_the_first_matching_row() {
    let (db, gate, heard, go) = gated();
    let s = db.session("X");
    s.run("CREATE TABLE P (K INTEGER PRIMARY KEY)").unwrap();
    s.run("INSERT INTO P VALUES (1)").unwrap();
    s.run("UPDATE G SET K = 1").unwrap();
    let more: Vec<String> = (3..CHILDREN).map(|_| "(1)".to_owned()).collect();
    s.run(&format!("INSERT INTO G VALUES {}", more.join(", "))).unwrap();
    s.run("ALTER TABLE G ADD FOREIGN KEY (K) REFERENCES P").unwrap();
    s.commit().unwrap();
    go.send(()).unwrap();
    arm(&gate);
    assert_eq!(s.run("DELETE FROM P").map_err(|e| e.code).err(), Some(Code::Constraint));
    assert!(!read_past_first(&heard), "G read past the first row that matched");
    assert_eq!(rows(&s, "SELECT K FROM P"), Ok(vec!["1".into()]), "undone");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM G"), Ok(vec![CHILDREN.to_string()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR2b, §8.10 GR4];
/// [DESIGN] RULES.md D38. Regression test (R-TEST-7): whether a row's
/// referencing values match gathered every referenced row its values not
/// null equal. A row of N of MATCH PARTIAL, one value null, whose other
/// value `CHILDREN` rows of the gated table Q have, is inserted, the scan
/// of Q ending at its first row.
#[test]
fn a_match_stops_at_the_first_row() {
    let (db, gate, heard, go) = gated();
    db.place_table("X.Q", "gated").unwrap();
    let s: Session = db.session("X");
    s.run("CREATE TABLE Q (A INTEGER, B INTEGER, UNIQUE (A, B))").unwrap();
    s.run("CREATE TABLE N (A INTEGER, B INTEGER, FOREIGN KEY (A, B) REFERENCES Q (A, B) MATCH PARTIAL)").unwrap();
    let referenced: Vec<String> = (0..CHILDREN).map(|b| format!("(1, {b})")).collect();
    s.run(&format!("INSERT INTO Q VALUES {}", referenced.join(", "))).unwrap();
    s.commit().unwrap();
    go.send(()).unwrap();
    arm(&gate);
    s.run("INSERT INTO N VALUES (1, NULL)").unwrap();
    assert!(!read_past_first(&heard), "Q read past the first row that matched");
    assert_eq!(s.run("INSERT INTO N VALUES (2, NULL)").map_err(|e| e.code).err(), Some(Code::Constraint), "no match");
}

/// Rows of Q, each with a long string: their bytes past the smallest work
/// area.
const REFERENCED: i64 = 300;
/// Rows of C, half referencing each row of P.
const CASCADED: i64 = 3000;
/// Rows of T, a chain, each referencing the one before.
const CHAINED: i64 = 500;

/// What a statement came to: done, or its error's SQLSTATE.
type Outcome = Result<(), String>;

/// Schema A, spilling to a counted provider, its work areas the smallest
/// if `small`, and a session of it: Q (A, B, S), `REFERENCED` rows (1, B),
/// and R referencing it MATCH PARTIAL, cascading deletes and updates: rows
/// (1, NULL), three, matching every row of Q, one (1, B) for each, and one
/// (NULL, NULL); P (K PRIMARY KEY) of rows 0 and 1, and C, `CASCADED` rows
/// referencing them, cascading updates, deletes setting null; T, `CHAINED`
/// rows, each referencing the one before, cascading updates.
fn database(small: bool) -> (Database, Counted, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE Q (A INTEGER, B INTEGER, S CHARACTER(250), UNIQUE (A, B))
           CREATE TABLE R (A INTEGER, B INTEGER,
             FOREIGN KEY (A, B) REFERENCES Q (A, B) MATCH PARTIAL ON DELETE CASCADE ON UPDATE CASCADE)
           CREATE TABLE P (K INTEGER PRIMARY KEY)
           CREATE TABLE C (K INTEGER, P INTEGER REFERENCES P ON UPDATE CASCADE ON DELETE SET NULL, S CHARACTER(60))
           CREATE TABLE T (K INTEGER PRIMARY KEY, UP INTEGER REFERENCES T ON UPDATE CASCADE, S CHARACTER(60))",
    )
    .unwrap();
    let s = db.session("A");
    let long = "s".repeat(240);
    let referenced: Vec<String> = (0..REFERENCED).map(|b| format!("(1, {b}, '{long}')")).collect();
    s.run(&format!("INSERT INTO Q VALUES {}", referenced.join(", "))).unwrap();
    let referencing: Vec<String> = (0..REFERENCED).map(|b| format!("(1, {b})")).collect();
    s.run(&format!("INSERT INTO R VALUES (1, NULL), (1, NULL), (1, NULL), (NULL, NULL), {}", referencing.join(", "))).unwrap();
    s.run("INSERT INTO P VALUES (0), (1)").unwrap();
    for start in (0..CASCADED).step_by(500) {
        let rows: Vec<String> = (start..start + 500).map(|k| format!("({k}, {}, '{}')", k % 2, "c".repeat(50))).collect();
        s.run(&format!("INSERT INTO C VALUES {}", rows.join(", "))).unwrap();
    }
    let chain: Vec<String> =
        (0..CHAINED).map(|k| format!("({k}, {}, 't')", if k == 0 { "NULL".to_owned() } else { (k - 1).to_string() })).collect();
    s.run(&format!("INSERT INTO T VALUES {}", chain.join(", "))).unwrap();
    s.commit().unwrap();
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    if small {
        db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    }
    (db, counted, s)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR3b, GR4, GR5b];
/// [DESIGN] RULES.md D38. Regression test (R-TEST-7): a referenced table
/// as it was before the statement, for MATCH PARTIAL, was read into memory
/// whole for each row changed. A row of Q deleted, one row of R cascading,
/// holds Q as it was in a spill area once Q passes the smallest work area,
/// and ends as it does in memory.
#[test]
fn the_table_before_a_statement_spills() {
    let mut ends = Vec::new();
    for small in [false, true] {
        let (_db, counted, s) = database(small);
        s.run("DELETE FROM Q WHERE B = 0").unwrap();
        assert_eq!(counted.taken() > 0, small, "Q as it was spilled");
        ends.push(sorted(&s, "SELECT A, B FROM R WHERE B IS NULL OR B < 2"));
    }
    assert_eq!(ends[0], ends[1]);
    assert_eq!(ends[1], ["1,1", "1,NULL", "1,NULL", "1,NULL", "NULL,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR3-GR7]; [DESIGN]
/// RULES.md D38: with MEMORY.WORK_BYTES at its least, referential actions
/// on thousands of rows spill what they keep (the rows matching each row
/// changed, the associations of MATCH PARTIAL dropped, the values the
/// statement gave) and end as they do in memory: MATCH PARTIAL cascades
/// done to the last row each row matched; a CASCADE of MATCH PARTIAL to
/// distinct values, or a row of T given two values, refused with SQLSTATE
/// 27000 and undone; cascades and SET NULL of thousands done.
#[test]
fn large_cascades_spill_with_the_same_outcomes() {
    let statements = [
        "UPDATE Q SET A = 3",
        "UPDATE Q SET A = B + 10",
        "DELETE FROM Q",
        "UPDATE P SET K = K + 10",
        "DELETE FROM P",
        "UPDATE T SET K = K + 10000",
        "UPDATE T SET K = K + 10000, UP = UP",
    ];
    let mut ends = Vec::new();
    for small in [false, true] {
        let (_db, counted, s) = database(small);
        let mut outcomes: Vec<Outcome> = Vec::new();
        for sql in statements {
            outcomes.push(s.run(sql).map(|_| ()).map_err(|e| e.sqlstate().as_str().to_owned()));
            assert_eq!(counted.taken() > 0, small, "{sql}");
        }
        s.commit().unwrap();
        let ends_of = |sql: &str| sorted(&s, sql);
        ends.push((
            outcomes,
            ends_of("SELECT A, B FROM R"),
            ends_of("SELECT P, COUNT(*) FROM C GROUP BY P"),
            ends_of("SELECT K, UP FROM T"),
        ));
    }
    let refused: Vec<Option<&str>> = ends[1].0.iter().map(|o| o.as_ref().err().map(String::as_str)).collect();
    assert_eq!(refused, [None, Some("27000"), None, None, None, None, Some("27000")]);
    assert_eq!(ends[0], ends[1]);
    assert_eq!((ends[1].1.clone(), ends[1].2.clone()), (vec!["NULL,NULL".to_owned()], vec![format!("NULL,{CASCADED}")]));
    assert_eq!(ends[1].3.len(), usize::try_from(CHAINED).unwrap());
    assert!(ends[1].3.contains(&"10001,10000".to_owned()), "{:?}", ends[1].3.first());
}
