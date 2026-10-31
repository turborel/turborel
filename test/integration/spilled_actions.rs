//! Referential checks and actions, bounded (RULES.md D38): whether a row
//! has a match, and NO ACTION's look for rows left without one, stop at
//! the first row found [ISO/IEC 9075:1992 §11.8 GR2].

use std::sync::mpsc::Receiver;

use turborel::db::Session;
use turborel::error::Code;

use crate::common::{arm, gated, rows};

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
