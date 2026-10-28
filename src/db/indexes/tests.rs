//! Unit tests of building an index again: what happens when the new one
//! cannot take the old one's place. Split from db/indexes.rs (RULES.md
//! R-SIZE-2).

use super::*;
use crate::index::BTREE_METHOD;

/// Database with S.T (K INTEGER) of three rows and index T_K on it.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("S");
    for k in 0_i32..3_i32 {
        s.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    db.create_index("T_K", "S.T", &["K"], BTREE_METHOD).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D41: an index built again takes the place of
/// the old one only if both are there; else SQLCODE -910, and the state is
/// as it was. The old one cannot go meanwhile in a running database, as
/// the build holds its table's lock, which DROP INDEX takes exclusively:
/// this is the guard behind that.
#[test]
fn an_index_takes_only_the_place_of_one_there() {
    let st = database().snapshot().unwrap();
    let mut next = State::clone(&st);
    assert_eq!(next.swap_index(("T_K (rebuilt)", "T_K")).err().map(|e| e.code), Some(Code::Api), "no new one");
    assert_eq!(next.swap_index(("T_K", "GONE")).err().map(|e| e.code), Some(Code::Api), "no old one");
    assert!(next.catalog.index("T_K").is_some() && next.indexes.contains_key("T_K"));
}

/// Verifies: [DESIGN] RULES.md D41, D27: an index built while transactions
/// write its table, whose last change fails, is taken out of the catalog
/// and of those being built, and the error returned; the index it was to
/// replace stands, and the table is read as before.
#[test]
fn a_build_whose_last_change_fails_is_taken_out() {
    let db = database();
    let table = QualifiedName { auth: "S".into(), ident: "T".into() };
    let built = "T_K (rebuilt)";
    let spec = IndexSpec { name: built.into(), table, columns: vec!["K".into()], method: BTREE_METHOD.into(), options: Vec::new() };
    let failed = db.build_index(&spec, &|st, _| st.swap_index((built, "GONE")));
    assert_eq!(failed.err().map(|e| e.code), Some(Code::Api));
    let st = db.snapshot().unwrap();
    assert!(st.catalog.index(built).is_none() && !st.building.contains(built), "taken out");
    assert!(st.catalog.index("T_K").is_some() && st.indexes.contains_key("T_K"));
    let s = db.session("S");
    let mut cursor = s.open_cursor("SELECT K FROM T WHERE K = 1").unwrap();
    assert!(cursor.fetch().unwrap().is_some() && cursor.fetch().unwrap().is_none(), "its one row");
}
