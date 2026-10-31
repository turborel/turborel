//! Unit tests of `spill/map.rs`: entries held, then spilled, read,
//! replaced and taken away alike; a provider without tables refused
//! (RULES.md R-TEST-1, R-TEST-2).

use std::sync::Arc;

use super::*;
use crate::spill::Spill;
use crate::spill::testing::{Held, areas};
use crate::value::types::TypeRegistry;

/// A map holding `limit` bytes before it spills, of the built-in provider,
/// or of one with no tables if `tableless`.
fn map(limit: usize, tableless: bool) -> SpillMap {
    let spill: Option<Arc<dyn Spill>> = if tableless { Some(Arc::new(Held { broken: false })) } else { None };
    SpillMap::new(ListSpace { limit, batch: 4096, areas: Some(areas(spill, usize::MAX)), types: TypeRegistry::default() })
}

/// Key `n`, of 8 bytes.
fn key(n: u64) -> [u8; 8] {
    n.to_le_bytes()
}

/// Verifies: [DESIGN] RULES.md D38, `SpillMap`: entries held, then, past
/// its bytes, moved to a spill area, are read as given: a value replaced,
/// one taken away gone, a key never given none; held and spilled alike.
#[test]
fn entries_are_read_back_held_and_spilled() {
    for (limit, spills) in [(usize::MAX, false), (512_usize, true)] {
        let mut m = map(limit, false);
        for n in 0..100_u64 {
            m.insert(&key(n), &key(n * 2)).unwrap();
        }
        assert_eq!(m.spilled(), spills);
        m.insert(&key(7), b"seven").unwrap();
        assert!(m.remove(&key(8)).unwrap());
        assert!(!m.remove(&key(8)).unwrap(), "already gone");
        assert!(!m.remove(&key(1000)).unwrap(), "never given");
        assert_eq!(m.get(&key(7)).unwrap(), Some(b"seven".to_vec()));
        assert_eq!(m.get(&key(9)).unwrap(), Some(key(18).to_vec()));
        assert_eq!(m.get(&key(8)).unwrap(), None);
        assert!(m.contains(&key(99)).unwrap());
        assert!(!m.contains(&key(8)).unwrap());
        assert!(!m.contains(&key(100)).unwrap());
    }
}

/// Verifies: [DESIGN] RULES.md D38, `SpillMap`: held, a value replaced or
/// taken away gives back its bytes, so it spills only once what it holds
/// passes its limit; with no work areas it never spills.
#[test]
fn held_bytes_follow_the_entries() {
    let mut m = map(200, false);
    for _ in 0..50_u32 {
        m.insert(&key(1), &key(2)).unwrap();
        m.insert(&key(3), b"").unwrap();
        assert!(m.remove(&key(3)).unwrap());
    }
    assert!(!m.spilled(), "one entry held");
    let mut unbounded = SpillMap::new(ListSpace { limit: 0, ..ListSpace::default() });
    unbounded.insert(&key(1), &key(1)).unwrap();
    assert!(!unbounded.spilled());
}

/// Verifies: [DESIGN] RULES.md D38, `SpillMap`: a provider that gives no
/// table refuses the spill; what was held stays held.
#[test]
fn a_spill_refused_keeps_what_is_held() {
    let mut m = map(64, true);
    m.insert(&key(1), &key(1)).unwrap();
    assert_eq!(m.insert(&key(2), &key(2)).err().map(|e| e.code), Some(Code::Api));
    assert!(!m.spilled());
    assert_eq!(m.get(&key(1)).unwrap(), Some(key(1).to_vec()));
    assert_eq!(m.get(&key(2)).unwrap(), Some(key(2).to_vec()));
}
