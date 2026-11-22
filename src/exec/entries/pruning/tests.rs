//! Unit tests of the entries a scan takes back: only of a row gone at
//! or before the horizon, the horizon read again past the one last read,
//! and a key that cannot be decoded left (RULES.md R-TEST-1, R-TEST-2).

use super::Pruning;
use crate::db::Database;
use crate::index::{BTREE_METHOD, Key};
use crate::value::{DataType, Decimal, Spelled, Value};

/// A database of `S.T` (K), with an index `T_K` on K and the row K = 7.
fn indexed() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL)").unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T VALUES (7)").unwrap();
    s.commit().unwrap();
    db.create_index("T_K", "S.T", &["K"], BTREE_METHOD).unwrap();
    db
}

fn seven() -> Key {
    Key(vec![Value::Exact(Decimal::new(7, 0))])
}

/// Verifies: [DESIGN] RULES.md D837: an entry is taken back only of a row
/// gone at or before the horizon. MC/DC of the two decisions: no deletion
/// (`gone` none) takes nothing; a deletion past the horizon read again
/// (`at > horizon` both times) takes nothing; one at the horizon, past the
/// horizon first read (0) but not the one read again, takes the entry.
#[test]
fn only_an_entry_of_a_row_gone_before_every_snapshot_is_taken_back() {
    let db = indexed();
    let st = db.snapshot().unwrap();
    let index = st.index_of("T_K").unwrap();
    let id = index.rows(&seven()).unwrap()[0];
    let horizon = st.tx.horizon();
    let mut pruning = Pruning::of(index, &st.tx);
    assert!(!pruning.prune(None, &seven(), id), "no deletion");
    assert!(!pruning.prune(Some(horizon + 1), &seven(), id), "a deletion some snapshot may not see");
    assert_eq!(db.index_entries("T_K").unwrap(), 1);
    assert!(pruning.prune(Some(horizon), &seven(), id), "a deletion every snapshot sees");
    assert_eq!(db.index_entries("T_K").unwrap(), 0, "taken back");
    assert!(pruning.prune(Some(horizon), &seven(), id), "an entry gone already is no failure");
}

/// Verifies: [DESIGN] RULES.md D837: an entry given as its key's bytes is
/// decoded only of a row gone, and one that cannot be decoded is left.
#[test]
fn an_encoded_key_is_decoded_only_of_a_row_gone() {
    let db = indexed();
    let st = db.snapshot().unwrap();
    let index = st.index_of("T_K").unwrap();
    let id = index.rows(&seven()).unwrap()[0];
    let mut pruning = Pruning::of(index, &st.tx);
    let types = [DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }];
    assert!(!pruning.prune_encoded(None, (&[0xFF], &types), id), "no deletion, the bytes not read");
    assert!(!pruning.prune_encoded(Some(st.tx.horizon()), (&[], &types), id), "a key that cannot be decoded");
    assert_eq!(db.index_entries("T_K").unwrap(), 1);
}
