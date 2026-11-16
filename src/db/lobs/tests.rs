//! Unit tests of a session's locators and of values made a piece at a
//! time (RULES.md D79, R-TEST-1).

use std::sync::Arc;

use super::writing::{Appender, resolved};
use super::{Entry, Locators};
use crate::error::Code;
use crate::memory::MemoryBudget;
use crate::spill::{TempLobStore, WorkAreas};
use crate::value::Value;
use crate::value::lob::{CHUNK_OCTETS, Lob, LobKind};

/// A temporary store of its own.
fn store() -> TempLobStore {
    TempLobStore::new(Arc::new(WorkAreas::new(MemoryBudget::new(64 << 20))))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.37.5] each locator is new,
/// never 0, and a commit ends every one but a held one, a rollback every one.
#[test]
fn locators_end_with_their_transaction_unless_held() {
    let mut locators = Locators::default();
    let a = locators.assign(Entry::Value { lob: Lob::of_text("a"), holdable: false });
    let b = locators.assign(Entry::Value { lob: Lob::of_text("b"), holdable: true });
    assert!(a != 0 && b != 0 && a != b);
    locators.ended(true);
    assert!(locators.value(a).is_err(), "not held: ended by the commit");
    assert_eq!(locators.value(b).unwrap().inline(), Some(&b"b"[..]), "held past the commit");
    locators.ended(false);
    assert_eq!(locators.value(b).err().map(|e| e.sqlstate().as_str().to_owned()), Some("0F001".to_owned()), "a rollback ends a held one");
}

/// Verifies: [DESIGN] RULES.md D79: a character large object made in
/// pieces, characters split between them, longer than two chunks, reads
/// back as given; a piece not UTF-8 is refused (SQLSTATE 22021); a binary
/// one takes any octets.
#[test]
fn a_value_is_made_a_piece_at_a_time() {
    let temp = store();
    let text: String = (0..CHUNK_OCTETS).map(|i| if i % 3 == 0 { '€' } else { 'q' }).collect();
    let mut appender = Appender::new(LobKind::Character);
    for piece in text.as_bytes().chunks(65_537) {
        appender.append(piece, &temp).unwrap();
    }
    let lob = appender.finish().unwrap();
    assert_eq!(lob.octets(), u64::try_from(text.len()).unwrap());
    assert_eq!(lob.chars(), u64::try_from(text.chars().count()).unwrap());
    assert_eq!(lob.reader().read(0, lob.octets()).unwrap(), text.as_bytes());
    let mut bad = Appender::new(LobKind::Character);
    bad.append(&[0xFF, 0xFE], &temp).unwrap();
    let refused = bad.finish().err().unwrap();
    assert_eq!((refused.code, refused.sqlstate().as_str().to_owned()), (Code::Data, "22021".to_owned()));
    let mut octets = Appender::new(LobKind::Binary);
    octets.append(&[0xFF, 0, 0xFE], &temp).unwrap();
    assert_eq!(octets.finish().unwrap().inline(), Some(&[0xFF, 0, 0xFE][..]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.37.5] a large object sent by
/// its locator is the value it names, in an array or a row too; a value held
/// otherwise is left as it is.
#[test]
fn values_sent_by_locators_are_found() {
    let found = Lob::of_text("found");
    let sent = Value::Row(
        vec![Value::Array(vec![Value::Lob(Lob::of_locator(LobKind::Character, 7, (5, 5)))].into()), Value::Lob(Lob::of_text("kept"))]
            .into(),
    );
    let got = resolved(&sent, &mut |l| if l == 7 { Ok(found.clone()) } else { Err(super::invalid(l)) }).unwrap();
    let Value::Row(fields) = got else { panic!() };
    let [Value::Array(items), Value::Lob(kept)] = &*fields else { panic!() };
    let [Value::Lob(l)] = &**items else { panic!() };
    assert_eq!((l.inline(), kept.inline()), (Some(&b"found"[..]), Some(&b"kept"[..])));
}
