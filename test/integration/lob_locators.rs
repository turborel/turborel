//! Large object locators [ISO/IEC 9075-2:2023 §4.37.5, §14.17, §14.18]:
//! a value assigned a locator and read by ranges through it, valid until
//! its transaction ends; FREE LOCATOR and HOLD LOCATOR (Feature T561) of
//! dynamic parameters; a value a client makes a piece at a time and sends
//! by its locator; and a locator of a value read under READ COMMITTED
//! still read after the row is changed and collected (RULES.md D79).

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::lob::{Lob, LobKind};
use turborel::value::{Decimal, Value};

use crate::common::rows;

/// Schema A: table T of an INTEGER K and a CLOB C, row 1 a text of
/// 700000 characters, `text`.
fn lobs() -> (Database, String) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, C CLOB)").unwrap();
    let text: String = (0..700_000).map(|i| if i % 5 == 0 { 'ü' } else { 'k' }).collect();
    let a = db.session("A");
    let p = a.prepare("INSERT INTO T VALUES (1, ?)").unwrap();
    a.execute_prepared(&p, &[Value::Lob(Lob::of_text(&text))]).unwrap();
    a.commit().unwrap();
    drop(a);
    (db, text)
}

/// Row `k`'s CLOB, as `s` reads it.
fn clob(s: &Session, k: i64) -> Lob {
    let row = s.open_cursor(&format!("SELECT C FROM T WHERE K = {k}")).unwrap().fetch().unwrap().unwrap();
    let Value::Lob(c) = &row[0] else { panic!("{row:?}") };
    c.clone()
}

/// The SQLSTATE of `outcome`'s error, if it failed.
fn state<T>(outcome: turborel::error::Result<T>) -> Option<String> {
    outcome.err().map(|e| e.sqlstate().as_str().to_owned())
}

/// A locator, as a `<locator reference>` takes it.
fn locator(l: u32) -> Value {
    Value::Exact(Decimal::new(i128::from(l), 0))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.37.5] a value assigned a
/// locator is read through it by ranges of octets and of characters, at most a
/// chunk at a time; the locator is valid until its transaction ends, and
/// invalid after (SQLSTATE 0F001), and so after a rollback.
#[test]
fn a_locator_is_valid_until_its_transaction_ends() {
    let (db, text) = lobs();
    let a = db.session("A");
    let l = a.locator_of(clob(&a, 1));
    assert_eq!(a.read_lob(l, (0, 6), false).unwrap(), "ükkkk".as_bytes()[..6].to_vec(), "octets");
    let chars: String = text.chars().skip(699_990).collect();
    assert_eq!(a.read_lob(l, (699_990, 100), true).unwrap(), chars.as_bytes(), "characters, to the end");
    let most = a.read_lob(l, (0, u64::MAX), true).unwrap();
    assert!(
        most.len() <= usize::try_from(turborel::db::LOB_READ_OCTETS).unwrap() && std::str::from_utf8(&most).is_ok(),
        "a chunk, whole characters"
    );
    a.commit().unwrap();
    assert_eq!(state(a.read_lob(l, (0, 1), false)), Some("0F001".to_owned()), "ended by the commit");
    let l = a.locator_of(clob(&a, 1));
    a.rollback().unwrap();
    assert_eq!(state(a.located(l)), Some("0F001".to_owned()), "ended by the rollback");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.17 GR1, §14.18 GR1; §17.6
/// GR5] FREE LOCATOR and HOLD LOCATOR of dynamic parameters, of type INTEGER: a
/// freed locator is invalid (SQLSTATE 0F001), as is freeing or holding one not
/// valid \[GR1a\]; a held one is valid past the commit, its value kept though
/// the row is then deleted and collected, until a rollback; a locator reference
/// other than a dynamic parameter is refused \[SR1\] (SQLCODE -103).
#[test]
fn free_locator_and_hold_locator_14_17_14_18() {
    let (db, text) = lobs();
    let a = db.session("A");
    let freeing = a.prepare("FREE LOCATOR ?").unwrap();
    let holding = a.prepare("HOLD LOCATOR ?, ?").unwrap();
    let (freed, held, other) = (a.locator_of(clob(&a, 1)), a.locator_of(clob(&a, 1)), a.locator_of(clob(&a, 1)));
    a.execute_prepared(&freeing, &[locator(freed)]).unwrap();
    assert_eq!(state(a.located(freed)), Some("0F001".to_owned()));
    assert_eq!(state(a.execute_prepared(&freeing, &[locator(freed)])), Some("0F001".to_owned()), "freed already");
    a.execute_prepared(&holding, &[locator(held), locator(other)]).unwrap();
    a.run("DELETE FROM T").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    let kept = a.located(held).unwrap();
    assert_eq!(kept.reader().read(0, kept.octets()).unwrap(), text.as_bytes(), "held past the commit, the row gone");
    a.run("INSERT INTO T VALUES (2, 'x')").unwrap();
    a.rollback().unwrap();
    assert_eq!(state(a.located(held)), Some("0F001".to_owned()), "a later transaction rolled back");
    assert_eq!(state(a.execute_prepared(&holding, &[locator(held), locator(other)])), Some("0F001".to_owned()));
    assert_eq!(a.prepare("FREE LOCATOR 5").err().map(|e| e.code), Some(Code::SyntaxRule));
}

/// Verifies: [DESIGN] RULES.md D79: a value made through a locator a
/// piece at a time, longer than a chunk, characters split between pieces,
/// and sent by its locator as a parameter, is inserted as made; a locator
/// sent that names none is refused (SQLSTATE 0F001).
#[test]
fn a_value_made_by_pieces_is_sent_by_its_locator() {
    let (db, _) = lobs();
    let a = db.session("A");
    let made: String = (0..400_000).map(|i| if i % 4 == 0 { '漢' } else { 'z' }).collect();
    let l = a.create_lob(LobKind::Character);
    for piece in made.as_bytes().chunks(100_001) {
        a.append_lob(l, piece).unwrap();
    }
    let p = a.prepare("INSERT INTO T VALUES (3, ?)").unwrap();
    a.execute_prepared(&p, &[Value::Lob(Lob::of_locator(LobKind::Character, l, (0, 0)))]).unwrap();
    assert_eq!(state(a.append_lob(l, b"more")), Some("0F001".to_owned()), "made, and used");
    let read = clob(&a, 3);
    assert_eq!(read.reader().read(0, read.octets()).unwrap(), made.as_bytes());
    let none = Value::Lob(Lob::of_locator(LobKind::Character, 999_999, (0, 0)));
    assert_eq!(state(a.execute_prepared(&p, &[none])), Some("0F001".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.37.5] a locator of a value
/// read by a READ COMMITTED statement reads it after a later statement, though
/// another session meanwhile replaced the row and its old value was collected:
/// the transaction keeps that snapshot open (RULES.md D79).
#[test]
fn a_read_committed_locator_keeps_its_value() {
    let (db, text) = lobs();
    let a = db.session("A");
    a.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    let l = a.locator_of(clob(&a, 1));
    let b = db.session("A");
    b.run("UPDATE T SET C = 'new'").unwrap();
    b.commit().unwrap();
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(C) FROM T").unwrap(), ["3"], "a later statement sees the change");
    // A commit collects what no open snapshot reads: A's is the later.
    b.run("UPDATE T SET K = K").unwrap();
    b.commit().unwrap();
    db.maintain().unwrap();
    let kept = a.located(l).unwrap();
    assert_eq!(kept.reader().read(0, kept.octets()).unwrap(), text.as_bytes());
    a.commit().unwrap();
}
